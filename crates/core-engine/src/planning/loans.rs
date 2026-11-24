use crate::sql::{OptionalExtension, params};
use chrono::Datelike;
use rust_decimal::{Decimal, RoundingStrategy, prelude::ToPrimitive};

use crate::{
    domain::{Result, parse_date},
    ledger_engine::{
        Context, Money, Page, PageReq, ReadPage, ReceiptValue, VersionedId, accounts,
        check_revision, dto, fx::major, invalid, minor, missing, money, revision, scale,
        transactions,
    },
};

dto! {
    pub enum LoanKind { Mortgage, Auto, Personal, Student, Other }
    pub enum RatePeriod { Monthly, YearlyNominal, YearlyEffective }
    pub enum Amortization { Price, Sac }
    pub struct LoanTerms {
        pub account_id:String,pub principal:Money,pub interest_rate_percent:String,
        pub rate_period:RatePeriod,pub amortization:Amortization,pub kind:LoanKind,
        pub term_months:u16,pub first_payment_date:String,
    }
    pub struct LoanDetail {pub terms:LoanTerms,pub version:String,pub actual_balance:Money,pub outstanding_schedule_balance:Money}
    pub struct Installment {
        pub number:u16,pub due_date:String,pub principal:Money,pub interest:Money,pub payment:Money,pub balance:Money,pub overdue:bool,
    }
    pub struct LoanSchedule {pub rows:Vec<Installment>,pub increased_payment:bool,pub monthly_rate:String}
    pub struct LoanPayment {pub transaction_id:String,pub account_id:String,pub principal:Money,pub interest:Money}
}
macro_rules! db_values {
    ($kind:ident; $($variant:ident => $text:literal),+) => {
        impl $kind {
            fn as_str(&self)->&'static str {match self {$(Self::$variant=>$text),+}}
            fn read(text:&str)->crate::sql::Result<Self> {match text {$($text=>Ok(Self::$variant)),+,_=>Err(crate::sql::Error::InvalidQuery)}}
        }
    };
}
db_values!(LoanKind; Mortgage=>"mortgage", Auto=>"auto", Personal=>"personal", Student=>"student", Other=>"other");
db_values!(RatePeriod; Monthly=>"monthly",YearlyNominal=>"yearly_nominal",YearlyEffective=>"yearly_effective");
db_values!(Amortization; Price=>"price",Sac=>"sac");

fn precision() -> crate::domain::AppError {
    invalid("interestRatePercent", "loan_precision_exceeded")
}
fn parse_rate(value: &str) -> Result<Decimal> {
    Decimal::from_str_exact(value)
        .ok()
        .filter(|value| *value >= Decimal::ZERO && *value <= Decimal::from(100_000))
        .ok_or_else(|| invalid("interestRatePercent", "invalid_loan_rate"))
}
fn pow(mut base: Decimal, mut exp: u16) -> Option<Decimal> {
    let mut result = Decimal::ONE;
    while exp > 0 {
        if exp % 2 == 1 {
            result = result.checked_mul(base)?;
        }
        exp /= 2;
        if exp > 0 {
            base = base.checked_mul(base)?;
        }
    }
    Some(result)
}
fn monthly_bounds(annual: Decimal, period: &RatePeriod) -> Result<(Decimal, Decimal)> {
    let rate = annual
        .checked_div(Decimal::from(100))
        .ok_or_else(precision)?;
    match period {
        RatePeriod::Monthly => Ok((rate, rate)),
        RatePeriod::YearlyNominal => {
            let rate = rate.checked_div(Decimal::from(12)).ok_or_else(precision)?;
            Ok((rate, rate))
        }
        RatePeriod::YearlyEffective => {
            if rate == Decimal::ZERO {
                return Ok((rate, rate));
            }
            let target = Decimal::ONE + rate;
            let mut low = Decimal::ONE;
            let mut high = target;
            for _ in 0..128 {
                let mid = (low + high) / Decimal::TWO;
                if mid == low || mid == high {
                    break;
                }
                match pow(mid, 12) {
                    Some(value) if value < target => low = mid,
                    Some(value) if value == target => {
                        let unit = Decimal::from_i128_with_scale(1, 26);
                        return Ok((
                            (mid - Decimal::ONE - unit).max(Decimal::ZERO),
                            mid - Decimal::ONE + unit,
                        ));
                    }
                    _ => high = mid,
                }
            }
            Ok((low - Decimal::ONE, high - Decimal::ONE))
        }
    }
}
fn rounded(number: Decimal) -> Result<i64> {
    number
        .round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
        .to_i64()
        .ok_or_else(precision)
}
fn stable_round(
    input: impl Fn(Decimal) -> Option<Decimal>,
    bounds: (Decimal, Decimal),
) -> Result<i64> {
    let low = rounded(input(bounds.0).ok_or_else(precision)?)?;
    let high = rounded(input(bounds.1).ok_or_else(precision)?)?;
    if low != high {
        return Err(precision());
    }
    Ok(low)
}
fn price_payment(principal: Decimal, r: Decimal, n: u16) -> Option<Decimal> {
    if r == Decimal::ZERO {
        return principal.checked_div(Decimal::from(n));
    }
    let discount = Decimal::ONE.checked_div(Decimal::ONE.checked_add(r)?)?;
    let factor = Decimal::ONE.checked_sub(pow(discount, n)?)?;
    principal.checked_mul(r)?.checked_div(factor)
}

pub fn calculate(terms: &LoanTerms, as_of: chrono::NaiveDate) -> Result<LoanSchedule> {
    let principal = minor(&terms.principal.minor)?;
    if principal <= 0 || !(1..=1200).contains(&terms.term_months) {
        return Err(invalid("terms", "invalid_loan_terms"));
    }
    let due = parse_date(&terms.first_payment_date)?;
    let rate = monthly_bounds(
        parse_rate(&terms.interest_rate_percent)?,
        &terms.rate_period,
    )?;
    let fixed = match terms.amortization {
        Amortization::Price => stable_round(
            |r| price_payment(Decimal::from(principal), r, terms.term_months),
            rate,
        )?,
        Amortization::Sac => rounded(Decimal::from(principal) / Decimal::from(terms.term_months))?,
    };
    let first_interest = stable_round(|r| Decimal::from(principal).checked_mul(r), rate)?;
    let adjusted = matches!(terms.amortization, Amortization::Price) && fixed <= first_interest;
    let fixed = if adjusted {
        first_interest.checked_add(1).ok_or_else(precision)?
    } else {
        fixed
    };
    let mut balance = principal;
    let mut rows = Vec::with_capacity(usize::from(terms.term_months));
    let currency = &terms.principal.currency;
    for index in 1..=terms.term_months {
        let current_date = accounts::month_shift(due, i32::from(index) - 1, due.day())?;
        let interest = stable_round(|r| Decimal::from(balance).checked_mul(r), rate)?;
        let part = if index == terms.term_months {
            balance
        } else if matches!(terms.amortization, Amortization::Sac) {
            fixed.min(balance)
        } else {
            fixed.saturating_sub(interest).min(balance)
        };
        balance -= part;
        rows.push(Installment {
            number: index,
            due_date: current_date.to_string(),
            principal: money(currency, part),
            interest: money(currency, interest),
            payment: money(currency, i128::from(part) + i128::from(interest)),
            balance: money(currency, balance),
            overdue: current_date < as_of,
        });
    }
    Ok(LoanSchedule {
        rows,
        increased_payment: adjusted,
        monthly_rate: ((rate.0 + rate.1) / Decimal::TWO).normalize().to_string(),
    })
}

pub fn terms(ctx: &Context<'_>, id: &str) -> Result<LoanTerms> {
    ctx.tx.query_row("SELECT currency_code,CAST(principal_minor AS TEXT),rate_decimal,rate_period,amortization,kind,term_months,first_payment_date
        FROM loans WHERE account_id=?1",[id],|r|{
        let raw:String=r.get(2)?;
        let percent=Decimal::from_str_exact(&raw).ok().and_then(|value|value.checked_mul(Decimal::from(100)))
            .ok_or(crate::sql::Error::InvalidQuery)?;
        Ok(LoanTerms{account_id:id.to_owned(),principal:Money{currency:r.get(0)?,minor:r.get(1)?},
            interest_rate_percent:percent.normalize().to_string(),rate_period:RatePeriod::read(r.get_ref(3)?.as_str()?)?,
            amortization:Amortization::read(r.get_ref(4)?.as_str()?)?,kind:LoanKind::read(r.get_ref(5)?.as_str()?)?,
            term_months:r.get(6)?,first_payment_date:r.get(7)?})
    }).optional()?.ok_or_else(||missing("loanAccountId"))
}
pub fn get(ctx: &Context<'_>, id: &str) -> Result<LoanDetail> {
    let terms = terms(ctx, id)?;
    let actual = accounts::balance(ctx, id, ctx.today, None)?;
    let after = calculate(&terms, ctx.today)?
        .rows
        .into_iter()
        .take_while(|row| row.due_date <= ctx.today.to_string())
        .last()
        .map_or_else(|| terms.principal.clone(), |row| row.balance);
    Ok(LoanDetail {
        version: revision(&ctx.tx)?,
        actual_balance: money(&terms.principal.currency, actual),
        outstanding_schedule_balance: after,
        terms,
    })
}
pub fn save(
    ctx: &Context<'_>,
    target: Option<&VersionedId>,
    terms: &LoanTerms,
) -> Result<LoanDetail> {
    let account = accounts::raw(ctx, &terms.account_id)?.0;
    if account.kind != accounts::AccountKind::Loan
        || account.currency_code != terms.principal.currency
    {
        return Err(invalid("accountId", "loan_account_currency_mismatch"));
    }
    let existing: bool = ctx.tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM loans WHERE account_id=?1)",
        [&terms.account_id],
        |r| r.get(0),
    )?;
    if let Some(target) = target {
        check_revision(ctx, target)?;
        if target.id != terms.account_id || !existing {
            return Err(invalid("accountId", "loan_account_mismatch"));
        }
    } else if existing {
        return Err(crate::ledger_engine::conflict("loan_already_exists"));
    }
    calculate(terms, ctx.today)?;
    let percent = parse_rate(&terms.interest_rate_percent)? / Decimal::from(100);
    ctx.tx.execute("INSERT INTO loans VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
        ON CONFLICT(account_id) DO UPDATE SET kind=excluded.kind,principal_minor=excluded.principal_minor,rate_decimal=excluded.rate_decimal,
        rate_period=excluded.rate_period,amortization=excluded.amortization,term_months=excluded.term_months,first_payment_date=excluded.first_payment_date",
        params![terms.account_id,terms.principal.currency,terms.kind.as_str(),minor(&terms.principal.minor)?,percent.normalize().to_string(),
            terms.rate_period.as_str(),terms.amortization.as_str(),terms.term_months,terms.first_payment_date])?;
    let mut record = get(ctx, &terms.account_id)?;
    record.version = accounts::next_version(ctx)?;
    Ok(record)
}
pub fn schedule(
    ctx: &Context<'_>,
    id: &str,
    as_of: Option<&str>,
    page: &PageReq,
) -> Result<Page<Installment>> {
    let as_of = as_of.map(parse_date).transpose()?.unwrap_or(ctx.today);
    let cursor = ReadPage::new(ctx, page, &("loan_schedule", id, as_of.to_string()))?;
    let original = calculate(&terms(ctx, id)?, as_of)?;
    let after = cursor
        .after
        .as_ref()
        .map(|(key, _)| {
            key.parse::<u16>()
                .map_err(|_| invalid("cursor", "invalid_cursor"))
        })
        .transpose()?
        .unwrap_or(0);
    let rows = original
        .rows
        .into_iter()
        .filter(|row| row.number > after)
        .take(cursor.limit + 1)
        .collect();
    cursor.finish(rows, |row| (row.number.to_string(), row.due_date.clone()))
}

pub fn allocation(ctx: &Context<'_>, transaction_id: &str) -> Result<LoanPayment> {
    ctx.tx.query_row("SELECT a.loan_account_id,l.currency_code,CAST(a.principal_minor AS TEXT),CAST(a.interest_minor AS TEXT)
        FROM loan_payment_allocations a JOIN loans l ON l.account_id=a.loan_account_id WHERE a.transaction_id=?1",[transaction_id],|row|{
        let currency:String=row.get(1)?;
        Ok(LoanPayment{transaction_id:transaction_id.to_owned(),account_id:row.get(0)?,
            principal:money(&currency,row.get::<_,String>(2)?),interest:money(&currency,row.get::<_,String>(3)?)})
    }).optional()?.ok_or_else(||missing("transactionId"))
}
pub fn allocate(
    ctx: &Context<'_>,
    transaction_id: &str,
    account_id: &str,
    principal: &Money,
    interest: &Money,
) -> Result<LoanPayment> {
    let loan = terms(ctx, account_id)?;
    let posting = transactions::get(ctx, transaction_id)?;
    let principal_minor = accounts::nonnegative(&principal.minor)?;
    let interest_minor = accounts::nonnegative(&interest.minor)?;
    if loan.principal.currency != principal.currency
        || principal.currency != interest.currency
        || posting.row.amount.currency != principal.currency
        || posting.row.account_id != account_id
        || posting.row.direction.as_str() != "credit"
        || posting.row.status.as_str() != "posted"
        || posting.row.ignored
        || principal_minor.checked_add(interest_minor) != Some(minor(&posting.row.amount.minor)?)
    {
        return Err(invalid("payment", "loan_allocation_posting_mismatch"));
    }
    ctx.tx.execute("INSERT INTO loan_payment_allocations VALUES(?1,?2,?3,?4)
        ON CONFLICT(transaction_id) DO UPDATE SET principal_minor=excluded.principal_minor,interest_minor=excluded.interest_minor",
        params![transaction_id,account_id,principal_minor,interest_minor])?;
    allocation(ctx, transaction_id)
}

/// Project payoff of the current debt at the supplied fixed payment.
pub fn payoff_months(ctx: &Context<'_>, id: &str, payment: &Money) -> Result<Option<u16>> {
    let terms = terms(ctx, id)?;
    if payment.currency != terms.principal.currency {
        return Err(invalid("currency", "currency_mismatch"));
    }
    let payment = minor(&payment.minor)?;
    if payment <= 0 {
        return Err(invalid("payment", "invalid_loan_payment"));
    }
    let mut balance = (-accounts::balance(ctx, id, ctx.today, None)?).max(0);
    let unit = 10i128.pow(scale(&ctx.tx, &terms.principal.currency)?);
    let bounds = monthly_bounds(
        parse_rate(&terms.interest_rate_percent)?,
        &terms.rate_period,
    )?;
    for month in 0..=1200 {
        if balance <= unit {
            return Ok(Some(month));
        }
        let interest = stable_round(|rate| major(balance, 0).ok()?.checked_mul(rate), bounds)?;
        if interest >= payment {
            return Ok(None);
        }
        balance -= i128::from(payment - interest);
    }
    Ok(None)
}

impl ReceiptValue for LoanDetail {
    type Reference = String;
    fn reference(&self, _: &Context<'_>) -> String {
        self.terms.account_id.clone()
    }
    fn reload(ctx: &Context<'_>, id: String) -> Result<Self> {
        get(ctx, &id)
    }
}
impl ReceiptValue for LoanPayment {
    type Reference = String;
    fn reference(&self, _: &Context<'_>) -> String {
        self.transaction_id.clone()
    }
    fn reload(ctx: &Context<'_>, id: String) -> Result<Self> {
        allocation(ctx, &id)
    }
}
