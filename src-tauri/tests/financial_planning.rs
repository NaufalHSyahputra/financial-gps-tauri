use financial_gps::{
    domain::{Clock, Protection, Result, Scope, parse_date},
    ledger_engine::{
        Context, Mutation, VersionedId, accounts, catalog, corrections, money, new_id,
        transactions, transfers,
    },
    planning::{calendar, cash_plan, cycle_budgets, recurring},
    storage::{CreateLedger, LedgerManager, SqliteSession},
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn dto<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}

struct Fixture {
    _dir: tempfile::TempDir,
    manager: LedgerManager,
    scope: Scope,
}

struct FixedTime(chrono::DateTime<chrono::Utc>);
impl Clock for FixedTime {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.0
    }
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let manager = LedgerManager::new(dir.path()).unwrap();
        let scope = manager
            .create(
                "main",
                CreateLedger {
                    name: "Financial planning".into(),
                    base_currency: "IDR".into(),
                    timezone: Some("UTC".into()),
                    protection: Protection::Plain,
                    destination: None,
                    credential: None,
                    remember_key: false,
                },
            )
            .unwrap()
            .scope;
        Self {
            _dir: dir,
            manager,
            scope,
        }
    }

    fn write<T>(&self, now: &str, work: impl FnOnce(&Context<'_>) -> Result<T>) -> Result<T> {
        self.manager.write_domain("main", &self.scope, |tx| {
            let ctx = Context::new(
                SqliteSession(tx),
                &self.scope,
                &FixedTime(now.parse().unwrap()),
            )?;
            work(&ctx)
        })
    }
}

const NOW: &str = "2026-11-08T04:00:00Z";

fn configure(ctx: &Context<'_>, timezone: &str, anchor: u8) -> Result<()> {
    calendar::save(
        ctx,
        &dto(json!({"timezone":timezone,"anchor":anchor,"weekdayPolicy":"none"})),
    )?;
    Ok(())
}
fn account(ctx: &Context<'_>, name: &str, kind: &str, currency: &str) -> Result<String> {
    Ok(accounts::save(
        ctx,
        None,
        &dto(json!({"name":name,"kind":kind,"currencyCode":currency})),
    )?
    .id)
}
fn posting(account: &str, date: &str, amount: i64, direction: &str) -> transactions::TxDraft {
    dto(
        json!({"accountId":account,"bankDate":date,"amount":money("IDR",amount),
        "direction":direction,"status":"posted","description":"Posting"}),
    )
}
fn version(posting: &transactions::TxDetail) -> VersionedId {
    VersionedId {
        id: posting.row.id.clone(),
        version: posting.row.version.clone(),
    }
}

fn confirm(ctx: &Context<'_>, request: corrections::CorrectionRequest) -> Result<()> {
    let preview = corrections::preview(ctx, &request)?;
    let mutation = Mutation {
        scope: ctx.scope.clone(),
        request_id: new_id(),
        expected_revision: Some(preview.revision),
    };
    let input = corrections::CorrectionConfirm {
        request,
        token: preview.token,
    };
    ctx.mutate(
        "posting_correction_apply",
        &mutation,
        &input,
        &["transactions"],
        |ctx| corrections::apply(ctx, &input),
    )?;
    Ok(())
}

#[test]
fn calendar_rolls_at_ledger_local_midnight_and_changes_only_after_active_cycle() {
    let fixture = Fixture::new();
    fixture
        .write("2026-11-08T16:59:59Z", |ctx| {
            assert!(calendar::get(ctx)?.is_none());
            configure(ctx, "Asia/Jakarta", 1)?;
            configure(ctx, "Asia/Jakarta", 15)?;
            let view = calendar::get(ctx)?.unwrap();
            assert_eq!(view.today, "2026-11-08");
            assert_eq!(view.current.end, "2026-11-30");
            assert_eq!(view.pending.unwrap().effective_from, "2026-12-01");
            Ok(())
        })
        .unwrap();
    fixture
        .write("2026-11-08T17:00:00Z", |ctx| {
            assert_eq!(calendar::get(ctx)?.unwrap().today, "2026-11-09");
            configure(ctx, "Asia/Makassar", 16)?;
            assert_eq!(
                calendar::get(ctx)?
                    .unwrap()
                    .pending
                    .unwrap()
                    .settings
                    .anchor,
                16
            );
            configure(ctx, "Asia/Jakarta", 1)?;
            assert!(calendar::get(ctx)?.unwrap().pending.is_none());
            configure(ctx, "Asia/Jakarta", 15)
        })
        .unwrap();
    fixture
        .write("2026-12-01T00:00:00Z", |ctx| {
            let view = calendar::get(ctx)?.unwrap();
            assert_eq!(view.current.start, "2026-12-01");
            assert_eq!(view.current.end, "2027-01-14");
            assert!(matches!(
                view.current.kind,
                calendar::FinancialCycleKind::Extended
            ));
            assert_eq!(
                cycle_budgets::get(ctx, &dto(json!({"start":"2026-10-31"})))
                    .unwrap_err()
                    .message_key,
                "financial_calendar_history_unavailable"
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn cycle_budgets_split_spending_link_refunds_and_copy_only_definitions() {
    let fixture = Fixture::new();
    let account = fixture.write(NOW, |ctx| {
        configure(ctx, "Asia/Jakarta", 1)?;
        let account = account(ctx, "Cash", "cash", "IDR")?;
        let food = catalog::category_save(ctx, None, &dto(json!({"name":"Food","kind":"expense"})))?.id;
        let home = catalog::category_save(ctx, None, &dto(json!({"name":"Home","kind":"expense"})))?.id;
        let add = |name, id, limit| cycle_budgets::edit(ctx, &dto(json!({"start":"2026-11-01","change":{
            "save":{"name":name,"limitMinor":limit,"categoryIds":[id]}}})));
        add("Food", &food, "50")?;
        add("Home", &home, "0")?;
        let mut item = posting(&account, "2026-11-07", 100, "debit");
        item.splits = dto(json!([{"categoryId":food,"minor":"60"},{"categoryId":home,"minor":"40"}]));
        let purchase = transactions::save(ctx, None, &item)?;
        let refund = transactions::save(ctx, None, &posting(&account, "2026-11-08", 25, "credit"))?;
        confirm(ctx, dto(json!({"action":{"refund":{"target":version(&refund)}},"reason":"Merchant return"})))?;
        let view = cycle_budgets::get(ctx, &dto(json!({})))?.unwrap();
        assert_eq!(view.rows[0].spent.minor, "60");
        let report = || financial_gps::ledger_engine::reports::report(
            ctx, &financial_gps::ledger_engine::reports::ReportKind::IncomeVsExpense,
            &dto(json!({"range":{"from":"2026-11-01","to":"2026-11-08"}})),
            &financial_gps::ledger_engine::reports::Granularity::Month, Some("IDR"),
        );
        assert_eq!(report()?.separate_postings[0].label, "Unlinked refund");
        let refund = transactions::get(ctx, &refund.row.id)?;
        confirm(ctx, dto(json!({"action":{"refund":{"target":version(&refund),"originalId":purchase.row.id}},"reason":"Found receipt"})))?;
        let view = cycle_budgets::get(ctx, &dto(json!({})))?.unwrap();
        assert_eq!(view.rows[0].spent.minor, "45");
        assert_eq!(view.rows[0].run_rate.as_ref().unwrap().minor, "168");
        assert_eq!(view.rows[1].spent.minor, "30");
        let net_report = report()?;
        assert!(net_report.separate_postings.is_empty());
        assert_eq!(net_report.points[0].expense.as_ref().unwrap().reliable.minor, "75");
        assert!(matches!(view.rows[0].status, cycle_budgets::LimitStatus::Near));
        assert!(matches!(view.rows[1].status, cycle_budgets::LimitStatus::ZeroExceeded));
        let extra = transactions::save(ctx,None,&posting(&account,"2026-11-08",76,"credit"))?;
        assert_eq!(corrections::preview(ctx,&dto(json!({"action":{"refund":{"target":version(&extra),"originalId":purchase.row.id}},"reason":"Overpayment"}))).unwrap_err().message_key,"refund_exceeds_purchase");
        Ok((account,food))
    }).unwrap();
    fixture
        .write("2026-12-01T03:00:00Z", |ctx| {
            let mut row = posting(&account.0, "2026-12-01", 9, "debit");
            row.category_id = Some(account.1);
            transactions::save(ctx, None, &row)?;
            cycle_budgets::edit(
                ctx,
                &dto(json!({"start":"2026-12-01","change":"copyPrevious"})),
            )?;
            let copied = cycle_budgets::get(ctx, &dto(json!({})))?.unwrap();
            assert_eq!(copied.rows[0].limit.minor, "50");
            assert_eq!(copied.rows[0].spent.minor, "9");
            assert!(copied.rows[0].run_rate.is_none());
            Ok(())
        })
        .unwrap();
}

#[test]
fn opening_snapshot_keeps_history_and_stale_audit_preview_cannot_void() {
    let fixture = Fixture::new();
    fixture.write(NOW, |ctx| {
        let account = account(ctx,"Bank","checking","IDR")?;
        let old = transactions::save(ctx,None,&posting(&account,"2026-11-01",100,"credit"))?;
        let newer = transactions::save(ctx,None,&posting(&account,"2026-11-08",25,"debit"))?;
        confirm(ctx,dto(json!({"action":{"opening":{"accountId":account,"amount":money("IDR",400),"date":"2026-11-02"}},
            "reason":"Bank snapshot"})))?;
        assert_eq!(accounts::balance(ctx,&account,parse_date("2026-11-08")?,None)?,375);
        assert_eq!(transactions::get(ctx,&old.row.id)?.row.amount.minor,"100");
        assert_eq!(corrections::history(ctx,&account)?.len(),1);
        let request=dto(json!({"action":{"void":{"target":version(&newer)}},"reason":"Duplicate expense"}));
        let outdated=corrections::preview(ctx,&request)?;
        confirm(ctx,dto(json!({"action":{"opening":{"accountId":account,"amount":money("IDR",450),"date":"2026-11-02"}},
            "reason":"Corrected snapshot"})))?;
        assert_eq!(corrections::apply(ctx,&corrections::CorrectionConfirm{request:request.clone(),token:outdated.token}).unwrap_err().code,
            financial_gps::domain::ErrorCode::StaleRevision);
        confirm(ctx,request)?;
        assert_eq!(accounts::balance(ctx,&account,parse_date("2026-11-08")?,None)?,450);
        assert!(transactions::get(ctx,&newer.row.id)?.draft.ignored);
        assert!(transactions::remove(ctx,&newer.row.id,false).is_err());
        assert_eq!(corrections::history(ctx,&newer.row.id)?[0].before.as_ref().unwrap().row.amount.minor,"25");
        Ok(())
    }).unwrap();
}

#[test]
fn forecast_excludes_foreign_and_credit_counts_prior_income_for_due_and_resolves_stages() {
    let fixture = Fixture::new();
    fixture.write(NOW, |ctx| {
        configure(ctx,"Asia/Jakarta",1)?;
        let cash=account(ctx,"Cash","cash","IDR")?;
        account(ctx,"Travel","checking","USD")?;
        account(ctx,"Card","creditCard","IDR")?;
        transactions::save(ctx,None,&posting(&cash,"2026-11-07",100,"credit"))?;
        let edit = |name:&str,due:&str,amount:&str,kind:&str| {
            cash_plan::edit(ctx,&dto(json!({"save":{"name":name,"dueOn":due,"minor":amount,"kind":kind}})))
        };
        let obligation=edit("Past due","2026-11-07","120","obligation")?;
        edit("Salary","2026-11-08","100","stableIncome")?;
        edit("Bonus","2026-11-09","20","uncertainIncome")?;
        let rule=recurring::save(ctx,None,&dto(json!({"accountId":cash,"amount":money("IDR",10),"direction":"debit","description":"Membership",
            "frequency":"monthly","startsOn":"2026-11-09","active":true,"weekendPolicy":"none"})))?;
        cash_plan::edit(ctx,&dto(json!({"setStages":{"ruleId":rule.id,"stages":[{"startsOn":"2026-11-09","endsOn":null,"minor":"50"}]}})))?;
        let forecast=cash_plan::get(ctx,&dto(json!({"through":"2026-11-10","uncertainIds":[]})))?.unwrap();
        assert_eq!(forecast.cash_start.minor,"100");
        assert_eq!(forecast.excluded_foreign_accounts,"1");
        assert_eq!(forecast.days[0].obligations.minor,"120");
        assert_eq!(forecast.days[0].expected_gap.minor,"20");
        assert_eq!(forecast.days[0].expected_end.minor,"80");
        assert_eq!(forecast.days[1].obligations.minor,"50");
        assert_eq!(forecast.days[1].cash_only_gap.minor,"70");
        assert_eq!(forecast.days[1].expected_gap.minor,"0");
        assert!(forecast.days[1].depends_on_expected_income);

        let paid=transactions::save(ctx,None,&posting(&cash,"2026-11-08",30,"debit"))?;
        cash_plan::edit(ctx,&dto(json!({"save":{"id":obligation.id,"name":"Past due","dueOn":"2026-11-07",
            "minor":"120","kind":"obligation","transactionId":paid.row.id}})))?;
        let updated=cash_plan::get(ctx,&dto(json!({"through":"2026-11-08","uncertainIds":[]})))?.unwrap();
        assert_eq!(updated.cash_start.minor,"70");
        assert_eq!(updated.days[0].obligations.minor,"90");
        assert_eq!(updated.days[0].expected_gap.minor,"20");
        let duplicate=cash_plan::edit(ctx,&dto(json!({"save":{"name":"Duplicate","dueOn":"2026-11-08",
            "minor":"30","kind":"obligation","transactionId":paid.row.id}}))).unwrap_err();
        assert_eq!(duplicate.message_key,"cash_plan_posting_already_linked");
        Ok(())
    }).unwrap();
}

#[test]
fn transfer_fee_is_atomic_and_a_distinct_expense() {
    let fixture = Fixture::new();
    let (source, destination, category) = fixture
        .write(NOW, |ctx| {
            let source = account(ctx, "Bank", "checking", "IDR")?;
            let destination = account(ctx, "Cash", "cash", "IDR")?;
            let category = catalog::category_save(
                ctx,
                None,
                &dto(json!({"name":"Bank fees","kind":"expense"})),
            )?
            .id;
            Ok((source, destination, category))
        })
        .unwrap();
    let transfer = |valid: bool| {
        fixture.write(NOW, |ctx| {
            let debit = posting(&source, "2026-11-08", 100, "debit");
            let credit = posting(&destination, "2026-11-08", 100, "credit");
            let mut fee = posting(&source, "2026-11-08", 10, "debit");
            fee.category_id = valid.then(|| category.clone());
            transfers::save_with_fee(
                ctx,
                None,
                &dto(json!({"kind":"draft","draft":debit})),
                &dto(json!({"kind":"draft","draft":credit})),
                None,
                Some(&fee),
            )
        })
    };
    assert_eq!(
        transfer(false).unwrap_err().fields.unwrap()["categoryId"],
        "fee_category_required"
    );
    fixture
        .write(NOW, |ctx| {
            assert_eq!(accounts::balance(ctx, &source, ctx.today, None)?, 0);
            assert_eq!(accounts::balance(ctx, &destination, ctx.today, None)?, 0);
            Ok(())
        })
        .unwrap();
    let saved = transfer(true).unwrap();
    fixture
        .write(NOW, |ctx| {
            assert_eq!(accounts::balance(ctx, &source, ctx.today, None)?, -110);
            assert_eq!(accounts::balance(ctx, &destination, ctx.today, None)?, 100);
            assert_eq!(
                transfers::unlink(ctx, &saved.id).unwrap_err().message_key,
                "transfer_fee_requires_review"
            );
            Ok(())
        })
        .unwrap();
}
