mod storage;
pub use storage::*;

use crate::{
    domain::{Result, normalize, parse_date},
    ledger_engine::{Context, dto, fx, invalid, minor, name, scale, transactions::TxDraft},
};
use regex::{Regex, RegexBuilder};
use rust_decimal::Decimal;
use std::cmp::Ordering;

dto! {
    pub enum Logic { And, Or }
    pub enum Field { Description, Payee, Notes, SignedAmount, Type, AccountId, PayeeId, BankDate, Status, ProviderCategory, Ignored }
    pub enum Operator { Contains, NotContains, Equal, NotEqual, Prefix, Suffix, Regex, Greater, GreaterEqual, Less, LessEqual }
    #[serde(tag="type", content="value")]
    pub enum Operand { Text(String), Amount(DecimalAmount), Boolean(bool) }
    pub struct DecimalAmount { pub currency:String, pub decimal:String }
    pub struct Predicate { pub field:Field, pub operator:Operator, pub value:Operand }
    pub struct LeafGroup { pub op:Logic, pub children:Vec<Predicate> }
    #[serde(tag="kind", content="value")]
    pub enum Child { Predicate(Predicate), Group(LeafGroup) }
    pub struct Group { pub op:Logic, pub children:Vec<Child> }
    #[serde(tag="kind", content="value")]
    pub enum Action { Category(String), Payee(String), Description(String), AppendNotes(String), Ignore(bool) }
    pub struct RuleDraft {
        pub name:String, pub priority:i32, pub enabled:bool, pub apply_existing:bool,
        pub overwrite_existing_category:bool, pub schema_version:u8, pub conditions:Group, pub actions:Vec<Action>,
    }
    pub struct Rule {pub id:String,pub version:String,pub draft:RuleDraft}
    pub enum ReferenceKind {Account,Category,Payee}
    pub struct PortableReference {pub entity:ReferenceKind,pub name:String,pub qualifier:Option<String>}
    pub struct RulePack {pub format:String,pub version:u8,pub rules:Vec<RuleDraft>,
        #[serde(default,skip_serializing_if="std::collections::BTreeMap::is_empty")]
        pub references:std::collections::BTreeMap<String,PortableReference>}
    pub enum CollisionPolicy { Reject, Skip, Replace }
}

enum Test {
    Text(String),
    Regex(Regex),
    Amount(String, Decimal),
    Date(chrono::NaiveDate),
    Boolean(bool),
}
struct CompiledPredicate {
    field: Field,
    operator: Operator,
    test: Test,
}
enum CompiledChild {
    Predicate(CompiledPredicate),
    Group(Logic, Vec<CompiledPredicate>),
}
pub struct CompiledRule {
    op: Logic,
    nodes: Vec<CompiledChild>,
}
pub struct Snapshot<'a> {
    pub draft: &'a TxDraft,
    pub payee: Option<&'a str>,
    pub provider_category: Option<&'a str>,
    pub scale: u32,
}

fn invalid_rule() -> crate::domain::AppError {
    invalid("conditions", "invalid_rule_predicate")
}
fn compares(operator: &Operator) -> bool {
    matches!(
        operator,
        Operator::Equal
            | Operator::NotEqual
            | Operator::Greater
            | Operator::GreaterEqual
            | Operator::Less
            | Operator::LessEqual
    )
}

impl CompiledPredicate {
    fn compile(predicate: &Predicate) -> Result<Self> {
        use Field::*;
        use Operand::*;
        use Operator::*;
        let test = match (&predicate.field, &predicate.operator, &predicate.value) {
            (SignedAmount, op, Amount(value)) if compares(op) => {
                crate::domain::CurrencyCode::parse(&value.currency)?;
                let amount = Decimal::from_str_exact(&value.decimal).map_err(|_| invalid_rule())?;
                Test::Amount(value.currency.clone(), amount)
            }
            (BankDate, op, Text(value)) if compares(op) => Test::Date(parse_date(value)?),
            (Ignored, Equal | NotEqual, Boolean(value)) => Test::Boolean(*value),
            (Type | Status | AccountId | PayeeId, Equal | NotEqual, Text(value))
                if !value.trim().is_empty() && value.len() <= 128 =>
            {
                if matches!(predicate.field, Type) && !matches!(value.as_str(), "debit" | "credit")
                    || matches!(predicate.field, Status)
                        && !matches!(value.as_str(), "posted" | "pending" | "projected")
                {
                    return Err(invalid_rule());
                }
                Test::Text(value.to_owned())
            }
            (Description | Payee | Notes | ProviderCategory, Regex, Text(value))
                if !value.trim().is_empty() && value.len() <= 512 =>
            {
                let regex = RegexBuilder::new(value)
                    .size_limit(256 * 1024)
                    .dfa_size_limit(256 * 1024)
                    .nest_limit(32)
                    .build()
                    .map_err(|_| invalid("conditions", "invalid_or_oversize_regex"))?;
                if regex.is_match("") {
                    return Err(invalid("conditions", "regex_matches_empty"));
                }
                Test::Regex(regex)
            }
            (
                Description | Payee | Notes | ProviderCategory,
                Contains | NotContains | Equal | NotEqual | Prefix | Suffix,
                Text(value),
            ) if !value.trim().is_empty() && value.len() <= 512 => {
                let value = normalize(value);
                if value.is_empty() {
                    return Err(invalid_rule());
                }
                Test::Text(value)
            }
            _ => return Err(invalid_rule()),
        };
        Ok(Self {
            field: predicate.field.clone(),
            operator: predicate.operator.clone(),
            test,
        })
    }

    fn matches(&self, row: &Snapshot<'_>) -> Result<bool> {
        let draft = row.draft;
        let text = match self.field {
            Field::Description => Some(draft.description.as_str()),
            Field::Notes => Some(draft.notes.as_str()),
            Field::Payee => row.payee,
            Field::ProviderCategory => row.provider_category,
            Field::Type => Some(draft.direction.as_str()),
            Field::Status => Some(draft.status.as_str()),
            Field::AccountId => Some(draft.account_id.as_str()),
            Field::PayeeId => draft.payee_id.as_deref(),
            _ => None,
        };
        let output = match &self.test {
            Test::Text(value) => {
                let Some(text) = text else {
                    return Ok(matches!(
                        self.operator,
                        Operator::NotEqual | Operator::NotContains
                    ));
                };
                let text = if matches!(
                    self.field,
                    Field::Description | Field::Notes | Field::ProviderCategory | Field::Payee
                ) {
                    normalize(text)
                } else {
                    text.to_owned()
                };
                match self.operator {
                    Operator::Contains => text.contains(value),
                    Operator::NotContains => !text.contains(value),
                    Operator::Prefix => text.starts_with(value),
                    Operator::Suffix => text.ends_with(value),
                    _ => compare(text.cmp(value), &self.operator),
                }
            }
            Test::Regex(regex) => text.is_some_and(|text| regex.is_match(&normalize(text))),
            Test::Amount(currency, amount) => {
                if &draft.amount.currency != currency {
                    return Ok(false);
                }
                let sign = if draft.direction.as_str() == "debit" {
                    -1
                } else {
                    1
                };
                compare(
                    fx::major(i128::from(minor(&draft.amount.minor)?) * sign, row.scale)?
                        .cmp(amount),
                    &self.operator,
                )
            }
            Test::Date(date) => compare(parse_date(&draft.bank_date)?.cmp(date), &self.operator),
            Test::Boolean(value) => compare(draft.ignored.cmp(value), &self.operator),
        };
        Ok(output)
    }
}

fn compare(order: Ordering, operator: &Operator) -> bool {
    match operator {
        Operator::Equal => order == Ordering::Equal,
        Operator::NotEqual => order != Ordering::Equal,
        Operator::Greater => order == Ordering::Greater,
        Operator::Less => order == Ordering::Less,
        Operator::GreaterEqual => order != Ordering::Less,
        Operator::LessEqual => order != Ordering::Greater,
        _ => false,
    }
}
fn evaluate<T>(op: &Logic, nodes: &[T], test: impl Fn(&T) -> Result<bool>) -> Result<bool> {
    let all = matches!(op, Logic::And);
    for node in nodes {
        if test(node)? != all {
            return Ok(!all);
        }
    }
    Ok(all)
}

impl CompiledRule {
    pub fn new(rule: &RuleDraft) -> Result<Self> {
        name(&rule.name)?;
        if rule.schema_version != 1
            || rule.conditions.children.is_empty()
            || rule.actions.is_empty()
            || rule.actions.len() > 16
        {
            return Err(invalid("rule", "invalid_rule_structure"));
        }
        let mut count = 0;
        let mut nodes = Vec::new();
        for child in &rule.conditions.children {
            let node = match child {
                Child::Predicate(predicate) => {
                    count += 1;
                    CompiledChild::Predicate(CompiledPredicate::compile(predicate)?)
                }
                Child::Group(group) => {
                    if group.children.is_empty() {
                        return Err(invalid("conditions", "empty_rule_group"));
                    }
                    count += group.children.len();
                    if count > 64 {
                        return Err(invalid("conditions", "rule_predicate_limit"));
                    }
                    CompiledChild::Group(
                        group.op.clone(),
                        group
                            .children
                            .iter()
                            .map(CompiledPredicate::compile)
                            .collect::<Result<_>>()?,
                    )
                }
            };
            if count > 64 {
                return Err(invalid("conditions", "rule_predicate_limit"));
            }
            nodes.push(node);
        }
        for action in &rule.actions {
            let bound = match action {
                Action::Category(text) | Action::Payee(text) => (text, 128),
                Action::Description(text) => (text, 1024),
                Action::AppendNotes(text) => (text, 1000),
                Action::Ignore(_) => continue,
            };
            if bound.0.trim().is_empty() || bound.0.chars().count() > bound.1 {
                return Err(invalid("actions", "invalid_rule_action"));
            }
        }
        Ok(Self {
            op: rule.conditions.op.clone(),
            nodes,
        })
    }

    pub fn matches(&self, row: &Snapshot<'_>) -> Result<bool> {
        evaluate(&self.op, &self.nodes, |node| match node {
            CompiledChild::Predicate(predicate) => predicate.matches(row),
            CompiledChild::Group(op, predicates) => {
                evaluate(op, predicates, |predicate| predicate.matches(row))
            }
        })
    }
}

pub(crate) fn input_scale(ctx: &Context<'_>, draft: &TxDraft) -> Result<u32> {
    scale(&ctx.tx, &draft.amount.currency)
}
