use super::assets;
use crate::sql::{OptionalExtension, params};
use crate::{
    domain::{Result, parse_date},
    ledger_engine::{
        self as e,
        fx::{self, ConversionStatus, Resolver},
        *,
    },
};
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;

dto! {
    #[serde(tag="kind",rename_all_fields="camelCase")]
    pub enum Source {Manual{current:Money},Account{account_id:String},Asset{asset_id:String},AssetGroup{group_id:String},NetWorth}
    pub enum StoredState {Active,Completed,Paused,Archived}
    pub enum State {Active,Completed,Paused,Archived,Overdue}
    pub enum Pace {Behind,OnTrack,Ahead}
    pub struct GoalDraft {pub name:String,pub target:Money,pub starts_on:String,pub target_on:String,pub source:Source,pub state:StoredState}
    pub struct Goal {pub id:String,pub version:String,pub draft:GoalDraft,pub as_of:String,pub current:Money,pub initial:Money,
        pub monthly_contribution:Money,pub progress_percent:String,pub state:State,pub pace:Option<Pace>,
        pub native:Vec<Money>,pub missing_rates:String,pub prior_rates:String}
}
impl StoredState {
    fn key(&self) -> &str {
        match self {
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Paused => "paused",
            Self::Archived => "archived",
        }
    }
    fn parse(key: &str) -> crate::sql::Result<Self> {
        match key {
            "active" => Ok(Self::Active),
            "completed" => Ok(Self::Completed),
            "paused" => Ok(Self::Paused),
            "archived" => Ok(Self::Archived),
            _ => Err(crate::sql::Error::InvalidQuery),
        }
    }
}
impl State {
    fn key(&self) -> &str {
        match self {
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Paused => "paused",
            Self::Archived => "archived",
            Self::Overdue => "overdue",
        }
    }
}
fn load(ctx: &Context<'_>, id: &str) -> Result<(GoalDraft, i64)> {
    ctx.tx.query_row("SELECT name,target_minor,initial_minor,currency_code,starts_on,target_on,source,manual_minor,account_id,asset_id,asset_group_id,state
        FROM goals WHERE id=?1",[id],|r|{
        let currency:String=r.get(3)?;
        let source=match r.get_ref(6)?.as_str()?{
            "manual"=>Source::Manual{current:money(&currency,r.get::<_,i64>(7)?)},
            "account"=>Source::Account{account_id:r.get(8)?},"asset"=>Source::Asset{asset_id:r.get(9)?},
            "asset_group"=>Source::AssetGroup{group_id:r.get(10)?},"net_worth"=>Source::NetWorth,_=>return Err(crate::sql::Error::InvalidQuery),
        };
        Ok((GoalDraft{name:r.get(0)?,target:money(&currency,r.get::<_,i64>(1)?),starts_on:r.get(4)?,target_on:r.get(5)?,
            source,state:StoredState::parse(r.get_ref(11)?.as_str()?)?},r.get(2)?))
    }).optional()?.ok_or_else(||missing("goalId"))
}
fn values(ctx: &Context<'_>, date: NaiveDate, source: &Source) -> Result<Vec<Money>> {
    let date = date.to_string();
    let asset =
        |id: &str| Ok::<_, crate::domain::AppError>(assets::get(ctx, id, Some(&date))?.value);
    Ok(match source {
        Source::Manual { current } => vec![current.clone()],
        Source::Account { account_id } => {
            vec![accounts::get(ctx, account_id, Some(&date))?.balance]
        }
        Source::Asset { asset_id } => vec![asset(asset_id)?],
        Source::AssetGroup { group_id } => assets::group_get(ctx, group_id)?
            .asset_ids
            .iter()
            .map(|id| asset(id))
            .collect::<Result<_>>()?,
        Source::NetWorth => {
            let account_ids = ctx
                .tx
                .prepare_cached("SELECT id FROM accounts ORDER BY id")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<crate::sql::Result<Vec<_>>>()?;
            let asset_ids = ctx
                .tx
                .prepare_cached(
                    "SELECT id FROM assets WHERE archived_on IS NULL OR archived_on>?1 ORDER BY id",
                )?
                .query_map([&date], |r| r.get::<_, String>(0))?
                .collect::<crate::sql::Result<Vec<_>>>()?;
            let mut result = Vec::new();
            for id in account_ids {
                result.push(accounts::get(ctx, &id, Some(&date))?.balance);
            }
            for id in asset_ids {
                result.push(asset(&id)?);
            }
            result
        }
    })
}
struct Measured {
    native: Vec<Money>,
    minor: i128,
    missing: u64,
    prior: u64,
}
fn measure(ctx: &Context<'_>, date: NaiveDate, draft: &GoalDraft) -> Result<Measured> {
    let native = values(ctx, date, &draft.source)?;
    let mut totals = std::collections::BTreeMap::<String, i128>::new();
    let mut resolver = Resolver::new(ctx);
    let mut measured = Measured {
        native: Vec::new(),
        minor: 0,
        missing: 0,
        prior: 0,
    };
    for value in native {
        *totals.entry(value.currency.clone()).or_default() += fx::minor_i128(&value.minor)?;
        let conversion = resolver.convert(&value, &draft.target.currency, date)?;
        if let Some(amount) = conversion.displayed {
            measured.minor += fx::minor_i128(&amount.minor)?;
        }
        measured.missing += u64::from(conversion.status == ConversionStatus::Missing);
        measured.prior += u64::from(conversion.status == ConversionStatus::Prior);
    }
    measured.native = totals
        .into_iter()
        .map(|(currency, minor)| money(&currency, minor))
        .collect();
    Ok(measured)
}
fn detail(
    ctx: &Context<'_>,
    id: &str,
    date: NaiveDate,
    draft: GoalDraft,
    initial: i64,
    version: String,
) -> Result<Goal> {
    let start = parse_date(&draft.starts_on)?;
    let end = parse_date(&draft.target_on)?;
    let target = i128::from(minor(&draft.target.minor)?);
    let measured = measure(ctx, date, &draft)?;
    let current = measured.minor;
    let funded = current >= target && measured.missing == 0;
    let state = match &draft.state {
        StoredState::Paused => State::Paused,
        StoredState::Archived => State::Archived,
        StoredState::Completed => State::Completed,
        StoredState::Active if funded => State::Completed,
        StoredState::Active if end < date => State::Overdue,
        StoredState::Active => State::Active,
    };
    let elapsed = Decimal::from((date - start).num_days().max(0))
        / Decimal::from((end - start).num_days().max(1));
    let to_decimal = |amount: i128| {
        Decimal::try_from_i128_with_scale(amount, 0).map_err(|_| integrity("goal_amount_overflow"))
    };
    let base = i128::from(initial);
    let pace = if matches!(state, State::Active) && measured.missing == 0 && target > base {
        let delta =
            to_decimal(current - base)? / to_decimal(target - base)? - elapsed.min(Decimal::ONE);
        Some(if delta >= Decimal::new(10, 2) {
            Pace::Ahead
        } else if delta >= Decimal::new(-5, 2) {
            Pace::OnTrack
        } else {
            Pace::Behind
        })
    } else {
        None
    };
    let progress = if target > 0 {
        (to_decimal(current)? / to_decimal(target)? * Decimal::ONE_HUNDRED)
            .clamp(Decimal::ZERO, Decimal::ONE_HUNDRED)
    } else if current > 0 {
        Decimal::ONE_HUNDRED
    } else {
        Decimal::ZERO
    };
    let calendar_months =
        (end.year() - date.year()) * 12 + end.month() as i32 - date.month() as i32;
    let monthly = if end <= date || funded {
        0
    } else {
        (target - current).max(0) / i128::from(calendar_months.max(1))
    };
    Ok(Goal {
        id: id.to_owned(),
        version,
        as_of: date.to_string(),
        current: money(&draft.target.currency, current),
        initial: money(&draft.target.currency, initial),
        monthly_contribution: money(&draft.target.currency, monthly),
        progress_percent: progress.round_dp(4).normalize().to_string(),
        state,
        pace,
        draft,
        native: measured.native,
        missing_rates: measured.missing.to_string(),
        prior_rates: measured.prior.to_string(),
    })
}
pub fn get(ctx: &Context<'_>, id: &str, as_of: Option<&str>) -> Result<Goal> {
    let (draft, initial) = load(ctx, id)?;
    detail(
        ctx,
        id,
        date_option(as_of)?.unwrap_or(ctx.today),
        draft,
        initial,
        revision(&ctx.tx)?,
    )
}
pub fn list(
    ctx: &Context<'_>,
    state: Option<&State>,
    as_of: Option<&str>,
    page: &PageReq,
) -> Result<Page<Goal>> {
    let cursor = ReadPage::new(ctx, page, &("goals_list", state, as_of))?;
    let after = cursor.after.as_ref().map(|(_, id)| id);
    let date = date_option(as_of)?.unwrap_or(ctx.today);
    let version = revision(&ctx.tx)?;
    let mut stmt = ctx
        .tx
        .prepare_cached("SELECT id FROM goals WHERE (?1 IS NULL OR id>?1) ORDER BY id")?;
    let mut ids = stmt.query([after])?;
    let mut result = Vec::new();
    while let Some(row) = ids.next()? {
        let id = row.get::<_, String>(0)?;
        let (draft, initial) = load(ctx, &id)?;
        let entry = detail(ctx, &id, date, draft, initial, version.clone())?;
        if state.is_none_or(|state| state.key() == entry.state.key()) {
            result.push(entry);
        }
        if result.len() > cursor.limit {
            break;
        }
    }
    cursor.finish(result, |row| (row.id.clone(), row.id.clone()))
}
pub fn save(ctx: &Context<'_>, target: Option<&VersionedId>, draft: &GoalDraft) -> Result<Goal> {
    let title = name(&draft.name)?;
    scale(&ctx.tx, &draft.target.currency)?;
    let target_amount = minor(&draft.target.minor)?;
    let start = parse_date(&draft.starts_on)?;
    let end = parse_date(&draft.target_on)?;
    if end < start {
        return Err(invalid("targetOn", "goal_reversed_dates"));
    }
    let (source, manual, account, asset, group) = match &draft.source {
        Source::Manual { current } => {
            if current.currency != draft.target.currency {
                return Err(invalid("source", "goal_currency_mismatch"));
            }
            ("manual", Some(minor(&current.minor)?), None, None, None)
        }
        Source::Account { account_id } => ("account", None, Some(account_id), None, None),
        Source::Asset { asset_id } => ("asset", None, None, Some(asset_id), None),
        Source::AssetGroup { group_id } => ("asset_group", None, None, None, Some(group_id)),
        Source::NetWorth => ("net_worth", None, None, None, None),
    };
    let amount = measure(ctx, ctx.today, draft)?;
    let (id, initial) = if let Some(target) = target {
        check_revision(ctx, target)?;
        let (previous, initial) = load(ctx, &target.id)?;
        if previous.target.currency != draft.target.currency {
            return Err(conflict("goal_currency_immutable"));
        }
        (target.id.clone(), initial)
    } else {
        (
            new_id(),
            i64::try_from(amount.minor).map_err(|_| invalid("source", "goal_amount_overflow"))?,
        )
    };
    ctx.tx.execute("INSERT INTO goals VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
        ON CONFLICT(id) DO UPDATE SET name=excluded.name,target_minor=excluded.target_minor,starts_on=excluded.starts_on,
        target_on=excluded.target_on,source=excluded.source,manual_minor=excluded.manual_minor,account_id=excluded.account_id,
        asset_id=excluded.asset_id,asset_group_id=excluded.asset_group_id,state=excluded.state",
        params![id,title,target_amount,initial,draft.target.currency,draft.starts_on,draft.target_on,source,manual,account,asset,group,draft.state.key()])?;
    let (draft, initial) = load(ctx, &id)?;
    detail(
        ctx,
        &id,
        ctx.today,
        draft,
        initial,
        e::accounts::next_version(ctx)?,
    )
}
pub fn delete(ctx: &Context<'_>, id: &str, confirmed: bool) -> Result<Deleted> {
    if !confirmed {
        return Err(invalid("confirm", "confirmation_required"));
    }
    if ctx.tx.execute("DELETE FROM goals WHERE id=?1", [id])? == 0 {
        return Err(missing("goalId"));
    }
    Ok(Deleted { id: id.to_owned() })
}
impl ReceiptValue for Goal {
    type Reference = (String, String);
    fn reference(&self, _: &Context<'_>) -> Self::Reference {
        (self.id.clone(), self.as_of.clone())
    }
    fn reload(ctx: &Context<'_>, (id, date): Self::Reference) -> Result<Self> {
        get(ctx, &id, Some(&date))
    }
}
