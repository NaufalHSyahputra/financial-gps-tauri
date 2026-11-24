use super::*;
use core_engine::ledger_engine::{codec_error, invalid, revision};
use core_engine::preferences::{LayoutChange, LayoutQuery};
use serde_json::json;

impl WebEngine {
    pub(super) fn extra(
        &self,
        db: &sqlite::JsDatabase,
        command: &str,
        request: &Value,
        name: &str,
        bytes: &[u8],
        observer: Option<&JsJobObserver>,
    ) -> Result<Value> {
        let cancelled = || observer.is_some_and(|observer| observer.cancelled().unwrap_or(true));
        let publish = || -> Result<()> {
            if observer.is_some_and(|observer| !observer.publishing().unwrap_or(false)) {
                Err(AppError::new(ErrorCode::Cancelled, "operation_cancelled"))
            } else {
                Ok(())
            }
        };
        match command {
            "layout_get" => {
                let scope: Scope = field(request, "scope")?;
                let input: LayoutQuery = field(request, "input")?;
                read(db, &scope, |ctx| preferences::layout(ctx, &input.view))
            }
            "layout_set" => {
                let mutation: Mutation = field(request, "mutation")?;
                let input: LayoutChange = field(request, "input")?;
                write(db, &mutation.scope, |ctx| {
                    ctx.mutate(command, &mutation, &input, &["layout"], |ctx| {
                        preferences::layout_save(ctx, &input)
                    })
                })
            }
            "rule_preview_start" => {
                let scope: Scope = field(request, "scope")?;
                let input: RuleStart = field(request, "input")?;
                read(db, &scope, |ctx| {
                    rules::preview(
                        ctx,
                        &self.phase_four.rules,
                        input.draft_or_ids,
                        input.filter,
                        input.sample_limit,
                        input.override_policy.unwrap_or(OverridePolicy::Preserve),
                        &cancelled,
                    )
                })
            }
            "rule_apply_start" => {
                let mutation: Mutation = field(request, "mutation")?;
                let input: RuleApply = field(request, "input")?;
                write(db, &mutation.scope, |ctx| {
                    let value = ctx.mutate(
                        command,
                        &mutation,
                        &input,
                        &["transactions", "rules"],
                        |ctx| {
                            rules::apply(
                                ctx,
                                &self.phase_four.rules,
                                &input.preview_token,
                                &input.override_policy,
                                &cancelled,
                            )
                        },
                    )?;
                    publish()?;
                    Ok(value)
                })
            }
            "transactions_export_start" => {
                let scope: Scope = field(request, "scope")?;
                let input: ExportQuery = field(request, "input")?;
                read(db, &scope, |ctx| {
                    let export = CsvExport::new(
                        input.selection.as_ref(),
                        input.filter.as_ref(),
                        &input.columns,
                    )?;
                    let mut contents = Vec::new();
                    let count = export.write(ctx, &mut contents, cancelled)?;
                    publish()?;
                    output(ctx, name, count, contents)
                })
            }
            "rule_pack_transfer" => {
                let mutation: Mutation = field(request, "mutation")?;
                let input: RulePackTransfer = field(request, "input")?;
                match input.direction {
                    FileDirection::Import => write(db, &mutation.scope, |ctx| {
                        ctx.mutate(command, &mutation, &input, &["rules"], |ctx| {
                            let pack = serde_json::from_slice(bytes)
                                .map_err(|_| invalid("file", "invalid_rule_pack"))?;
                            rules::import_pack(
                                ctx,
                                pack,
                                input
                                    .collision_policy
                                    .as_ref()
                                    .unwrap_or(&CollisionPolicy::Skip),
                            )
                        })
                        .map(RuleTransferResult::Imported)
                    }),
                    FileDirection::Export => read(db, &mutation.scope, |ctx| {
                        let pack = rules::export_pack(ctx, input.rule_ids.as_deref())?;
                        output(
                            ctx,
                            name,
                            pack.rules.len() as u64,
                            serde_json::to_vec_pretty(&pack).map_err(codec_error)?,
                        )
                    }),
                }
            }
            _ => Err(AppError::validation(
                "operation",
                "unknown_database_operation",
            )),
        }
    }
}

fn output(ctx: &Context<'_>, name: &str, count: u64, bytes: Vec<u8>) -> Result<Value> {
    let result = FileResult {
        file_name: name.to_owned(),
        row_count: count.to_string(),
        revision: revision(&ctx.tx)?,
    };
    Ok(json!({ "result": result, "bytes": bytes }))
}
