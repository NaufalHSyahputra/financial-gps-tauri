use crate::sql::{OptionalExtension, params};
use crate::{
    domain::Result,
    ledger_engine::{self as e, Context, ReceiptValue, dto, invalid},
};
use std::collections::BTreeSet;

dto! {
    pub enum Theme {
        System,
        #[serde(alias = "light")]
        Latte,
        Frappe,
        Macchiato,
        #[serde(alias = "dark")]
        Mocha,
    }
    pub enum DateFormat {Iso,DayFirst,MonthFirst}
    pub enum NumberFormat {International,Indonesian}
    pub struct AppPreferences {
        pub language:String,pub privacy_mask:bool,pub theme:Theme,pub date_format:DateFormat,pub number_format:NumberFormat,
        pub display_currency:Option<String>,pub hidden_features:Vec<String>,
    }
    pub struct AppPreferencesPatch {
        pub privacy_mask:Option<bool>,pub theme:Option<Theme>,pub date_format:Option<DateFormat>,pub number_format:Option<NumberFormat>,
        pub display_currency:Option<e::accounts::Patch<String>>,pub hidden_features:Option<Vec<String>>,
    }
    pub enum LayoutView {Dashboard,IncomeVsExpense,CategoryTrends,NetWorth,CashFlow,Calendar}
    pub struct LayoutItem {pub id:String,pub column:u8,pub row:u8,pub width:u8,pub height:u8}
    pub struct LayoutSpec {pub items:Vec<LayoutItem>}
    pub struct LayoutQuery {pub view:LayoutView}
    pub struct LayoutChange {pub view:LayoutView,pub layout:LayoutSpec}
}
impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            language: "en".into(),
            privacy_mask: false,
            theme: Theme::System,
            date_format: DateFormat::Iso,
            number_format: NumberFormat::International,
            display_currency: None,
            hidden_features: Feature::ALL
                .iter()
                .map(|feature| feature.key().to_owned())
                .collect(),
        }
    }
}
#[derive(Clone, Copy)]
pub enum Feature {
    Budgets,
    Goals,
    Assets,
    Recurring,
    SharedGroups,
    FxProvider,
}
impl Feature {
    const ALL: [Self; 6] = [
        Self::Budgets,
        Self::Goals,
        Self::Assets,
        Self::Recurring,
        Self::SharedGroups,
        Self::FxProvider,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Budgets => "budgets",
            Self::Goals => "goals",
            Self::Assets => "assets",
            Self::Recurring => "recurring",
            Self::SharedGroups => "shared-groups",
            Self::FxProvider => "fx-provider",
        }
    }
}
impl AppPreferences {
    pub fn enables(&self, feature: Feature) -> bool {
        !self.hidden_features.iter().any(|id| {
            let id = id.replace('_', "-");
            id.eq_ignore_ascii_case(feature.key())
                || matches!(feature, Feature::SharedGroups)
                    && matches!(id.as_str(), "groups" | "shared" | "sharedGroups")
                || matches!(feature, Feature::FxProvider)
                    && matches!(id.as_str(), "fx" | "fxProvider")
        })
    }
}
pub fn apply_patch(
    mut prefs: AppPreferences,
    patch: AppPreferencesPatch,
) -> Result<AppPreferences> {
    if let Some(value) = patch.theme {
        prefs.theme = value;
    }
    if let Some(value) = patch.date_format {
        prefs.date_format = value;
    }
    if let Some(value) = patch.number_format {
        prefs.number_format = value;
    }
    if let Some(value) = patch.privacy_mask {
        prefs.privacy_mask = value;
    }
    match patch.display_currency {
        Some(e::accounts::Patch::Set(code)) => {
            prefs.display_currency = Some(
                crate::domain::CurrencyCode::parse(&code)?
                    .as_str()
                    .to_owned(),
            )
        }
        Some(e::accounts::Patch::Clear) => prefs.display_currency = None,
        _ => {}
    }
    if let Some(features) = patch.hidden_features {
        if features.len() > 32
            || !features.iter().all(|id| identifier(id))
            || features.iter().collect::<BTreeSet<_>>().len() != features.len()
        {
            return Err(invalid("hiddenFeatures", "invalid_feature_preferences"));
        }
        prefs.hidden_features = features;
    }
    Ok(prefs)
}
fn identifier(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}
impl LayoutView {
    fn key(&self) -> &'static str {
        match self {
            Self::Dashboard => "layout.dashboard",
            Self::IncomeVsExpense => "layout.income_vs_expense",
            Self::CategoryTrends => "layout.category_trends",
            Self::NetWorth => "layout.net_worth",
            Self::CashFlow => "layout.cash_flow",
            Self::Calendar => "layout.calendar",
        }
    }
}
pub fn layout(ctx: &Context<'_>, view: &LayoutView) -> Result<LayoutSpec> {
    let value = ctx
        .tx
        .query_row(
            "SELECT value_json FROM ledger_settings WHERE key=?1",
            [view.key()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    value
        .map(|json| serde_json::from_str(&json).map_err(e::codec_error))
        .transpose()
        .map(|value| value.unwrap_or(LayoutSpec { items: Vec::new() }))
}
pub fn layout_save(ctx: &Context<'_>, change: &LayoutChange) -> Result<LayoutSpec> {
    let value = &change.layout;
    if value.items.len() > 32 {
        return Err(invalid("layout", "layout_item_limit"));
    }
    let mut placed = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for item in &value.items {
        let (right, bottom) = (
            u16::from(item.column) + u16::from(item.width),
            u16::from(item.row) + u16::from(item.height),
        );
        if !identifier(&item.id)
            || !ids.insert(&item.id)
            || item.width == 0
            || item.height == 0
            || right > 12
            || bottom > 100
        {
            return Err(invalid("layout", "invalid_layout_item"));
        }
        for row in u16::from(item.row)..bottom {
            for column in u16::from(item.column)..right {
                if !placed.insert((row, column)) {
                    return Err(invalid("layout", "overlapping_layout_items"));
                }
            }
        }
    }
    ctx.tx.execute("INSERT INTO ledger_settings VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",
        params![change.view.key(),serde_json::to_string(value).map_err(e::codec_error)?])?;
    Ok(value.clone())
}
impl ReceiptValue for LayoutSpec {
    type Reference = Self;
    fn reference(&self, _: &Context<'_>) -> Self {
        self.clone()
    }
    fn reload(_: &Context<'_>, value: Self) -> Result<Self> {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_preferences_accept_legacy_aliases_and_serialize_canonical_names() {
        use serde_json::{json, to_value};
        for (input, expected) in [
            ("system", "system"),
            ("latte", "latte"),
            ("frappe", "frappe"),
            ("macchiato", "macchiato"),
            ("mocha", "mocha"),
            ("light", "latte"),
            ("dark", "mocha"),
        ] {
            let mut saved = to_value(AppPreferences::default()).unwrap();
            saved["theme"] = json!(input);
            let saved: AppPreferences = serde_json::from_value(saved).unwrap();
            assert_eq!(to_value(saved).unwrap()["theme"], expected);

            let patch = serde_json::from_value(json!({"theme": input})).unwrap();
            let updated = apply_patch(AppPreferences::default(), patch).unwrap();
            assert_eq!(to_value(updated).unwrap()["theme"], expected);
        }
        assert!(serde_json::from_value::<AppPreferencesPatch>(json!({"theme": "unknown"})).is_err());
    }
}
