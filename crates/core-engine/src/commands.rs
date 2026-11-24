//! Command definitions shared by the native endpoint and the browser worker.

#[macro_export]
macro_rules! ledger_types {
    () => {
        dto! {
            pub struct AccountsQuery { pub include_archived: Option<bool>, pub as_of: Option<String> }
            pub struct AccountChoicesQuery { pub include_archived: Option<bool> }
            pub struct AccountQuery { pub id:String, pub as_of:Option<String> }
            pub struct AccountSave { pub target:Option<VersionedId>, pub draft:AccountDraft, pub starting_balance:Option<Money> }
            pub struct AccountArchive { pub target:VersionedId, pub archived_on:Option<String> }
            pub struct AccountDelete { pub target:VersionedId, pub confirm:bool }
            pub struct ChangeBalance { pub account_id:String, pub signed_money:Money, pub date:String, pub reason:Option<String> }
            pub struct LineSave { pub target:Option<VersionedId>, pub draft:CardLineDraft }
            pub struct StatementQuery { pub account_id:String, pub range:DateRange }
            pub struct StatementSave { pub target:Option<VersionedId>, pub draft:StatementDraft }
            pub struct LedgerSettingsSave { pub patch:SettingsPatch }
            pub struct RecordId { pub id:String }
            pub struct TransactionsQuery { pub filter:LedgerFilter, pub page:PageReq, pub running_balance:Option<bool> }
            pub struct TransactionSave { pub target:Option<VersionedId>, pub draft:TxDraft, pub group_id:Option<String> }
            pub struct TransactionPost { pub target:VersionedId, pub booking_date:Option<String> }
            pub struct TransactionDelete { pub target:VersionedId, pub confirm:bool }
            pub struct BulkPreviewQuery { pub selection:IdsOrFilter, pub action:BulkAction }
            pub struct BulkApply { pub preview_token:String, pub confirm:Option<bool> }
            pub struct InstallmentPreviewQuery { pub series:Option<String>, pub draft:InstallmentDraft, pub scope:InstallmentScope }
            pub struct ApplyPreview { pub preview_token:String }
            pub struct TransferCandidatesQuery { pub anchor_id:String, pub criteria:CandidateCriteria, pub page:PageReq }
            pub struct TransferSave { pub link_id:Option<String>, pub source:ExistingOrDraft, pub destination:ExistingOrDraft, pub rate:Option<TransferRate>, pub fee:Option<TxDraft> }
            pub struct TransferId { pub link_id:String }
            pub struct TransferDelete { pub link_id:String, pub disposition:Disposition, pub confirm:bool }
            pub struct CatalogQuery { pub query:Option<String>, pub include_archived:Option<bool>, pub page:PageReq }
            pub struct CategoryGroupSave { pub target:Option<VersionedId>, pub draft:CategoryGroupDraft }
            pub struct CategorySave { pub target:Option<VersionedId>, pub draft:CategoryDraft }
            pub struct PayeeSave { pub target:Option<VersionedId>, pub draft:PayeeDraft }
            pub struct ReferenceDelete { pub id:String, pub replacement_id:Option<String>, pub confirm:Option<bool> }
            pub struct PayeeMerge { pub from_ids:Vec<String>, pub into_id:String, pub confirm:bool }
            pub struct ConversionQuery { pub amount:Money, pub quote_currency:String, pub date:Option<String> }
            pub struct RateList { pub pair:Option<CurrencyPair>, pub range:Option<DateRange>, pub page:PageReq }
            pub struct RateSave { pub target:Option<VersionedId>, pub draft:FxRateDraft }
            pub struct RateProvider { pub provider:Provider, pub enabled:bool, pub credential:Option<String> }
            pub struct RateRefresh { pub provider:Provider, pub pairs:Vec<CurrencyPair>, pub date:String }
            pub struct DashboardQuery { pub as_of:Option<String>, #[serde(default)] pub account_ids:Vec<String>, #[serde(default)] pub widgets:Vec<DashboardWidget> }
            pub struct ReportQuery { pub kind:ReportKind, pub filter:LedgerFilter, pub granularity:Granularity, pub currency:Option<String> }
            pub struct CalendarQuery { pub range:DateRange, pub basis:DateBasis, #[serde(default)] pub account_ids:Vec<String>, pub include_projection:Option<bool> }
            pub struct SearchQuery { pub query:String, #[serde(default)] pub types:Vec<SearchKind>, pub page:PageReq }
            pub struct ExportQuery { pub selection:Option<IdsOrFilter>, pub filter:Option<LedgerFilter>, #[serde(default)] pub columns:Vec<ExportColumn>, pub save_grant:String }
        }
        page_response!(TransactionPage, TxRow);
        page_response!(TransferCandidatePage, TransferCandidate);
        page_response!(CategoryGroupPage, CategoryGroup);
        page_response!(CategoryPage, Category);
        page_response!(PayeePage, Payee);
        page_response!(RatePage, FxRate);
        page_response!(SearchPage, SearchHit);
    };
}

#[macro_export]
macro_rules! ledger_operations {
    () => {
        scoped_read!(ledger_settings_get, Settings, |ctx| accounts::settings(ctx));
        scoped_write!(
            ledger_settings_set,
            LedgerSettingsSave,
            Settings,
            &["settings", "accounts", "transactions", "reports"],
            |ctx, input, _state, _label| accounts::settings_save(ctx, input.patch.clone())
        );
        scoped_read!(
            accounts_list,
            AccountsQuery,
            Vec<AccountDetail>,
            |ctx, input, _state, _label| accounts::list(
                ctx,
                input.include_archived.unwrap_or(false),
                input.as_of.as_deref()
            )
        );
        scoped_read!(
            account_choices,
            AccountChoicesQuery,
            Vec<accounts::AccountChoice>,
            |ctx, input, _state, _label| accounts::choices(
                ctx,
                input.include_archived.unwrap_or(false)
            )
        );
        scoped_read!(
            account_get,
            AccountQuery,
            AccountDetail,
            |ctx, input, _state, _label| accounts::get(ctx, &input.id, input.as_of.as_deref())
        );
        scoped_write!(
            account_save,
            AccountSave,
            AccountDetail,
            &["accounts", "transactions", "reports"],
            |ctx, input, _state, _label| {
                if input.starting_balance.is_some() && input.target.is_some() {
                    return Err($crate::ledger_engine::invalid(
                        "startingBalance",
                        "starting_balance_only_for_new_accounts",
                    ));
                }
                let account = accounts::save(ctx, input.target.as_ref(), &input.draft)?;
                match &input.starting_balance {
                    Some(amount) => accounts::change_balance(
                        ctx,
                        &account.id,
                        amount,
                        &ctx.today.to_string(),
                        None,
                        true,
                    ),
                    None => Ok(account),
                }
            }
        );
        scoped_write!(
            account_archive,
            AccountArchive,
            AccountDetail,
            &["accounts", "reports"],
            |ctx, input, _state, _label| accounts::archive(
                ctx,
                &input.target,
                input.archived_on.as_deref()
            )
        );
        scoped_write!(
            account_delete,
            AccountDelete,
            Deleted,
            &["accounts", "reports", "search"],
            |ctx, input, _state, _label| accounts::delete(ctx, &input.target, input.confirm)
        );
        scoped_write!(
            account_set_opening,
            ChangeBalance,
            AccountDetail,
            &["accounts", "transactions", "reports"],
            |ctx, input, _state, _label| accounts::change_balance(
                ctx,
                &input.account_id,
                &input.signed_money,
                &input.date,
                input.reason.as_deref(),
                true
            )
        );
        scoped_write!(
            account_adjust_balance,
            ChangeBalance,
            AccountDetail,
            &["accounts", "transactions", "reports"],
            |ctx, input, _state, _label| accounts::change_balance(
                ctx,
                &input.account_id,
                &input.signed_money,
                &input.date,
                input.reason.as_deref(),
                false
            )
        );
        scoped_read!(card_lines_list, Vec<CardLineDetail>, |ctx| accounts::lines(
            ctx
        ));
        scoped_write!(
            card_line_save,
            LineSave,
            CardLineDetail,
            &["cardLines", "accounts"],
            |ctx, input, _state, _label| accounts::line_save(
                ctx,
                input.target.as_ref(),
                &input.draft
            )
        );
        scoped_write!(
            card_line_delete,
            RecordId,
            Deleted,
            &["cardLines", "accounts"],
            |ctx, input, _state, _label| accounts::line_delete(ctx, &input.id)
        );
        scoped_read!(
            card_statements_list,
            StatementQuery,
            Vec<StatementSummary>,
            |ctx, input, _state, _label| accounts::statements(ctx, &input.account_id, &input.range)
        );
        scoped_write!(
            card_statement_save,
            StatementSave,
            StatementSummary,
            &["cardStatements", "transactions", "reports"],
            |ctx, input, _state, _label| accounts::statement_save(
                ctx,
                input.target.as_ref(),
                &input.draft
            )
        );
        scoped_write!(
            card_statement_delete,
            RecordId,
            Deleted,
            &["cardStatements", "transactions", "reports"],
            |ctx, input, _state, _label| accounts::statement_delete(ctx, &input.id)
        );
        scoped_read!(
            transactions_list,
            TransactionsQuery,
            TransactionPage,
            |ctx, input, _state, _label| transactions::list(
                ctx,
                &input.filter,
                &input.page,
                input.running_balance.unwrap_or(false)
            )
        );
        scoped_read!(
            transaction_get,
            RecordId,
            TxDetail,
            |ctx, input, _state, _label| transactions::get(ctx, &input.id)
        );
        scoped_write!(
            transaction_save,
            TransactionSave,
            TxDetail,
            &["transactions", "accounts", "reports", "search", "groups"],
            |ctx, input, state, _label| {
                if input.group_id.is_some() {
                    state.check_module("group_save")?;
                }
                let transaction = transactions::save(ctx, input.target.as_ref(), &input.draft)?;
                if let Some(group) = &input.group_id {
                    workflows::assign_group(ctx, &transaction, group)?;
                    transactions::get(ctx, &transaction.row.id)
                } else {
                    Ok(transaction)
                }
            }
        );
        scoped_write!(
            transaction_post,
            TransactionPost,
            TxDetail,
            &["transactions", "accounts", "reports", "search"],
            |ctx, input, _state, _label| transactions::post(
                ctx,
                &input.target,
                input.booking_date.as_deref()
            )
        );
        scoped_write!(
            transaction_delete,
            TransactionDelete,
            Deleted,
            &["transactions", "accounts", "reports", "search"],
            |ctx, input, _state, _label| transactions::delete(ctx, &input.target, input.confirm)
        );
        scoped_read!(
            transactions_bulk_preview,
            BulkPreviewQuery,
            MutationPreview,
            |ctx, input, state, label| {
                if matches!(input.action, BulkAction::AssignGroup { .. }) {
                    state.check_module("groups_list")?;
                }
                workflows::bulk_preview(
                    ctx,
                    &state.previews,
                    &label,
                    &input.selection,
                    &input.action,
                )
            }
        );
        scoped_write!(
            transactions_bulk_apply,
            BulkApply,
            BulkResult,
            &["transactions", "accounts", "reports", "search"],
            |ctx, input, state, label| {
                if state
                    .previews
                    .assigns_group(ctx, &label, &input.preview_token)?
                {
                    state.check_module("groups_list")?;
                }
                workflows::bulk_apply(
                    ctx,
                    &state.previews,
                    &label,
                    &input.preview_token,
                    input.confirm.unwrap_or(false),
                )
            }
        );
        scoped_read!(
            installment_preview,
            InstallmentPreviewQuery,
            InstallmentPreview,
            |ctx, input, state, label| workflows::installment_preview(
                ctx,
                &state.previews,
                &label,
                input.series.as_deref(),
                &input.draft,
                &input.scope
            )
        );
        scoped_write!(
            installment_apply,
            ApplyPreview,
            InstallmentDetail,
            &["transactions", "accounts", "reports", "search"],
            |ctx, input, state, label| workflows::installment_apply(
                ctx,
                &state.previews,
                &label,
                &input.preview_token
            )
        );
        scoped_read!(
            transfer_get,
            TransferId,
            TransferDetail,
            |ctx, input, _state, _label| { transfers::get(ctx, &input.link_id) }
        );
        scoped_read!(
            transfer_candidates,
            TransferCandidatesQuery,
            TransferCandidatePage,
            |ctx, input, _state, _label| transfers::candidates(
                ctx,
                &input.anchor_id,
                &input.criteria,
                &input.page
            )
        );
        scoped_write!(
            transfer_save,
            TransferSave,
            TransferDetail,
            &["transfers", "transactions", "accounts", "reports", "search"],
            |ctx, input, _state, _label| transfers::save_with_fee(
                ctx,
                input.link_id.as_deref(),
                &input.source,
                &input.destination,
                input.rate.as_ref(),
                input.fee.as_ref()
            )
        );
        scoped_write!(
            transfer_unlink,
            TransferId,
            TxPair,
            &["transfers", "transactions", "accounts", "reports"],
            |ctx, input, _state, _label| transfers::unlink(ctx, &input.link_id)
        );
        scoped_write!(
            transfer_delete,
            TransferDelete,
            Deleted,
            &["transfers", "transactions", "accounts", "reports", "search"],
            |ctx, input, _state, _label| transfers::delete(
                ctx,
                &input.link_id,
                &input.disposition,
                input.confirm
            )
        );
        scoped_read!(
            category_groups_list,
            CatalogQuery,
            CategoryGroupPage,
            |ctx, input, _state, _label| catalog::groups(
                ctx,
                input.query.as_deref(),
                input.include_archived.unwrap_or(false),
                &input.page
            )
        );
        scoped_read!(
            categories_list,
            CatalogQuery,
            CategoryPage,
            |ctx, input, _state, _label| catalog::categories(
                ctx,
                input.query.as_deref(),
                input.include_archived.unwrap_or(false),
                &input.page
            )
        );
        scoped_read!(
            payees_list,
            CatalogQuery,
            PayeePage,
            |ctx, input, _state, _label| catalog::payees(
                ctx,
                input.query.as_deref(),
                input.include_archived.unwrap_or(false),
                &input.page
            )
        );
        scoped_write!(
            category_group_save,
            CategoryGroupSave,
            CategoryGroup,
            &["categoryGroups", "categories"],
            |ctx, input, _state, _label| catalog::group_save(
                ctx,
                input.target.as_ref(),
                &input.draft
            )
        );
        scoped_write!(
            category_save,
            CategorySave,
            Category,
            &["categories", "accounts", "reports", "search"],
            |ctx, input, _state, _label| catalog::category_save(
                ctx,
                input.target.as_ref(),
                &input.draft
            )
        );
        scoped_write!(
            payee_save,
            PayeeSave,
            Payee,
            &["payees", "transactions", "search"],
            |ctx, input, _state, _label| catalog::payee_save(
                ctx,
                input.target.as_ref(),
                &input.draft
            )
        );
        scoped_write!(
            category_group_delete,
            ReferenceDelete,
            ReferenceChange,
            &["categoryGroups", "categories"],
            |ctx, input, _state, _label| catalog::group_delete(
                ctx,
                &input.id,
                input.replacement_id.as_deref(),
                input.confirm.unwrap_or(false)
            )
        );
        scoped_write!(
            category_delete,
            ReferenceDelete,
            ReferenceChange,
            &[
                "categories",
                "transactions",
                "accounts",
                "reports",
                "search"
            ],
            |ctx, input, _state, _label| catalog::category_delete(
                ctx,
                &input.id,
                input.replacement_id.as_deref(),
                input.confirm.unwrap_or(false)
            )
        );
        scoped_write!(
            payee_delete,
            ReferenceDelete,
            ReferenceChange,
            &["payees", "transactions", "search"],
            |ctx, input, _state, _label| catalog::payee_delete(
                ctx,
                &input.id,
                input.replacement_id.as_deref(),
                input.confirm.unwrap_or(false)
            )
        );
        scoped_write!(
            payee_merge,
            PayeeMerge,
            ReferenceChange,
            &["payees", "transactions", "search"],
            |ctx, input, _state, _label| catalog::payee_merge(
                ctx,
                &input.from_ids,
                &input.into_id,
                input.confirm
            )
        );
        scoped_read!(currencies_list, Vec<Currency>, |ctx| fx::currencies(ctx));
        scoped_read!(
            fx_convert,
            ConversionQuery,
            Conversion,
            |ctx, input, _state, _label| {
                let day =
                    $crate::ledger_engine::date_option(input.date.as_deref())?.unwrap_or(ctx.today);
                fx::Resolver::new(ctx).convert(&input.amount, &input.quote_currency, day)
            }
        );
        scoped_read!(
            fx_rates_list,
            RateList,
            RatePage,
            |ctx, input, _state, _label| fx::list(
                ctx,
                input.pair.as_ref(),
                input.range.as_ref(),
                &input.page
            )
        );
        scoped_write!(
            fx_rate_save,
            RateSave,
            FxRate,
            &["fxRates", "accounts", "reports"],
            |ctx, input, _state, _label| fx::save(ctx, input.target.as_ref(), &input.draft)
        );
        scoped_write!(
            fx_rate_delete,
            RecordId,
            Deleted,
            &["fxRates", "accounts", "reports"],
            |ctx, input, _state, _label| fx::delete(ctx, &input.id)
        );
        scoped_read!(
            dashboard_get,
            DashboardQuery,
            DashboardView,
            |ctx, input, _state, _label| reports::dashboard(
                ctx,
                input.as_of.as_deref(),
                &input.account_ids,
                &input.widgets
            )
        );
        scoped_read!(
            report_get,
            ReportQuery,
            ReportView,
            |ctx, input, _state, _label| reports::report(
                ctx,
                &input.kind,
                &input.filter,
                &input.granularity,
                input.currency.as_deref()
            )
        );
        scoped_read!(
            calendar_get,
            CalendarQuery,
            CalendarView,
            |ctx, input, _state, _label| reports::calendar(
                ctx,
                &input.range,
                &input.basis,
                &input.account_ids,
                input.include_projection.unwrap_or(false)
            )
        );
        scoped_read!(
            search_global,
            SearchQuery,
            SearchPage,
            |ctx, input, _state, _label| reports::search(
                ctx,
                &input.query,
                &input.types,
                &input.page
            )
        );
    };
}

#[macro_export]
macro_rules! planning_types {
    () => {
        dto! {
            pub struct Id { pub id:String }
            pub struct ConfirmedId { pub id:String, pub confirm:Option<bool> }
            pub struct PageQuery { pub page:PageReq }
            pub struct TemplateQuery { pub format:Option<StatementFormat> }
            pub struct TemplateSave { pub target:Option<VersionedId>, pub draft:TemplateDraft }
            pub enum FileDirection { Import, Export }
            pub struct TemplateFile { pub direction:FileDirection, pub grant:String, pub template_id:Option<String>, pub format:DocumentFormat }
            pub enum TemplateFileResult { Imported(Write<ImportTemplate>), Exported(FileResult) }
            pub struct ImportStart { pub file_grant:String, pub kind:String, pub account_id:Option<String>, pub format:Option<StatementFormat>,
                pub template_id:Option<String>, pub parser_options:Option<ParserOptions>, pub detect_duplicates:Option<bool> }
            pub struct ImportPage { pub preview_token:String, pub status:Option<PreviewStatus>, pub page:PageReq }
            pub struct ImportCommit { pub preview_token:String, pub row_decisions:Vec<RowDecision> }
            pub struct BatchQuery { pub batch_id:String }
            pub struct BatchesQuery { pub kind:Option<String>, pub page:PageReq }
            pub struct UndoQuery { pub batch_id:String, pub selection:Option<Vec<u32>> }
            pub struct UndoApply { pub preview_token:String, pub selected_ordinals:Vec<u32> }
            pub struct RuleSave { pub target:Option<VersionedId>, pub draft:RuleDraft }
            pub struct RuleReorder { pub ordered_ids:Vec<String> }
            pub struct RuleStart { pub draft_or_ids:RuleSelection, pub filter:LedgerFilter, pub sample_limit:u16, pub override_policy:Option<OverridePolicy> }
            pub struct RuleApply { pub preview_token:String, pub override_policy:OverridePolicy }
            pub struct RulePackTransfer {pub direction:FileDirection, pub grant:String, pub rule_ids:Option<Vec<String>>,
                pub collision_policy:Option<CollisionPolicy>}
            pub enum RuleTransferResult { Imported(Write<RulePackResult>), Exported(FileResult) }
            pub struct RecurringList { pub range:Option<DateRange>, pub page:PageReq }
            pub struct RecurringSave { pub target:Option<VersionedId>, pub draft:recurring::RecurringDraft }
            pub struct Occurrences { pub rule_id:Option<String>, pub range:DateRange, pub page:PageReq }
            pub struct OccurrenceApply { pub rule_id:String, pub nominal_date:String, pub action:recurring::OccurrenceAction, pub transaction_id:Option<String> }
            pub struct LoanId { pub account_id:String }
            pub struct LoanSave { pub target:Option<VersionedId>, pub terms:loans::LoanTerms }
            pub struct LoanScheduleQuery { pub account_id:String, pub as_of:Option<String>, pub page:PageReq }
            pub struct LoanAllocation { pub transaction_id:String, pub account_id:String, pub principal:Money, pub interest:Money }
            pub struct AssetQuery { pub id:String, pub as_of:Option<String> }
            pub struct AssetList { pub group_id:Option<String>, pub as_of:Option<String>, pub page:PageReq }
            pub struct AssetSave { pub target:Option<VersionedId>, pub draft:assets::AssetDraft }
            pub struct AssetArchive { pub id:String, pub archived_on:Option<String> }
            pub struct AssetGroupSave { pub target:Option<VersionedId>, pub draft:assets::AssetGroupDraft, pub asset_ids:Vec<String> }
            pub struct ValuationSave { pub target:Option<VersionedId>, pub draft:assets::ValuationDraft }
            pub struct TradeSave { pub target:Option<VersionedId>, pub draft:assets::TradeDraft }
            pub struct AssetHistory { pub asset_id:String, pub range:DateRange, pub page:PageReq, pub view:assets::HistoryView }
        }
        page_response!(ImportPreviewPage, ImportPreviewRow);
        page_response!(ImportBatchPage, ImportBatch);
        page_response!(RecurringPage, recurring::RecurringDetail);
        page_response!(OccurrencePage, recurring::Occurrence);
        page_response!(LoanSchedulePage, loans::Installment);
        page_response!(AssetPage, assets::Asset);
        page_response!(AssetGroupPage, assets::AssetGroup);
        page_response!(AssetHistoryPage, assets::HistoryItem);
        dto! {
            pub struct BudgetMonth {pub month:String}
            pub struct BudgetsSave {pub entries:Vec<budgets::Entry>,pub overwrite:bool}
            pub struct BudgetsDelete {pub keys:Vec<budgets::Key>}
            pub struct BudgetsCopy {pub from_month:String,pub to_month:String,pub adjust_percent:String,pub overwrite:bool}
            pub struct BudgetSuggestions {pub months:u8,pub category_ids:Option<Vec<String>>}
            pub struct GoalList {pub state:Option<goals::State>,pub as_of:Option<String>,pub page:PageReq}
            pub struct GoalSave {pub target:Option<VersionedId>,pub draft:goals::GoalDraft}
            pub struct GroupSave {pub target:Option<VersionedId>,pub draft:shared::SharedGroupDraft}
            pub struct GroupArchive {pub id:String,pub archived:bool}
            pub struct GroupMemberSave {pub group_id:String,pub target:Option<VersionedId>,pub draft:shared::MemberDraft}
            pub struct GroupMemberRemove {pub group_id:String,pub member_id:String}
            pub struct GroupExpenseSave {pub target:Option<VersionedId>,pub draft:shared::SharedExpenseDraft}
            pub struct GroupSettlementSave {pub target:Option<VersionedId>,pub draft:shared::SettlementDraft}
        }
        page_response!(GoalPage, goals::Goal);
        page_response!(GroupPage, shared::GroupSummary);
    };
}

#[macro_export]
macro_rules! planning_operations {
    () => {
        scoped_read!(
            financial_calendar_get,
            Option<calendar::FinancialCalendarView>,
            |ctx| calendar::get(ctx)
        );
        scoped_write!(
            financial_calendar_save,
            calendar::CalendarInput,
            Deleted,
            &["financialCalendar", "cycleBudgets", "cashPlan"],
            |ctx, args, _desktop, _window| calendar::save(ctx, &args)
        );
        scoped_read!(
            budget_cycle_get,
            cycle_budgets::CycleBudgetQuery,
            Option<cycle_budgets::CycleBudgetView>,
            |ctx, args, _desktop, _window| cycle_budgets::get(ctx, &args)
        );
        scoped_write!(
            budget_cycle_edit,
            cycle_budgets::CycleBudgetEdit,
            Deleted,
            &["cycleBudgets"],
            |ctx, args, _desktop, _window| cycle_budgets::edit(ctx, &args)
        );
        scoped_read!(
            cash_plan_get,
            cash_plan::CashPlanQuery,
            Option<cash_plan::CashPlanView>,
            |ctx, args, _desktop, _window| cash_plan::get(ctx, &args)
        );
        scoped_write!(
            cash_plan_edit,
            cash_plan::CashPlanEdit,
            Deleted,
            &["cashPlan"],
            |ctx, args, desktop, _window| {
                if matches!(args, cash_plan::CashPlanEdit::SetStages(_)) {
                    desktop.check_module("recurring_list")?;
                }
                cash_plan::edit(ctx, &args)
            }
        );
        scoped_read!(
            posting_correction_preview,
            corrections::CorrectionRequest,
            corrections::CorrectionPreview,
            |ctx, args, _desktop, _window| corrections::preview(ctx, &args)
        );
        scoped_write!(
            posting_correction_apply,
            corrections::CorrectionConfirm,
            Deleted,
            &[
                "transactions",
                "accounts",
                "reports",
                "cashPlan",
                "cycleBudgets"
            ],
            |ctx, args, _desktop, _window| corrections::apply(ctx, &args)
        );
        scoped_read!(
            posting_audit_list,
            Id,
            Vec<corrections::PostingAuditItem>,
            |ctx, args, _desktop, _window| corrections::history(ctx, &args.id)
        );
        scoped_read!(
            import_templates_list,
            TemplateQuery,
            Vec<ImportTemplate>,
            |ctx, args, _desktop, _window| imports::templates(ctx, args.format)
        );
        scoped_write!(
            import_template_save,
            TemplateSave,
            ImportTemplate,
            &["importTemplates"],
            |ctx, args, _desktop, _window| imports::template_save(
                ctx,
                args.target.as_ref(),
                &args.draft
            )
        );
        scoped_write!(
            import_template_delete,
            Id,
            Deleted,
            &["importTemplates"],
            |ctx, args, _desktop, _window| imports::template_delete(ctx, &args.id)
        );
        scoped_read!(
            import_preview_page,
            ImportPage,
            ImportPreviewPage,
            |ctx, args, desktop, _window| {
                if imports::is_asset_preview(ctx, &desktop.phase_four.imports, &args.preview_token)?
                {
                    desktop.check_module("assets_list")?;
                }
                imports::preview_rows(
                    ctx,
                    &desktop.phase_four.imports,
                    &args.preview_token,
                    args.status.as_ref(),
                    &args.page,
                )
            }
        );
        scoped_read!(
            import_batches_list,
            BatchesQuery,
            ImportBatchPage,
            |ctx, args, _desktop, _window| imports::batches(ctx, args.kind.as_deref(), &args.page)
        );
        scoped_read!(
            import_batch_get,
            BatchQuery,
            ImportBatch,
            |ctx, args, _desktop, _window| imports::batch_get(ctx, &args.batch_id)
        );
        scoped_read!(
            import_undo_preview,
            UndoQuery,
            UndoPreview,
            |ctx, args, desktop, _window| {
                if imports::batch_get(ctx, &args.batch_id)?.kind == "asset" {
                    desktop.check_module("assets_list")?;
                }
                imports::undo_preview(
                    ctx,
                    &desktop.phase_four.undos,
                    &args.batch_id,
                    args.selection.as_deref(),
                )
            }
        );
        scoped_write!(
            import_undo_apply,
            UndoApply,
            UndoResult,
            &["importBatches", "transactions"],
            |ctx, args, desktop, _window| {
                if imports::is_asset_undo(ctx, &desktop.phase_four.undos, &args.preview_token)? {
                    desktop.check_module("assets_list")?;
                }
                imports::undo_apply(
                    ctx,
                    &desktop.phase_four.undos,
                    &args.preview_token,
                    &args.selected_ordinals,
                )
            }
        );
        scoped_read!(rules_list, Vec<Rule>, |ctx| rules::list(ctx));
        scoped_read!(rule_get, Id, Rule, |ctx, args, _desktop, _window| {
            rules::get(ctx, &args.id)
        });
        scoped_write!(
            rule_save,
            RuleSave,
            Rule,
            &["rules"],
            |ctx, args, _desktop, _window| rules::save(ctx, args.target.as_ref(), &args.draft)
        );
        scoped_write!(
            rule_delete,
            Id,
            Deleted,
            &["rules"],
            |ctx, args, _desktop, _window| rules::remove(ctx, &args.id)
        );
        scoped_write!(
            rules_reorder,
            RuleReorder,
            Vec<Rule>,
            &["rules"],
            |ctx, args, _desktop, _window| rules::reorder(ctx, &args.ordered_ids)
        );
        scoped_read!(
            recurring_list,
            RecurringList,
            RecurringPage,
            |ctx, args, _desktop, _window| recurring::list(ctx, args.range.as_ref(), &args.page)
        );
        scoped_read!(
            recurring_get,
            Id,
            recurring::RecurringDetail,
            |ctx, args, _desktop, _window| recurring::get(ctx, &args.id)
        );
        scoped_write!(
            recurring_save,
            RecurringSave,
            recurring::RecurringDetail,
            &["recurring"],
            |ctx, args, _desktop, _window| recurring::save(ctx, args.target.as_ref(), &args.draft)
        );
        scoped_write!(
            recurring_archive,
            Id,
            recurring::RecurringDetail,
            &["recurring"],
            |ctx, args, _desktop, _window| recurring::archive(ctx, &args.id)
        );
        scoped_read!(
            recurring_occurrences,
            Occurrences,
            OccurrencePage,
            |ctx, args, _desktop, _window| recurring::occurrences(
                ctx,
                args.rule_id.as_deref(),
                &args.range,
                &args.page
            )
        );
        scoped_write!(
            recurring_occurrence_apply,
            OccurrenceApply,
            recurring::Occurrence,
            &["recurring", "transactions"],
            |ctx, args, _desktop, _window| recurring::occurrence_apply(
                ctx,
                &args.rule_id,
                &args.nominal_date,
                &args.action,
                args.transaction_id.as_deref()
            )
        );
        scoped_read!(
            loan_get,
            LoanId,
            loans::LoanDetail,
            |ctx, args, _desktop, _window| loans::get(ctx, &args.account_id)
        );
        scoped_write!(
            loan_save,
            LoanSave,
            loans::LoanDetail,
            &["loans"],
            |ctx, args, _desktop, _window| loans::save(ctx, args.target.as_ref(), &args.terms)
        );
        scoped_read!(
            loan_schedule,
            LoanScheduleQuery,
            LoanSchedulePage,
            |ctx, args, _desktop, _window| loans::schedule(
                ctx,
                &args.account_id,
                args.as_of.as_deref(),
                &args.page
            )
        );
        scoped_write!(
            loan_payment_allocate,
            LoanAllocation,
            loans::LoanPayment,
            &["loans", "transactions"],
            |ctx, args, _desktop, _window| loans::allocate(
                ctx,
                &args.transaction_id,
                &args.account_id,
                &args.principal,
                &args.interest
            )
        );
        scoped_read!(
            assets_list,
            AssetList,
            AssetPage,
            |ctx, args, _desktop, _window| assets::list(
                ctx,
                args.group_id.as_deref(),
                args.as_of.as_deref(),
                &args.page
            )
        );
        scoped_read!(
            asset_get,
            AssetQuery,
            assets::Asset,
            |ctx, args, _desktop, _window| assets::get(ctx, &args.id, args.as_of.as_deref())
        );
        scoped_write!(
            asset_save,
            AssetSave,
            assets::Asset,
            &["assets"],
            |ctx, args, _desktop, _window| assets::save(ctx, args.target.as_ref(), &args.draft)
        );
        scoped_write!(
            asset_archive,
            AssetArchive,
            assets::Asset,
            &["assets"],
            |ctx, args, _desktop, _window| assets::archive(
                ctx,
                &args.id,
                args.archived_on.as_deref()
            )
        );
        scoped_write!(
            asset_delete,
            ConfirmedId,
            Deleted,
            &["assets"],
            |ctx, args, _desktop, _window| assets::delete(
                ctx,
                &args.id,
                args.confirm.unwrap_or(false)
            )
        );
        scoped_read!(
            asset_groups_list,
            PageQuery,
            AssetGroupPage,
            |ctx, args, _desktop, _window| assets::groups(ctx, &args.page)
        );
        scoped_read!(
            asset_group_get,
            AssetQuery,
            assets::AssetGroupDetail,
            |ctx, args, _desktop, _window| assets::group_detail(
                ctx,
                assets::group_get(ctx, &args.id)?,
                args.as_of.as_deref()
            )
        );
        scoped_write!(
            asset_group_save,
            AssetGroupSave,
            assets::AssetGroupDetail,
            &["assetGroups"],
            |ctx, args, _desktop, _window| assets::group_detail(
                ctx,
                assets::group_save(ctx, args.target.as_ref(), &args.draft, &args.asset_ids)?,
                None
            )
        );
        scoped_write!(
            asset_group_delete,
            ConfirmedId,
            Deleted,
            &["assetGroups"],
            |ctx, args, _desktop, _window| assets::group_delete(
                ctx,
                &args.id,
                args.confirm.unwrap_or(true)
            )
        );
        scoped_read!(
            asset_history,
            AssetHistory,
            AssetHistoryPage,
            |ctx, args, _desktop, _window| assets::history(
                ctx,
                &args.asset_id,
                &args.range,
                &args.page,
                &args.view
            )
        );
        scoped_write!(
            asset_valuation_save,
            ValuationSave,
            assets::Asset,
            &["assets"],
            |ctx, args, _desktop, _window| {
                let value = assets::valuation_save(ctx, args.target.as_ref(), &args.draft)?;
                assets::get(ctx, &value.draft.asset_id, None)
            }
        );
        scoped_write!(
            asset_valuation_delete,
            ConfirmedId,
            assets::Asset,
            &["assets"],
            |ctx, args, _desktop, _window| {
                let id = assets::valuation_owner(ctx, &args.id)?;
                assets::valuation_delete(ctx, &args.id, args.confirm.unwrap_or(true))?;
                assets::get(ctx, &id, None)
            }
        );
        scoped_write!(
            asset_trade_save,
            TradeSave,
            assets::Asset,
            &["assets"],
            |ctx, args, _desktop, _window| {
                let value = assets::trade_save(ctx, args.target.as_ref(), &args.draft)?;
                assets::get(ctx, &value.draft.asset_id, None)
            }
        );
        scoped_write!(
            asset_trade_delete,
            ConfirmedId,
            assets::Asset,
            &["assets"],
            |ctx, args, _desktop, _window| {
                let id = assets::trade_owner(ctx, &args.id)?;
                assets::trade_delete(ctx, &args.id, args.confirm.unwrap_or(false))?;
                assets::get(ctx, &id, None)
            }
        );
        scoped_read!(
            budgets_get,
            BudgetMonth,
            budgets::BudgetView,
            |ctx, args, _desktop, _window| budgets::get(ctx, &args.month)
        );
        scoped_write!(
            budgets_save,
            BudgetsSave,
            budgets::BudgetView,
            &["budgets"],
            |ctx, args, _desktop, _window| budgets::save(ctx, &args.entries, args.overwrite)
        );
        scoped_write!(
            budgets_delete,
            BudgetsDelete,
            budgets::BudgetView,
            &["budgets"],
            |ctx, args, _desktop, _window| budgets::delete(ctx, &args.keys)
        );
        scoped_write!(
            budgets_copy,
            BudgetsCopy,
            budgets::BudgetView,
            &["budgets"],
            |ctx, args, _desktop, _window| budgets::copy(
                ctx,
                &args.from_month,
                &args.to_month,
                &args.adjust_percent,
                args.overwrite
            )
        );
        scoped_read!(
            budget_suggestions,
            BudgetSuggestions,
            Vec<budgets::Suggestion>,
            |ctx, args, _desktop, _window| budgets::suggestions(
                ctx,
                args.months,
                args.category_ids.as_deref()
            )
        );
        scoped_read!(
            goals_list,
            GoalList,
            GoalPage,
            |ctx, args, _desktop, _window| goals::list(
                ctx,
                args.state.as_ref(),
                args.as_of.as_deref(),
                &args.page
            )
        );
        scoped_read!(
            goal_get,
            AssetQuery,
            goals::Goal,
            |ctx, args, _desktop, _window| goals::get(ctx, &args.id, args.as_of.as_deref())
        );
        scoped_write!(
            goal_save,
            GoalSave,
            goals::Goal,
            &["goals"],
            |ctx, args, _desktop, _window| goals::save(ctx, args.target.as_ref(), &args.draft)
        );
        scoped_write!(
            goal_delete,
            ConfirmedId,
            Deleted,
            &["goals"],
            |ctx, args, _desktop, _window| goals::delete(
                ctx,
                &args.id,
                args.confirm.unwrap_or(true)
            )
        );
        scoped_read!(
            groups_list,
            PageQuery,
            GroupPage,
            |ctx, args, _desktop, _window| shared::list(ctx, &args.page)
        );
        scoped_read!(
            group_get,
            AssetQuery,
            shared::GroupDetail,
            |ctx, args, _desktop, _window| shared::get(ctx, &args.id, args.as_of.as_deref())
        );
        scoped_write!(
            group_save,
            GroupSave,
            shared::GroupDetail,
            &["groups"],
            |ctx, args, _desktop, _window| shared::save(ctx, args.target.as_ref(), &args.draft)
        );
        scoped_write!(
            group_archive,
            GroupArchive,
            shared::GroupDetail,
            &["groups"],
            |ctx, args, _desktop, _window| shared::archive(ctx, &args.id, args.archived)
        );
        scoped_write!(
            group_member_save,
            GroupMemberSave,
            shared::GroupDetail,
            &["groups"],
            |ctx, args, _desktop, _window| shared::member_save(
                ctx,
                &args.group_id,
                args.target.as_ref(),
                &args.draft
            )
        );
        scoped_write!(
            group_member_remove,
            GroupMemberRemove,
            shared::GroupDetail,
            &["groups"],
            |ctx, args, _desktop, _window| shared::member_remove(
                ctx,
                &args.group_id,
                &args.member_id
            )
        );
        scoped_write!(
            group_expense_save,
            GroupExpenseSave,
            shared::GroupDetail,
            &["groups", "budgets"],
            |ctx, args, _desktop, _window| shared::expense_save(
                ctx,
                args.target.as_ref(),
                &args.draft
            )
        );
        scoped_write!(
            group_expense_remove,
            ConfirmedId,
            shared::GroupDetail,
            &["groups", "budgets"],
            |ctx, args, _desktop, _window| shared::expense_remove(
                ctx,
                &args.id,
                args.confirm.unwrap_or(false)
            )
        );
        scoped_write!(
            group_settlement_save,
            GroupSettlementSave,
            shared::GroupDetail,
            &["groups", "transactions"],
            |ctx, args, _desktop, _window| shared::settlement_save(
                ctx,
                args.target.as_ref(),
                &args.draft
            )
        );
        scoped_write!(
            group_settlement_remove,
            ConfirmedId,
            shared::GroupDetail,
            &["groups", "transactions"],
            |ctx, args, _desktop, _window| shared::settlement_remove(
                ctx,
                &args.id,
                args.confirm.unwrap_or(false)
            )
        );
    };
}
