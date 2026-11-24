use crate::sql::{OptionalExtension, params};
use chrono::{Datelike, NaiveDate};
use rust_decimal::{Decimal, RoundingStrategy, prelude::ToPrimitive};

use crate::{
    domain::{Result, parse_date},
    ledger_engine::{
        Context, Deleted, Money, ReceiptValue, VersionedId, accounts, check_revision, conflict,
        dto, invalid, minor, missing, money, name, new_id, revision, scale, transactions as tx,
    },
};

dto! {
    pub enum AssetKind { RealEstate, Vehicle, Valuable, Investment, Other }
    pub enum GrowthInterval { Daily, Weekly, Monthly, Yearly }
    #[serde(tag="kind",content="value")]
    pub enum GrowthChange { Percentage(String), Fixed(Money) }
    pub struct GrowthDraft {pub starts_on:String,pub interval:GrowthInterval,pub change:GrowthChange}
    pub struct AssetDraft {pub name:String,pub kind:AssetKind,pub currency:String,#[serde(default)] pub growth:Vec<GrowthDraft>}
    pub struct Asset {pub id:String,pub version:String,pub draft:AssetDraft,pub archived_on:Option<String>,pub value:Money,
        pub valued_on:Option<String>,pub valuation_source:Option<String>,pub holding:Holding,pub unrealized:Money}
    pub struct ValuationDraft {pub asset_id:String,pub valued_on:String,pub value:Money}
    pub struct Valuation {pub id:String,pub version:String,pub draft:ValuationDraft,pub source:String}
    #[serde(tag="kind",rename_all_fields="camelCase")]
    pub enum TradeKind { Buy{units:String,gross:Money,fees:Money},Sell{units:String,gross:Money,fees:Money},Split{old_units:String,new_units:String} }
    pub struct TradeDraft {pub asset_id:String,pub traded_on:String,pub trade:TradeKind,pub linked_transaction_id:Option<String>}
    pub struct Trade {pub id:String,pub version:String,pub created_at:String,pub draft:TradeDraft}
    pub struct Holding {pub units:String,pub cost_basis:Money,pub realized:Money}
    pub struct AssetGroupDraft {pub name:String,#[serde(default)] pub archived:bool}
    pub struct AssetGroup {pub id:String,pub version:String,pub draft:AssetGroupDraft,pub asset_ids:Vec<String>}
    pub struct AssetGroupDetail {pub group:AssetGroup,pub as_of:String,pub value:Money,pub native:Vec<Money>,
        pub prior_rates:String,pub missing_rates:String}
}
macro_rules! values {
    ($ty:ident;$($v:ident=>$tag:literal),+) => {
        impl $ty{
            fn as_str(&self)->&'static str {match self {$(Self::$v=>$tag),+}}
            fn read(s:&str)->crate::sql::Result<Self>{match s {$($tag=>Ok(Self::$v)),+,_=>Err(crate::sql::Error::InvalidQuery)}}
        }
    };
}
values!(AssetKind;RealEstate=>"real_estate",Vehicle=>"vehicle",Valuable=>"valuable",Investment=>"investment",Other=>"other");
values!(GrowthInterval;Daily=>"daily",Weekly=>"weekly",Monthly=>"monthly",Yearly=>"yearly");
fn overflow() -> crate::domain::AppError {
    invalid("asset", "asset_decimal_overflow")
}
fn decimal(value: &str) -> Result<Decimal> {
    if value.len() > 40 {
        return Err(overflow());
    }
    Decimal::from_str_exact(value).map_err(|_| overflow())
}
fn units(value: &str) -> Result<Decimal> {
    let value = decimal(value)?;
    if value <= Decimal::ZERO {
        return Err(invalid("units", "positive_units_required"));
    }
    Ok(value)
}
fn round(value: Decimal) -> Result<i128> {
    value
        .round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
        .to_i128()
        .ok_or_else(overflow)
}
fn native(value: &Money, code: &str) -> Result<i64> {
    if value.currency != code {
        return Err(invalid("currency", "currency_mismatch"));
    }
    minor(&value.minor)
}
pub fn replay(currency: &str, trades: &[Trade], as_of: NaiveDate) -> Result<Holding> {
    let mut ordered = Vec::new();
    for trade in trades {
        let day = parse_date(&trade.draft.traded_on)?;
        if day <= as_of {
            let rank = if matches!(trade.draft.trade, TradeKind::Split { .. }) {
                0
            } else {
                1
            };
            ordered.push((day, rank, trade));
        }
    }
    ordered.sort_by(|(day, rank, a), (other_day, other_rank, b)| {
        (day, rank, &a.created_at, &a.id).cmp(&(other_day, other_rank, &b.created_at, &b.id))
    });
    let (mut quantity, mut cost, mut realized) = (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
    for (_, _, trade) in ordered {
        match &trade.draft.trade {
            TradeKind::Buy {
                units: amount,
                gross,
                fees,
            } => {
                let gross = native(gross, currency)?;
                let fees = native(fees, currency)?;
                if gross <= 0 || fees < 0 {
                    return Err(invalid("trade", "invalid_asset_trade_amount"));
                }
                quantity = quantity.checked_add(units(amount)?).ok_or_else(overflow)?;
                cost = cost
                    .checked_add(Decimal::from(gross))
                    .and_then(|value| value.checked_add(Decimal::from(fees)))
                    .ok_or_else(overflow)?;
            }
            TradeKind::Sell {
                units: amount,
                gross,
                fees,
            } => {
                let sold = units(amount)?;
                let gross = native(gross, currency)?;
                let fees = native(fees, currency)?;
                if gross <= 0 || fees < 0 || sold > quantity {
                    return Err(invalid("trade", "invalid_asset_sale"));
                }
                let removed = if sold == quantity {
                    cost
                } else {
                    sold.checked_div(quantity)
                        .and_then(|ratio| cost.checked_mul(ratio))
                        .ok_or_else(overflow)?
                };
                cost = cost.checked_sub(removed).ok_or_else(overflow)?;
                quantity = quantity.checked_sub(sold).ok_or_else(overflow)?;
                realized = realized
                    .checked_add(Decimal::from(gross))
                    .and_then(|value| value.checked_sub(Decimal::from(fees)))
                    .and_then(|value| value.checked_sub(removed))
                    .ok_or_else(overflow)?;
            }
            TradeKind::Split {
                old_units,
                new_units,
            } => {
                let old = minor(old_units)?;
                let new = minor(new_units)?;
                if old <= 0 || new <= 0 || quantity == Decimal::ZERO {
                    return Err(invalid("trade", "invalid_asset_split"));
                }
                quantity = quantity
                    .checked_mul(Decimal::from(new))
                    .and_then(|value| value.checked_div(Decimal::from(old)))
                    .ok_or_else(overflow)?;
                if quantity <= Decimal::ZERO {
                    return Err(overflow());
                }
            }
        }
    }
    Ok(Holding {
        units: quantity.normalize().to_string(),
        cost_basis: money(currency, round(cost)?),
        realized: money(currency, round(realized)?),
    })
}
fn completed(start: NaiveDate, end: NaiveDate, interval: &GrowthInterval) -> Result<u32> {
    if end <= start {
        return Ok(0);
    }
    let count = match interval {
        GrowthInterval::Daily => (end - start).num_days(),
        GrowthInterval::Weekly => (end - start).num_days() / 7,
        GrowthInterval::Monthly | GrowthInterval::Yearly => {
            let step = if matches!(interval, GrowthInterval::Yearly) {
                12
            } else {
                1
            };
            let mut cycles = ((end.year() - start.year()) * 12 + end.month() as i32
                - start.month() as i32)
                / step;
            if accounts::month_shift(start, cycles * step, start.day())? > end {
                cycles -= 1;
            }
            i64::from(cycles)
        }
    };
    u32::try_from(count).map_err(|_| overflow())
}
fn compound(mut base: Decimal, mut count: u32) -> Result<Decimal> {
    let mut total = Decimal::ONE;
    while count > 0 {
        if !count.is_multiple_of(2) {
            total = total.checked_mul(base).ok_or_else(overflow)?;
        }
        count /= 2;
        if count > 0 {
            base = base.checked_mul(base).ok_or_else(overflow)?;
        }
    }
    Ok(total)
}
pub fn project_value(
    start: NaiveDate,
    end: NaiveDate,
    amount: &Money,
    growth: &[GrowthDraft],
) -> Result<Money> {
    let mut balance = Decimal::from(accounts::nonnegative(&amount.minor)?);
    let mut rules = growth
        .iter()
        .map(|rule| Ok((parse_date(&rule.starts_on)?, rule)))
        .collect::<Result<Vec<_>>>()?;
    rules.sort_by_key(|(date, _)| *date);
    for (index, (date, rule)) in rules.iter().enumerate() {
        if *date > end {
            break;
        }
        let next = rules.get(index + 1).map_or(end, |(date, _)| end.min(*date));
        if next <= start {
            continue;
        }
        let anchor = (*date).max(start);
        let periods = completed(anchor, next, &rule.interval)?;
        match &rule.change {
            GrowthChange::Fixed(delta) => {
                let delta = Decimal::from(native(delta, &amount.currency)?);
                balance = delta
                    .checked_mul(Decimal::from(periods))
                    .and_then(|delta| balance.checked_add(delta))
                    .ok_or_else(overflow)?
                    .max(Decimal::ZERO);
            }
            GrowthChange::Percentage(percent) => {
                let percent = decimal(percent)?;
                if percent < Decimal::from(-100) {
                    return Err(invalid("growth", "invalid_growth_rate"));
                }
                let rate = Decimal::ONE
                    .checked_add(percent / Decimal::from(100))
                    .ok_or_else(overflow)?;
                balance = balance
                    .checked_mul(compound(rate, periods)?)
                    .ok_or_else(overflow)?;
            }
        }
    }
    Ok(money(&amount.currency, round(balance)?))
}

fn base(ctx: &Context<'_>, id: &str) -> Result<(AssetDraft, Option<String>)> {
    let (mut draft, archived) = ctx
        .tx
        .query_row(
            "SELECT name,kind,currency_code,archived_on FROM assets WHERE id=?1",
            [id],
            |row| {
                Ok((
                    AssetDraft {
                        name: row.get(0)?,
                        kind: AssetKind::read(row.get_ref(1)?.as_str()?)?,
                        currency: row.get(2)?,
                        growth: Vec::new(),
                    },
                    row.get(3)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| missing("assetId"))?;
    draft.growth = ctx
        .tx
        .prepare_cached(
            "SELECT starts_on,interval_kind,growth_kind,percent_decimal,CAST(delta_minor AS TEXT)
        FROM asset_growth_rules WHERE asset_id=?1 ORDER BY starts_on",
        )?
        .query_map([id], |row| {
            let change = if row.get_ref(2)?.as_str()? == "percentage" {
                GrowthChange::Percentage(row.get(3)?)
            } else {
                GrowthChange::Fixed(money(&draft.currency, row.get::<_, String>(4)?))
            };
            Ok(GrowthDraft {
                starts_on: row.get(0)?,
                interval: GrowthInterval::read(row.get_ref(1)?.as_str()?)?,
                change,
            })
        })?
        .collect::<crate::sql::Result<_>>()?;
    Ok((draft, archived))
}

fn trade_from(row: &crate::sql::Row<'_>, version: &str) -> crate::sql::Result<Trade> {
    let currency: String = row.get(10)?;
    let gross = || Ok::<Money, crate::sql::Error>(money(&currency, row.get::<_, String>(5)?));
    let fees = || Ok::<Money, crate::sql::Error>(money(&currency, row.get::<_, String>(6)?));
    let trade = match row.get_ref(3)?.as_str()? {
        "buy" => TradeKind::Buy {
            units: row.get(4)?,
            gross: gross()?,
            fees: fees()?,
        },
        "sell" => TradeKind::Sell {
            units: row.get(4)?,
            gross: gross()?,
            fees: fees()?,
        },
        "split" => TradeKind::Split {
            old_units: row.get(7)?,
            new_units: row.get(8)?,
        },
        _ => return Err(crate::sql::Error::InvalidQuery),
    };
    let ms: i64 = row.get(11)?;
    let dt = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .ok_or(crate::sql::Error::InvalidQuery)?;
    Ok(Trade {
        id: row.get(0)?,
        version: version.to_owned(),
        created_at: dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        draft: TradeDraft {
            asset_id: row.get(1)?,
            traded_on: row.get(2)?,
            trade,
            linked_transaction_id: row.get(9)?,
        },
    })
}
const TRADE:&str="SELECT t.id,t.asset_id,t.trade_date,t.kind,t.units_decimal,CAST(t.gross_minor AS TEXT),CAST(t.fees_minor AS TEXT),
    CAST(t.split_old_units AS TEXT),CAST(t.split_new_units AS TEXT),t.linked_ledger_tx_id,a.currency_code,t.created_at_ms
    FROM asset_transactions t JOIN assets a ON a.id=t.asset_id";
pub fn trades(ctx: &Context<'_>, id: &str) -> Result<Vec<Trade>> {
    let version = revision(&ctx.tx)?;
    Ok(ctx
        .tx
        .prepare(&format!(
            "{TRADE} WHERE t.asset_id=?1 ORDER BY t.trade_date,t.id"
        ))?
        .query_map([id], |row| trade_from(row, &version))?
        .collect::<crate::sql::Result<_>>()?)
}
pub(crate) fn trade_get(ctx: &Context<'_>, id: &str) -> Result<Trade> {
    let version = revision(&ctx.tx)?;
    ctx.tx
        .query_row(&format!("{TRADE} WHERE t.id=?1"), [id], |row| {
            trade_from(row, &version)
        })
        .optional()?
        .ok_or_else(|| missing("tradeId"))
}
pub fn get(ctx: &Context<'_>, id: &str, as_of: Option<&str>) -> Result<Asset> {
    let (draft, archived_on) = base(ctx, id)?;
    let as_of = as_of.map(parse_date).transpose()?.unwrap_or(ctx.today);
    let value=ctx.tx.query_row("SELECT valued_on,CAST(value_minor AS TEXT),source FROM asset_valuations WHERE asset_id=?1 AND valued_on<=?2
        ORDER BY valued_on DESC LIMIT 1",params![id,as_of.to_string()],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?))).optional()?;
    let (value, valued_on, source) = if let Some((date, amount, source)) = value {
        (
            project_value(
                parse_date(&date)?,
                as_of,
                &money(&draft.currency, amount),
                &draft.growth,
            )?,
            Some(date),
            Some(source),
        )
    } else {
        (money(&draft.currency, 0), None, None)
    };
    let holding = replay(&draft.currency, &trades(ctx, id)?, as_of)?;
    let unrealized = money(
        &draft.currency,
        crate::ledger_engine::currency::minor_i128(&value.minor)?
            .checked_sub(crate::ledger_engine::currency::minor_i128(
                &holding.cost_basis.minor,
            )?)
            .ok_or_else(overflow)?,
    );
    Ok(Asset {
        id: id.to_owned(),
        version: revision(&ctx.tx)?,
        draft,
        archived_on,
        value,
        valued_on,
        valuation_source: source,
        holding,
        unrealized,
    })
}
pub fn save(ctx: &Context<'_>, target: Option<&VersionedId>, draft: &AssetDraft) -> Result<Asset> {
    let label = name(&draft.name)?;
    scale(&ctx.tx, &draft.currency)?;
    if draft.growth.len() > 100 {
        return Err(invalid("growth", "asset_growth_rule_limit"));
    }
    let id = if let Some(target) = target {
        check_revision(ctx, target)?;
        if base(ctx, &target.id)?.0.currency != draft.currency {
            return Err(conflict("asset_currency_immutable"));
        }
        target.id.clone()
    } else {
        new_id()
    };
    let mode = if draft.growth.is_empty() {
        "manual"
    } else {
        "growth"
    };
    ctx.tx.execute(
        "INSERT INTO assets VALUES(?1,?2,?3,?4,?5,NULL) ON CONFLICT(id) DO UPDATE SET
        name=excluded.name,kind=excluded.kind,valuation_mode=excluded.valuation_mode",
        params![id, label, draft.kind.as_str(), draft.currency, mode],
    )?;
    ctx.tx
        .execute("DELETE FROM asset_growth_rules WHERE asset_id=?1", [&id])?;
    for rule in &draft.growth {
        parse_date(&rule.starts_on)?;
        let (kind, rate, delta) = match &rule.change {
            GrowthChange::Fixed(value) => ("fixed", None, Some(native(value, &draft.currency)?)),
            GrowthChange::Percentage(value) => {
                let rate = decimal(value)?;
                if rate < Decimal::from(-100) {
                    return Err(invalid("growth", "invalid_growth_rate"));
                }
                ("percentage", Some(rate.normalize().to_string()), None)
            }
        };
        ctx.tx.execute(
            "INSERT INTO asset_growth_rules VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                id,
                rule.starts_on,
                rule.interval.as_str(),
                kind,
                rate,
                delta
            ],
        )?;
    }
    changed(ctx, &id)
}
fn changed(ctx: &Context<'_>, id: &str) -> Result<Asset> {
    let mut asset = get(ctx, id, None)?;
    asset.version = accounts::next_version(ctx)?;
    Ok(asset)
}
pub fn archive(ctx: &Context<'_>, id: &str, date: Option<&str>) -> Result<Asset> {
    let day = date
        .map(parse_date)
        .transpose()?
        .unwrap_or(ctx.today)
        .to_string();
    if ctx.tx.execute(
        "UPDATE assets SET archived_on=?2 WHERE id=?1",
        params![id, day],
    )? == 0
    {
        return Err(missing("assetId"));
    }
    changed(ctx, id)
}
pub fn delete(ctx: &Context<'_>, id: &str, confirmed: bool) -> Result<Deleted> {
    if !confirmed {
        return Err(invalid("confirm", "confirmation_required"));
    }
    base(ctx, id)?;
    let blocked:bool=ctx.tx.query_row("SELECT EXISTS(SELECT 1 FROM goals WHERE asset_id=?1) OR
        EXISTS(SELECT 1 FROM asset_transactions WHERE asset_id=?1 AND linked_ledger_tx_id IS NOT NULL)",[id],|row|row.get(0))?;
    if blocked {
        return Err(conflict("asset_has_dependencies"));
    }
    for table in [
        "asset_group_members",
        "asset_growth_rules",
        "asset_valuations",
        "asset_transactions",
    ] {
        ctx.tx
            .execute(&format!("DELETE FROM {table} WHERE asset_id=?1"), [id])?;
    }
    ctx.tx.execute("DELETE FROM assets WHERE id=?1", [id])?;
    Ok(Deleted { id: id.to_owned() })
}

fn valuation_from(row: &crate::sql::Row<'_>, version: &str) -> crate::sql::Result<Valuation> {
    Ok(Valuation {
        id: row.get(0)?,
        version: version.to_owned(),
        source: row.get(5)?,
        draft: ValuationDraft {
            asset_id: row.get(1)?,
            valued_on: row.get(2)?,
            value: Money {
                currency: row.get(3)?,
                minor: row.get(4)?,
            },
        },
    })
}
const VALUATIONS: &str = "SELECT v.id,v.asset_id,v.valued_on,a.currency_code,CAST(v.value_minor AS TEXT),v.source FROM asset_valuations v JOIN assets a ON a.id=v.asset_id";
pub fn valuations(ctx: &Context<'_>, id: &str) -> Result<Vec<Valuation>> {
    let version = revision(&ctx.tx)?;
    Ok(ctx
        .tx
        .prepare(&format!(
            "{VALUATIONS} WHERE v.asset_id=?1 ORDER BY v.valued_on,v.id"
        ))?
        .query_map([id], |row| valuation_from(row, &version))?
        .collect::<crate::sql::Result<_>>()?)
}
pub(crate) fn valuation_get(ctx: &Context<'_>, id: &str) -> Result<Valuation> {
    let version = revision(&ctx.tx)?;
    ctx.tx
        .query_row(&format!("{VALUATIONS} WHERE v.id=?1"), [id], |row| {
            valuation_from(row, &version)
        })
        .optional()?
        .ok_or_else(|| missing("valuationId"))
}
pub fn valuation_save(
    ctx: &Context<'_>,
    target: Option<&VersionedId>,
    draft: &ValuationDraft,
) -> Result<Valuation> {
    let (asset, archived) = base(ctx, &draft.asset_id)?;
    parse_date(&draft.valued_on)?;
    let magnitude = native(&draft.value, &asset.currency)?;
    if magnitude < 0 || (target.is_none() && archived.is_some()) {
        return Err(invalid("value", "invalid_asset_valuation"));
    }
    let id = if let Some(target) = target {
        check_revision(ctx, target)?;
        if valuation_get(ctx, &target.id)?.draft.asset_id != draft.asset_id {
            return Err(conflict("valuation_asset_immutable"));
        }
        target.id.clone()
    } else {
        new_id()
    };
    ctx.tx.execute(
        "INSERT INTO asset_valuations VALUES(?1,?2,?3,?4,'manual',?5) ON CONFLICT(id) DO UPDATE SET
        valued_on=excluded.valued_on,value_minor=excluded.value_minor",
        params![id, draft.asset_id, draft.valued_on, magnitude, ctx.now_ms],
    )?;
    let mut row = valuation_get(ctx, &id)?;
    row.version = accounts::next_version(ctx)?;
    Ok(row)
}
pub fn valuation_delete(ctx: &Context<'_>, id: &str, confirmed: bool) -> Result<Deleted> {
    if !confirmed {
        return Err(invalid("confirm", "confirmation_required"));
    }
    if ctx
        .tx
        .execute("DELETE FROM asset_valuations WHERE id=?1", [id])?
        == 0
    {
        return Err(missing("valuationId"));
    }
    Ok(Deleted { id: id.to_owned() })
}
pub fn trade_save(
    ctx: &Context<'_>,
    target: Option<&VersionedId>,
    draft: &TradeDraft,
) -> Result<Trade> {
    let (asset, archived) = base(ctx, &draft.asset_id)?;
    parse_date(&draft.traded_on)?;
    if target.is_none() && archived.is_some() {
        return Err(conflict("asset_archived"));
    }
    let id = if let Some(target) = target {
        check_revision(ctx, target)?;
        if trade_get(ctx, &target.id)?.draft.asset_id != draft.asset_id {
            return Err(conflict("trade_asset_immutable"));
        }
        target.id.clone()
    } else {
        new_id()
    };
    let (kind, units, gross, fees, old, new) = match &draft.trade {
        TradeKind::Buy { units, gross, fees } | TradeKind::Sell { units, gross, fees } => {
            let kind = if matches!(&draft.trade, TradeKind::Buy { .. }) {
                "buy"
            } else {
                "sell"
            };
            (
                kind,
                Some(self::units(units)?.normalize().to_string()),
                Some(native(gross, &asset.currency)?),
                native(fees, &asset.currency)?,
                None,
                None,
            )
        }
        TradeKind::Split {
            old_units,
            new_units,
        } => (
            "split",
            None,
            None,
            0,
            Some(minor(old_units)?),
            Some(minor(new_units)?),
        ),
    };
    if let Some(link) = &draft.linked_transaction_id {
        let posting = tx::get(ctx, link)?;
        let expected = match kind {
            "buy" => gross.and_then(|gross| gross.checked_add(fees)),
            "sell" => gross.and_then(|gross| gross.checked_sub(fees)),
            _ => None,
        };
        if expected != Some(minor(&posting.row.amount.minor)?)
            || posting.row.amount.currency != asset.currency
            || posting.row.direction.as_str() != if kind == "buy" { "debit" } else { "credit" }
            || posting.row.status.as_str() != "posted"
            || posting.row.ignored
        {
            return Err(invalid("linkedTransactionId", "asset_cash_leg_mismatch"));
        }
    }
    ctx.tx.execute("INSERT INTO asset_transactions VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(id) DO UPDATE SET
        trade_date=excluded.trade_date,kind=excluded.kind,units_decimal=excluded.units_decimal,gross_minor=excluded.gross_minor,
        fees_minor=excluded.fees_minor,split_old_units=excluded.split_old_units,split_new_units=excluded.split_new_units,
        linked_ledger_tx_id=excluded.linked_ledger_tx_id",params![id,draft.asset_id,draft.traded_on,kind,units,gross,fees,old,new,draft.linked_transaction_id,ctx.now_ms])?;
    replay(
        &asset.currency,
        &trades(ctx, &draft.asset_id)?,
        parse_date("9999-12-31")?,
    )?;
    let mut row = trade_get(ctx, &id)?;
    row.version = accounts::next_version(ctx)?;
    Ok(row)
}
pub fn trade_delete(ctx: &Context<'_>, id: &str, confirmed: bool) -> Result<Deleted> {
    if !confirmed {
        return Err(invalid("confirm", "confirmation_required"));
    }
    let trade = trade_get(ctx, id)?;
    let asset = base(ctx, &trade.draft.asset_id)?.0;
    ctx.tx
        .execute("DELETE FROM asset_transactions WHERE id=?1", [id])?;
    replay(
        &asset.currency,
        &trades(ctx, &trade.draft.asset_id)?,
        parse_date("9999-12-31")?,
    )?;
    Ok(Deleted { id: id.to_owned() })
}
pub fn group_get(ctx: &Context<'_>, id: &str) -> Result<AssetGroup> {
    let draft = ctx
        .tx
        .query_row(
            "SELECT name,archived FROM asset_groups WHERE id=?1",
            [id],
            |row| {
                Ok(AssetGroupDraft {
                    name: row.get(0)?,
                    archived: row.get(1)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| missing("groupId"))?;
    let asset_ids = accounts::strings(
        &ctx.tx,
        "SELECT asset_id FROM asset_group_members WHERE group_id=?1 ORDER BY asset_id",
        id,
    )?;
    Ok(AssetGroup {
        id: id.to_owned(),
        version: revision(&ctx.tx)?,
        draft,
        asset_ids,
    })
}
pub fn group_detail(
    ctx: &Context<'_>,
    group: AssetGroup,
    as_of: Option<&str>,
) -> Result<AssetGroupDetail> {
    use crate::ledger_engine::currency::{ConversionStatus, Resolver, minor_i128};
    let date = as_of.map(parse_date).transpose()?.unwrap_or(ctx.today);
    let as_of = date.to_string();
    let target = accounts::settings(ctx)?.display_currency;
    let mut currencies = std::collections::BTreeMap::<String, i128>::new();
    let mut value = 0i128;
    let mut prior = 0u64;
    let mut missing = 0u64;
    let mut rates = Resolver::new(ctx);
    for id in &group.asset_ids {
        let asset = get(ctx, id, Some(&as_of))?;
        if asset
            .archived_on
            .as_ref()
            .is_some_and(|archived| archived <= &as_of)
        {
            continue;
        }
        if asset.valued_on.is_none() {
            continue;
        }
        *currencies.entry(asset.value.currency.clone()).or_default() +=
            minor_i128(&asset.value.minor)?;
        let result = rates.convert(&asset.value, &target, date)?;
        if let Some(amount) = result.displayed {
            value += minor_i128(&amount.minor)?;
        }
        prior += u64::from(result.status == ConversionStatus::Prior);
        missing += u64::from(result.status == ConversionStatus::Missing);
    }
    let native = currencies
        .into_iter()
        .map(|(code, minor)| money(&code, minor))
        .collect();
    Ok(AssetGroupDetail {
        group,
        as_of,
        value: money(&target, value),
        native,
        prior_rates: prior.to_string(),
        missing_rates: missing.to_string(),
    })
}
impl ReceiptValue for AssetGroupDetail {
    type Reference = (String, String);
    fn reference(&self, _: &Context<'_>) -> Self::Reference {
        (self.group.id.clone(), self.as_of.clone())
    }
    fn reload(ctx: &Context<'_>, (id, date): Self::Reference) -> Result<Self> {
        group_detail(ctx, group_get(ctx, &id)?, Some(&date))
    }
}
pub fn group_save(
    ctx: &Context<'_>,
    target: Option<&VersionedId>,
    draft: &AssetGroupDraft,
    asset_ids: &[String],
) -> Result<AssetGroup> {
    let name = name(&draft.name)?;
    let ids = asset_ids.iter().collect::<std::collections::BTreeSet<_>>();
    if ids.len() != asset_ids.len() || ids.len() > 1000 {
        return Err(invalid("assetIds", "invalid_asset_group_members"));
    }
    let id = if let Some(target) = target {
        check_revision(ctx, target)?;
        group_get(ctx, &target.id)?;
        target.id.clone()
    } else {
        new_id()
    };
    ctx.tx.execute("INSERT INTO asset_groups VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET name=excluded.name,archived=excluded.archived",
        params![id,name,draft.archived])?;
    ctx.tx
        .execute("DELETE FROM asset_group_members WHERE group_id=?1", [&id])?;
    for member in ids {
        base(ctx, member)?;
        ctx.tx.execute(
            "INSERT INTO asset_group_members VALUES(?1,?2)",
            params![id, member],
        )?;
    }
    let mut group = group_get(ctx, &id)?;
    group.version = accounts::next_version(ctx)?;
    Ok(group)
}
pub fn group_delete(ctx: &Context<'_>, id: &str, confirmed: bool) -> Result<Deleted> {
    if !confirmed {
        return Err(invalid("confirm", "confirmation_required"));
    }
    let linked: bool = ctx.tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM goals WHERE asset_group_id=?1)",
        [id],
        |row| row.get(0),
    )?;
    if linked {
        return Err(conflict("asset_group_has_goals"));
    }
    if ctx
        .tx
        .execute("DELETE FROM asset_groups WHERE id=?1", [id])?
        == 0
    {
        return Err(missing("groupId"));
    }
    Ok(Deleted { id: id.to_owned() })
}
macro_rules! receipt {
    ($ty:ty,$function:path) => {
        impl ReceiptValue for $ty {
            type Reference = String;
            fn reference(&self, _: &Context<'_>) -> String {
                self.id.clone()
            }
            fn reload(ctx: &Context<'_>, id: String) -> Result<Self> {
                $function(ctx, &id)
            }
        }
    };
}
receipt!(AssetGroup, group_get);
receipt!(Trade, trade_get);
receipt!(Valuation, valuation_get);
impl ReceiptValue for Asset {
    type Reference = (String, String);
    fn reference(&self, ctx: &Context<'_>) -> Self::Reference {
        (self.id.clone(), ctx.today.to_string())
    }
    fn reload(ctx: &Context<'_>, (id, date): Self::Reference) -> Result<Self> {
        get(ctx, &id, Some(&date))
    }
}

dto! {
    pub enum HistoryView { Valuations, Trades }
    #[serde(tag="kind",content="value")]
    pub enum HistoryItem { Valuation(Valuation), Trade(Trade) }
}
use crate::ledger_engine::{DateRange, Page, PageReq, ReadPage, date_range};

pub fn list(
    ctx: &Context<'_>,
    group: Option<&str>,
    as_of: Option<&str>,
    page: &PageReq,
) -> Result<Page<Asset>> {
    let cursor = ReadPage::new(ctx, page, &("assets_list", group, as_of))?;
    if let Some(group) = group {
        group_get(ctx, group)?;
    }
    let after = cursor.after.as_ref().map(|(_, id)| id);
    let ids=ctx.tx.prepare_cached("SELECT a.id FROM assets a WHERE (?1 IS NULL OR a.id>?1) AND
        (?2 IS NULL OR EXISTS(SELECT 1 FROM asset_group_members m WHERE m.asset_id=a.id AND m.group_id=?2))
        ORDER BY a.id LIMIT ?3")?.query_map(params![after,group,(cursor.limit+1) as i64],|row|row.get::<_,String>(0))?
        .collect::<crate::sql::Result<Vec<_>>>()?;
    cursor.finish(
        ids.iter()
            .map(|id| get(ctx, id, as_of))
            .collect::<Result<Vec<_>>>()?,
        |row| (row.id.clone(), row.id.clone()),
    )
}

pub fn groups(ctx: &Context<'_>, page: &PageReq) -> Result<Page<AssetGroup>> {
    let cursor = ReadPage::new(ctx, page, &"asset_groups_list")?;
    let after = cursor.after.as_ref().map(|(_, id)| id);
    let ids = ctx
        .tx
        .prepare_cached(
            "SELECT id FROM asset_groups WHERE (?1 IS NULL OR id>?1) ORDER BY id LIMIT ?2",
        )?
        .query_map(params![after, (cursor.limit + 1) as i64], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<crate::sql::Result<Vec<_>>>()?;
    cursor.finish(
        ids.iter()
            .map(|id| group_get(ctx, id))
            .collect::<Result<Vec<_>>>()?,
        |row| (row.id.clone(), row.id.clone()),
    )
}
impl HistoryItem {
    fn key(&self) -> (&str, &str) {
        match self {
            Self::Valuation(row) => (&row.draft.valued_on, &row.id),
            Self::Trade(row) => (&row.draft.traded_on, &row.id),
        }
    }
}
pub fn history(
    ctx: &Context<'_>,
    id: &str,
    range: &DateRange,
    page: &PageReq,
    view: &HistoryView,
) -> Result<Page<HistoryItem>> {
    base(ctx, id)?;
    let (from, to) = date_range(range)?;
    let cursor = ReadPage::new(ctx, page, &("asset_history", id, range, view))?;
    let (table, date) = match view {
        HistoryView::Valuations => ("asset_valuations", "valued_on"),
        HistoryView::Trades => ("asset_transactions", "traded_on"),
    };
    let after = cursor
        .after
        .as_ref()
        .map(|(date, id)| (date.as_str(), id.as_str()));
    let ids=ctx.tx.prepare(&format!("SELECT id FROM {table} WHERE asset_id=?1 AND (?2 IS NULL OR {date}>=?2)
        AND (?3 IS NULL OR {date}<=?3) AND (?4 IS NULL OR ({date},id)>(?4,?5)) ORDER BY {date},id LIMIT ?6"))?
        .query_map(params![id,from,to,after.map(|a|a.0),after.map(|a|a.1),(cursor.limit+1) as i64],|row|row.get::<_,String>(0))?
        .collect::<crate::sql::Result<Vec<_>>>()?;
    let rows = ids
        .iter()
        .map(|id| match view {
            HistoryView::Valuations => valuation_get(ctx, id).map(HistoryItem::Valuation),
            HistoryView::Trades => trade_get(ctx, id).map(HistoryItem::Trade),
        })
        .collect::<Result<Vec<_>>>()?;
    cursor.finish(rows, |row| {
        let (date, id) = row.key();
        (date.to_owned(), id.to_owned())
    })
}
pub fn valuation_owner(ctx: &Context<'_>, id: &str) -> Result<String> {
    Ok(valuation_get(ctx, id)?.draft.asset_id)
}
pub fn trade_owner(ctx: &Context<'_>, id: &str) -> Result<String> {
    Ok(trade_get(ctx, id)?.draft.asset_id)
}
