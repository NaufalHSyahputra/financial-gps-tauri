use crate::{
    domain::Result,
    ledger_engine::{dto, invalid, minor},
};
use rust_decimal::{Decimal, RoundingStrategy, prelude::ToPrimitive};
use std::collections::HashSet;

dto! {
    pub struct ShareInput {pub member_id:String,pub value:Option<String>}
    pub enum Method {Exact,Percentage,Equal}
    pub struct Share {pub member_id:String,pub minor:String,pub percentage:Option<String>}
}
impl Method {
    pub(crate) fn key(&self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Percentage => "percentage",
            Self::Equal => "equal",
        }
    }
    pub(crate) fn parse(value: &str) -> crate::sql::Result<Self> {
        match value {
            "exact" => Ok(Self::Exact),
            "percentage" => Ok(Self::Percentage),
            "equal" => Ok(Self::Equal),
            _ => Err(crate::sql::Error::InvalidQuery),
        }
    }
}
pub fn allocate(total: i64, method: &Method, entries: &[ShareInput]) -> Result<Vec<Share>> {
    let error = || invalid("shares", "invalid_shared_allocations");
    if total <= 0
        || entries.is_empty()
        || entries.len() > 200
        || entries.iter().any(|entry| entry.member_id.is_empty())
        || entries
            .iter()
            .map(|entry| &entry.member_id)
            .collect::<HashSet<_>>()
            .len()
            != entries.len()
    {
        return Err(error());
    }
    let mut remaining = total;
    let mut weight_sum = Decimal::ZERO;
    let mut result = Vec::with_capacity(entries.len());
    for (position, entry) in entries.iter().enumerate() {
        let mut percentage = None;
        let amount = match method {
            Method::Exact => {
                let amount = minor(entry.value.as_deref().ok_or_else(error)?)?;
                if amount < 0 || amount > remaining {
                    return Err(error());
                }
                amount
            }
            Method::Equal | Method::Percentage => {
                let raw = if matches!(method, Method::Percentage) {
                    let text = entry.value.as_deref().ok_or_else(error)?;
                    let weight = Decimal::from_str_exact(text).map_err(|_| error())?;
                    if weight <= Decimal::ZERO
                        || weight > Decimal::ONE_HUNDRED
                        || weight.scale() > 12
                    {
                        return Err(error());
                    }
                    weight_sum += weight;
                    percentage = Some(weight.normalize().to_string());
                    Decimal::from(total) * weight / Decimal::ONE_HUNDRED
                } else {
                    if entry.value.is_some() {
                        return Err(error());
                    }
                    Decimal::from(total) / Decimal::from(entries.len())
                };
                if position + 1 == entries.len() {
                    remaining
                } else {
                    raw.round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
                        .to_i64()
                        .ok_or_else(error)?
                        .min(remaining)
                }
            }
        };
        remaining -= amount;
        result.push(Share {
            member_id: entry.member_id.clone(),
            minor: amount.to_string(),
            percentage,
        });
    }
    if remaining != 0
        || (matches!(method, Method::Percentage) && weight_sum != Decimal::ONE_HUNDRED)
    {
        return Err(error());
    }
    Ok(result)
}

/// Distribute an economic share across the original category-split proportions.
pub(crate) fn distribute(total: i64, amount: i64, parts: &[i64]) -> Vec<i64> {
    let mut remainders = Vec::with_capacity(parts.len());
    let mut assigned = 0;
    let mut result = Vec::with_capacity(parts.len());
    for (index, part) in parts.iter().enumerate() {
        let numerator = i128::from(*part) * i128::from(amount);
        let base = (numerator / i128::from(total)) as i64;
        assigned += base;
        result.push(base);
        remainders.push((numerator % i128::from(total), index));
    }
    remainders.sort_by_key(|(residue, index)| (std::cmp::Reverse(*residue), *index));
    for (_, index) in remainders.into_iter().take((amount - assigned) as usize) {
        result[index] += 1;
    }
    result
}
