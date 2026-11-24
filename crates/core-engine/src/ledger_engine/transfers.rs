use crate::sql::{OptionalExtension, params, params_from_iter};
use chrono::{Datelike, Days};

use super::{
    accounts, fx,
    transactions::{self as tx, EntryKind, Query, TxDetail, TxDraft, TxRow},
    *,
};

dto! {
    #[serde(tag = "kind", rename_all_fields = "camelCase")]
    pub enum ExistingOrDraft {
        Existing { target: VersionedId },
        Draft { draft: Box<TxDraft>, derive_amount: Option<bool> },
    }
    pub struct TransferRate { pub decimal: String, pub date: String, pub source: String }
    pub struct TxPair { pub source: TxDetail, pub destination: TxDetail }
    pub struct TransferDetail { pub id: String, pub pair: TxPair, pub rate: Option<TransferRate> }
    pub struct CandidateCriteria {
        pub date_window_days: Option<u16>, pub account_id: Option<String>, pub currency: Option<String>, pub text: Option<String>,
    }
    pub struct TransferCandidate { pub posting: TxRow, pub difference_days: u16 }
    pub enum Disposition { Both, Source, Destination, Neither }
}

pub fn get(ctx: &Context<'_>, id: &str) -> Result<TransferDetail> {
    let (debit,credit,decimal,date,source)=ctx.tx.query_row(
        "SELECT debit_id,credit_id,applied_rate_decimal,rate_date,rate_source FROM transfer_links WHERE id=?1",[id],|r|Ok((
            r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,Option<String>>(4)?
        ))).optional()?.ok_or_else(||missing("linkId"))?;
    let rate = match (decimal, date, source) {
        (Some(decimal), Some(date), Some(source)) => Some(TransferRate {
            decimal,
            date,
            source,
        }),
        (None, None, None) => None,
        _ => return Err(integrity("transfer_rate_incomplete")),
    };
    Ok(TransferDetail {
        id: id.to_owned(),
        pair: TxPair {
            source: tx::get(ctx, &debit)?,
            destination: tx::get(ctx, &credit)?,
        },
        rate,
    })
}
fn info(ctx: &Context<'_>, leg: &ExistingOrDraft) -> Result<(TxDraft, bool)> {
    match leg {
        ExistingOrDraft::Existing { target } => {
            let detail = tx::get(ctx, &target.id)?;
            check_version(&detail.row.version, target)?;
            if detail.row.entry_kind != EntryKind::Ordinary {
                return Err(conflict("invalid_transfer_leg"));
            }
            Ok((detail.draft, false))
        }
        ExistingOrDraft::Draft {
            draft,
            derive_amount,
        } => Ok(((**draft).clone(), derive_amount.unwrap_or(false))),
    }
}
fn eligible(draft: &TxDraft, direction: Direction) -> Result<()> {
    if draft.direction.as_str() != direction.as_str()
        || draft.ignored
        || draft.category_id.is_some()
        || !draft.splits.is_empty()
    {
        return Err(invalid("transfer", "invalid_transfer_leg"));
    }
    Ok(())
}
fn create_leg(
    ctx: &Context<'_>,
    leg: &ExistingOrDraft,
    draft: &TxDraft,
    previous: Option<&TxDetail>,
) -> Result<String> {
    match leg {
        ExistingOrDraft::Existing { target } => {
            let linked: bool = ctx.tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM transfer_links WHERE debit_id=?1 OR credit_id=?1)",
                [&target.id],
                |r| r.get(0),
            )?;
            if linked {
                return Err(conflict("posting_already_linked"));
            }
            Ok(target.id.clone())
        }
        ExistingOrDraft::Draft { .. } => {
            let target = previous.map(|before| VersionedId {
                id: before.row.id.clone(),
                version: before.row.version.clone(),
            });
            Ok(tx::save(ctx, target.as_ref(), draft)?.row.id)
        }
    }
}
pub fn save(
    ctx: &Context<'_>,
    link_id: Option<&str>,
    source: &ExistingOrDraft,
    destination: &ExistingOrDraft,
    quoted: Option<&TransferRate>,
) -> Result<TransferDetail> {
    let (mut debit, derive_debit) = info(ctx, source)?;
    let (mut credit, derive_credit) = info(ctx, destination)?;
    eligible(&debit, Direction::Debit)?;
    eligible(&credit, Direction::Credit)?;
    if debit.account_id == credit.account_id {
        return Err(invalid("destination", "distinct_accounts_required"));
    }
    let same = debit.amount.currency == credit.amount.currency;
    if !same && !ctx.currency_conversion {
        return Err(invalid("destination", "same_currency_accounts_required"));
    }
    let mut rate = None;
    if same {
        if derive_debit
            || derive_credit
            || quoted.is_some()
            || positive(&debit.amount.minor)? != positive(&credit.amount.minor)?
        {
            return Err(invalid("rate", "same_currency_amounts_must_match"));
        }
    } else {
        if derive_debit && derive_credit {
            return Err(invalid("amount", "both_legs_cannot_be_derived"));
        }
        let parsed = quoted
            .map(|q| {
                parse_date(&q.date)?;
                if q.source.trim().is_empty() || q.source.len() > 128 {
                    return Err(invalid("source", "invalid_rate_source"));
                }
                fx::rate(&q.decimal)
            })
            .transpose()?;
        if let Some(value) = parsed {
            if derive_debit {
                positive(&credit.amount.minor)?;
                debit.amount =
                    fx::convert_quoted(ctx, &credit.amount, &debit.amount.currency, value, true)?;
            } else if derive_credit {
                positive(&debit.amount.minor)?;
                credit.amount =
                    fx::convert_at_rate(ctx, &debit.amount, &credit.amount.currency, value)?;
            } else if fx::convert_at_rate(ctx, &debit.amount, &credit.amount.currency, value)?.minor
                != credit.amount.minor
            {
                return Err(invalid("rate", "quoted_rate_conflicts_with_amounts"));
            }
        } else if derive_debit || derive_credit {
            return Err(invalid("rate", "explicit_rate_required"));
        }
        let debit_amount = positive(&debit.amount.minor)?;
        let credit_amount = positive(&credit.amount.minor)?;
        let decimal = if let Some(value) = parsed {
            value
        } else {
            fx::major(
                i128::from(credit_amount),
                scale(&ctx.tx, &credit.amount.currency)?,
            )?
            .checked_div(fx::major(
                i128::from(debit_amount),
                scale(&ctx.tx, &debit.amount.currency)?,
            )?)
            .ok_or_else(|| invalid("rate", "decimal_range_exceeded"))?
        };
        rate = Some(TransferRate {
            decimal: decimal.normalize().to_string(),
            date: quoted.map_or_else(|| debit.bank_date.clone(), |q| q.date.clone()),
            source: quoted.map_or_else(|| "implied".to_owned(), |q| q.source.clone()),
        });
    }
    let previous = link_id.map(|id| unlink(ctx, id)).transpose()?;
    let id = link_id.map_or_else(new_id, str::to_owned);
    let debit = create_leg(
        ctx,
        source,
        &debit,
        previous.as_ref().map(|pair| &pair.source),
    )?;
    let credit = create_leg(
        ctx,
        destination,
        &credit,
        previous.as_ref().map(|pair| &pair.destination),
    )?;
    ctx.tx.execute(
        "INSERT INTO transfer_links VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            id,
            debit,
            credit,
            rate.as_ref().map(|q| &q.decimal),
            rate.as_ref().map(|q| &q.date),
            rate.as_ref().map(|q| &q.source),
            ctx.now_ms
        ],
    )?;
    ctx.tx.execute(
        "UPDATE transactions SET version=version+1 WHERE id IN (?1,?2)",
        params![debit, credit],
    )?;
    get(ctx, &id)
}
pub fn unlink(ctx: &Context<'_>, id: &str) -> Result<TxPair> {
    let detail = get(ctx, id)?;
    let debit = detail.pair.source.row.id;
    let credit = detail.pair.destination.row.id;
    ctx.tx
        .execute("DELETE FROM transfer_links WHERE id=?1", [id])?;
    ctx.tx.execute(
        "UPDATE transactions SET version=version+1 WHERE id IN (?1,?2)",
        params![debit, credit],
    )?;
    Ok(TxPair {
        source: tx::get(ctx, &debit)?,
        destination: tx::get(ctx, &credit)?,
    })
}
pub fn delete(
    ctx: &Context<'_>,
    id: &str,
    disposition: &Disposition,
    confirm: bool,
) -> Result<Deleted> {
    if !confirm {
        return Err(invalid("confirm", "confirmation_required"));
    }
    let pair = unlink(ctx, id)?;
    if matches!(disposition, Disposition::Both | Disposition::Source) {
        tx::remove(ctx, &pair.source.row.id, false)?;
    }
    if matches!(disposition, Disposition::Both | Disposition::Destination) {
        tx::remove(ctx, &pair.destination.row.id, false)?;
    }
    Ok(Deleted { id: id.to_owned() })
}

pub fn candidates(
    ctx: &Context<'_>,
    id: &str,
    criteria: &CandidateCriteria,
    page: &PageReq,
) -> Result<Page<TransferCandidate>> {
    let anchor = tx::get(ctx, id)?;
    eligible(&anchor.draft, anchor.row.direction.clone())?;
    if anchor.row.transfer_id.is_some() || anchor.row.entry_kind != EntryKind::Ordinary {
        return Err(conflict("invalid_transfer_leg"));
    }
    let window = criteria.date_window_days.unwrap_or(7);
    if window > 31 {
        return Err(invalid("dateWindowDays", "date_window_out_of_range"));
    }
    let date = parse_date(&anchor.row.bank_date)?;
    let offset = Days::new(u64::from(window));
    let from = date
        .checked_sub_days(offset)
        .ok_or_else(|| invalid("date", "date_out_of_range"))?;
    let to = date
        .checked_add_days(offset)
        .ok_or_else(|| invalid("date", "date_out_of_range"))?;
    if !(1..=9999).contains(&from.year()) || !(1..=9999).contains(&to.year()) {
        return Err(invalid("date", "date_out_of_range"));
    }
    let paging = ReadPage::new(ctx, page, &("transfer_candidates", id, criteria))?;
    let mut query = Query::new(format!(
        "SELECT {},{}{}",
        tx::ROW_COLUMNS,
        tx::LINK_ID,
        tx::SOURCE
    ));
    query.equals(
        "t.direction",
        anchor.row.direction.opposite().as_str().to_owned(),
    );
    let account = query.bind(anchor.row.account_id.clone());
    query.and(&format!("t.account_id<>{account} AND t.entry_kind='ordinary' AND t.assembly_state=1 AND t.ignored=0 AND t.category_id IS NULL"));
    query.and(tx::NO_TRANSFER);
    query.and("NOT EXISTS(SELECT 1 FROM transaction_splits s WHERE s.transaction_id=t.id)");
    let lo = query.bind(from.to_string());
    let hi = query.bind(to.to_string());
    query.and(&format!("t.bank_date BETWEEN {lo} AND {hi}"));
    let currency = query.bind(anchor.row.amount.currency.clone());
    let amount = query.bind(positive(&anchor.row.amount.minor)?);
    query.and(&format!(
        "(t.currency_code<>{currency} OR t.amount_minor={amount})"
    ));
    if let Some(account) = &criteria.account_id {
        accounts::raw(ctx, account)?;
        query.equals("t.account_id", account.clone());
    }
    if let Some(currency) = &criteria.currency {
        scale(&ctx.tx, currency)?;
        query.equals("t.currency_code", currency.clone());
    }
    if let Some(terms) = criteria
        .text
        .as_deref()
        .map(tx::search_terms)
        .transpose()?
        .flatten()
    {
        let terms = query.bind(terms);
        query.and(&format!("t.id IN(SELECT transaction_id FROM transaction_search WHERE transaction_search MATCH {terms})"));
    }
    tx::sort_cursor(&mut query, &paging, "t.bank_date", "t.id", false, false)?;
    query.limit(paging.limit + 1);
    let mut stmt = ctx.tx.prepare(&query.sql)?;
    let rows = stmt
        .query_map(params_from_iter(query.args), tx::read_row)?
        .map(|row| {
            let posting = row?;
            let difference_days = (parse_date(&posting.bank_date)? - date)
                .num_days()
                .unsigned_abs() as u16;
            Ok(TransferCandidate {
                posting,
                difference_days,
            })
        })
        .collect::<Result<_>>()?;
    paging.finish(rows, |candidate| {
        (
            candidate.posting.bank_date.clone(),
            candidate.posting.id.clone(),
        )
    })
}
