use crate::sql::{OptionalExtension, params};
use chrono::NaiveDate;
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive};
use rust_decimal::Decimal;
use std::collections::HashMap;

use super::*;

dto! {
    #[derive(PartialEq, Eq, Hash)]
    pub struct CurrencyPair { pub base_code: String, pub quote_code: String }
    pub enum RateSource { Manual, Csv, Provider }
    pub struct Currency { pub code: String, pub scale: u8, pub name: String }
    pub struct FxLeg { pub rate_id: String, pub date: String, pub source_kind: RateSource, pub source_key: String, pub pair: CurrencyPair, pub rate_decimal: String, pub inverted: bool }
    #[derive(PartialEq, Eq)]
    pub enum ConversionStatus { Exact, Prior, Missing }
    pub struct Conversion {
        pub native: Money, pub displayed: Option<Money>, pub estimate: Option<Money>, pub status: ConversionStatus, pub legs: Vec<FxLeg>,
    }
}

impl RateSource {
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
const SELECT: &str = "SELECT id,base_code,quote_code,rate_date,rate_decimal,source_kind,source_key FROM fx_rates WHERE 1=1";
fn read_leg(row: &crate::sql::Row<'_>) -> crate::sql::Result<FxLeg> {
    Ok(FxLeg {
        rate_id: row.get(0)?,
        pair: CurrencyPair {
            base_code: row.get(1)?,
            quote_code: row.get(2)?,
        },
        date: row.get(3)?,
        rate_decimal: row.get(4)?,
        source_kind: RateSource::read(row.get_ref(5)?.as_str()?)?,
        source_key: row.get(6)?,
        inverted: false,
    })
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
        params![from,to,on.to_string()],read_leg).optional()?;
    result
        .map(|mut leg| {
            leg.inverted = leg.pair.base_code != from;
            let value = RateRatio::new(rate(&leg.rate_decimal)?, leg.inverted);
            Ok((value, leg))
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
