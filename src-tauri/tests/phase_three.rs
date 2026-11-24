use chrono::{DateTime, NaiveDate, Utc};
use financial_gps::{
    domain::Protection,
    domain::{Clock, ErrorCode, Result, Scope, parse_date},
    ledger_engine::{
        AccountingMode, Context, DateBasis, DateRange, Direction, Mutation, PageReq, TxStatus,
        VersionedId,
        accounts::{
            self, AccountDraft, AccountKind, CardLineDraft, StatementDraft, StatementSource,
        },
        catalog::{self, CategoryDraft, CategoryKind, PayeeDraft},
        currency::{self, ConversionStatus, Resolver},
        export::{self, ExportColumn},
        money, new_id,
        reports::{self, DashboardWidget, Granularity, ReportKind, SearchKind},
        revision,
        transactions::{self as tx, LedgerFilter, SplitDraft, TxDraft},
        transfers::{self, ExistingOrDraft, TransferRate},
        workflows::{
            self, BulkAction, IdsOrFilter, InstallmentDraft, InstallmentScope, PreviewStore,
        },
    },
    storage::{CreateLedger, LedgerManager, PathGrants, PathPurpose},
};

const WINDOW: &str = "main";
struct FixedClock;
impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        "2026-08-01T09:00:00Z".parse().unwrap()
    }
}
fn date(date: &str) -> NaiveDate {
    parse_date(date).unwrap()
}
fn day(from: &str, to: &str) -> DateRange {
    DateRange {
        from: Some(from.to_owned()),
        to: Some(to.to_owned()),
    }
}
fn page(limit: u16) -> PageReq {
    PageReq {
        cursor: None,
        limit,
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    manager: LedgerManager,
    scope: Scope,
}
impl Fixture {
    fn new() -> Self {
        Self::create(tempfile::tempdir().unwrap(), None, false)
    }
    fn create(
        root: tempfile::TempDir,
        credential: Option<financial_gps::crypto::Credential>,
        packaged: bool,
    ) -> Self {
        let app_data = if packaged {
            root.path().join("app.financialgps.desktop")
        } else {
            root.path().to_owned()
        };
        let manager = LedgerManager::new(&app_data).unwrap();
        let session = manager
            .create(
                WINDOW,
                CreateLedger {
                    name: "Core Ledger test".to_owned(),
                    destination: None,
                    base_currency: "USD".to_owned(),
                    timezone: Some("Asia/Jakarta".to_owned()),
                    protection: if credential.is_some() {
                        Protection::Encrypted
                    } else {
                        Protection::Plain
                    },
                    credential,
                    remember_key: false,
                },
            )
            .unwrap();
        Self {
            _dir: root,
            manager,
            scope: session.scope,
        }
    }
    fn write<T>(&self, work: impl FnOnce(&Context<'_>) -> Result<T>) -> Result<T> {
        self.manager.write_domain(WINDOW, &self.scope, |db| {
            work(&Context::new(
                financial_gps::storage::SqliteSession(db),
                &self.scope,
                &FixedClock,
            )?)
        })
    }
    fn read<T>(&self, work: impl FnOnce(&Context<'_>) -> Result<T>) -> Result<T> {
        self.manager.read_domain(WINDOW, &self.scope, |db| {
            work(&Context::new(
                financial_gps::storage::SqliteSession(db),
                &self.scope,
                &FixedClock,
            )?)
        })
    }
}

fn account(name: &str, currency: &str, kind: AccountKind) -> AccountDraft {
    let card = kind == AccountKind::CreditCard;
    AccountDraft {
        name: name.to_owned(),
        institution: None,
        kind,
        currency_code: currency.to_owned(),
        payment_method: None,
        close_day: card.then_some(24),
        due_day: card.then_some(18),
        card_limit_minor: None,
        credit_line_id: None,
        accounting_override: card.then_some(AccountingMode::Accrual),
    }
}
fn posting(account: &str, amount: i64, currency: &str, date: &str) -> TxDraft {
    TxDraft {
        account_id: account.to_owned(),
        bank_date: date.to_owned(),
        amount: money(currency, amount),
        direction: Direction::Debit,
        status: TxStatus::Posted,
        description: "Groceries".to_owned(),
        notes: String::new(),
        payee_id: None,
        category_id: None,
        splits: Vec::new(),
        effective_bill_date: None,
        reporting_date_override: None,
        ignored: false,
        exclude_from_pnl: false,
        flagged: false,
        flag_due_date: None,
        alias_id: None,
        statement_id: None,
        installment: None,
        original: None,
    }
}
fn category(name: &str, kind: CategoryKind) -> CategoryDraft {
    CategoryDraft {
        icon: None,
        name: name.to_owned(),
        group_id: None,
        kind,
        hidden: false,
        ignored: false,
        archived: false,
    }
}

#[test]
fn category_icon_is_validated_and_returned_with_postings() {
    let fixture = Fixture::new();
    let mut draft = category("Groceries", CategoryKind::Expense);
    for invalid in ["", "a b", "a/b", "a--b", "-a", "🛒", &"a".repeat(65)] {
        draft.icon = Some(invalid.to_owned());
        let error = fixture
            .write(|ctx| catalog::category_save(ctx, None, &draft))
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Validation);
        assert_eq!(error.fields.unwrap()["icon"], "invalid_category_icon");
    }
    draft.icon = Some(" shopping-cart ".to_owned());
    let saved = fixture
        .write(|ctx| catalog::category_save(ctx, None, &draft))
        .unwrap();
    assert_eq!(saved.draft.icon.as_deref(), Some("shopping-cart"));
    let listed = fixture
        .read(|ctx| catalog::categories(ctx, None, false, &page(10)))
        .unwrap();
    assert_eq!(listed.rows[0].draft.icon, saved.draft.icon);

    let bank = fixture
        .write(|ctx| accounts::save(ctx, None, &account("Bank", "USD", AccountKind::Checking)))
        .unwrap();
    let mut transaction = posting(&bank.id, 500, "USD", "2026-08-01");
    transaction.category_id = Some(saved.id.clone());
    let posting = fixture
        .write(|ctx| tx::save(ctx, None, &transaction))
        .unwrap();
    assert_eq!(posting.row.category.unwrap().icon, saved.draft.icon);
    let target = VersionedId {
        id: saved.id,
        version: fixture.read(|ctx| revision(&ctx.tx)).unwrap(),
    };
    draft.icon = Some("coffee".to_owned());
    fixture
        .write(|ctx| catalog::category_save(ctx, Some(&target), &draft))
        .unwrap();
    let detail = fixture.read(|ctx| tx::get(ctx, &posting.row.id)).unwrap();
    assert_eq!(detail.row.category.unwrap().icon, draft.icon);
}

fn version(row: &tx::TxRow) -> VersionedId {
    VersionedId {
        id: row.id.clone(),
        version: row.version.clone(),
    }
}

#[test]
fn close_date_exclusion_and_rust_month_end_clamping() {
    for (bank, close_day, due_day, previous, close, due) in [
        (
            "2026-07-23",
            24,
            18,
            "2026-06-24",
            "2026-07-24",
            "2026-08-18",
        ),
        (
            "2026-07-24",
            24,
            18,
            "2026-07-24",
            "2026-08-24",
            "2026-09-18",
        ),
        (
            "2026-07-05",
            10,
            25,
            "2026-06-10",
            "2026-07-10",
            "2026-07-25",
        ),
        (
            "2025-02-27",
            31,
            31,
            "2025-01-31",
            "2025-02-28",
            "2025-03-31",
        ),
        (
            "2024-02-28",
            31,
            31,
            "2024-01-31",
            "2024-02-29",
            "2024-03-31",
        ),
        (
            "2024-02-29",
            31,
            31,
            "2024-02-29",
            "2024-03-31",
            "2024-04-30",
        ),
    ] {
        let result = accounts::cycle(date(bank), close_day, due_day).unwrap();
        assert_eq!(
            (result.previous_close, result.close, result.due),
            (date(previous), date(close), date(due))
        );
    }
}

#[test]
fn booking_dates_rebuild_when_statement_closing_and_due_dates_change() {
    let f = Fixture::new();
    f.write(|ctx| {
        let card = accounts::save(ctx, None, &account("Card", "USD", AccountKind::CreditCard))?;
        let mut draft = posting(&card.id, 100, "USD", "2026-07-24");
        let before = tx::save(ctx, None, &draft)?;
        assert_eq!(before.row.reporting_date, "2026-09-18");
        let statement = accounts::statement_save(
            ctx,
            None,
            &StatementDraft {
                account_id: card.id.clone(),
                close_on: "2026-07-25".to_owned(),
                due_on: "2026-08-20".to_owned(),
                source: StatementSource::Manual,
            },
        )?;
        let mapped = tx::get(ctx, &before.row.id)?;
        assert_eq!(
            mapped.row.statement_id.as_deref(),
            Some(statement.id.as_str())
        );
        assert_eq!(mapped.row.effective_date, "2026-08-20");
        assert_eq!(mapped.row.bank_date, "2026-07-24");
        draft.effective_bill_date = Some("2026-09-02".to_owned());
        draft.reporting_date_override = Some("2026-10-05".to_owned());
        draft.statement_id = Some(statement.id.clone());
        let edited = tx::save(ctx, Some(&version(&mapped.row)), &draft)?;
        assert_eq!(edited.row.effective_date, "2026-09-02");
        assert_eq!(edited.row.reporting_date, "2026-10-05");
        draft.direction = Direction::Credit;
        draft.effective_bill_date = None;
        let payment = tx::save(ctx, None, &draft)?;
        assert_eq!(payment.row.effective_date, "2026-07-24");
        accounts::statement_delete(ctx, &statement.id)?;
        let retained = tx::get(ctx, &edited.row.id)?;
        assert_eq!(retained.row.reporting_date, "2026-10-05");
        assert!(retained.row.statement_id.is_none());
        Ok(())
    })
    .unwrap();
}

#[test]
fn balances_archives_pending_authorizations_and_running_balance_across_display_filters() {
    let f = Fixture::new();
    f.write(|ctx| {
        let checking = accounts::save(
            ctx,
            None,
            &account("Checking", "USD", AccountKind::Checking),
        )?;
        accounts::change_balance(
            ctx,
            &checking.id,
            &money("USD", 1000),
            "2026-07-01",
            None,
            true,
        )?;
        accounts::change_balance(
            ctx,
            &checking.id,
            &money("USD", 1200),
            "2026-07-01",
            None,
            true,
        )?;
        accounts::change_balance(
            ctx,
            &checking.id,
            &money("USD", -50),
            "2026-07-02",
            Some("Bank correction"),
            false,
        )?;
        let mut draft = posting(&checking.id, 200, "USD", "2026-07-03");
        draft.description = "Visible purchase".to_owned();
        let visible = tx::save(ctx, None, &draft)?;
        draft.bank_date = "2026-07-04".to_owned();
        draft.description = "Hidden by text filter".to_owned();
        draft.exclude_from_pnl = true;
        tx::save(ctx, None, &draft)?;
        draft.bank_date = "2026-07-05".to_owned();
        draft.description = "Visible purchase two".to_owned();
        let visible2 = tx::save(ctx, None, &draft)?;
        draft.bank_date = "2026-07-06".to_owned();
        draft.status = TxStatus::Pending;
        tx::save(ctx, None, &draft)?;
        draft.status = TxStatus::Projected;
        tx::save(ctx, None, &draft)?;
        draft.status = TxStatus::Posted;
        draft.bank_date = "2026-08-10".to_owned();
        tx::save(ctx, None, &draft)?;
        assert_eq!(accounts::get(ctx, &checking.id, None)?.balance.minor, "550");
        assert_eq!(
            accounts::get(ctx, &checking.id, Some("2026-07-03"))?
                .balance
                .minor,
            "950"
        );
        let filter = LedgerFilter {
            account_ids: vec![checking.id.clone()],
            text: Some("visible".to_owned()),
            basis: Some(DateBasis::Bank),
            range: Some(day("2026-07-01", "2026-07-05")),
            ..Default::default()
        };
        let first = tx::list(ctx, &filter, &page(1), true)?;
        assert_eq!(first.rows[0].id, visible2.row.id);
        assert_eq!(first.rows[0].running_balance.as_ref().unwrap().minor, "550");
        let second = tx::list(
            ctx,
            &filter,
            &PageReq {
                cursor: first.next_cursor,
                limit: 1,
            },
            true,
        )?;
        assert_eq!(second.rows[0].id, visible.row.id);
        assert_eq!(
            second.rows[0].running_balance.as_ref().unwrap().minor,
            "950"
        );
        accounts::change_balance(
            ctx,
            &checking.id,
            &money("USD", 0),
            "2026-07-01",
            None,
            true,
        )?;
        assert_eq!(
            accounts::get(ctx, &checking.id, None)?.balance.minor,
            "-650"
        );
        accounts::archive(
            ctx,
            &VersionedId {
                id: checking.id.clone(),
                version: revision(&ctx.tx)?,
            },
            None,
        )?;
        assert_eq!(
            tx::save(ctx, None, &draft).unwrap_err().code,
            ErrorCode::Conflict
        );
        assert_eq!(
            accounts::get(ctx, &checking.id, None)?.balance.minor,
            "-650"
        );
        assert!(accounts::list(ctx, false, None)?.is_empty());
        Ok(())
    })
    .unwrap();
}

#[test]
fn shared_credit_lines_include_archived_debt_once_and_cap_available_credit() {
    let f = Fixture::new();
    f.write(|ctx| {
        let line = accounts::line_save(
            ctx,
            None,
            &CardLineDraft {
                name: "Shared line".to_owned(),
                currency_code: "USD".to_owned(),
                limit_minor: "1000".to_owned(),
            },
        )?;
        let mut draft = account("First card", "USD", AccountKind::CreditCard);
        draft.credit_line_id = Some(line.id.clone());
        let first = accounts::save(ctx, None, &draft)?;
        draft.name = "Second card".to_owned();
        let second = accounts::save(ctx, None, &draft)?;
        let mut charge = posting(&first.id, 900, "USD", "2026-07-12");
        charge.status = TxStatus::Pending;
        tx::save(ctx, None, &charge)?;
        charge.account_id = second.id.clone();
        charge.amount.minor = "500".to_owned();
        charge.status = TxStatus::Posted;
        tx::save(ctx, None, &charge)?;
        accounts::archive(
            ctx,
            &VersionedId {
                id: second.id.clone(),
                version: revision(&ctx.tx)?,
            },
            None,
        )?;
        let dashboard = reports::dashboard(
            ctx,
            None,
            &[],
            &[DashboardWidget::Accounts, DashboardWidget::Credit],
        )?;
        assert_eq!(dashboard.accounts.len(), 1);
        assert_eq!(dashboard.credit.len(), 1);
        let exposure = &dashboard.credit[0];
        assert_eq!(exposure.debt.minor, "1400");
        assert_eq!(exposure.available.as_ref().unwrap().minor, "-400");
        charge.account_id = first.id.clone();
        charge.amount.minor = "1600".to_owned();
        charge.direction = Direction::Credit;
        tx::save(ctx, None, &charge)?;
        let exposure = accounts::lines(ctx)?.pop().unwrap().credit;
        assert_eq!(exposure.debt.minor, "0");
        assert_eq!(exposure.available.as_ref().unwrap().minor, "1000");
        Ok(())
    })
    .unwrap();
}

#[test]
fn split_edit_rollback_unicode_tags_bulk_and_pnl_calculation() {
    let f = Fixture::new();
    let (posting, kind) = f
        .write(|ctx| {
            let account =
                accounts::save(ctx, None, &account("Wallet", "USD", AccountKind::Checking))?;
            let groceries =
                catalog::category_save(ctx, None, &category("Groceries", CategoryKind::Expense))?;
            let excluded =
                catalog::category_save(ctx, None, &category("Internal", CategoryKind::Transfer))?;
            let payee = catalog::payee_save(
                ctx,
                None,
                &PayeeDraft {
                    name: "Straße Café".to_owned(),
                    archived: false,
                },
            )?;
            assert_eq!(
                catalog::payee_save(
                    ctx,
                    None,
                    &PayeeDraft {
                        name: "STRASSE CAFE".to_owned(),
                        archived: false
                    }
                )
                .unwrap_err()
                .code,
                ErrorCode::Duplicate
            );
            assert_eq!(tx::weighted_split(101, &[1, 1, 1])?, [34, 34, 33]);
            assert_eq!(
                tx::weighted_split(9_000_000_000_000, &[u64::MAX, u64::MAX])?,
                [4_500_000_000_000, 4_500_000_000_000]
            );
            let mut draft = posting(&account.id, 101, "USD", "2026-07-10");
            draft.notes = "leave #foó and #Food alone".to_owned();
            draft.payee_id = Some(payee.id);
            draft.splits = vec![
                SplitDraft {
                    category_id: groceries.id.clone(),
                    minor: "67".to_owned(),
                },
                SplitDraft {
                    category_id: excluded.id,
                    minor: "34".to_owned(),
                },
            ];
            let inserted = tx::save(ctx, None, &draft)?;
            assert_eq!(inserted.tags, ["foo", "food"]);
            Ok((inserted, groceries.id))
        })
        .unwrap();

    let mut wrong = posting.draft.clone();
    wrong.splits[1].minor = "35".to_owned();
    assert_eq!(
        f.write(|ctx| tx::save(ctx, Some(&version(&posting.row)), &wrong))
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );

    f.write(|ctx| {
        let saved = tx::get(ctx, &posting.row.id)?;
        assert_eq!(saved.draft.splits[1].minor, "34");
        let filter = LedgerFilter {
            range: Some(day("2026-07-01", "2026-07-31")),
            ..Default::default()
        };
        let report = reports::report(
            ctx,
            &ReportKind::CategoryTrends,
            &filter,
            &Granularity::Month,
            None,
        )?;
        assert_eq!(
            report.points[0].expense.as_ref().unwrap().reliable.minor,
            "67"
        );
        assert_eq!(
            report.points[0].categories[0].category_id.as_deref(),
            Some(kind.as_str())
        );
        let drilldown = LedgerFilter {
            pnl_only: Some(true),
            category_ids: vec![kind.clone()],
            ..filter.clone()
        };
        let page_rows = tx::list(ctx, &drilldown, &page(10), false)?;
        assert_eq!(page_rows.rows[0].pnl_amount.as_ref().unwrap().minor, "67");
        let excluded = LedgerFilter {
            category_ids: vec![saved.draft.splits[1].category_id.clone()],
            ..drilldown
        };
        assert!(tx::list(ctx, &excluded, &page(10), false)?.rows.is_empty());
        let hits = reports::search(ctx, "strasse cafe", &[SearchKind::Transaction], &page(10))?;
        assert_eq!(hits.rows[0].id, saved.row.id);
        let store = PreviewStore::default();
        let select = IdsOrFilter::Ids {
            ids: vec![saved.row.id.clone()],
        };
        let preview = workflows::bulk_preview(
            ctx,
            &store,
            WINDOW,
            &select,
            &BulkAction::Categorize {
                category_id: Some(kind.clone()),
            },
        )?;
        assert!(preview.rows[0].blocked.is_some());
        let preview = workflows::bulk_preview(
            ctx,
            &store,
            WINDOW,
            &select,
            &BulkAction::RemoveTag {
                tag: "#FOO".to_owned(),
            },
        )?;
        let outcome = workflows::bulk_apply(ctx, &store, WINDOW, &preview.preview_token, false)?;
        assert_eq!(outcome.changed_ids, std::slice::from_ref(&saved.row.id));
        let edited = tx::get(ctx, &saved.row.id)?;
        assert_eq!(edited.draft.notes, "leave  and #Food alone");
        assert_eq!(edited.tags, ["food"]);
        tx::delete(ctx, &version(&edited.row), true)?;
        assert!(
            reports::search(ctx, "strasse cafe", &[SearchKind::Transaction], &page(10))?
                .rows
                .is_empty()
        );
        Ok(())
    })
    .unwrap();
}

fn insert_saved_rate(
    ctx: &Context<'_>,
    base: &str,
    quote: &str,
    date: &str,
    rate: &str,
) -> Result<()> {
    ctx.tx.execute(
        "INSERT INTO fx_rates VALUES(?1,?2,?3,?4,?5,'manual','fixture',0)",
        core_engine::sql::params![new_id(), base, quote, date, rate],
    )?;
    Ok(())
}

#[test]
fn currency_scales_exact_inverse_cross_and_missing_rates() {
    Fixture::new()
        .write(|ctx| {
            for (amount, source, dest, rate, expected) in [
                (1, "IDR", "USD", "0.005", "1"),
                (-1, "IDR", "USD", "0.005", "-1"),
                (105, "USD", "KWD", "0.123", "129"),
                (1, "KWD", "IDR", "1499.9", "1"),
                (1, "KWD", "IDR", "1500", "2"),
            ] {
                let result = currency::convert_at_rate(
                    ctx,
                    &money(source, amount),
                    dest,
                    currency::rate(rate)?,
                )?;
                assert_eq!(result.minor, expected);
            }
            insert_saved_rate(ctx, "USD", "EUR", "2026-08-01", "3")?;
            insert_saved_rate(ctx, "USD", "JPY", "2026-08-01", "0.015")?;
            insert_saved_rate(ctx, "EUR", "JPY", "2026-07-25", "999")?;
            insert_saved_rate(ctx, "EUR", "JPY", "2026-08-02", "1000")?;
            let mut resolver = Resolver::new(ctx);
            let exact = resolver.convert(&money("EUR", 100), "JPY", date("2026-09-01"))?;
            assert_eq!(exact.status, ConversionStatus::Exact);
            assert_eq!(exact.legs.len(), 2);
            assert!(exact.legs[0].inverted);
            assert_eq!(exact.displayed.unwrap().minor, "0");
            // EUR 100.00 / 3 * 0.015 is JPY 0.5 exactly.
            let cross = resolver.convert(&money("EUR", 10000), "JPY", date("2026-08-01"))?;
            assert_eq!(cross.displayed.unwrap().minor, "1");
            let prior = resolver.convert(&money("EUR", 100), "JPY", date("2026-07-30"))?;
            assert_eq!(prior.status, ConversionStatus::Prior);
            assert_eq!(prior.displayed.unwrap().minor, "999");
            let missing = resolver.convert(&money("EUR", 150), "KWD", date("2026-08-01"))?;
            assert_eq!(missing.status, ConversionStatus::Missing);
            assert!(missing.displayed.is_none());
            assert_eq!(missing.estimate.unwrap().minor, "1500");
            Ok(())
        })
        .unwrap();
}

fn leg(draft: TxDraft) -> ExistingOrDraft {
    ExistingOrDraft::Draft {
        draft: Box::new(draft),
        derive_amount: None,
    }
}

#[test]
fn cross_currency_transfer_validation_pair_effects_and_unlinking() {
    let fixture = Fixture::new();
    let (a, b) = fixture
        .write(|ctx| {
            let a = accounts::save(
                ctx,
                None,
                &account("USD source", "USD", AccountKind::Checking),
            )?;
            let b = accounts::save(
                ctx,
                None,
                &account("JPY savings", "JPY", AccountKind::Savings),
            )?;
            Ok((a.id, b.id))
        })
        .unwrap();
    let source = leg(posting(&a, 675, "USD", "2026-07-08"));
    let mut credit = posting(&b, 1000, "JPY", "2026-07-08");
    credit.direction = Direction::Credit;
    let dest = leg(credit.clone());
    let bad = TransferRate {
        decimal: "100".to_owned(),
        date: "2026-07-08".to_owned(),
        source: "quoted".to_owned(),
    };
    assert_eq!(
        fixture
            .write(|ctx| transfers::save(ctx, None, &source, &dest, Some(&bad)))
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    fixture
        .write(|ctx| {
            let linked = transfers::save(ctx, None, &source, &dest, None)?;
            assert_eq!(linked.rate.as_ref().unwrap().source, "implied");
            assert_eq!(
                linked.pair.source.row.transfer_id.as_deref(),
                Some(linked.id.as_str())
            );
            assert_eq!(accounts::get(ctx, &a, None)?.balance.minor, "-675");
            assert_eq!(accounts::get(ctx, &b, None)?.balance.minor, "1000");
            let filter = LedgerFilter {
                range: Some(day("2026-07-01", "2026-07-31")),
                ..Default::default()
            };
            let report = reports::report(
                ctx,
                &ReportKind::IncomeVsExpense,
                &filter,
                &Granularity::Month,
                None,
            )?;
            assert_eq!(
                report.points[0].expense.as_ref().unwrap().reliable.minor,
                "0"
            );
            let replaced = transfers::save(ctx, Some(&linked.id), &source, &dest, None)?;
            assert_eq!(replaced.pair.source.row.id, linked.pair.source.row.id);
            assert_eq!(
                replaced.pair.destination.row.id,
                linked.pair.destination.row.id
            );
            assert_eq!(
                tx::list(ctx, &LedgerFilter::default(), &page(20), false)?
                    .rows
                    .len(),
                2
            );
            let pair = transfers::unlink(ctx, &linked.id)?;
            assert!(pair.source.row.transfer_id.is_none());
            let existing_a = ExistingOrDraft::Existing {
                target: version(&pair.source.row),
            };
            let existing_b = ExistingOrDraft::Existing {
                target: version(&pair.destination.row),
            };
            let linked = transfers::save(ctx, None, &existing_a, &existing_b, None)?;
            assert_eq!(
                tx::list(ctx, &LedgerFilter::default(), &page(20), false)?
                    .rows
                    .len(),
                2
            );
            transfers::delete(ctx, &linked.id, &transfers::Disposition::Both, true)?;
            assert_eq!(accounts::get(ctx, &a, None)?.balance.minor, "0");
            assert_eq!(accounts::get(ctx, &b, None)?.balance.minor, "0");
            Ok(())
        })
        .unwrap();
}

#[test]
fn installment_schedule_preview_applies_only_the_selected_scope() {
    let f = Fixture::new();
    f.write(|ctx| {
        let card = accounts::save(ctx, None, &account("Card", "USD", AccountKind::CreditCard))?;
        let draft = InstallmentDraft {
            posting: posting(&card.id, 100, "USD", "2026-01-31"),
            purchase_date: "2026-01-31".to_owned(),
            occurrence_count: 3,
            anchor_id: None,
        };
        let store = PreviewStore::default();
        let preview = workflows::installment_preview(
            ctx,
            &store,
            WINDOW,
            None,
            &draft,
            &InstallmentScope::All,
        )?;
        assert_eq!(
            preview
                .rows
                .iter()
                .map(|r| (r.bank_date.as_str(), r.amount.minor.as_str()))
                .collect::<Vec<_>>(),
            [
                ("2026-01-31", "34"),
                ("2026-02-28", "33"),
                ("2026-03-31", "33")
            ]
        );
        let created = workflows::installment_apply(ctx, &store, WINDOW, &preview.preview_token)?;
        let mut edit = draft;
        edit.anchor_id = Some(created.transactions[1].id.clone());
        edit.posting.description = "New label".to_owned();
        edit.posting.amount.minor = "33".to_owned();
        edit.posting.bank_date = "2026-02-28".to_owned();
        let preview = workflows::installment_preview(
            ctx,
            &store,
            WINDOW,
            Some(&created.series_id),
            &edit,
            &InstallmentScope::Future,
        )?;
        assert_eq!(
            preview
                .rows
                .iter()
                .map(|row| row.number)
                .collect::<Vec<_>>(),
            [2, 3]
        );
        workflows::installment_apply(ctx, &store, WINDOW, &preview.preview_token)?;
        assert_eq!(
            tx::get(ctx, &created.transactions[0].id)?.row.description,
            "Groceries"
        );
        assert_eq!(
            tx::get(ctx, &created.transactions[2].id)?.row.description,
            "New label"
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn calendar_and_flow_keep_booked_and_reporting_timing_separate() {
    let f = Fixture::new();
    f.write(|ctx| {
        let card = accounts::save(ctx, None, &account("Card", "USD", AccountKind::CreditCard))?;
        let mut draft = posting(&card.id, 135, "USD", "2026-07-23");
        draft.reporting_date_override = Some("2026-07-30".to_owned());
        let item = tx::save(ctx, None, &draft)?;
        let calendar = reports::calendar(
            ctx,
            &day("2026-07-20", "2026-07-31"),
            &DateBasis::Reporting,
            &[],
            true,
        )?;
        assert_eq!(calendar.days.first().unwrap().date, "2026-07-19");
        let booked = calendar
            .days
            .iter()
            .find(|day| day.date == "2026-07-23")
            .unwrap();
        assert!(booked.events.is_empty());
        assert_eq!(booked.booked_movements[0].minor, "-135");
        let reported = calendar
            .days
            .iter()
            .find(|day| day.date == "2026-07-30")
            .unwrap();
        assert_eq!(reported.events[0].transaction.id, item.row.id);
        assert_eq!(reported.events[0].forecast_date, "2026-08-18");
        let bank_calendar = reports::calendar(
            ctx,
            &day("2026-07-20", "2026-07-31"),
            &DateBasis::Bank,
            &[],
            false,
        )?;
        assert_eq!(
            bank_calendar
                .days
                .iter()
                .find(|day| day.date == "2026-07-23")
                .unwrap()
                .events[0]
                .transaction
                .id,
            item.row.id
        );
        let flow = reports::report(
            ctx,
            &ReportKind::CashFlow,
            &LedgerFilter {
                range: Some(day("2026-07-01", "2026-08-31")),
                ..Default::default()
            },
            &Granularity::Month,
            None,
        )?;
        assert_eq!(
            flow.points[0].booked.as_ref().unwrap().reliable.minor,
            "-135"
        );
        assert_eq!(
            flow.points[1].forecast.as_ref().unwrap().reliable.minor,
            "-135"
        );
        assert_eq!(
            flow.points[1]
                .projected_movement
                .as_ref()
                .unwrap()
                .reliable
                .minor,
            "0"
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn confirmed_card_payments_reduce_forecast_without_changing_booked_amounts() {
    let f = Fixture::new();
    f.write(|ctx| {
        let card = accounts::save(ctx, None, &account("Card", "USD", AccountKind::CreditCard))?;
        let checking = accounts::save(ctx, None, &account("Wallet", "USD", AccountKind::Checking))?;
        let statement = accounts::statement_save(
            ctx,
            None,
            &StatementDraft {
                account_id: card.id.clone(),
                close_on: "2026-07-24".to_owned(),
                due_on: "2026-08-18".to_owned(),
                source: StatementSource::Manual,
            },
        )?;
        tx::save(ctx, None, &posting(&card.id, 250, "USD", "2026-07-18"))?;
        tx::save(ctx, None, &posting(&card.id, 150, "USD", "2026-07-20"))?;
        let from = leg(posting(&checking.id, 300, "USD", "2026-07-26"));
        let mut credit = posting(&card.id, 300, "USD", "2026-07-26");
        credit.direction = Direction::Credit;
        credit.statement_id = Some(statement.id);
        transfers::save(ctx, None, &from, &leg(credit), None)?;
        let filter = LedgerFilter {
            range: Some(day("2026-07-01", "2026-08-31")),
            ..Default::default()
        };
        let flow = reports::report(
            ctx,
            &ReportKind::CashFlow,
            &filter,
            &Granularity::Month,
            None,
        )?;
        assert_eq!(
            flow.points[0].booked.as_ref().unwrap().reliable.minor,
            "-400"
        );
        assert_eq!(
            flow.points[1].forecast.as_ref().unwrap().reliable.minor,
            "-100"
        );
        assert_eq!(
            flow.points[1]
                .projected_movement
                .as_ref()
                .unwrap()
                .reliable
                .minor,
            "0"
        );
        let calendar = reports::calendar(
            ctx,
            &day("2026-08-01", "2026-08-31"),
            &DateBasis::Reporting,
            &[],
            true,
        )?;
        let due = calendar
            .days
            .iter()
            .find(|day| day.date == "2026-08-18")
            .unwrap();
        assert_eq!(due.forecast_movements[0].minor, "-100");
        assert_eq!(accounts::get(ctx, &card.id, None)?.balance.minor, "-100");
        Ok(())
    })
    .unwrap();
}

#[test]
fn receipts_and_revision_guards_prevent_duplicate_and_stale_writes() {
    let f = Fixture::new();
    let mutation = Mutation {
        scope: f.scope.clone(),
        request_id: new_id(),
        expected_revision: Some("0".to_owned()),
    };
    let draft = account("Wallet", "USD", AccountKind::Checking);
    let write = || {
        f.write(|ctx| {
            ctx.mutate("account_save", &mutation, &draft, &["accounts"], |ctx| {
                accounts::save(ctx, None, &draft)
            })
        })
    };
    let result = write().unwrap();
    assert_eq!(result.revision, "1");
    assert_eq!(write().unwrap().value.id, result.value.id);
    let receipt = f
        .read(|ctx| {
            Ok(ctx.tx.query_row(
                "SELECT result_json FROM operation_receipts WHERE request_id=?1",
                [&mutation.request_id],
                |row| row.get::<_, String>(0),
            )?)
        })
        .unwrap();
    assert!(!receipt.contains("Wallet"));
    let receipt: serde_json::Value = serde_json::from_str(&receipt).unwrap();
    assert_eq!(receipt["reference"][0], result.value.id);
    let another = Mutation {
        request_id: new_id(),
        ..mutation.clone()
    };
    let stale = f.write(|ctx| {
        ctx.mutate("account_save", &another, &draft, &[], |ctx| {
            accounts::save(ctx, None, &draft)
        })
    });
    assert_eq!(stale.unwrap_err().code, ErrorCode::StaleRevision);
    let reused = f.write(|ctx| {
        ctx.mutate("account_archive", &mutation, &draft, &[], |ctx| {
            accounts::save(ctx, None, &draft)
        })
    });
    assert_eq!(reused.unwrap_err().code, ErrorCode::Conflict);
    assert_eq!(f.manager.session(WINDOW).unwrap().unwrap().revision, "1");
    assert_eq!(
        f.read(|ctx| accounts::list(ctx, true, None)).unwrap().len(),
        1
    );
    let next = Mutation {
        request_id: new_id(),
        expected_revision: Some("1".to_owned()),
        ..mutation.clone()
    };
    let edited = account("Renamed wallet", "USD", AccountKind::Checking);
    let target = VersionedId {
        id: result.value.id,
        version: result.value.version,
    };
    f.write(|ctx| {
        ctx.mutate("account_save", &next, &edited, &["accounts"], |ctx| {
            accounts::save(ctx, Some(&target), &edited)
        })
    })
    .unwrap();
    let retry = write().unwrap_err();
    assert_eq!(retry.message_key, "mutation_already_committed");
    assert_eq!(retry.fields.unwrap()["committedRevision"], "1");
    assert_eq!(f.manager.session(WINDOW).unwrap().unwrap().revision, "2");
    assert_eq!(
        f.read(|ctx| accounts::list(ctx, true, None)).unwrap().len(),
        1
    );
}

#[test]
fn portable_csv_export_preserves_native_units_and_neutralizes_text_formulas() {
    use std::fs;
    let f = Fixture::new();
    let selected = f
        .write(|ctx| {
            let wallet =
                accounts::save(ctx, None, &account("Wallet", "USD", AccountKind::Checking))?;
            let mut draft = posting(&wallet.id, 101, "USD", "2026-07-02");
            draft.description = "=SUM(1,2) \"quote\"".to_owned();
            draft.reporting_date_override = Some("2026-07-07".to_owned());
            Ok(tx::save(ctx, None, &draft)?.row.id)
        })
        .unwrap();
    let output = f._dir.path().join("selection.csv");
    let grants = PathGrants::default();
    let grant = grants
        .issue(WINDOW, PathPurpose::TransactionExport, &output)
        .unwrap();
    let approved = grants
        .consume(WINDOW, PathPurpose::TransactionExport, &grant.id)
        .unwrap();
    let result = f
        .read(|ctx| {
            export::export(
                ctx,
                Some(&IdsOrFilter::Ids {
                    ids: vec![selected],
                }),
                None,
                &[
                    ExportColumn::NativeMinor,
                    ExportColumn::BankDate,
                    ExportColumn::ReportingDate,
                    ExportColumn::Description,
                ],
                &approved,
                || false,
                || Ok(()),
            )
        })
        .unwrap();
    assert_eq!(result.row_count, "1");
    let content = fs::read_to_string(output).unwrap();
    assert!(
        content.contains("\"101\",\"2026-07-02\",\"2026-07-07\",\"'=SUM(1,2) \"\"quote\"\"\"\r\n")
    );
}

fn seed_reference_ledger(f: &Fixture) {
    f.write(|ctx| {
        ctx.tx.execute_batch(
            "WITH RECURSIVE n(x) AS (SELECT 0 UNION ALL SELECT x+1 FROM n WHERE x<49)
             INSERT INTO accounts(id,name,kind,currency_code,close_day,due_day)
             SELECT printf('a%02d',x),printf('Account %02d',x),
                CASE WHEN x=1 THEN 'credit_card' ELSE 'checking' END,
                CASE x%5 WHEN 2 THEN 'IDR' WHEN 3 THEN 'KWD' ELSE 'USD' END,
                CASE WHEN x=1 THEN 24 END,CASE WHEN x=1 THEN 18 END FROM n;
             INSERT INTO categories(id,name,kind) VALUES ('fixture-food','Food','expense'),('fixture-travel','Travel','expense');
             INSERT INTO card_statements VALUES ('fixture-card','a01','2025-06-24','2025-07-18','computed');
             WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<250000),
             sources AS (SELECT x,printf('a%02d',CASE WHEN x%7=0 THEN x%2 ELSE x%50 END) AS id FROM n)
             INSERT INTO transactions(id,account_id,currency_code,amount_minor,direction,bank_date,effective_date,applied_mode,status,description,assembly_state,created_at_ms,category_id,statement_id)
             SELECT printf('00000000-0000-4000-8000-%012d',x),a.id,a.currency_code,
                CASE WHEN x IN (1,5) THEN 100 ELSE x END,CASE WHEN x=5 THEN 'credit' ELSE 'debit' END,
                date('2025-01-01','+'||(x%365)||' day'),date('2025-01-01','+'||(x%365)||' day'),'cash',
                CASE WHEN x IN (1,5,100) THEN 'posted' WHEN (x/50)%12=0 THEN 'pending'
                     WHEN (x/50)%12=1 THEN 'projected' ELSE 'posted' END,
                'fixture',0,0,CASE WHEN x%4=2 AND x<>100 THEN 'fixture-food' WHEN x%4=3 THEN 'fixture-travel' END,
                CASE WHEN a.id='a01' AND x%365 BETWEEN 148 AND 173 THEN 'fixture-card' END
             FROM sources JOIN accounts a ON a.id=sources.id;
             INSERT INTO transaction_splits VALUES ('00000000-0000-4000-8000-000000000100',1,'fixture-food',40),
                ('00000000-0000-4000-8000-000000000100',2,'fixture-travel',60);
             UPDATE transactions SET assembly_state=1;
             INSERT INTO transfer_links(id,debit_id,credit_id,created_at_ms)
                VALUES ('fixture-transfer','00000000-0000-4000-8000-000000000001','00000000-0000-4000-8000-000000000005',0);
             INSERT INTO transaction_tags(transaction_id,token) SELECT id,CASE WHEN rowid%100=0 THEN 'groceries'
                ELSE printf('fixture-tag-%02d',(rowid/20)%17) END FROM transactions WHERE rowid%20=0;
             INSERT INTO transaction_search(rowid,transaction_id,description)
                SELECT rowid,id,CASE WHEN rowid%20=0 THEN 'fixture groceries' ELSE 'fixture' END FROM transactions;
             ANALYZE;"
        )?;
        Ok(())
    }).unwrap();
}

#[test]
fn representative_250k_ledger_uses_composite_date_and_fts_indexes() {
    let f = Fixture::new();
    seed_reference_ledger(&f);
    f.read(|ctx| {
        assert_eq!(ctx.tx.query_row("SELECT count(*) FROM transactions",[],|r|r.get::<_,i64>(0))?,250_000);
        let filter=LedgerFilter{account_ids:vec!["a00".to_owned()],range:Some(day("2025-04-01","2025-06-30")),basis:Some(DateBasis::Bank),..Default::default()};
        let cursor=financial_gps::ledger_engine::ReadPage::new(ctx,&page(200),&filter)?;
        let mut query=tx::Query::new(format!("SELECT {}{}",tx::ROW_COLUMNS,tx::SOURCE));
        tx::filter_query(ctx,&mut query,&filter)?;
        tx::sort_cursor(&mut query,&cursor,"t.bank_date","t.id",false,false)?;
        query.limit(201);
        let mut stmt=ctx.tx.prepare(&format!("EXPLAIN QUERY PLAN {}",query.sql))?;
        let plan=stmt.query_map(query.args,|r|r.get::<_,String>(3))?.collect::<core_engine::sql::Result<Vec<_>>>()?.join(" ");
        assert!(plan.contains("tx_account_bank"),"{plan}");
        let plan=ctx.tx.prepare(&format!("EXPLAIN QUERY PLAN SELECT t.bank_date,t.id,t.amount_minor FROM transactions t JOIN accounts a ON a.id=t.account_id
            WHERE t.account_id=?1 AND t.bank_date<=?2 AND {} ORDER BY t.bank_date,t.id",accounts::BALANCE_VISIBLE))?
            .query_map(core_engine::sql::params!["a00","2025-07-01"],|r|r.get::<_,String>(3))?.collect::<core_engine::sql::Result<Vec<_>>>()?.join(" ");
        assert!(plan.contains("tx_balance") || plan.contains("tx_account_bank"),"{plan}");
        let mut pnl_query=tx::Query::new(format!("SELECT t.reporting_date,t.currency_code,t.amount_minor{}",tx::SOURCE));
        tx::pnl_query(&mut pnl_query,ctx.today);
        let from=pnl_query.bind("2025-04-01".to_owned());
        let to=pnl_query.bind("2025-04-08".to_owned());
        pnl_query.and(&format!("t.reporting_date BETWEEN {from} AND {to}"));
        let pnl=ctx.tx.prepare(&format!("EXPLAIN QUERY PLAN {}",pnl_query.sql))?
            .query_map(pnl_query.args,|r|r.get::<_,String>(3))?.collect::<core_engine::sql::Result<Vec<_>>>()?.join(" ");
        assert!(pnl.contains("SEARCH t USING INDEX") && (pnl.contains("tx_pnl_report") || pnl.contains("tx_reporting")),"{pnl}");
        let fts=ctx.tx.prepare("EXPLAIN QUERY PLAN SELECT transaction_id FROM transaction_search WHERE transaction_search MATCH ?1")?
            .query_map(["groceries"],|r|r.get::<_,String>(3))?.collect::<core_engine::sql::Result<Vec<_>>>()?.join(" ");
        assert!(fts.contains("VIRTUAL TABLE INDEX")&&fts.contains("M"),"{fts}");
        let statement=LedgerFilter{statement_id:Some("fixture-card".into()),range:Some(day("2020-01-01","2020-01-02")),..Default::default()};
        let statement_page=tx::list(ctx,&statement,&page(200),false)?;
        assert!(!statement_page.rows.is_empty());
        assert!(statement_page.rows.iter().all(|row|row.statement_id.as_deref()==Some("fixture-card")));
        let results=tx::list(ctx,&filter,&page(200),true)?;
        assert_eq!(results.rows.len(),200);
        assert!(results.rows.iter().all(|row|row.running_balance.is_some()));
        let all = accounts::list(ctx, false, Some("2025-12-31"))?;
        assert_eq!(all.len(),50);
        for entry in all.iter().take(5) {
            let individually = accounts::get(ctx,&entry.id,Some("2025-12-31"))?;
            assert_eq!(entry.balance.minor,individually.balance.minor);
            assert_eq!(entry.credit.as_ref().map(|credit| &credit.debt.minor),
                individually.credit.as_ref().map(|credit| &credit.debt.minor));
        }
        Ok(())
    }).unwrap();
    if std::env::var_os("FINANCIAL_GPS_BENCHMARK").is_some() {
        benchmark_ledger(f);
        if let Some(parent) = std::env::var_os("FINANCIAL_GPS_PACKAGED_FIXTURES") {
            for (mode, credential) in [
                ("plain", None),
                (
                    "encrypted",
                    Some(
                        serde_json::from_value(serde_json::json!("release fixture only")).unwrap(),
                    ),
                ),
            ] {
                let directory = tempfile::Builder::new()
                    .prefix(mode)
                    .tempdir_in(&parent)
                    .unwrap();
                let fixture = Fixture::create(directory, credential, true);
                seed_reference_ledger(&fixture);
                fixture.manager.close(WINDOW, &fixture.scope).unwrap();
                let Fixture { _dir, manager, .. } = fixture;
                drop(manager);
                eprintln!("Packaged fixture ({mode}) = {}", _dir.keep().display());
            }
        }
    }
}

fn benchmark_ledger(fixture: Fixture) {
    use std::time::{Duration, Instant};
    let instance: uuid::Uuid = fixture.scope.ledger_instance_id.parse().unwrap();
    fixture.manager.close(WINDOW, &fixture.scope).unwrap();
    let Fixture {
        _dir: root,
        manager: old,
        ..
    } = fixture;
    drop(old);
    let open_start = Instant::now();
    let manager = LedgerManager::new(root.path()).unwrap();
    let session = manager
        .open(WINDOW, uuid::Uuid::new_v4(), instance.to_string(), || {
            Ok(financial_gps::storage::OpenTarget::Registered(instance))
        })
        .unwrap()
        .session
        .unwrap();
    let deferred = !manager.integrity_verified(WINDOW, &session.scope).unwrap();
    eprintln!("Background verification deferred = {deferred}");
    let reopened = Fixture {
        _dir: root,
        manager,
        scope: session.scope,
    };
    let date_range = day("2025-04-01", "2025-06-30");
    let filters = [
        (
            "account + date",
            LedgerFilter {
                account_ids: vec!["a00".into()],
                range: Some(date_range.clone()),
                basis: Some(DateBasis::Bank),
                ..Default::default()
            },
        ),
        (
            "reporting category",
            LedgerFilter {
                category_ids: vec!["fixture-food".into()],
                range: Some(date_range.clone()),
                basis: Some(DateBasis::Reporting),
                ..Default::default()
            },
        ),
        (
            "status + date",
            LedgerFilter {
                status: Some(TxStatus::Pending),
                range: Some(date_range),
                basis: Some(DateBasis::Bank),
                ..Default::default()
            },
        ),
        (
            "statement",
            LedgerFilter {
                statement_id: Some("fixture-card".into()),
                basis: Some(DateBasis::Bank),
                ..Default::default()
            },
        ),
        (
            "indexed tag",
            LedgerFilter {
                tags: vec!["groceries".into()],
                basis: Some(DateBasis::Bank),
                ..Default::default()
            },
        ),
    ];
    let first = reopened
        .read(|ctx| tx::list(ctx, &filters[0].1, &page(200), true))
        .unwrap();
    assert_eq!(first.rows.len(), 200);
    let open = open_start.elapsed();
    eprintln!(
        "Fresh manager + ledger open + first 200-row page (warm OS cache) = {:.3} ms",
        open.as_secs_f64() * 1000.0
    );
    let mut missed = Vec::new();
    if open >= Duration::from_secs(3) {
        missed.push(format!("backend open: {open:?}"));
    }
    for (name, filter) in filters {
        let plan = reopened
            .read(|ctx| {
                let column = if matches!(filter.basis, Some(DateBasis::Bank)) {
                    "t.bank_date"
                } else {
                    "t.reporting_date"
                };
                let cursor = financial_gps::ledger_engine::ReadPage::new(ctx, &page(200), &filter)?;
                let mut query = tx::Query::new(format!("SELECT {}{}", tx::ROW_COLUMNS, tx::SOURCE));
                tx::filter_query(ctx, &mut query, &filter)?;
                tx::sort_cursor(&mut query, &cursor, column, "t.id", false, false)?;
                query.limit(201);
                let mut explain = ctx
                    .tx
                    .prepare(&format!("EXPLAIN QUERY PLAN {}", query.sql))?;
                Ok(explain
                    .query_map(query.args, |row| row.get::<_, String>(3))?
                    .collect::<core_engine::sql::Result<Vec<_>>>()?
                    .join(" / "))
            })
            .unwrap();
        eprintln!("{name} plan = {plan}");
        if name == "indexed tag" {
            assert!(plan.contains("tag_lookup"), "{plan}");
        }
        let mut samples = Vec::new();
        for _ in 0..100 {
            let start = Instant::now();
            let result = reopened
                .read(|ctx| tx::list(ctx, &filter, &page(200), name == "account + date"))
                .unwrap();
            samples.push(start.elapsed());
            assert!(!result.rows.is_empty(), "{name}");
        }
        samples.sort_unstable();
        let p95 = samples[94];
        eprintln!(
            "{name} warm backend p95 = {:.3} ms (n=100)",
            p95.as_secs_f64() * 1000.0
        );
        if p95 >= Duration::from_millis(500) {
            missed.push(format!("{name}: {p95:?}"));
        }
    }
    assert!(
        missed.is_empty(),
        "missed backend budgets: {}",
        missed.join(", ")
    );
}

#[test]
fn future_reporting_override_leaves_balance_and_waits_for_actual_pnl() {
    let fixture = Fixture::new();
    let (account_id, tx_id) = fixture
        .write(|ctx| {
            let wallet = accounts::save(ctx, None, &account("Cash", "USD", AccountKind::Checking))?;
            let mut draft = posting(&wallet.id, 123, "USD", "2026-07-30");
            draft.reporting_date_override = Some("2026-08-10".into());
            let posting = tx::save(ctx, None, &draft)?;
            Ok((wallet.id, posting.row.id))
        })
        .unwrap();
    fixture
        .read(|ctx| {
            let wallet = accounts::get(ctx, &account_id, None)?;
            assert_eq!(wallet.balance.minor, "-123");
            let filter = LedgerFilter {
                basis: Some(DateBasis::Reporting),
                range: Some(day("2026-08-01", "2026-08-31")),
                ..Default::default()
            };
            assert_eq!(tx::list(ctx, &filter, &page(10), false)?.rows[0].id, tx_id);
            assert!(
                tx::list(
                    ctx,
                    &LedgerFilter {
                        pnl_only: Some(true),
                        ..filter.clone()
                    },
                    &page(10),
                    false
                )?
                .rows
                .is_empty()
            );
            assert_eq!(
                reports::report(
                    ctx,
                    &ReportKind::IncomeVsExpense,
                    &filter,
                    &Granularity::Month,
                    Some("USD")
                )?
                .points[0]
                    .expense
                    .as_ref()
                    .unwrap()
                    .reliable
                    .minor,
                "0"
            );
            Ok(())
        })
        .unwrap();
}
