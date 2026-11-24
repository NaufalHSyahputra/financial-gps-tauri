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
        let progress = |done| {
            if let Some(observer) = observer {
                let _ = observer.progress(done);
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
            "fx_provider_configure" => {
                let scope: Scope = field(request, "scope")?;
                let input: RateProvider = field(request, "input")?;
                write(db, &scope, |ctx| {
                    let status = fx::configure(
                        ctx,
                        &input.provider,
                        input.enabled,
                        input.credential.as_deref(),
                    )?;
                    ctx.tx.execute_batch(
                        "UPDATE ledger_meta SET revision=revision+1 WHERE singleton=1",
                    )?;
                    Ok(status)
                })
            }
            "import_preview_start" => {
                let scope: Scope = field(request, "scope")?;
                let input: ImportStart = field(request, "input")?;
                read(db, &scope, |ctx| match input.kind.as_str() {
                    "asset" | "fx" => {
                        if input.kind == "asset" {
                            self.check_module("assets_list")?;
                        }
                        if input
                            .format
                            .is_some_and(|format| format != StatementFormat::Csv)
                            || input.account_id.is_some()
                            || input.template_id.is_some()
                            || input.parser_options.is_some()
                        {
                            return Err(invalid("kind", "invalid_import_kind_options"));
                        }
                        imports::preview_structured_observed(
                            ctx,
                            &self.phase_four.imports,
                            name,
                            bytes,
                            &input.kind,
                            &cancelled,
                            &progress,
                        )
                    }
                    "statement" => {
                        let options = PreviewRequest {
                            account_id: input
                                .account_id
                                .ok_or_else(|| invalid("accountId", "account_required"))?,
                            format: input.format,
                            template_id: input.template_id,
                            parser_options: input.parser_options,
                            detect_duplicates: input.detect_duplicates,
                        };
                        imports::preview_observed(
                            ctx,
                            &self.phase_four.imports,
                            name,
                            bytes,
                            &options,
                            &cancelled,
                            &progress,
                        )
                    }
                    _ => Err(invalid("kind", "invalid_import_kind")),
                })
            }
            "import_commit_start" => {
                let mutation: Mutation = field(request, "mutation")?;
                let input: ImportCommit = field(request, "input")?;
                write(db, &mutation.scope, |ctx| {
                    let value = ctx.mutate(
                        command,
                        &mutation,
                        &input,
                        &["importBatches", "transactions"],
                        |ctx| {
                            if imports::is_asset_preview(
                                ctx,
                                &self.phase_four.imports,
                                &input.preview_token,
                            )? {
                                self.check_module("assets_list")?;
                            }
                            imports::commit(
                                ctx,
                                &self.phase_four.imports,
                                &input.preview_token,
                                &input.row_decisions,
                                &cancelled,
                            )
                        },
                    )?;
                    publish()?;
                    Ok(value)
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
            "fx_refresh_date" => {
                let scope: Scope = field(request, "scope")?;
                let input: RateRefresh = field(request, "input")?;
                self.check_module("fx_refresh_start")?;
                read(db, &scope, |ctx| {
                    fx::check_refresh(ctx, &input.provider, &input.pairs, &input.date)
                        .map(|date| date.to_string())
                })
            }
            "fx_refresh_save" => {
                let mutation: Mutation = field(request, "mutation")?;
                let input: RateRefresh = field(request, "input")?;
                let drafts: Vec<FxRateDraft> = field(request, "drafts")?;
                self.check_module("fx_refresh_start")?;
                write(db, &mutation.scope, |ctx| {
                    ctx.mutate(
                        "fx_refresh_start",
                        &mutation,
                        &input,
                        &["fxRates", "reports"],
                        |ctx| fx::save_provider(ctx, &input.provider, &drafts),
                    )
                })
            }
            "recurring_generate_due" => {
                let scope: Scope = field(request, "scope")?;
                write(db, &scope, recurring::generate_due)
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
            "import_template_file" => {
                let mutation: Mutation = field(request, "mutation")?;
                let input: TemplateFile = field(request, "input")?;
                match input.direction {
                    FileDirection::Import => write(db, &mutation.scope, |ctx| {
                        ctx.mutate(command, &mutation, &input, &["importTemplates"], |ctx| {
                            let draft = imports::decode_template(bytes, &input.format)?;
                            imports::template_save(ctx, None, &draft)
                        })
                        .map(TemplateFileResult::Imported)
                    }),
                    FileDirection::Export => read(db, &mutation.scope, |ctx| {
                        let template = imports::template_get(
                            ctx,
                            input
                                .template_id
                                .as_deref()
                                .ok_or_else(|| invalid("templateId", "template_required"))?,
                        )?;
                        output(
                            ctx,
                            name,
                            1,
                            imports::encode_template(&template, &input.format)?,
                        )
                    }),
                }
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

#[wasm_bindgen]
pub fn parse_provider_rate(
    pair: &str,
    day: &str,
    response: &[u8],
) -> std::result::Result<String, JsValue> {
    let work = || -> Result<String> {
        let pair = from_str(pair).map_err(invalid_request)?;
        let date = core_engine::domain::parse_date(day)?;
        let draft = fx::parse_provider_rate(&pair, date, response)?;
        to_string(&draft).map_err(invalid_request)
    };
    work().map_err(error_value)
}
