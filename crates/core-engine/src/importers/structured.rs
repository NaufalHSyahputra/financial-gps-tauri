use super::{
    ImportPlan, ImportPreview, ImportPreviewRow, MatchCandidate, ParserOptions, PreviewStatus,
    StatementFormat, StatementIssue,
};
use crate::sql::{OptionalExtension, params};
use crate::{
    domain::{AppError, ErrorCode, Result, parse_date},
    ledger_engine::{
        Context, TxStatus, VersionedId, codec_error, conflict, digest, dto,
        fx::{self, CurrencyPair, FxRateDraft, RateSource},
        invalid, minor, missing, money, revision, scale,
        staging::Stage,
    },
    planning::assets::{self, TradeDraft, TradeKind, ValuationDraft},
};
use std::collections::BTreeSet;

dto! {
    #[serde(tag="kind",content="value")]
    pub enum StructuredRecord {FxRate(FxRateDraft), AssetValuation(ValuationDraft),AssetTrade(TradeDraft)}
}
impl StructuredRecord {
    pub(super) fn kind(&self) -> &'static str {
        match self {
            Self::FxRate(_) => "fx_rate",
            Self::AssetValuation(_) => "asset_valuation",
            Self::AssetTrade(_) => "asset_transaction",
        }
    }
    fn date(&self) -> &str {
        match self {
            Self::FxRate(v) => &v.rate_date,
            Self::AssetValuation(v) => &v.valued_on,
            Self::AssetTrade(v) => &v.traded_on,
        }
    }
    fn magnitude(&self) -> &str {
        match self {
            Self::FxRate(_) => "0",
            Self::AssetValuation(v) => &v.value.minor,
            Self::AssetTrade(v) => match &v.trade {
                TradeKind::Buy { gross, .. } | TradeKind::Sell { gross, .. } => &gross.minor,
                TradeKind::Split { .. } => "0",
            },
        }
    }
}
fn parse_record(
    ctx: &Context<'_>,
    kind: &str,
    headers: &csv::StringRecord,
    row: &csv::StringRecord,
) -> Result<StructuredRecord> {
    let cell = |name: &str| {
        headers
            .iter()
            .position(|column| column == name)
            .and_then(|index| row.get(index))
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let required =
        |field: &'static str| cell(field).ok_or_else(|| invalid(field, "missing_csv_field"));
    let date = required("date")?;
    parse_date(date)?;
    if kind == "fx" {
        let pair = CurrencyPair {
            base_code: required("base")?.to_uppercase(),
            quote_code: required("quote")?.to_uppercase(),
        };
        fx::validate_pair(ctx, &pair)?;
        let rate_decimal = fx::rate(required("rate")?)?.normalize().to_string();
        let source_key = cell("source").unwrap_or("statement-csv");
        if source_key.chars().count() > 128 {
            return Err(invalid("source", "invalid_rate_source"));
        }
        return Ok(StructuredRecord::FxRate(FxRateDraft {
            pair,
            rate_date: date.to_owned(),
            rate_decimal,
            source_kind: RateSource::Csv,
            source_key: source_key.to_owned(),
        }));
    }
    let asset_id = required("asset_id")?;
    let (currency, archived) = ctx
        .tx
        .query_row(
            "SELECT currency_code,archived_on FROM assets WHERE id=?1",
            [asset_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?
        .ok_or_else(|| missing("assetId"))?;
    if archived.is_some() {
        return Err(conflict("asset_archived"));
    }
    let currency_scale = scale(&ctx.tx, &currency)?;
    let amount = |field: &'static str, default: Option<&str>| {
        let value = cell(field)
            .or(default)
            .ok_or_else(|| invalid(field, "missing_csv_field"))?;
        let minor = super::decimal_minor(value, currency_scale, '.', None)?;
        if minor < 0 {
            return Err(invalid(field, "negative_asset_amount"));
        }
        Ok(money(&currency, minor))
    };
    let exact_units = |field: &'static str| -> Result<String> {
        let value = required(field)?;
        let decimal = rust_decimal::Decimal::from_str_exact(value)
            .map_err(|_| invalid(field, "invalid_asset_units"))?;
        if decimal <= rust_decimal::Decimal::ZERO || decimal.scale() > 12 {
            return Err(invalid(field, "invalid_asset_units"));
        }
        Ok(decimal.normalize().to_string())
    };
    let trade = match cell("kind").unwrap_or("valuation") {
        "valuation" => {
            return Ok(StructuredRecord::AssetValuation(ValuationDraft {
                asset_id: asset_id.to_owned(),
                valued_on: date.to_owned(),
                value: amount("value", None)?,
            }));
        }
        kind @ ("buy" | "sell") => {
            let units = exact_units("units")?;
            let gross = amount("gross", None)?;
            let fees = amount("fees", Some("0"))?;
            if minor(&gross.minor)? == 0
                || (kind == "sell" && minor(&fees.minor)? > minor(&gross.minor)?)
            {
                return Err(invalid("gross", "invalid_asset_trade_amount"));
            }
            if kind == "buy" {
                TradeKind::Buy { units, gross, fees }
            } else {
                TradeKind::Sell { units, gross, fees }
            }
        }
        "split" => {
            let ratio = |field| -> Result<String> {
                let raw = required(field)?;
                let value = raw
                    .parse::<i64>()
                    .map_err(|_| invalid(field, "invalid_split_ratio"))?;
                if value <= 0 {
                    return Err(invalid(field, "invalid_split_ratio"));
                }
                Ok(value.to_string())
            };
            TradeKind::Split {
                old_units: ratio("old_units")?,
                new_units: ratio("new_units")?,
            }
        }
        _ => return Err(invalid("kind", "invalid_asset_csv_kind")),
    };
    Ok(StructuredRecord::AssetTrade(TradeDraft {
        asset_id: asset_id.to_owned(),
        traded_on: date.to_owned(),
        trade,
        linked_transaction_id: None,
    }))
}
fn candidate(ctx: &Context<'_>, record: &StructuredRecord) -> Result<Option<(String, bool)>> {
    match record{
        StructuredRecord::FxRate(draft)=>Ok(ctx.tx.query_row(
            "SELECT id,rate_decimal=?5 FROM fx_rates WHERE base_code=?1 AND quote_code=?2 AND rate_date=?3 AND source_kind='csv' AND source_key=?4",
            params![draft.pair.base_code,draft.pair.quote_code,draft.rate_date,draft.source_key,draft.rate_decimal],|row|Ok((row.get(0)?,row.get(1)?))
        ).optional()?),
        StructuredRecord::AssetValuation(draft)=>Ok(ctx.tx.query_row(
            "SELECT id,value_minor=?3 FROM asset_valuations WHERE asset_id=?1 AND valued_on=?2",
            params![draft.asset_id,draft.valued_on,minor(&draft.value.minor)?],|row|Ok((row.get(0)?,row.get(1)?))
        ).optional()?),
        StructuredRecord::AssetTrade(draft)=>{
            let id:Option<String>=ctx.tx.query_row("SELECT e.entity_id FROM import_effects e JOIN import_batches b ON b.id=e.batch_id
                JOIN asset_transactions t ON t.id=e.entity_id WHERE e.entity_kind='asset_transaction' AND e.after_hash=?1
                AND t.asset_id=?2 AND t.trade_date=?3 AND b.undone_at_ms IS NULL ORDER BY b.committed_at_ms DESC LIMIT 1",
                params![digest(record)?,draft.asset_id,draft.traded_on],|r|r.get(0)).optional()?;
            if let Some(id)=id && load(ctx,record.kind(),&id).is_ok_and(|existing|digest(&existing).ok()==digest(record).ok()){return Ok(Some((id,true)));}
            Ok(None)
        }
    }
}
fn identity(record: &StructuredRecord) -> Result<String> {
    match record {
        StructuredRecord::FxRate(draft) => {
            digest(&(&draft.pair, &draft.rate_date, &draft.source_key))
        }
        StructuredRecord::AssetValuation(draft) => digest(&(&draft.asset_id, &draft.valued_on)),
        StructuredRecord::AssetTrade(_) => digest(record),
    }
}
pub(super) fn preview(
    ctx: &Context<'_>,
    store: &Stage<ImportPlan>,
    file: &str,
    content: &[u8],
    kind: &str,
    cancelled: &dyn Fn() -> bool,
    progress: &dyn Fn(u32),
) -> Result<ImportPreview> {
    if !matches!(kind, "fx" | "asset") || content.len() > super::MAX_FILE_BYTES || file.len() > 256
    {
        return Err(invalid("kind", "invalid_import_kind"));
    }
    let text = super::parsers::content(&mut &content[..], None, false)?;
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .flexible(true)
        .from_reader(text.as_bytes());
    let headers = reader
        .headers()
        .map_err(|_| invalid("file", "invalid_csv_headers"))?
        .clone();
    if headers.len() > 32
        || headers.iter().collect::<BTreeSet<_>>().len() != headers.len()
        || !headers.iter().any(|field| field == "date")
    {
        return Err(invalid("file", "invalid_csv_headers"));
    }
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, row) in reader.records().enumerate() {
        if cancelled() {
            return Err(AppError::new(ErrorCode::Cancelled, "operation_cancelled"));
        }
        if index >= super::parsers::MAX_ROWS {
            return Err(invalid("file", "statement_row_limit"));
        }
        let ordinal = (index + 1) as u32;
        let mut result = ImportPreviewRow {
            ordinal,
            status: PreviewStatus::Ready,
            raw: None,
            proposed: None,
            candidates: Vec::new(),
            issue: None,
            record: None,
        };
        let parsed = row
            .map_err(|_| invalid("row", "malformed_csv_row"))
            .and_then(|row| {
                if row.len() != headers.len() || row.iter().any(|cell| cell.len() > 4096) {
                    return Err(invalid("row", "invalid_csv_row"));
                }
                parse_record(ctx, kind, &headers, &row)
            });
        match parsed {
            Ok(record) => {
                let existing = candidate(ctx, &record)?;
                if !seen.insert(identity(&record)?) {
                    result.status = PreviewStatus::Error;
                    result.issue = Some(StatementIssue {
                        ordinal,
                        field: "row".into(),
                        code: "duplicate_identity_in_file".into(),
                    });
                } else if let Some((id, equal)) = existing {
                    result.status = if equal {
                        PreviewStatus::Duplicate
                    } else {
                        PreviewStatus::Ambiguous
                    };
                    result.candidates.push(MatchCandidate {
                        id,
                        bank_date: record.date().to_owned(),
                        amount_minor: record.magnitude().to_owned(),
                        status: TxStatus::Posted,
                    });
                }
                result.record = Some(record);
            }
            Err(error) => {
                result.status = PreviewStatus::Error;
                result.issue = Some(StatementIssue {
                    ordinal,
                    field: "row".into(),
                    code: error.message_key.to_owned(),
                });
            }
        }
        rows.push(result);
        progress(rows.len() as u32);
    }
    let plan = ImportPlan {
        rows,
        account: None,
        kind: kind.to_owned(),
        path_name: file.to_owned(),
        hash: crate::domain::sha256(content),
        format: StatementFormat::Csv,
        template: None,
        options: ParserOptions::default(),
        day: ctx.today.to_string(),
    };
    super::store_preview(ctx, store, plan)
}
pub(super) fn load(ctx: &Context<'_>, kind: &str, id: &str) -> Result<StructuredRecord> {
    match kind{
        "fx_rate"=>Ok(StructuredRecord::FxRate(ctx.tx.query_row(
            "SELECT base_code,quote_code,rate_date,rate_decimal,source_key FROM fx_rates WHERE id=?1 AND source_kind='csv'",[id],|r|
            Ok(FxRateDraft{pair:CurrencyPair{base_code:r.get(0)?,quote_code:r.get(1)?},rate_date:r.get(2)?,rate_decimal:r.get(3)?,
                source_kind:RateSource::Csv,source_key:r.get(4)?})).optional()?.ok_or_else(||missing("fxRateId"))?)),
        "asset_valuation"=>Ok(StructuredRecord::AssetValuation(assets::valuation_get(ctx,id)?.draft)),
        "asset_transaction"=>Ok(StructuredRecord::AssetTrade(assets::trade_get(ctx,id)?.draft)),
        _=>Err(invalid("kind","invalid_import_kind")),
    }
}
type Stored = (String, Option<String>, String);
pub(super) fn save(
    ctx: &Context<'_>,
    record: &StructuredRecord,
    id: Option<&str>,
) -> Result<Stored> {
    let kind = record.kind();
    let target = id
        .map(|id| {
            Ok::<_, AppError>(VersionedId {
                id: id.to_owned(),
                version: revision(&ctx.tx)?,
            })
        })
        .transpose()?;
    let before = id
        .map(|id| -> Result<String> {
            if kind == "asset_valuation" {
                return serde_json::to_string(&assets::valuation_get(ctx, id)?)
                    .map_err(codec_error);
            }
            serde_json::to_string(&load(ctx, kind, id)?).map_err(codec_error)
        })
        .transpose()?;
    let saved = match record {
        StructuredRecord::FxRate(draft) => fx::save(ctx, target.as_ref(), draft)?.id,
        StructuredRecord::AssetValuation(draft) => {
            let v = assets::valuation_save(ctx, target.as_ref(), draft)?;
            ctx.tx.execute(
                "UPDATE asset_valuations SET source='import' WHERE id=?1",
                [&v.id],
            )?;
            v.id
        }
        StructuredRecord::AssetTrade(draft) => assets::trade_save(ctx, target.as_ref(), draft)?.id,
    };
    Ok((saved.clone(), before, digest(&load(ctx, kind, &saved)?)?))
}
pub(super) fn revert(
    ctx: &Context<'_>,
    kind: &str,
    id: &str,
    before: Option<String>,
) -> Result<()> {
    if let Some(before) = before {
        if kind == "asset_valuation" {
            let before = serde_json::from_str::<assets::Valuation>(&before).map_err(codec_error)?;
            ctx.tx.execute(
                "UPDATE asset_valuations SET value_minor=?2,source=?3 WHERE id=?1",
                params![id, minor(&before.draft.value.minor)?, before.source],
            )?;
        } else {
            let record = serde_json::from_str(&before).map_err(codec_error)?;
            save(ctx, &record, Some(id))?;
        }
    } else {
        match kind {
            "fx_rate" => fx::delete(ctx, id)?,
            "asset_valuation" => assets::valuation_delete(ctx, id, true)?,
            "asset_transaction" => assets::trade_delete(ctx, id, true)?,
            _ => return Err(invalid("kind", "invalid_import_kind")),
        };
    }
    Ok(())
}
pub(super) fn effect_hash(ctx: &Context<'_>, kind: &str, id: &str) -> Result<String> {
    digest(&load(ctx, kind, id)?)
}
