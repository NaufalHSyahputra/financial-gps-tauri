use super::{StructuredRecord, structured};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use crate::sql::{OptionalExtension, params, params_from_iter};

use super::{
    ParseConfig, ParsedRow, ParsedStatementRow, ParserOptions, StatementFormat, StatementIssue,
    validate_options,
};
use crate::{
    domain::{AppError, ErrorCode, Result},
    ledger_engine::{
        Context, Deleted, Page, PageReq, ReadPage, ReceiptValue, TxStatus, VersionedId, accounts,
        check_revision, codec_error, conflict, digest, dto, invalid, missing, money, name, new_id,
        positive, revision, scale,
        staging::Stage,
        transactions::{self as tx, Query, TxDetail, TxDraft},
    },
    rules::ImportRules,
};

pub const MAX_FILE_BYTES: usize = 20 * 1024 * 1024;

dto! {
    pub struct TemplateDraft {pub name:String,pub institution:Option<String>,pub format:StatementFormat,
        pub schema_version:u8,pub options:ParserOptions}
    pub struct ImportTemplate {pub id:String,pub version:String,pub draft:TemplateDraft}
    pub enum DocumentFormat { Json, Toml }
    #[derive(PartialEq,Eq)]
    pub enum PreviewStatus { Ready, Error, Duplicate, Ambiguous, PendingMatch }
    pub struct ImportPreviewRow {pub ordinal:u32,pub status:PreviewStatus,pub raw:Option<ParsedStatementRow>,pub proposed:Option<TxDraft>,
        pub candidates:Vec<MatchCandidate>,pub issue:Option<StatementIssue>,pub record:Option<StructuredRecord>}
    pub struct MatchCandidate {pub id:String,pub bank_date:String,pub amount_minor:String,pub status:TxStatus}
    pub struct ImportPreview {pub preview_token:String,pub revision:String,pub format:StatementFormat,pub ready:u32,pub duplicate:u32,pub review:u32,pub errors:u32}
    #[serde(tag="kind",rename_all_fields="camelCase")]
    pub enum RowAction { Insert, Skip, Reconcile { transaction_id:String } }
    pub struct RowDecision {pub ordinal:u32,pub action:RowAction}
    pub struct ImportBatch {pub id:String,pub name:String,pub account_id:Option<String>,pub kind:String,pub format:StatementFormat,
        pub committed_at:String,pub undone_at:Option<String>,pub inserted:u32,pub updated:u32,pub skipped:u32}
    pub struct UndoRow {pub ordinal:u32,pub entity_id:String,pub blocked:Option<String>}
    pub struct UndoPreview {pub preview_token:String,pub rows:Vec<UndoRow>,pub revision:String}
    pub struct UndoResult {pub batch_id:String,pub completed:Vec<u32>,pub fully_undone:bool}
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Counts {
    #[serde(default)]
    inserted: u32,
    #[serde(default)]
    updated: u32,
    #[serde(default)]
    skipped: u32,
    #[serde(default)]
    resolved: BTreeSet<u32>,
}

pub struct ImportPlan {
    pub(super) rows: Vec<ImportPreviewRow>,
    pub(super) account: Option<String>,
    pub(super) kind: String,
    pub(super) path_name: String,
    pub(super) hash: String,
    pub(super) format: StatementFormat,
    pub(super) template: Option<String>,
    pub(super) options: ParserOptions,
    pub(super) day: String,
}
pub struct UndoPlan {
    batch: String,
    rows: Vec<UndoRow>,
}
pub struct PreviewRequest {
    pub account_id: String,
    pub format: Option<StatementFormat>,
    pub template_id: Option<String>,
    pub parser_options: Option<ParserOptions>,
    pub detect_duplicates: Option<bool>,
}

const TEMPLATES: &str =
    "SELECT id,name,institution,format,schema_version,mapping_json FROM import_templates";
fn template_from(row: &crate::sql::Row<'_>, version: &str) -> crate::sql::Result<ImportTemplate> {
    let json = row.get::<_, String>(5)?;
    let options = serde_json::from_str(&json).map_err(|_| crate::sql::Error::InvalidDocument)?;
    Ok(ImportTemplate {
        id: row.get(0)?,
        version: version.to_owned(),
        draft: TemplateDraft {
            name: row.get(1)?,
            institution: row.get(2)?,
            format: StatementFormat::from_sql(row.get_ref(3)?.as_str()?)?,
            schema_version: row.get(4)?,
            options,
        },
    })
}
pub fn templates(
    ctx: &Context<'_>,
    format: Option<StatementFormat>,
) -> Result<Vec<ImportTemplate>> {
    let version = revision(&ctx.tx)?;
    Ok(ctx
        .tx
        .prepare(&format!(
            "{TEMPLATES} WHERE ?1 IS NULL OR format=?1 ORDER BY name,id"
        ))?
        .query_map([format.map(StatementFormat::as_str)], |row| {
            template_from(row, &version)
        })?
        .collect::<crate::sql::Result<_>>()?)
}
pub fn template_get(ctx: &Context<'_>, id: &str) -> Result<ImportTemplate> {
    let version = revision(&ctx.tx)?;
    ctx.tx
        .query_row(&format!("{TEMPLATES} WHERE id=?1"), [id], |row| {
            template_from(row, &version)
        })
        .optional()?
        .ok_or_else(|| missing("templateId"))
}
pub fn template_save(
    ctx: &Context<'_>,
    target: Option<&VersionedId>,
    draft: &TemplateDraft,
) -> Result<ImportTemplate> {
    if draft.schema_version != 1 {
        return Err(invalid("schemaVersion", "unsupported_template_schema"));
    }
    validate_options(&draft.options, draft.format)?;
    let template_name = name(&draft.name)?;
    if draft
        .institution
        .as_ref()
        .is_some_and(|value| value.len() > 256)
    {
        return Err(invalid("institution", "invalid_institution"));
    }
    let id = if let Some(target) = target {
        check_revision(ctx, target)?;
        template_get(ctx, &target.id)?;
        target.id.clone()
    } else {
        new_id()
    };
    let options = serde_json::to_string(&draft.options).map_err(codec_error)?;
    ctx.tx.execute("INSERT INTO import_templates VALUES(?1,?2,?3,?4,1,?5,?6) ON CONFLICT(id) DO UPDATE SET
        name=excluded.name,institution=excluded.institution,format=excluded.format,schema_version=excluded.schema_version,
        mapping_json=excluded.mapping_json,updated_at_ms=excluded.updated_at_ms",
        params![id,template_name,draft.institution,draft.format.as_str(),options,ctx.now_ms])?;
    let mut saved = template_get(ctx, &id)?;
    saved.version = accounts::next_version(ctx)?;
    Ok(saved)
}
pub fn template_delete(ctx: &Context<'_>, id: &str) -> Result<Deleted> {
    if ctx
        .tx
        .execute("DELETE FROM import_templates WHERE id=?1", [id])?
        == 0
    {
        return Err(missing("templateId"));
    }
    Ok(Deleted { id: id.to_owned() })
}

pub fn decode_template(content: &[u8], format: &DocumentFormat) -> Result<TemplateDraft> {
    if content.len() > 65_536 {
        return Err(invalid("file", "template_size_limit"));
    }
    let text =
        std::str::from_utf8(content).map_err(|_| invalid("file", "invalid_template_encoding"))?;
    let draft: TemplateDraft = match format {
        DocumentFormat::Json => {
            serde_json::from_str(text).map_err(|_| invalid("file", "invalid_template_document"))?
        }
        DocumentFormat::Toml => {
            toml::from_str(text).map_err(|_| invalid("file", "invalid_template_document"))?
        }
    };
    if draft.schema_version != 1 {
        return Err(invalid("schemaVersion", "unsupported_template_schema"));
    }
    validate_options(&draft.options, draft.format)?;
    Ok(draft)
}
pub fn encode_template(template: &ImportTemplate, format: &DocumentFormat) -> Result<Vec<u8>> {
    let text = match format {
        DocumentFormat::Json => {
            serde_json::to_string_pretty(&template.draft).map_err(codec_error)?
        }
        DocumentFormat::Toml => toml::to_string_pretty(&template.draft)
            .map_err(|_| invalid("file", "invalid_template_document"))?,
    };
    Ok(text.into_bytes())
}

fn tx_draft(account: &str, row: &ParsedStatementRow) -> TxDraft {
    TxDraft {
        account_id: account.to_owned(),
        bank_date: row.bank_date.clone(),
        amount: money(&row.currency, &row.amount_minor),
        direction: row.direction.clone(),
        status: row.status.clone(),
        description: row.raw_description.clone(),
        notes: row.notes.clone().unwrap_or_default(),
        payee_id: None,
        category_id: None,
        splits: Vec::new(),
        effective_bill_date: None,
        reporting_date_override: None,
        ignored: false,
        exclude_from_pnl: false,
        flagged: false,
        flag_due_date: None,
        alias_id: None,
        statement_id: None,
        installment: None,
        original: None,
    }
}
fn match_candidates(
    ctx: &Context<'_>,
    account: &str,
    row: &ParsedStatementRow,
    fallback: bool,
) -> Result<(Vec<MatchCandidate>, bool)> {
    let hard = row.external_key(account);
    let fingerprint = row.fingerprint(account);
    let mut stmt=ctx.tx.prepare_cached("SELECT t.id,t.bank_date,CAST(t.amount_minor AS TEXT),t.status,
        max(k.external_key_hash IS ?2 AND ?2 IS NOT NULL) FROM transactions t JOIN transaction_import_keys k ON k.transaction_id=t.id
        WHERE t.account_id=?1 AND ((?2 IS NOT NULL AND k.external_key_hash=?2) OR
          (?3 AND k.fingerprint=?4) OR (?5 IS NOT NULL AND k.provider_namespace=?6 AND k.external_id=?5))
        GROUP BY t.id ORDER BY t.id LIMIT 21")?;
    let values = stmt
        .query_map(
            params![
                account,
                hard,
                fallback,
                fingerprint,
                row.external_id,
                row.provider_namespace
            ],
            |r| Ok((candidate(r)?, r.get::<_, bool>(4)?)),
        )?
        .collect::<crate::sql::Result<Vec<_>>>()?;
    let exact = values.iter().any(|(_, hard)| *hard);
    let mut candidates = values
        .into_iter()
        .map(|(candidate, _)| candidate)
        .collect::<Vec<_>>();
    if !exact && matches!(row.status, TxStatus::Posted) && (fallback || row.external_id.is_some()) {
        let mut stmt=ctx.tx.prepare_cached("SELECT id,bank_date,CAST(amount_minor AS TEXT),status FROM transactions
            WHERE account_id=?1 AND (status='pending' OR (status='projected' AND EXISTS(
                SELECT 1 FROM recurring_occurrences occurrence WHERE occurrence.transaction_id=transactions.id AND occurrence.state='generated')))
            AND currency_code=?2 AND direction=?3 AND amount_minor=?4
            AND bank_date BETWEEN date(?5,'-7 days') AND date(?5,'+7 days')
            AND financial_normalize(description)=?6 ORDER BY bank_date,id LIMIT 21")?;
        let mut pending = stmt
            .query_map(
                params![
                    account,
                    row.currency,
                    row.direction.as_str(),
                    positive(&row.amount_minor)?,
                    row.bank_date,
                    crate::domain::normalize(&row.raw_description)
                ],
                candidate,
            )?
            .collect::<crate::sql::Result<Vec<_>>>()?;
        pending.retain(|candidate| !candidates.iter().any(|current| current.id == candidate.id));
        candidates.extend(pending);
    }
    Ok((candidates, exact))
}
fn candidate(r: &crate::sql::Row<'_>) -> crate::sql::Result<MatchCandidate> {
    let status = match r.get_ref(3)?.as_str()? {
        "posted" => TxStatus::Posted,
        "pending" => TxStatus::Pending,
        "projected" => TxStatus::Projected,
        _ => return Err(crate::sql::Error::InvalidQuery),
    };
    Ok(MatchCandidate {
        id: r.get(0)?,
        bank_date: r.get(1)?,
        amount_minor: r.get(2)?,
        status,
    })
}

pub fn preview(
    ctx: &Context<'_>,
    store: &Stage<ImportPlan>,
    file_name: &str,
    bytes: &[u8],
    request: &PreviewRequest,
    cancelled: &dyn Fn() -> bool,
) -> Result<ImportPreview> {
    preview_observed(ctx, store, file_name, bytes, request, cancelled, &|_| {})
}

pub fn preview_observed(
    ctx: &Context<'_>,
    store: &Stage<ImportPlan>,
    file_name: &str,
    bytes: &[u8],
    request: &PreviewRequest,
    cancelled: &dyn Fn() -> bool,
    progress: &dyn Fn(u32),
) -> Result<ImportPreview> {
    if bytes.len() > MAX_FILE_BYTES || file_name.len() > 256 {
        return Err(invalid("file", "statement_size_limit"));
    }
    let (account, archived) = accounts::raw(ctx, &request.account_id)?;
    if archived.is_some() {
        return Err(conflict("account_archived"));
    }
    let template = request
        .template_id
        .as_deref()
        .map(|id| template_get(ctx, id))
        .transpose()?;
    let format = match request.format.or(template.as_ref().map(|t| t.draft.format)) {
        Some(format) => format,
        None => StatementFormat::detect(
            &bytes[..bytes.len().min(8192)],
            Path::new(file_name).extension().and_then(|s| s.to_str()),
        )?,
    };
    if template.as_ref().is_some_and(|t| t.draft.format != format) {
        return Err(invalid("format", "template_format_mismatch"));
    }
    let options = request
        .parser_options
        .clone()
        .or_else(|| template.map(|t| t.draft.options))
        .unwrap_or_default();
    validate_options(&options, format)?;
    let currency = options
        .currency
        .as_deref()
        .unwrap_or(&account.currency_code);
    if currency != account.currency_code {
        return Err(invalid("currency", "statement_currency_mismatch"));
    }
    let mut rows = Vec::new();
    let mut fingerprint_counts = BTreeMap::new();
    let mut external_counts = BTreeMap::new();
    let rules = ImportRules::load(ctx)?;
    let fallback = format != StatementFormat::Csv || request.detect_duplicates.unwrap_or(true);
    format.parser().parse(
        &mut &*bytes,
        &ParseConfig {
            options: &options,
            currency,
            scale: scale(&ctx.tx, currency)?,
        },
        cancelled,
        &mut |input| {
            match input {
                ParsedRow::Error(issue) => rows.push(ImportPreviewRow {
                    ordinal: issue.ordinal,
                    status: PreviewStatus::Error,
                    raw: None,
                    proposed: None,
                    candidates: Vec::new(),
                    issue: Some(issue),
                    record: None,
                }),
                ParsedRow::Ready(row) => {
                    let (candidates, exact) =
                        match_candidates(ctx, &request.account_id, &row, fallback)?;
                    let status = if candidates
                        .iter()
                        .any(|c| matches!(c.status, TxStatus::Pending | TxStatus::Projected))
                        && matches!(row.status, TxStatus::Posted)
                    {
                        PreviewStatus::PendingMatch
                    } else if exact {
                        PreviewStatus::Duplicate
                    } else if !candidates.is_empty() {
                        PreviewStatus::Ambiguous
                    } else {
                        PreviewStatus::Ready
                    };
                    if fallback {
                        *fingerprint_counts
                            .entry(row.fingerprint(&request.account_id))
                            .or_insert(0usize) += 1;
                    }
                    if let Some(key) = row.external_key(&request.account_id) {
                        *external_counts.entry(key).or_insert(0usize) += 1;
                    }
                    let proposed = rules.categorize(
                        ctx,
                        &tx_draft(&request.account_id, &row),
                        row.provider_category.as_deref(),
                    )?;
                    rows.push(ImportPreviewRow {
                        ordinal: row.ordinal,
                        status,
                        raw: Some(row),
                        proposed: Some(proposed),
                        candidates,
                        issue: None,
                        record: None,
                    });
                }
            }
            progress(rows.len() as u32);
            Ok(())
        },
    )?;
    for row in &mut rows {
        if row.status != PreviewStatus::Ready {
            continue;
        }
        if let Some(raw) = &row.raw
            && ((fallback
                && fingerprint_counts
                    .get(&raw.fingerprint(&request.account_id))
                    .is_some_and(|n| *n > 1))
                || raw
                    .external_key(&request.account_id)
                    .and_then(|key| external_counts.get(&key))
                    .is_some_and(|n| *n > 1))
        {
            row.status = PreviewStatus::Ambiguous;
        }
    }
    let hash = crate::domain::sha256(bytes);
    store_preview(
        ctx,
        store,
        ImportPlan {
            rows,
            account: Some(request.account_id.clone()),
            kind: "statement".to_owned(),
            path_name: file_name.to_owned(),
            hash,
            format,
            template: request.template_id.clone(),
            options,
            day: ctx.today.to_string(),
        },
    )
}

pub fn preview_structured(
    ctx: &Context<'_>,
    store: &Stage<ImportPlan>,
    file: &str,
    bytes: &[u8],
    kind: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<ImportPreview> {
    preview_structured_observed(ctx, store, file, bytes, kind, cancelled, &|_| {})
}

pub fn preview_structured_observed(
    ctx: &Context<'_>,
    store: &Stage<ImportPlan>,
    file: &str,
    bytes: &[u8],
    kind: &str,
    cancelled: &dyn Fn() -> bool,
    progress: &dyn Fn(u32),
) -> Result<ImportPreview> {
    structured::preview(ctx, store, file, bytes, kind, cancelled, progress)
}

pub(super) fn store_preview(
    ctx: &Context<'_>,
    store: &Stage<ImportPlan>,
    plan: ImportPlan,
) -> Result<ImportPreview> {
    let count = |status| plan.rows.iter().filter(|row| row.status == status).count() as u32;
    let ready = count(PreviewStatus::Ready);
    let duplicate = count(PreviewStatus::Duplicate);
    let errors = count(PreviewStatus::Error);
    let review = plan.rows.len() as u32 - ready - duplicate - errors;
    let format = plan.format;
    let preview_token = store.put(ctx, plan)?;
    Ok(ImportPreview {
        preview_token,
        revision: revision(&ctx.tx)?,
        format,
        ready,
        duplicate,
        review,
        errors,
    })
}

pub fn is_asset_preview(ctx: &Context<'_>, store: &Stage<ImportPlan>, token: &str) -> Result<bool> {
    Ok(store.get(ctx, token)?.kind == "asset")
}
pub fn is_asset_undo(ctx: &Context<'_>, store: &Stage<UndoPlan>, token: &str) -> Result<bool> {
    Ok(batch_get(ctx, &store.get(ctx, token)?.batch)?.kind == "asset")
}

pub fn preview_rows(
    ctx: &Context<'_>,
    store: &Stage<ImportPlan>,
    token: &str,
    status: Option<&PreviewStatus>,
    page: &PageReq,
) -> Result<Page<ImportPreviewRow>> {
    let plan = store.get(ctx, token)?;
    let cursor = ReadPage::new(ctx, page, &("import_preview", token, status))?;
    let after = cursor
        .after
        .as_ref()
        .map(|(_, id)| {
            id.parse::<u32>()
                .map_err(|_| invalid("cursor", "invalid_cursor"))
        })
        .transpose()?
        .unwrap_or(0);
    let rows = plan
        .rows
        .iter()
        .filter(|row| row.ordinal > after && status.is_none_or(|status| status == &row.status))
        .take(cursor.limit + 1)
        .cloned()
        .collect();
    cursor.finish(rows, |row| {
        (row.ordinal.to_string(), row.ordinal.to_string())
    })
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct OccurrenceSnapshot {
    rule_id: String,
    nominal_date: String,
    scheduled_date: String,
    state: String,
    rule_version: i64,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct PostingSnapshot {
    draft: TxDraft,
    occurrence: Option<OccurrenceSnapshot>,
}
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum BeforePosting {
    Snapshot(Box<PostingSnapshot>),
    Legacy(Box<TxDraft>),
}
fn occurrence(ctx: &Context<'_>, id: &str) -> Result<Option<OccurrenceSnapshot>> {
    Ok(ctx.tx.query_row("SELECT rule_id,nominal_date,scheduled_date,state,rule_version FROM recurring_occurrences WHERE transaction_id=?1",[id],
        |row|Ok(OccurrenceSnapshot{rule_id:row.get(0)?,nominal_date:row.get(1)?,scheduled_date:row.get(2)?,state:row.get(3)?,rule_version:row.get(4)?})).optional()?)
}
fn snapshot_hash(ctx: &Context<'_>, detail: &TxDetail) -> Result<String> {
    let fields = (&detail.draft, &detail.row.version, &detail.row.transfer_id);
    if let Some(occurrence) = occurrence(ctx, &detail.row.id)? {
        digest(&(fields, occurrence))
    } else {
        digest(&fields)
    }
}
fn before_snapshot(raw: &str) -> Result<PostingSnapshot> {
    match serde_json::from_str(raw).map_err(codec_error)? {
        BeforePosting::Snapshot(snapshot) => Ok(*snapshot),
        BeforePosting::Legacy(draft) => Ok(PostingSnapshot {
            draft: *draft,
            occurrence: None,
        }),
    }
}
fn save_key(
    ctx: &Context<'_>,
    batch: &str,
    txid: &str,
    account: &str,
    raw: &ParsedStatementRow,
) -> Result<()> {
    let key = raw.external_key(account);
    if let Some(key) = &key {
        let before = ctx
            .tx
            .query_row(
                "SELECT transaction_id FROM transaction_import_keys WHERE external_key_hash=?1",
                [key],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        if let Some(before) = before {
            if before == txid {
                return Ok(());
            }
            return Err(conflict("external_identity_already_imported"));
        }
    }
    ctx.tx.execute("INSERT INTO transaction_import_keys(transaction_id,batch_id,provider_namespace,external_id,external_key_hash,fingerprint,
        raw_description,provider_category,raw_bank_date,raw_amount_minor,raw_currency,raw_direction) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![txid,batch,raw.provider_namespace,raw.external_id,key,raw.fingerprint(account),raw.raw_description,raw.provider_category,raw.bank_date,
            positive(&raw.amount_minor)?,raw.currency,raw.direction.as_str()])?;
    Ok(())
}

pub fn commit(
    ctx: &Context<'_>,
    store: &Stage<ImportPlan>,
    token: &str,
    decisions: &[RowDecision],
    cancelled: &dyn Fn() -> bool,
) -> Result<ImportBatch> {
    let plan = store.get(ctx, token)?;
    if ctx.today.to_string() != plan.day {
        return Err(crate::ledger_engine::stale_revision());
    }
    let map = decisions
        .iter()
        .map(|item| (item.ordinal, &item.action))
        .collect::<BTreeMap<_, _>>();
    if map.len() != decisions.len()
        || map.keys().any(|ordinal| {
            plan.rows
                .binary_search_by_key(ordinal, |row| row.ordinal)
                .is_err()
        })
    {
        return Err(invalid("rowDecisions", "invalid_import_decisions"));
    }
    let batch = new_id();
    ctx.tx.execute("INSERT INTO import_batches(id,account_id,kind,template_id,file_name,content_sha256,format,committed_at_ms,mapping_snapshot_json,summary_json)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'{}')",
        params![batch,plan.account,plan.kind,plan.template,plan.path_name,plan.hash,plan.format.as_str(),ctx.now_ms,serde_json::to_string(&plan.options).map_err(codec_error)?])?;
    let mut counts = Counts::default();
    let mut updated = BTreeSet::new();
    for row in &plan.rows {
        if cancelled() {
            return Err(AppError::new(ErrorCode::Cancelled, "operation_cancelled"));
        }
        let action = match map.get(&row.ordinal).copied() {
            Some(action) => action,
            None => match row.status {
                PreviewStatus::Ready => &RowAction::Insert,
                PreviewStatus::Error | PreviewStatus::Duplicate => &RowAction::Skip,
                _ => return Err(invalid("rowDecisions", "import_review_required")),
            },
        };
        if matches!(action, RowAction::Skip) {
            counts.skipped += 1;
            continue;
        }
        if let Some(record) = row.record.as_ref() {
            if row.status == PreviewStatus::Error {
                return Err(invalid("rowDecisions", "invalid_import_row"));
            }
            let target = match action {
                RowAction::Insert if row.status == PreviewStatus::Ready => None,
                RowAction::Reconcile { transaction_id }
                    if row.status == PreviewStatus::Ambiguous
                        && row.candidates.iter().any(|item| &item.id == transaction_id) =>
                {
                    if !updated.insert(transaction_id.clone()) {
                        return Err(invalid("rowDecisions", "invalid_import_match"));
                    }
                    Some(transaction_id.as_str())
                }
                _ => return Err(invalid("rowDecisions", "invalid_import_match")),
            };
            let (id, before, hash) = structured::save(ctx, record, target)?;
            let operation = if before.is_some() {
                counts.updated += 1;
                "update"
            } else {
                counts.inserted += 1;
                "insert"
            };
            ctx.tx.execute(
                "INSERT INTO import_effects VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    batch,
                    row.ordinal,
                    record.kind(),
                    id,
                    operation,
                    before,
                    hash
                ],
            )?;
            continue;
        }
        let account = plan
            .account
            .as_deref()
            .ok_or_else(|| invalid("kind", "invalid_import_kind"))?;
        let raw = row
            .raw
            .as_ref()
            .ok_or_else(|| invalid("rowDecisions", "invalid_import_row"))?;
        let (detail, before) = match action {
            RowAction::Insert if row.status != PreviewStatus::Duplicate => {
                let draft = row.proposed.as_ref().expect("parsed proposal");
                counts.inserted += 1;
                (tx::save(ctx, None, draft)?, None)
            }
            RowAction::Reconcile { transaction_id } => {
                if !row.candidates.iter().any(|c| &c.id == transaction_id)
                    || !updated.insert(transaction_id.clone())
                {
                    return Err(invalid("rowDecisions", "invalid_import_match"));
                }
                let existing = tx::get(ctx, transaction_id)?;
                let mut draft = existing.draft.clone();
                if draft.account_id != account
                    || draft.amount.currency != raw.currency
                    || draft.direction.as_str() != raw.direction.as_str()
                    || (matches!(draft.status, TxStatus::Posted)
                        && !matches!(raw.status, TxStatus::Posted))
                {
                    return Err(conflict("incompatible_import_match"));
                }
                if draft.original.is_some() && draft.amount.minor != raw.amount_minor {
                    return Err(conflict("import_match_requires_fx_review"));
                }
                draft.amount.minor = raw.amount_minor.clone();
                draft.bank_date = raw.bank_date.clone();
                draft.status = raw.status.clone();
                counts.updated += 1;
                let occurrence = occurrence(ctx, transaction_id)?;
                let before = serde_json::to_string(&PostingSnapshot {
                    draft: existing.draft,
                    occurrence: occurrence.clone(),
                })
                .map_err(codec_error)?;
                let detail = tx::reconcile_import(
                    ctx,
                    &VersionedId {
                        id: transaction_id.clone(),
                        version: existing.row.version,
                    },
                    &draft,
                )?;
                if let Some(occurrence) = occurrence
                    && occurrence.state == "generated"
                    && matches!(raw.status, TxStatus::Posted)
                {
                    ctx.tx.execute("UPDATE recurring_occurrences SET state='matched' WHERE rule_id=?1 AND nominal_date=?2 AND transaction_id=?3",
                        params![occurrence.rule_id,occurrence.nominal_date,transaction_id])?;
                }
                (detail, Some(before))
            }
            _ => return Err(conflict("external_identity_already_imported")),
        };
        save_key(ctx, &batch, &detail.row.id, account, raw)?;
        ctx.tx.execute(
            "INSERT INTO import_effects VALUES(?1,?2,'transaction',?3,?4,?5,?6)",
            params![
                batch,
                row.ordinal,
                detail.row.id,
                if before.is_some() { "update" } else { "insert" },
                before,
                snapshot_hash(ctx, &detail)?
            ],
        )?;
    }
    ctx.tx.execute(
        "UPDATE import_batches SET summary_json=?2 WHERE id=?1",
        params![batch, serde_json::to_string(&counts).map_err(codec_error)?],
    )?;
    batch_get(ctx, &batch)
}

const BATCHES: &str = "SELECT id,file_name,account_id,kind,format,committed_at_ms,undone_at_ms,summary_json FROM import_batches WHERE 1=1";
type BatchRow = (
    String,
    String,
    Option<String>,
    String,
    StatementFormat,
    i64,
    Option<i64>,
    String,
);

fn batch_row(row: &crate::sql::Row<'_>) -> crate::sql::Result<BatchRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        StatementFormat::from_sql(row.get_ref(4)?.as_str()?)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}
fn batch(item: BatchRow) -> Result<ImportBatch> {
    let (id, name, account_id, kind, format, committed, undone, counts) = item;
    let counts: Counts = serde_json::from_str(&counts).map_err(codec_error)?;
    Ok(ImportBatch {
        id,
        name,
        account_id,
        kind,
        format,
        committed_at: crate::ledger_engine::utc_timestamp(committed)?,
        undone_at: undone
            .map(crate::ledger_engine::utc_timestamp)
            .transpose()?,
        inserted: counts.inserted,
        updated: counts.updated,
        skipped: counts.skipped,
    })
}
pub fn batch_get(ctx: &Context<'_>, id: &str) -> Result<ImportBatch> {
    batch(
        ctx.tx
            .query_row(&format!("{BATCHES} AND id=?1"), [id], batch_row)
            .optional()?
            .ok_or_else(|| missing("batchId"))?,
    )
}
pub fn batches(ctx: &Context<'_>, kind: Option<&str>, page: &PageReq) -> Result<Page<ImportBatch>> {
    if kind.is_some_and(|kind| !matches!(kind, "statement" | "fx" | "asset")) {
        return Err(invalid("kind", "invalid_import_kind"));
    }
    let cursor = ReadPage::new(ctx, page, &("import_batches", kind))?;
    let mut query = Query::new(BATCHES.to_owned());
    if let Some(kind) = kind {
        query.equals("kind", kind.to_owned());
    }
    tx::sort_cursor(&mut query, &cursor, "committed_at_ms", "id", true, false)?;
    query.limit(cursor.limit + 1);
    let mut stmt = ctx.tx.prepare(&query.sql)?;
    let rows = stmt
        .query_map(params_from_iter(query.args), batch_row)?
        .map(|row| {
            let row = row?;
            let ms = row.5;
            Ok((ms, batch(row)?))
        })
        .collect::<Result<_>>()?;
    let page = cursor.finish(rows, |(ms, batch)| (ms.to_string(), batch.id.clone()))?;
    Ok(Page {
        rows: page.rows.into_iter().map(|(_, item)| item).collect(),
        next_cursor: page.next_cursor,
        revision: page.revision,
    })
}

fn effect_block(
    ctx: &Context<'_>,
    batch: &str,
    id: &str,
    kind: &str,
    hash: &str,
) -> Result<Option<String>> {
    let current = if kind == "transaction" {
        let before:Option<String>=ctx.tx.query_row("SELECT before_json FROM import_effects WHERE batch_id=?1 AND entity_kind='transaction' AND entity_id=?2",
            params![batch,id],|row|row.get(0))?;
        let posting = if before
            .as_deref()
            .map(before_snapshot)
            .transpose()?
            .is_some_and(|snapshot| snapshot.occurrence.is_some())
        {
            tx::check_import_restore(ctx, id)
        } else {
            tx::check_delete(ctx, id, false)
        };
        posting.and_then(|row| snapshot_hash(ctx, &row))
    } else {
        structured::effect_hash(ctx, kind, id)
    };
    let current = match current {
        Ok(hash) => hash,
        Err(error) if matches!(error.code, ErrorCode::Validation | ErrorCode::Conflict) => {
            return Ok(Some(error.message_key.to_owned()));
        }
        Err(error) => return Err(error),
    };
    if current != hash {
        return Ok(Some("posting_changed_after_import".to_owned()));
    }
    if kind == "asset_transaction" {
        let row = crate::planning::assets::trade_get(ctx, id)?;
        if row.draft.linked_transaction_id.is_some() {
            return Ok(Some("asset_trade_has_dependencies".to_owned()));
        }
        let mut others = crate::planning::assets::trades(ctx, &row.draft.asset_id)?;
        others.retain(|trade| trade.id != id);
        let currency: String = ctx.tx.query_row(
            "SELECT currency_code FROM assets WHERE id=?1",
            [&row.draft.asset_id],
            |r| r.get(0),
        )?;
        if crate::planning::assets::replay(
            &currency,
            &others,
            crate::domain::parse_date("9999-12-31")?,
        )
        .is_err()
        {
            return Ok(Some("asset_trade_has_dependencies".to_owned()));
        }
    }
    let later:bool=ctx.tx.query_row("SELECT EXISTS(SELECT 1 FROM import_effects e JOIN import_batches other ON other.id=e.batch_id
        JOIN import_batches original ON original.id=?1 WHERE e.entity_id=?2 AND e.entity_kind=?3 AND other.id<>original.id AND other.undone_at_ms IS NULL
        AND (other.committed_at_ms>original.committed_at_ms OR (other.committed_at_ms=original.committed_at_ms AND other.rowid>original.rowid)))",
        params![batch,id,kind],|r|r.get(0))?;
    Ok(later.then(|| "later_import_depends_on_posting".to_owned()))
}
fn counts(ctx: &Context<'_>, id: &str) -> Result<Counts> {
    let json = ctx
        .tx
        .query_row(
            "SELECT summary_json FROM import_batches WHERE id=?1",
            [id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| missing("batchId"))?;
    serde_json::from_str(&json).map_err(codec_error)
}
pub fn undo_preview(
    ctx: &Context<'_>,
    store: &Stage<UndoPlan>,
    batch: &str,
    selection: Option<&[u32]>,
) -> Result<UndoPreview> {
    let counts = counts(ctx, batch)?;
    let mut stmt=ctx.tx.prepare("SELECT ordinal,entity_id,after_hash,entity_kind FROM import_effects WHERE batch_id=?1 ORDER BY ordinal DESC")?;
    let mut rows = Vec::new();
    let mut effects = stmt.query([batch])?;
    while let Some(effect) = effects.next()? {
        let ordinal: u32 = effect.get(0)?;
        if counts.resolved.contains(&ordinal)
            || selection.is_some_and(|ids| !ids.contains(&ordinal))
        {
            continue;
        }
        let entity_id: String = effect.get(1)?;
        let blocked = effect_block(
            ctx,
            batch,
            &entity_id,
            effect.get_ref(3)?.as_str()?,
            effect.get_ref(2)?.as_str()?,
        )?;
        rows.push(UndoRow {
            ordinal,
            entity_id,
            blocked,
        });
    }
    let preview_token = store.put(
        ctx,
        UndoPlan {
            batch: batch.to_owned(),
            rows: rows.clone(),
        },
    )?;
    Ok(UndoPreview {
        preview_token,
        rows,
        revision: revision(&ctx.tx)?,
    })
}
pub fn undo_apply(
    ctx: &Context<'_>,
    store: &Stage<UndoPlan>,
    token: &str,
    selected: &[u32],
) -> Result<UndoResult> {
    let plan = store.get(ctx, token)?;
    if selected.is_empty()
        || selected.iter().collect::<BTreeSet<_>>().len() != selected.len()
        || selected.iter().any(|id| {
            !plan
                .rows
                .iter()
                .any(|row| row.ordinal == *id && row.blocked.is_none())
        })
    {
        return Err(invalid("selectedOrdinals", "invalid_import_undo_selection"));
    }
    let mut counts = counts(ctx, &plan.batch)?;
    let mut completed = Vec::new();
    for row in &plan.rows {
        if !selected.contains(&row.ordinal) {
            continue;
        }
        let (op,before,hash,kind)=ctx.tx.query_row("SELECT operation,before_json,after_hash,entity_kind FROM import_effects WHERE batch_id=?1 AND ordinal=?2",
            params![plan.batch,row.ordinal],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))?;
        if effect_block(ctx, &plan.batch, &row.entity_id, &kind, &hash)?.is_some() {
            return Err(conflict("import_undo_dependency"));
        }
        if kind != "transaction" {
            structured::revert(ctx, &kind, &row.entity_id, before)?;
            counts.resolved.insert(row.ordinal);
            completed.push(row.ordinal);
            continue;
        }
        ctx.tx.execute(
            "DELETE FROM transaction_import_keys WHERE batch_id=?1 AND transaction_id=?2",
            params![plan.batch, row.entity_id],
        )?;
        if op == "insert" {
            tx::remove(ctx, &row.entity_id, false)?;
        } else {
            let before = before_snapshot(
                &before.ok_or_else(|| invalid("batchId", "missing_import_snapshot"))?,
            )?;
            let current = tx::get(ctx, &row.entity_id)?;
            tx::reconcile_import(
                ctx,
                &VersionedId {
                    id: row.entity_id.clone(),
                    version: current.row.version,
                },
                &before.draft,
            )?;
            if let Some(occurrence) = before.occurrence {
                let count = ctx.tx.execute(
                    "UPDATE recurring_occurrences SET scheduled_date=?3,state=?4,rule_version=?5
                    WHERE rule_id=?1 AND nominal_date=?2 AND transaction_id=?6",
                    params![
                        occurrence.rule_id,
                        occurrence.nominal_date,
                        occurrence.scheduled_date,
                        occurrence.state,
                        occurrence.rule_version,
                        row.entity_id
                    ],
                )?;
                if count != 1 {
                    return Err(conflict("import_undo_dependency"));
                }
            }
        }
        counts.resolved.insert(row.ordinal);
        completed.push(row.ordinal);
    }
    let all: i64 = ctx.tx.query_row(
        "SELECT count(*) FROM import_effects WHERE batch_id=?1",
        [&plan.batch],
        |r| r.get(0),
    )?;
    let fully_undone = all == counts.resolved.len() as i64;
    ctx.tx.execute(
        "UPDATE import_batches SET summary_json=?2,undone_at_ms=?3 WHERE id=?1",
        params![
            plan.batch,
            serde_json::to_string(&counts).map_err(codec_error)?,
            fully_undone.then_some(ctx.now_ms)
        ],
    )?;
    Ok(UndoResult {
        batch_id: plan.batch.clone(),
        completed,
        fully_undone,
    })
}

impl ReceiptValue for ImportTemplate {
    type Reference = String;
    fn reference(&self, _: &Context<'_>) -> String {
        self.id.clone()
    }
    fn reload(ctx: &Context<'_>, id: String) -> Result<Self> {
        template_get(ctx, &id)
    }
}
impl ReceiptValue for ImportBatch {
    type Reference = String;
    fn reference(&self, _: &Context<'_>) -> String {
        self.id.clone()
    }
    fn reload(ctx: &Context<'_>, id: String) -> Result<Self> {
        batch_get(ctx, &id)
    }
}
impl ReceiptValue for UndoResult {
    type Reference = Self;
    const NEEDS_SAME_REVISION: bool = false;
    fn reference(&self, _: &Context<'_>) -> Self {
        self.clone()
    }
    fn reload(_: &Context<'_>, value: Self) -> Result<Self> {
        Ok(value)
    }
}
