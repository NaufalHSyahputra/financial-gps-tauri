use crate::{
    domain::Result,
    ledger_engine::{
        Context,
        export::{CsvExport, ExportColumn, FileResult},
        invalid, revision,
        transactions::LedgerFilter,
        workflows::IdsOrFilter,
    },
    storage::{ApprovedPath, io_error},
};
use std::io::{BufWriter, Write};
pub fn export(
    ctx: &Context<'_>,
    selection: Option<&IdsOrFilter>,
    filter: Option<&LedgerFilter>,
    columns: &[ExportColumn],
    target: &ApprovedPath,
    cancelled: impl Fn() -> bool,
    publish: impl FnOnce() -> Result<()>,
) -> Result<FileResult> {
    let export = CsvExport::new(selection, filter, columns)?;
    let (destination, replace) = target.export_target()?;
    let parent = destination
        .parent()
        .ok_or_else(|| invalid("grant", "invalid_export_destination"))?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
    let mut output = BufWriter::new(file.as_file_mut());
    let count = export.write(ctx, &mut output, cancelled)?;
    output.flush().map_err(io_error)?;
    drop(output);
    file.as_file().sync_all().map_err(io_error)?;
    let result = FileResult {
        file_name: destination
            .file_name()
            .expect("validated file name")
            .to_string_lossy()
            .into_owned(),
        row_count: count.to_string(),
        revision: revision(&ctx.tx)?,
    };
    target.export_target()?;
    publish()?;
    if replace {
        file.persist(destination)
            .map_err(|error| io_error(error.error))?;
    } else {
        file.persist_noclobber(destination)
            .map_err(|error| io_error(error.error))?;
    }
    Ok(result)
}
