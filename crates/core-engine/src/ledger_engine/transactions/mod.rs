use crate::sql::{OptionalExtension, Row, Value, params, params_from_iter};
use chrono::{Days, NaiveDate};
use std::collections::{BTreeSet, HashMap};

use super::{accounts, currency, *};

dto! {
    pub struct SplitDraft { pub category_id: String, pub minor: String }
    pub struct InstallmentRef { pub series_id: String, pub number: u16 }
    pub struct OriginalAmount { pub amount: Money, pub rate_decimal: String, pub rate_date: String, pub rate_source: String }
    pub struct TxDraft {
        pub account_id: String, pub bank_date: String, pub amount: Money, pub direction: Direction,
        pub status: TxStatus, pub description: String,
        #[serde(default)] pub notes: String,
        pub payee_id: Option<String>, pub category_id: Option<String>,
        #[serde(default)] pub splits: Vec<SplitDraft>,
        pub effective_bill_date: Option<String>, pub reporting_date_override: Option<String>,
        #[serde(default)] pub ignored: bool,
        #[serde(default)] pub exclude_from_pnl: bool,
        #[serde(default)] pub flagged: bool,
        pub flag_due_date: Option<String>, pub alias_id: Option<String>, pub statement_id: Option<String>,
        pub installment: Option<InstallmentRef>, pub original: Option<OriginalAmount>,
    }
    #[derive(PartialEq, Eq)]
    pub enum EntryKind { Ordinary, Opening, Adjustment }
    pub struct TxRow {
        pub id: String, pub version: String, pub account_id: String, pub amount: Money, pub direction: Direction,
        pub bank_date: String, pub effective_date: String, pub reporting_date: String,
        pub status: TxStatus, pub entry_kind: EntryKind, pub description: String, pub payee_id: Option<String>,
        pub category_id: Option<String>, pub category: Option<TransactionCategory>, pub statement_id: Option<String>, pub ignored: bool, pub exclude_from_pnl: bool,
        pub flagged: bool, pub flag_due_date: Option<String>, pub transfer_id: Option<String>, pub running_balance: Option<Money>, pub pnl_amount:Option<Money>,
    }
    pub struct TransactionCategory { pub name: String, pub kind: super::catalog::CategoryKind, pub icon: Option<String> }
    pub struct ImportSource { pub namespace: Option<String>, pub external_id: Option<String>, pub bank_date: String, pub amount: Money, pub direction: String, pub description: String }
    pub struct TxDetail {
        pub row: TxRow, pub draft: TxDraft, pub applied_mode: AccountingMode, pub sources: Vec<ImportSource>,
        pub tags: Vec<String>, pub recurring_rule_id: Option<String>, pub shared_expense_id: Option<String>, pub attachment_ids: Vec<String>,
    }
    pub enum SortField { Date, Amount, Description, Payee, Category, Account, Type, Status, FlagDueDate }
    pub struct Sort { pub field: SortField, #[serde(default)] pub ascending: bool }
    pub enum FlagState { Flagged, Overdue, DueWithinWeek }
    pub struct AmountBounds { pub currency: String, pub minimum: Option<String>, pub maximum: Option<String> }
    #[derive(Default)]
    pub struct LedgerFilter {
        #[serde(default)] pub account_ids: Vec<String>, #[serde(default)] pub alias_ids: Vec<String>,
        #[serde(default)] pub category_ids: Vec<String>, #[serde(default)] pub payee_ids: Vec<String>,
        #[serde(default)] pub group_ids: Vec<String>,
        pub statement_id: Option<String>, #[serde(default)] pub unbilled: bool,
        pub text: Option<String>, #[serde(default)] pub uncategorized: bool, #[serde(default)] pub tags: Vec<String>,
        pub native_amount: Option<AmountBounds>, pub display_amount: Option<AmountBounds>,
        pub entry_kind: Option<EntryKind>, pub status: Option<TxStatus>, pub direction: Option<Direction>,
        pub include_opening: Option<bool>, pub transfer: Option<bool>, pub pnl_only: Option<bool>, pub ignored: Option<bool>,
        pub flag_state: Option<FlagState>, pub basis: Option<DateBasis>, pub range: Option<DateRange>, pub sort: Option<Sort>,
    }
}

impl Direction {
    fn read(s: &str) -> crate::sql::Result<Self> {
        match s {
            "debit" => Ok(Self::Debit),
            "credit" => Ok(Self::Credit),
            _ => Err(crate::sql::Error::InvalidQuery),
        }
    }
}
impl TxStatus {
    fn read(s: &str) -> crate::sql::Result<Self> {
        match s {
            "posted" => Ok(Self::Posted),
            "pending" => Ok(Self::Pending),
            "projected" => Ok(Self::Projected),
            _ => Err(crate::sql::Error::InvalidQuery),
        }
    }
}
impl EntryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ordinary => "ordinary",
            Self::Opening => "opening",
            Self::Adjustment => "adjustment",
        }
    }
    fn read(s: &str) -> crate::sql::Result<Self> {
        match s {
            "ordinary" => Ok(Self::Ordinary),
            "opening" => Ok(Self::Opening),
            "adjustment" => Ok(Self::Adjustment),
            _ => Err(crate::sql::Error::InvalidQuery),
        }
    }
}

mod bulk;
mod mutations;
mod queries;

pub use bulk::{bulk_apply, bulk_preview};
pub use mutations::*;
pub use queries::*;
