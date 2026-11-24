use super::allocations::{self, Method, Share, ShareInput};
use crate::sql::{OptionalExtension, params};
use crate::{
    domain::{Result, parse_date},
    ledger_engine::{self as e, transactions as tx, *},
};
use std::collections::BTreeMap;

dto! {
    pub enum GroupKind {Social,Trip,CostCenter,Project,Client,Other}
    pub struct SharedGroupDraft {pub name:String,pub kind:GroupKind, pub self_label:Option<String>}
    pub struct MemberDraft {pub label:String, #[serde(default)] pub is_self:bool}
    pub struct Member {pub id:String,pub draft:MemberDraft}
    pub struct GroupSummary {pub id:String,pub name:String,pub kind:GroupKind,pub archived:bool,pub version:String}
    pub struct SharedExpenseDraft {pub group_id:String,pub payer_id:String,pub transaction_id:Option<String>,pub date:String,
        pub description:String,pub amount:Money,pub method:Method,pub shares:Vec<ShareInput>}
    pub struct Expense {pub id:String,pub payer_id:String,pub transaction_id:Option<String>,pub date:String,pub description:String,
        pub amount:Money,pub method:Method,pub shares:Vec<Share>}
    pub struct SettlementDraft {pub group_id:String,pub from_member_id:String,pub to_member_id:String,pub date:String,
        pub amount:Money,pub transaction_id:Option<String>,#[serde(default)] pub notes:String}
    pub struct Settlement {pub id:String,pub draft:SettlementDraft}
    pub struct Balance {pub member_id:String,pub amount:Money}
    pub struct DisplayedBalance {pub member_id:String,pub amount:Money,pub prior_rates:String,pub missing_rates:String}
    pub struct GroupDetail {pub group:GroupSummary,pub as_of:String,pub members:Vec<Member>,pub expenses:Vec<Expense>,
        pub settlements:Vec<Settlement>,pub balances:Vec<Balance>,pub display_balances:Vec<DisplayedBalance>}
}
impl GroupKind {
    fn key(&self) -> &str {
        match self {
            Self::Social => "social",
            Self::Trip => "trip",
            Self::CostCenter => "cost_center",
            Self::Project => "project",
            Self::Client => "client",
            Self::Other => "other",
        }
    }
    fn parse(key: &str) -> crate::sql::Result<Self> {
        match key {
            "social" => Ok(Self::Social),
            "trip" => Ok(Self::Trip),
            "cost_center" => Ok(Self::CostCenter),
            "project" => Ok(Self::Project),
            "client" => Ok(Self::Client),
            "other" => Ok(Self::Other),
            _ => Err(crate::sql::Error::InvalidQuery),
        }
    }
}
fn summary(ctx: &Context<'_>, id: &str) -> Result<GroupSummary> {
    let version = revision(&ctx.tx)?;
    ctx.tx
        .query_row(
            "SELECT name,kind,archived FROM shared_groups WHERE id=?1",
            [id],
            |r| {
                Ok(GroupSummary {
                    id: id.to_owned(),
                    name: r.get(0)?,
                    kind: GroupKind::parse(r.get_ref(1)?.as_str()?)?,
                    archived: r.get(2)?,
                    version,
                })
            },
        )
        .optional()?
        .ok_or_else(|| missing("groupId"))
}
pub fn list(ctx: &Context<'_>, page: &PageReq) -> Result<Page<GroupSummary>> {
    let cursor = ReadPage::new(ctx, page, &"groups_list")?;
    let after = cursor.after.as_ref().map(|(_, id)| id);
    let ids = ctx
        .tx
        .prepare_cached(
            "SELECT id FROM shared_groups WHERE (?1 IS NULL OR id>?1) ORDER BY id LIMIT ?2",
        )?
        .query_map(params![after, (cursor.limit + 1) as i64], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<crate::sql::Result<Vec<_>>>()?;
    cursor.finish(
        ids.iter()
            .map(|id| summary(ctx, id))
            .collect::<Result<_>>()?,
        |r| (r.id.clone(), r.id.clone()),
    )
}
fn members(ctx: &Context<'_>, group: &str) -> Result<Vec<Member>> {
    Ok(ctx.tx.prepare_cached("SELECT id,label,is_self FROM shared_group_members WHERE group_id=?1 ORDER BY is_self DESC,id")?
        .query_map([group],|r|Ok(Member{id:r.get(0)?,draft:MemberDraft{label:r.get(1)?,is_self:r.get(2)?}}))?
        .collect::<crate::sql::Result<_>>()?)
}
fn expenses(ctx: &Context<'_>, group: &str, date: &str) -> Result<Vec<Expense>> {
    let mut result=ctx.tx.prepare_cached("SELECT id,payer_id,transaction_id,expense_on,description,currency_code,
        CAST(amount_minor AS TEXT),allocation_method FROM shared_expenses WHERE group_id=?1 AND expense_on<=?2 ORDER BY expense_on,id")?
        .query_map(params![group,date],|r|Ok(Expense{id:r.get(0)?,payer_id:r.get(1)?,transaction_id:r.get(2)?,date:r.get(3)?,description:r.get(4)?,
            amount:Money{currency:r.get(5)?,minor:r.get(6)?},method:Method::parse(r.get_ref(7)?.as_str()?)?,shares:Vec::new()}))?
        .collect::<crate::sql::Result<Vec<_>>>()?;
    let mut shares = ctx.tx.prepare_cached(
        "SELECT member_id,CAST(share_minor AS TEXT),percentage_decimal FROM shared_allocations
        WHERE expense_id=?1 ORDER BY position",
    )?;
    for record in &mut result {
        record.shares = shares
            .query_map([&record.id], |r| {
                Ok(Share {
                    member_id: r.get(0)?,
                    minor: r.get(1)?,
                    percentage: r.get(2)?,
                })
            })?
            .collect::<crate::sql::Result<_>>()?;
    }
    Ok(result)
}
fn settlements(ctx: &Context<'_>, group: &str, date: &str) -> Result<Vec<Settlement>> {
    Ok(ctx.tx.prepare_cached("SELECT id, from_member_id,to_member_id,settled_on,currency_code,CAST(amount_minor AS TEXT),
        transaction_id,notes FROM shared_settlements WHERE group_id=?1 AND settled_on<=?2 ORDER BY settled_on,id")?
        .query_map(params![group,date],|r|Ok(Settlement{id:r.get(0)?,draft:SettlementDraft{group_id:group.to_owned(),
            from_member_id:r.get(1)?,to_member_id:r.get(2)?,date:r.get(3)?,amount:Money{currency:r.get(4)?,minor:r.get(5)?},
            transaction_id:r.get(6)?,notes:r.get(7)?}}))?.collect::<crate::sql::Result<_>>()?)
}
fn balances(
    expenses: &[Expense],
    settlements: &[Settlement],
) -> Result<BTreeMap<(String, String), i128>> {
    let mut balances = BTreeMap::new();
    let mut add = |member: &str, currency: &str, amount: i64| {
        *balances
            .entry((member.to_owned(), currency.to_owned()))
            .or_insert(0) += i128::from(amount)
    };
    for expense in expenses {
        add(
            &expense.payer_id,
            &expense.amount.currency,
            positive(&expense.amount.minor)?,
        );
        for share in &expense.shares {
            add(
                &share.member_id,
                &expense.amount.currency,
                -minor(&share.minor)?,
            );
        }
    }
    for settlement in settlements {
        let draft = &settlement.draft;
        let value = positive(&draft.amount.minor)?;
        add(&draft.from_member_id, &draft.amount.currency, value);
        add(&draft.to_member_id, &draft.amount.currency, -value);
    }
    Ok(balances)
}
pub fn get(ctx: &Context<'_>, id: &str, as_of: Option<&str>) -> Result<GroupDetail> {
    let group = summary(ctx, id)?;
    let date = date_option(as_of)?.unwrap_or(ctx.today);
    let as_of = date.to_string();
    let expenses = expenses(ctx, id, &as_of)?;
    let settlements = settlements(ctx, id, &as_of)?;
    let balances = balances(&expenses, &settlements)?
        .into_iter()
        .filter(|(_, amount)| *amount != 0)
        .map(|((member_id, currency), amount)| Balance {
            member_id,
            amount: money(&currency, amount),
        })
        .collect::<Vec<_>>();
    let currency = accounts::settings(ctx)?.display_currency;
    let mut resolver = currency::Resolver::new(ctx);
    let mut converted = BTreeMap::<String, (i128, u64, u64)>::new();
    for balance in &balances {
        let amount = resolver.convert(&balance.amount, &currency, date)?;
        let result = converted.entry(balance.member_id.clone()).or_default();
        if let Some(value) = amount.displayed {
            result.0 += currency::minor_i128(&value.minor)?;
        }
        result.1 += u64::from(amount.status == currency::ConversionStatus::Prior);
        result.2 += u64::from(amount.status == currency::ConversionStatus::Missing);
    }
    let display_balances = converted
        .into_iter()
        .map(|(member_id, (amount, prior, missing))| DisplayedBalance {
            member_id,
            amount: money(&currency, amount),
            prior_rates: prior.to_string(),
            missing_rates: missing.to_string(),
        })
        .collect();
    Ok(GroupDetail {
        group,
        as_of,
        members: members(ctx, id)?,
        expenses,
        settlements,
        balances,
        display_balances,
    })
}
fn updated(ctx: &Context<'_>, id: &str) -> Result<GroupDetail> {
    let mut record = get(ctx, id, None)?;
    record.group.version = e::accounts::next_version(ctx)?;
    Ok(record)
}
pub fn save(
    ctx: &Context<'_>,
    target: Option<&VersionedId>,
    draft: &SharedGroupDraft,
) -> Result<GroupDetail> {
    let title = name(&draft.name)?;
    let id = if let Some(target) = target {
        check_revision(ctx, target)?;
        summary(ctx, &target.id)?;
        ctx.tx.execute(
            "UPDATE shared_groups SET name=?2,kind=?3 WHERE id=?1",
            params![target.id, title, draft.kind.key()],
        )?;
        target.id.clone()
    } else {
        let id = new_id();
        ctx.tx.execute(
            "INSERT INTO shared_groups VALUES(?1,?2,?3,0)",
            params![id, title, draft.kind.key()],
        )?;
        ctx.tx.execute(
            "INSERT INTO shared_group_members VALUES(?1,?2,?3,1)",
            params![
                id,
                new_id(),
                name(draft.self_label.as_deref().unwrap_or("Me"))?
            ],
        )?;
        id
    };
    if let Some(label) = draft.self_label.as_deref() {
        ctx.tx.execute(
            "UPDATE shared_group_members SET label=?2 WHERE group_id=?1 AND is_self=1",
            params![id, name(label)?],
        )?;
    }
    updated(ctx, &id)
}
pub fn archive(ctx: &Context<'_>, id: &str, archived: bool) -> Result<GroupDetail> {
    summary(ctx, id)?;
    ctx.tx.execute(
        "UPDATE shared_groups SET archived=?2 WHERE id=?1",
        params![id, archived],
    )?;
    updated(ctx, id)
}
fn active(ctx: &Context<'_>, id: &str) -> Result<Vec<Member>> {
    if summary(ctx, id)?.archived {
        return Err(conflict("shared_group_archived"));
    }
    members(ctx, id)
}
pub fn member_save(
    ctx: &Context<'_>,
    group: &str,
    target: Option<&VersionedId>,
    draft: &MemberDraft,
) -> Result<GroupDetail> {
    let members = active(ctx, group)?;
    let label = name(&draft.label)?;
    if let Some(target) = target {
        check_revision(ctx, target)?;
        let record = members
            .iter()
            .find(|member| member.id == target.id)
            .ok_or_else(|| missing("memberId"))?;
        if record.draft.is_self != draft.is_self {
            return Err(invalid("isSelf", "self_member_immutable"));
        }
        ctx.tx.execute(
            "UPDATE shared_group_members SET label=?3 WHERE group_id=?1 AND id=?2",
            params![group, target.id, label],
        )?;
    } else {
        if draft.is_self || members.len() >= 200 {
            return Err(invalid("member", "invalid_new_group_member"));
        }
        ctx.tx.execute(
            "INSERT INTO shared_group_members VALUES(?1,?2,?3,0)",
            params![group, new_id(), label],
        )?;
    }
    updated(ctx, group)
}
pub fn member_remove(ctx: &Context<'_>, group: &str, id: &str) -> Result<GroupDetail> {
    let members = active(ctx, group)?;
    let member = members
        .iter()
        .find(|member| member.id == id)
        .ok_or_else(|| missing("memberId"))?;
    if member.draft.is_self {
        return Err(invalid("isSelf", "self_member_immutable"));
    }
    let used:bool=ctx.tx.query_row("SELECT EXISTS(SELECT 1 FROM shared_expenses WHERE group_id=?1 AND payer_id=?2)
        OR EXISTS(SELECT 1 FROM shared_allocations WHERE group_id=?1 AND member_id=?2)
        OR EXISTS(SELECT 1 FROM shared_settlements WHERE group_id=?1 AND ?2 IN(from_member_id,to_member_id))",params![group,id],|r|r.get(0))?;
    if used {
        return Err(conflict("group_member_has_dependencies"));
    }
    ctx.tx.execute(
        "DELETE FROM shared_group_members WHERE group_id=?1 AND id=?2",
        params![group, id],
    )?;
    updated(ctx, group)
}
fn target_id(
    ctx: &Context<'_>,
    table: &str,
    target: Option<&VersionedId>,
    group: &str,
) -> Result<String> {
    let Some(target) = target else {
        return Ok(new_id());
    };
    check_revision(ctx, target)?;
    if record_group(ctx, table, &target.id)? != group {
        return Err(conflict("shared_record_group_immutable"));
    }
    Ok(target.id.clone())
}
fn record_group(ctx: &Context<'_>, table: &str, id: &str) -> Result<String> {
    ctx.tx
        .query_row(
            &format!("SELECT group_id FROM {table} WHERE id=?1"),
            [id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| missing("recordId"))
}
fn group_member<'a>(members: &'a [Member], id: &str) -> Result<&'a Member> {
    members
        .iter()
        .find(|member| member.id == id)
        .ok_or_else(|| missing("memberId"))
}
fn linked_movement(ctx: &Context<'_>, id: &str, direction: &Direction) -> Result<tx::TxDetail> {
    let item = tx::get(ctx, id)?;
    if item.row.direction.as_str() != direction.as_str()
        || !matches!(item.row.status, TxStatus::Posted)
        || item.row.ignored
        || item.row.entry_kind != tx::EntryKind::Ordinary
        || item.row.transfer_id.is_some()
    {
        return Err(conflict("ineligible_shared_posting"));
    }
    Ok(item)
}
pub fn expense_save(
    ctx: &Context<'_>,
    target: Option<&VersionedId>,
    draft: &SharedExpenseDraft,
) -> Result<GroupDetail> {
    let members = active(ctx, &draft.group_id)?;
    let payer = group_member(&members, &draft.payer_id)?;
    let id = target_id(ctx, "shared_expenses", target, &draft.group_id)?;
    parse_date(&draft.date)?;
    let description = name(&draft.description)?;
    let amount = positive(&draft.amount.minor)?;
    scale(&ctx.tx, &draft.amount.currency)?;
    if let Some(link) = draft.transaction_id.as_deref() {
        let posting = linked_movement(ctx, link, &Direction::Debit)?;
        let settled: bool = ctx.tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM shared_settlements WHERE transaction_id=?1)",
            [link],
            |r| r.get(0),
        )?;
        if !payer.draft.is_self
            || settled
            || posting.row.amount.currency != draft.amount.currency
            || posting.row.amount.minor != draft.amount.minor
            || posting.row.bank_date != draft.date
            || posting
                .shared_expense_id
                .as_ref()
                .is_some_and(|expense| expense != &id)
        {
            return Err(conflict("ineligible_shared_posting"));
        }
    }
    let shares = allocations::allocate(amount, &draft.method, &draft.shares)?;
    for share in &shares {
        group_member(&members, &share.member_id)?;
    }
    ctx.tx.execute("INSERT INTO shared_expenses VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
        ON CONFLICT(id) DO UPDATE SET payer_id=excluded.payer_id,transaction_id=excluded.transaction_id,expense_on=excluded.expense_on,
        description=excluded.description,currency_code=excluded.currency_code,amount_minor=excluded.amount_minor,allocation_method=excluded.allocation_method",
        params![id,draft.group_id,draft.payer_id,draft.transaction_id,draft.date,description,draft.amount.currency,amount,draft.method.key()])?;
    ctx.tx
        .execute("DELETE FROM shared_allocations WHERE expense_id=?1", [&id])?;
    for (position, share) in shares.iter().enumerate() {
        ctx.tx.execute(
            "INSERT INTO shared_allocations VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                id,
                draft.group_id,
                share.member_id,
                minor(&share.minor)?,
                share.percentage,
                (position + 1) as i64
            ],
        )?;
    }
    updated(ctx, &draft.group_id)
}
pub fn expense_remove(ctx: &Context<'_>, id: &str, confirmed: bool) -> Result<GroupDetail> {
    remove(ctx, "shared_expenses", id, confirmed)
}
pub fn settlement_remove(ctx: &Context<'_>, id: &str, confirmed: bool) -> Result<GroupDetail> {
    remove(ctx, "shared_settlements", id, confirmed)
}
fn remove(ctx: &Context<'_>, table: &str, id: &str, confirmed: bool) -> Result<GroupDetail> {
    if !confirmed {
        return Err(invalid("confirm", "confirmation_required"));
    }
    let group = record_group(ctx, table, id)?;
    ctx.tx
        .execute(&format!("DELETE FROM {table} WHERE id=?1"), [id])?;
    updated(ctx, &group)
}
pub fn settlement_save(
    ctx: &Context<'_>,
    target: Option<&VersionedId>,
    draft: &SettlementDraft,
) -> Result<GroupDetail> {
    let members = active(ctx, &draft.group_id)?;
    let from = group_member(&members, &draft.from_member_id)?;
    let to = group_member(&members, &draft.to_member_id)?;
    if from.id == to.id {
        return Err(invalid("memberId", "settlement_parties_must_differ"));
    }
    parse_date(&draft.date)?;
    if draft.notes.chars().count() > 1000 {
        return Err(invalid("notes", "notes_too_long"));
    }
    let amount = positive(&draft.amount.minor)?;
    scale(&ctx.tx, &draft.amount.currency)?;
    let id = target_id(ctx, "shared_settlements", target, &draft.group_id)?;
    let history = settlements(ctx, &draft.group_id, &draft.date)?
        .into_iter()
        .filter(|record| record.id != id)
        .collect::<Vec<_>>();
    let outstanding = balances(&expenses(ctx, &draft.group_id, &draft.date)?, &history)?;
    let balance = |party: &Member| {
        outstanding
            .get(&(party.id.clone(), draft.amount.currency.clone()))
            .copied()
            .unwrap_or_default()
    };
    if i128::from(amount) > balance(to).min(-balance(from)) {
        return Err(conflict("settlement_exceeds_outstanding_balance"));
    }
    if let Some(link) = draft.transaction_id.as_deref() {
        if !(from.draft.is_self || to.draft.is_self) {
            return Err(invalid("transactionId", "settlement_has_no_self_movement"));
        }
        let posting = linked_movement(
            ctx,
            link,
            &if from.draft.is_self {
                Direction::Debit
            } else {
                Direction::Credit
            },
        )?;
        if posting.shared_expense_id.is_some() || posting.row.bank_date != draft.date {
            return Err(conflict("ineligible_shared_posting"));
        }
        let recorded = if posting.row.amount.currency == draft.amount.currency {
            &posting.row.amount
        } else {
            &posting
                .draft
                .original
                .as_ref()
                .ok_or_else(|| conflict("settlement_fx_provenance_required"))?
                .amount
        };
        if recorded.currency != draft.amount.currency || recorded.minor != draft.amount.minor {
            return Err(conflict("settlement_link_amount_mismatch"));
        }
    }
    ctx.tx.execute("INSERT INTO shared_settlements VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
        ON CONFLICT(id) DO UPDATE SET from_member_id=excluded.from_member_id,to_member_id=excluded.to_member_id,settled_on=excluded.settled_on,
        currency_code=excluded.currency_code,amount_minor=excluded.amount_minor,transaction_id=excluded.transaction_id,notes=excluded.notes",
        params![id,draft.group_id,from.id,to.id,draft.date,draft.amount.currency,amount,draft.transaction_id,draft.notes])?;
    updated(ctx, &draft.group_id)
}
impl ReceiptValue for GroupDetail {
    type Reference = (String, String);
    fn reference(&self, _: &Context<'_>) -> Self::Reference {
        (self.group.id.clone(), self.as_of.clone())
    }
    fn reload(ctx: &Context<'_>, (id, date): Self::Reference) -> Result<Self> {
        get(ctx, &id, Some(&date))
    }
}
