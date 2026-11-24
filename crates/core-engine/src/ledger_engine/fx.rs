use crate::sql::{OptionalExtension, params, params_from_iter};
use chrono::NaiveDate;
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive};
use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};

use super::{
    accounts,
    transactions::{Query, sort_cursor},
    *,
};

dto! {
    #[derive(PartialEq, Eq, Hash)]
    pub struct CurrencyPair { pub base_code: String, pub quote_code: String }
    pub enum RateSource { Manual, Csv, Provider }
    pub struct FxRateDraft {
        pub pair: CurrencyPair, pub rate_date: String, pub rate_decimal: String, pub source_kind: RateSource, pub source_key: String,
    }
    pub struct FxRate { pub id: String, pub version: String, pub draft: FxRateDraft, pub created_at: String }
    pub struct Currency { pub code: String, pub scale: u8, pub name: String }
    pub struct FxLeg { pub rate_id: String, pub date: String, pub source_kind: RateSource, pub source_key: String, pub pair: CurrencyPair, pub rate_decimal: String, pub inverted: bool }
    #[derive(PartialEq, Eq)]
    pub enum ConversionStatus { Exact, Prior, Missing }
    pub struct Conversion {
        pub native: Money, pub displayed: Option<Money>, pub estimate: Option<Money>, pub status: ConversionStatus, pub legs: Vec<FxLeg>,
    }
    #[derive(PartialEq, Eq)]
    pub enum Provider { Frankfurter }
    pub struct ProviderStatus { pub provider: Provider, pub enabled: bool }
    pub struct FxRefreshResult { pub rates: Vec<FxRate> }
}

impl RateSource {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Csv => "csv",
            Self::Provider => "provider",
        }
    }
    fn read(s: &str) -> crate::sql::Result<Self> {
        match s {
            "manual" => Ok(Self::Manual),
            "csv" => Ok(Self::Csv),
            "provider" => Ok(Self::Provider),
            _ => Err(crate::sql::Error::InvalidQuery),
        }
    }
}

pub fn rate(value: &str) -> Result<Decimal> {
    if value.len() > 40 || !value.bytes().all(|ch| ch.is_ascii_digit() || ch == b'.') {
        return Err(invalid("rateDecimal", "invalid_positive_decimal_rate"));
    }
    Decimal::from_str_exact(value)
        .ok()
        .filter(|rate| *rate > Decimal::ZERO)
        .ok_or_else(|| invalid("rateDecimal", "invalid_positive_decimal_rate"))
}

pub fn major(minor: i128, scale: u32) -> Result<Decimal> {
    Decimal::try_from_i128_with_scale(minor, scale)
        .map_err(|_| invalid("amount", "decimal_range_exceeded"))
}
pub fn minor_i128(value: &str) -> Result<i128> {
    value
        .parse::<i128>()
        .ok()
        .filter(|amount| amount.to_string() == value)
        .ok_or_else(|| invalid("minor", "invalid_minor_units"))
}
#[derive(Clone)]
struct RateRatio {
    numerator: BigInt,
    denominator: BigInt,
}

impl RateRatio {
    fn new(value: Decimal, inverted: bool) -> Self {
        let magnitude = BigInt::from(value.mantissa());
        let divisor = BigInt::from(10u8).pow(value.scale());
        if inverted {
            Self {
                numerator: divisor,
                denominator: magnitude,
            }
        } else {
            Self {
                numerator: magnitude,
                denominator: divisor,
            }
        }
    }

    fn multiply(self, other: Self) -> Self {
        Self {
            numerator: self.numerator * other.numerator,
            denominator: self.denominator * other.denominator,
        }
    }

    fn convert(
        &self,
        amount: &Money,
        source_scale: u32,
        target: &str,
        target_scale: u32,
    ) -> Result<Money> {
        // Carry the complete ratio across inverse and cross-rate legs. Round only at the target minor unit.
        let source = BigInt::from(minor_i128(&amount.minor)?);
        let units = BigInt::from(10u8);
        let numerator = source.abs() * &self.numerator * units.pow(target_scale);
        let denominator = &self.denominator * units.pow(source_scale);
        let mut quotient = &numerator / &denominator;
        if (&numerator % &denominator) * 2u8 >= denominator {
            quotient += 1u8;
        }
        if source.is_negative() {
            quotient = -quotient;
        }
        let minor = quotient
            .to_i128()
            .ok_or_else(|| invalid("amount", "decimal_range_exceeded"))?;
        Ok(money(target, minor))
    }
}
pub fn convert_at_rate(
    ctx: &Context<'_>,
    amount: &Money,
    target: &str,
    rate: Decimal,
) -> Result<Money> {
    convert_quoted(ctx, amount, target, rate, false)
}
pub(crate) fn convert_quoted(
    ctx: &Context<'_>,
    amount: &Money,
    target: &str,
    rate: Decimal,
    inverted: bool,
) -> Result<Money> {
    if rate <= Decimal::ZERO {
        return Err(invalid("rate", "positive_rate_required"));
    }
    RateRatio::new(rate, inverted).convert(
        amount,
        scale(&ctx.tx, &amount.currency)?,
        target,
        scale(&ctx.tx, target)?,
    )
}

pub fn currencies(ctx: &Context<'_>) -> Result<Vec<Currency>> {
    Ok(ctx
        .tx
        .prepare("SELECT code,scale,name FROM currencies ORDER BY code")?
        .query_map([], |r| {
            Ok(Currency {
                code: r.get(0)?,
                scale: r.get(1)?,
                name: r.get(2)?,
            })
        })?
        .collect::<crate::sql::Result<_>>()?)
}
pub fn validate_pair(ctx: &Context<'_>, pair: &CurrencyPair) -> Result<()> {
    scale(&ctx.tx, &pair.base_code)?;
    scale(&ctx.tx, &pair.quote_code)?;
    if pair.base_code == pair.quote_code {
        return Err(invalid("pair", "distinct_currencies_required"));
    }
    Ok(())
}

const SELECT: &str = "SELECT r.id,r.base_code,r.quote_code,r.rate_date,r.rate_decimal,r.source_kind,r.source_key,r.created_at_ms FROM fx_rates r WHERE 1=1";
fn read_rate(row: &crate::sql::Row<'_>) -> crate::sql::Result<(String, FxRateDraft, i64)> {
    Ok((
        row.get(0)?,
        FxRateDraft {
            pair: CurrencyPair {
                base_code: row.get(1)?,
                quote_code: row.get(2)?,
            },
            rate_date: row.get(3)?,
            rate_decimal: row.get(4)?,
            source_kind: RateSource::read(row.get_ref(5)?.as_str()?)?,
            source_key: row.get(6)?,
        },
        row.get(7)?,
    ))
}
pub(super) fn get(ctx: &Context<'_>, id: &str) -> Result<FxRate> {
    let (id, draft, created_at) = ctx
        .tx
        .query_row(&format!("{SELECT} AND r.id=?1"), [id], read_rate)
        .optional()?
        .ok_or_else(|| missing("fxRateId"))?;
    Ok(FxRate {
        id,
        draft,
        version: revision(&ctx.tx)?,
        created_at: utc_timestamp(created_at)?,
    })
}

pub fn list(
    ctx: &Context<'_>,
    pair: Option<&CurrencyPair>,
    range: Option<&DateRange>,
    page: &PageReq,
) -> Result<Page<FxRate>> {
    let cursor = ReadPage::new(ctx, page, &("fx_rates", pair, range))?;
    let mut query = Query::new(SELECT.to_owned());
    if let Some(pair) = pair {
        validate_pair(ctx, pair)?;
        query.equals("r.base_code", pair.base_code.clone());
        query.equals("r.quote_code", pair.quote_code.clone());
    }
    if let Some(range) = range {
        let (from, to) = date_range(range)?;
        for (op, date) in [(">=", from), ("<=", to)] {
            if let Some(date) = date {
                let bound = query.bind(date);
                query.and(&format!("r.rate_date{op}{bound}"));
            }
        }
    }
    sort_cursor(&mut query, &cursor, "r.rate_date", "r.id", false, false)?;
    query.limit(cursor.limit + 1);
    let version = revision(&ctx.tx)?;
    let mut stmt = ctx.tx.prepare(&query.sql)?;
    let rows = stmt
        .query_map(params_from_iter(query.args), read_rate)?
        .map(|row| {
            let (id, draft, ms) = row?;
            Ok(FxRate {
                id,
                version: version.clone(),
                draft,
                created_at: utc_timestamp(ms)?,
            })
        })
        .collect::<Result<_>>()?;
    cursor.finish(rows, |row| (row.draft.rate_date.clone(), row.id.clone()))
}
pub fn save(
    ctx: &Context<'_>,
    target: Option<&VersionedId>,
    draft: &FxRateDraft,
) -> Result<FxRate> {
    if matches!(draft.source_kind, RateSource::Provider) {
        return Err(invalid("sourceKind", "provider_source_requires_refresh"));
    }
    save_rate(ctx, target, draft)
}
fn save_rate(
    ctx: &Context<'_>,
    target: Option<&VersionedId>,
    draft: &FxRateDraft,
) -> Result<FxRate> {
    validate_pair(ctx, &draft.pair)?;
    parse_date(&draft.rate_date)?;
    let mut draft = draft.clone();
    draft.rate_decimal = rate(&draft.rate_decimal)?.normalize().to_string();
    if draft.source_key.trim().is_empty() || draft.source_key.chars().count() > 128 {
        return Err(invalid("sourceKey", "invalid_rate_source"));
    }
    let (id, ms) = if let Some(target) = target {
        check_revision(ctx, target)?;
        let ms = ctx
            .tx
            .query_row(
                "SELECT created_at_ms FROM fx_rates WHERE id=?1",
                [&target.id],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .ok_or_else(|| missing("fxRateId"))?;
        (target.id.clone(), ms)
    } else {
        (new_id(), ctx.now_ms)
    };
    let duplicate=ctx.tx.query_row("SELECT EXISTS(SELECT 1 FROM fx_rates WHERE base_code=?1 AND quote_code=?2 AND rate_date=?3 AND source_kind=?4 AND source_key=?5 AND id<>?6)",
        params![draft.pair.base_code,draft.pair.quote_code,draft.rate_date,draft.source_kind.as_str(),draft.source_key,id],|r|r.get::<_,bool>(0))?;
    if duplicate {
        return Err(AppError::new(
            ErrorCode::Duplicate,
            "fx_rate_already_exists",
        ));
    }
    ctx.tx.execute("INSERT INTO fx_rates VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(id) DO UPDATE SET base_code=excluded.base_code,
        quote_code=excluded.quote_code,rate_date=excluded.rate_date,rate_decimal=excluded.rate_decimal,source_kind=excluded.source_kind,source_key=excluded.source_key",
        params![id,draft.pair.base_code,draft.pair.quote_code,draft.rate_date,draft.rate_decimal,draft.source_kind.as_str(),draft.source_key,ms])?;
    Ok(FxRate {
        id,
        version: accounts::next_version(ctx)?,
        draft,
        created_at: utc_timestamp(ms)?,
    })
}
pub fn delete(ctx: &Context<'_>, id: &str) -> Result<Deleted> {
    if ctx.tx.execute("DELETE FROM fx_rates WHERE id=?1", [id])? == 0 {
        return Err(missing("fxRateId"));
    }
    Ok(Deleted { id: id.to_owned() })
}

type ResolvedRate = (RateRatio, Vec<FxLeg>);
type RateCacheKey = (String, String, NaiveDate);

pub struct Resolver<'ctx, 'tx> {
    ctx: &'ctx Context<'tx>,
    scales: HashMap<String, u32>,
    rates: HashMap<RateCacheKey, Option<ResolvedRate>>,
}
impl<'ctx, 'tx> Resolver<'ctx, 'tx> {
    pub fn new(ctx: &'ctx Context<'tx>) -> Self {
        Self {
            ctx,
            scales: HashMap::new(),
            rates: HashMap::new(),
        }
    }
    fn scale(&mut self, currency: &str) -> Result<u32> {
        if let Some(value) = self.scales.get(currency) {
            return Ok(*value);
        }
        let value = scale(&self.ctx.tx, currency)?;
        self.scales.insert(currency.to_owned(), value);
        Ok(value)
    }
    pub fn convert(&mut self, amount: &Money, target: &str, date: NaiveDate) -> Result<Conversion> {
        let source_scale = self.scale(&amount.currency)?;
        let target_scale = self.scale(target)?;
        let minor = minor_i128(&amount.minor)?;
        if amount.currency == target || minor == 0 {
            return Ok(Conversion {
                native: amount.clone(),
                displayed: Some(if amount.currency == target {
                    amount.clone()
                } else {
                    money(target, 0)
                }),
                estimate: None,
                status: ConversionStatus::Exact,
                legs: Vec::new(),
            });
        }
        if !self.ctx.currency_conversion {
            return Ok(Conversion {
                native: amount.clone(),
                displayed: None,
                estimate: None,
                status: ConversionStatus::Missing,
                legs: Vec::new(),
            });
        }
        let on = date.min(self.ctx.today);
        let key = (amount.currency.clone(), target.to_owned(), on);
        if !self.rates.contains_key(&key) {
            let rate = resolve(self.ctx, &amount.currency, target, on)?;
            self.rates.insert(key.clone(), rate);
        }
        if let Some((rate, legs)) = &self.rates[&key] {
            let status = if legs.iter().all(|leg| leg.date == on.to_string()) {
                ConversionStatus::Exact
            } else {
                ConversionStatus::Prior
            };
            Ok(Conversion {
                native: amount.clone(),
                displayed: Some(rate.convert(amount, source_scale, target, target_scale)?),
                estimate: None,
                status,
                legs: legs.clone(),
            })
        } else {
            Ok(Conversion {
                native: amount.clone(),
                displayed: None,
                estimate: Some(RateRatio::new(Decimal::ONE, false).convert(
                    amount,
                    source_scale,
                    target,
                    target_scale,
                )?),
                status: ConversionStatus::Missing,
                legs: Vec::new(),
            })
        }
    }
}

fn leg(
    ctx: &Context<'_>,
    from: &str,
    to: &str,
    on: NaiveDate,
) -> Result<Option<(RateRatio, FxLeg)>> {
    let result=ctx.tx.query_row(&format!("{SELECT} AND ((base_code=?1 AND quote_code=?2) OR (base_code=?2 AND quote_code=?1)) AND rate_date<=?3
        ORDER BY rate_date DESC,CASE source_kind WHEN 'manual' THEN 0 WHEN 'csv' THEN 1 ELSE 2 END,id LIMIT 1"),
        params![from,to,on.to_string()],read_rate).optional()?;
    result
        .map(|(rate_id, draft, _)| {
            let inverted = draft.pair.base_code != from;
            let value = RateRatio::new(rate(&draft.rate_decimal)?, inverted);
            Ok((
                value,
                FxLeg {
                    rate_id,
                    date: draft.rate_date,
                    source_kind: draft.source_kind,
                    source_key: draft.source_key,
                    pair: draft.pair,
                    rate_decimal: draft.rate_decimal,
                    inverted,
                },
            ))
        })
        .transpose()
}
fn resolve(ctx: &Context<'_>, from: &str, to: &str, on: NaiveDate) -> Result<Option<ResolvedRate>> {
    if from == to {
        return Ok(Some((RateRatio::new(Decimal::ONE, false), Vec::new())));
    }
    let on = on.min(ctx.today);
    let direct = leg(ctx, from, to, on)?;
    if from == "USD"
        || to == "USD"
        || direct
            .as_ref()
            .is_some_and(|(_, leg)| leg.date == on.to_string())
    {
        return Ok(direct.map(|(value, leg)| (value, vec![leg])));
    }
    if let (Some((left, first)), Some((right, second))) =
        (leg(ctx, from, "USD", on)?, leg(ctx, "USD", to, on)?)
    {
        let oldest = first.date.as_str().min(second.date.as_str());
        if direct
            .as_ref()
            .is_none_or(|(_, leg)| oldest > leg.date.as_str())
        {
            let cross = left.multiply(right);
            return Ok(Some((cross, vec![first, second])));
        }
    }
    Ok(direct.map(|(value, leg)| (value, vec![leg])))
}

const PROVIDER_KEY: &str = "fx.frankfurter.enabled";
pub fn enabled(ctx: &Context<'_>, provider: &Provider) -> Result<ProviderStatus> {
    let enabled = ctx
        .tx
        .query_row(
            "SELECT value_json='true' FROM ledger_settings WHERE key=?1",
            [PROVIDER_KEY],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(false);
    Ok(ProviderStatus {
        provider: provider.clone(),
        enabled,
    })
}
pub fn configure(
    ctx: &Context<'_>,
    provider: &Provider,
    value: bool,
    credential: Option<&str>,
) -> Result<ProviderStatus> {
    if credential.is_some() {
        return Err(invalid("credential", "provider_does_not_use_credentials"));
    }
    ctx.tx.execute("INSERT INTO ledger_settings VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",
        params![PROVIDER_KEY,if value {"true"} else {"false"}])?;
    enabled(ctx, provider)
}
pub fn check_refresh(
    ctx: &Context<'_>,
    provider: &Provider,
    pairs: &[CurrencyPair],
    date: &str,
) -> Result<NaiveDate> {
    if !enabled(ctx, provider)?.enabled {
        return Err(conflict("fx_provider_disabled"));
    }
    if pairs.is_empty()
        || pairs.len() > 10
        || pairs.iter().collect::<HashSet<_>>().len() != pairs.len()
    {
        return Err(invalid("pairs", "invalid_fx_pair_batch"));
    }
    for pair in pairs {
        validate_pair(ctx, pair)?;
    }
    Ok(parse_date(date)?.min(ctx.today))
}

#[derive(Deserialize)]
struct ProviderRate {
    base: String,
    quote: String,
    date: String,
    rate: serde_json::Number,
}
fn offline() -> AppError {
    AppError::new(ErrorCode::Offline, "fx_provider_unavailable")
}
pub fn parse_provider_rate(
    pair: &CurrencyPair,
    date: NaiveDate,
    body: &[u8],
) -> Result<FxRateDraft> {
    let value: ProviderRate = serde_json::from_slice(body).map_err(|_| offline())?;
    if value.base != pair.base_code
        || value.quote != pair.quote_code
        || parse_date(&value.date).map_err(|_| offline())? > date
    {
        return Err(offline());
    }
    let quoted = value.rate.to_string();
    let decimal = if quoted.contains(['e', 'E']) {
        Decimal::from_scientific(&quoted).map_err(|_| offline())?
    } else {
        rate(&quoted).map_err(|_| offline())?
    };
    if decimal <= Decimal::ZERO {
        return Err(offline());
    }
    Ok(FxRateDraft {
        pair: pair.clone(),
        rate_date: value.date,
        rate_decimal: decimal.normalize().to_string(),
        source_kind: RateSource::Provider,
        source_key: "frankfurter".to_owned(),
    })
}
pub fn save_provider(
    ctx: &Context<'_>,
    provider: &Provider,
    drafts: &[FxRateDraft],
) -> Result<FxRefreshResult> {
    if !enabled(ctx, provider)?.enabled {
        return Err(conflict("fx_provider_disabled"));
    }
    let mut rates = Vec::new();
    for draft in drafts {
        let id=ctx.tx.query_row("SELECT id FROM fx_rates WHERE base_code=?1 AND quote_code=?2 AND rate_date=?3 AND source_kind='provider' AND source_key=?4",
            params![draft.pair.base_code,draft.pair.quote_code,draft.rate_date,draft.source_key],|r|r.get::<_,String>(0)).optional()?;
        let version = revision(&ctx.tx)?;
        let target = id.map(|id| VersionedId { id, version });
        rates.push(save_rate(ctx, target.as_ref(), draft)?);
    }
    Ok(FxRefreshResult { rates })
}
