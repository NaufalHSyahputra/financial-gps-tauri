use crate::domain::io_error;
use std::{collections::HashSet, io::Write as IoWrite};

use super::{
    transactions::{self as tx, LedgerFilter, TxDetail},
    workflows::IdsOrFilter,
    *,
};

dto! {
    #[derive(PartialEq, Eq, Hash)]
    pub enum ExportColumn {
        Id, AccountId, NativeCurrency, NativeMinor, OriginalCurrency, OriginalMinor, Direction,
        BankDate, EffectiveDate, EffectiveBillDate, ReportingDate, ReportingDateOverride,
        Status, Description, Notes, PayeeId, CategoryId, Tags,
    }
    pub struct FileResult { pub file_name:String, pub row_count:String, pub revision:String }
}

const DEFAULT_COLUMNS: &[ExportColumn] = &[
    ExportColumn::Id,
    ExportColumn::AccountId,
    ExportColumn::NativeCurrency,
    ExportColumn::NativeMinor,
    ExportColumn::OriginalCurrency,
    ExportColumn::OriginalMinor,
    ExportColumn::Direction,
    ExportColumn::BankDate,
    ExportColumn::EffectiveDate,
    ExportColumn::EffectiveBillDate,
    ExportColumn::ReportingDate,
    ExportColumn::ReportingDateOverride,
    ExportColumn::Status,
    ExportColumn::Description,
    ExportColumn::Notes,
    ExportColumn::PayeeId,
    ExportColumn::CategoryId,
    ExportColumn::Tags,
];
impl ExportColumn {
    fn header(&self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::AccountId => "account_id",
            Self::NativeCurrency => "native_currency",
            Self::NativeMinor => "native_minor",
            Self::OriginalCurrency => "original_currency",
            Self::OriginalMinor => "original_minor",
            Self::Direction => "direction",
            Self::BankDate => "bank_date",
            Self::EffectiveDate => "effective_date",
            Self::EffectiveBillDate => "effective_bill_date",
            Self::ReportingDate => "reporting_date",
            Self::ReportingDateOverride => "reporting_date_override",
            Self::Status => "status",
            Self::Description => "description",
            Self::Notes => "notes",
            Self::PayeeId => "payee_id",
            Self::CategoryId => "category_id",
            Self::Tags => "tags",
        }
    }
    fn value<'a>(&self, item: &'a TxDetail, tags: &'a str) -> &'a str {
        let row = &item.row;
        let draft = &item.draft;
        match self {
            Self::Id => &row.id,
            Self::AccountId => &row.account_id,
            Self::NativeCurrency => &row.amount.currency,
            Self::NativeMinor => &row.amount.minor,
            Self::OriginalCurrency => draft
                .original
                .as_ref()
                .map_or("", |original| &original.amount.currency),
            Self::OriginalMinor => draft
                .original
                .as_ref()
                .map_or("", |original| &original.amount.minor),
            Self::Direction => row.direction.as_str(),
            Self::BankDate => &row.bank_date,
            Self::EffectiveDate => &row.effective_date,
            Self::EffectiveBillDate => draft.effective_bill_date.as_deref().unwrap_or(""),
            Self::ReportingDate => &row.reporting_date,
            Self::ReportingDateOverride => draft.reporting_date_override.as_deref().unwrap_or(""),
            Self::Status => row.status.as_str(),
            Self::Description => &row.description,
            Self::Notes => &draft.notes,
            Self::PayeeId => row.payee_id.as_deref().unwrap_or(""),
            Self::CategoryId => row.category_id.as_deref().unwrap_or(""),
            Self::Tags => tags,
        }
    }
    fn textual(&self) -> bool {
        matches!(
            self,
            Self::Description
                | Self::Notes
                | Self::Tags
                | Self::AccountId
                | Self::PayeeId
                | Self::CategoryId
                | Self::Id
        )
    }
}
fn csv_row<'a>(
    writer: &mut impl IoWrite,
    cells: impl IntoIterator<Item = (&'a str, bool)>,
) -> Result<()> {
    for (index, (cell, text)) in cells.into_iter().enumerate() {
        if index != 0 {
            writer.write_all(b",").map_err(io_error)?;
        }
        writer.write_all(b"\"").map_err(io_error)?;
        if text
            && (cell.starts_with(['\t', '\r', '\n'])
                || cell.trim_start().starts_with(['=', '+', '-', '@']))
        {
            writer.write_all(b"'").map_err(io_error)?;
        }
        writer
            .write_all(cell.replace('"', "\"\"").as_bytes())
            .map_err(io_error)?;
        writer.write_all(b"\"").map_err(io_error)?;
    }
    writer.write_all(b"\r\n").map_err(io_error)?;
    Ok(())
}

pub struct CsvExport<'a> {
    selection: Option<&'a IdsOrFilter>,
    filter: Option<&'a LedgerFilter>,
    columns: &'a [ExportColumn],
}

impl<'a> CsvExport<'a> {
    pub fn new(
        selection: Option<&'a IdsOrFilter>,
        filter: Option<&'a LedgerFilter>,
        columns: &'a [ExportColumn],
    ) -> Result<Self> {
        let columns = if columns.is_empty() {
            DEFAULT_COLUMNS
        } else {
            columns
        };
        if columns.iter().collect::<HashSet<_>>().len() != columns.len() {
            return Err(invalid("columns", "duplicate_export_columns"));
        }
        if selection.is_some() && filter.is_some() {
            return Err(invalid("selection", "select_one_export_filter"));
        }
        Ok(Self {
            selection,
            filter,
            columns,
        })
    }

    pub fn write(
        &self,
        ctx: &Context<'_>,
        output: &mut impl IoWrite,
        cancelled: impl Fn() -> bool,
    ) -> Result<u64> {
        let Self {
            selection,
            filter,
            columns,
        } = *self;
        csv_row(
            output,
            columns.iter().map(|column| (column.header(), false)),
        )?;
        let mut count = 0u64;
        let mut append = |id: &str| -> Result<()> {
            if cancelled() {
                return Err(AppError::new(ErrorCode::Cancelled, "operation_cancelled"));
            }
            let item = tx::get(ctx, id)?;
            let tags = item.tags.join(" ");
            csv_row(
                output,
                columns
                    .iter()
                    .map(|column| (column.value(&item, &tags), column.textual())),
            )?;
            count += 1;
            Ok(())
        };
        if let Some(IdsOrFilter::Ids { ids }) = selection {
            if ids.len() > 5000
                || ids.is_empty()
                || ids.iter().collect::<HashSet<_>>().len() != ids.len()
            {
                return Err(invalid("selection", "invalid_export_selection"));
            }
            for id in ids {
                append(id)?;
            }
        } else {
            let default = LedgerFilter::default();
            let filter = match selection {
                Some(IdsOrFilter::Filter { filter }) => filter.as_ref(),
                _ => filter.unwrap_or(&default),
            };
            let mut page = PageReq {
                cursor: None,
                limit: 200,
            };
            loop {
                let rows = tx::list(ctx, filter, &page, false)?;
                for row in rows.rows {
                    append(&row.id)?;
                }
                if rows.next_cursor.is_none() {
                    break;
                }
                page.cursor = rows.next_cursor;
            }
        }
        Ok(count)
    }
}
