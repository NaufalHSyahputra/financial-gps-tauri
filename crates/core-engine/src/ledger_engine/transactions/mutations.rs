use super::*;

pub fn assignable_category(ctx: &Context<'_>, id: &str) -> Result<()> {
    let valid = ctx
        .tx
        .query_row(
            "SELECT hidden=0 AND archived=0 FROM categories WHERE id=?1",
            [id],
            |r| r.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false);
    if !valid {
        return Err(invalid("categoryId", "unassignable_category"));
    }
    Ok(())
}

pub fn save(ctx: &Context<'_>, target: Option<&VersionedId>, draft: &TxDraft) -> Result<TxDetail> {
    save_posting(ctx, target, draft)
}

fn save_posting(
    ctx: &Context<'_>,
    target: Option<&VersionedId>,
    draft: &TxDraft,
) -> Result<TxDetail> {
    let id = target.map_or_else(new_id, |t| t.id.clone());
    let old = target
        .map(|target| {
            let detail = get(ctx, &target.id)?;
            check_version(&detail.row.version, target)?;
            if detail.row.entry_kind != EntryKind::Ordinary {
                return Err(conflict("synthetic_posting_protected"));
            }
            Ok(detail)
        })
        .transpose()?;
    let (account, archived) = accounts::raw(ctx, &draft.account_id)?;
    if archived.is_some()
        && old
            .as_ref()
            .is_none_or(|old| old.row.account_id != draft.account_id)
    {
        return Err(conflict("account_archived"));
    }
    if account.currency_code != draft.amount.currency {
        return Err(invalid("currency", "currency_mismatch"));
    }
    let amount = positive(&draft.amount.minor)?;
    let bank = parse_date(&draft.bank_date)?;
    let bill = date_option(draft.effective_bill_date.as_deref())?;
    date_option(draft.reporting_date_override.as_deref())?;
    date_option(draft.flag_due_date.as_deref())?;
    if draft.flag_due_date.is_some() && !draft.flagged {
        return Err(invalid("flagDueDate", "flag_required"));
    }
    if draft.notes.chars().count() > 1000 {
        return Err(invalid("notes", "notes_too_long"));
    }
    if draft.description.trim().is_empty() || draft.description.chars().count() > 1024 {
        return Err(invalid("description", "invalid_description"));
    }
    if !draft.splits.is_empty()
        && (draft.splits.len() < 2 || draft.splits.len() > 1000 || draft.category_id.is_some())
    {
        return Err(invalid("splits", "invalid_split_shape"));
    }
    let mut split_total = 0i128;
    for split in &draft.splits {
        if old.as_ref().is_none_or(|old| {
            !old.draft
                .splits
                .iter()
                .any(|line| line.category_id == split.category_id)
        }) {
            assignable_category(ctx, &split.category_id)?;
        }
        split_total += i128::from(positive(&split.minor)?);
    }
    if !draft.splits.is_empty() && split_total != i128::from(amount) {
        return Err(invalid("splits", "split_sum_mismatch"));
    }
    if let Some(category) = &draft.category_id
        && old.as_ref().and_then(|old| old.row.category_id.as_ref()) != Some(category)
    {
        assignable_category(ctx, category)?;
    }
    if let Some(payee) = &draft.payee_id
        && old.as_ref().and_then(|old| old.row.payee_id.as_ref()) != Some(payee)
        && !ctx
            .tx
            .query_row("SELECT archived=0 FROM payees WHERE id=?1", [payee], |r| {
                r.get::<_, bool>(0)
            })
            .optional()?
            .unwrap_or(false)
    {
        return Err(invalid("payeeId", "unassignable_payee"));
    }
    if let Some(alias) = &draft.alias_id {
        let exists = ctx.tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM account_aliases WHERE id=?1 AND account_id=?2)",
            params![alias, draft.account_id],
            |r| r.get::<_, bool>(0),
        )?;
        if !exists {
            return Err(invalid("aliasId", "account_alias_mismatch"));
        }
    }
    if let Some(installment) = &draft.installment {
        let (purchase, count) = ctx
            .tx
            .query_row(
                "SELECT purchase_date,occurrence_count FROM installment_series WHERE id=?1",
                [&installment.series_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, u16>(1)?)),
            )
            .optional()?
            .ok_or_else(|| missing("seriesId"))?;
        if !(1..=count).contains(&installment.number) || parse_date(&purchase)? > bank {
            return Err(invalid("installment", "invalid_installment_metadata"));
        }
    }
    let mut original = (None, None, None, None, None);
    if let Some(source) = &draft.original {
        if source.amount.currency == draft.amount.currency {
            return Err(invalid("original", "original_currency_must_differ"));
        }
        let converted = currency::convert_at_rate(
            ctx,
            &source.amount,
            &draft.amount.currency,
            currency::rate(&source.rate_decimal)?,
        )?;
        parse_date(&source.rate_date)?;
        if converted.minor != draft.amount.minor
            || source.rate_source.trim().is_empty()
            || source.rate_source.len() > 256
        {
            return Err(invalid("original", "original_conversion_mismatch"));
        }
        original = (
            Some(source.amount.currency.as_str()),
            Some(positive(&source.amount.minor)?),
            Some(
                currency::rate(&source.rate_decimal)?
                    .normalize()
                    .to_string(),
            ),
            Some(source.rate_date.as_str()),
            Some(source.rate_source.as_str()),
        );
    }
    if let Some(old) = &old {
        let differs = old.row.account_id != draft.account_id
            || old.row.amount.currency != draft.amount.currency
            || old.row.amount.minor != draft.amount.minor
            || old.row.direction.as_str() != draft.direction.as_str();
        super::corrections::check_dependencies(
            ctx,
            &id,
            differs
                || old.row.bank_date != draft.bank_date
                || old.row.ignored != draft.ignored
                || old.row.exclude_from_pnl != draft.exclude_from_pnl
                || old.row.status.as_str() != draft.status.as_str(),
        )?;
        if !old.sources.is_empty()
            && (differs
                || old.row.bank_date != draft.bank_date
                || digest(&old.draft.original)? != digest(&draft.original)?
                || (matches!(old.row.status, TxStatus::Pending)
                    && matches!(draft.status, TxStatus::Posted)))
        {
            return Err(conflict("imported_posting_protected"));
        }
        if old.row.transfer_id.is_some()
            && (differs
                || old.row.ignored != draft.ignored
                || draft.category_id.is_some()
                || !draft.splits.is_empty())
        {
            return Err(conflict("transfer_leg_protected"));
        }
        if (old.shared_expense_id.is_some()
            || old.recurring_rule_id.is_some()
            || !accounts::strings(
                &ctx.tx,
                "SELECT transaction_id FROM loan_payment_allocations WHERE transaction_id=?1",
                &id,
            )?
            .is_empty()
            || !accounts::strings(
                &ctx.tx,
                "SELECT id FROM shared_settlements WHERE transaction_id=?1",
                &id,
            )?
            .is_empty()
            || !accounts::strings(
                &ctx.tx,
                "SELECT id FROM asset_transactions WHERE linked_ledger_tx_id=?1",
                &id,
            )?
            .is_empty())
            && differs
        {
            return Err(conflict("posting_has_dependencies"));
        }
    }
    let (effective, mode, statement) = accounts::booking(
        ctx,
        &draft.account_id,
        &account,
        &draft.direction,
        bank,
        bill,
        draft.statement_id.as_deref(),
    )?;
    let assembling = old
        .as_ref()
        .is_none_or(|before| !before.draft.splits.is_empty() || !draft.splits.is_empty());
    if old.is_some() && assembling {
        ctx.tx.execute(
            "UPDATE transactions SET assembly_state=0 WHERE id=?1",
            [&id],
        )?;
        ctx.tx.execute(
            "DELETE FROM transaction_splits WHERE transaction_id=?1",
            [&id],
        )?;
    }
    ctx.tx.execute("INSERT INTO transactions(id,account_id,currency_code,amount_minor,direction,bank_date,effective_date,effective_bill_date,
        reporting_date_override,applied_mode,status,description,notes,payee_id,category_id,alias_id,statement_id,ignored,exclude_from_pnl,
        flagged,flag_due_date,installment_series_id,installment_number,original_currency_code,original_amount_minor,account_fx_rate_decimal,
        account_fx_rate_date,account_fx_source,assembly_state,created_at_ms)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,0,?29)
        ON CONFLICT(id) DO UPDATE SET account_id=excluded.account_id,currency_code=excluded.currency_code,amount_minor=excluded.amount_minor,
        direction=excluded.direction,bank_date=excluded.bank_date,effective_date=excluded.effective_date,effective_bill_date=excluded.effective_bill_date,
        reporting_date_override=excluded.reporting_date_override,applied_mode=excluded.applied_mode,status=excluded.status,description=excluded.description,
        notes=excluded.notes,payee_id=excluded.payee_id,category_id=excluded.category_id,alias_id=excluded.alias_id,statement_id=excluded.statement_id,
        ignored=excluded.ignored,exclude_from_pnl=excluded.exclude_from_pnl,flagged=excluded.flagged,flag_due_date=excluded.flag_due_date,
        installment_series_id=excluded.installment_series_id,installment_number=excluded.installment_number,original_currency_code=excluded.original_currency_code,
        original_amount_minor=excluded.original_amount_minor,account_fx_rate_decimal=excluded.account_fx_rate_decimal,
        account_fx_rate_date=excluded.account_fx_rate_date,account_fx_source=excluded.account_fx_source,version=transactions.version+1",
        params![id,draft.account_id,draft.amount.currency,amount,draft.direction.as_str(),draft.bank_date,effective,draft.effective_bill_date,
            draft.reporting_date_override,mode.as_str(),draft.status.as_str(),draft.description,draft.notes,draft.payee_id,draft.category_id,draft.alias_id,
            statement,draft.ignored,draft.exclude_from_pnl,draft.flagged,draft.flag_due_date,draft.installment.as_ref().map(|i|&i.series_id),
            draft.installment.as_ref().map(|i|i.number),original.0,original.1,original.2,original.3,original.4,ctx.now_ms])?;
    for (index, split) in draft.splits.iter().enumerate() {
        ctx.tx.execute(
            "INSERT INTO transaction_splits VALUES(?1,?2,?3,?4)",
            params![
                id,
                (index + 1) as u32,
                split.category_id,
                positive(&split.minor)?
            ],
        )?;
    }
    if assembling {
        ctx.tx.execute(
            "UPDATE transactions SET assembly_state=1 WHERE id=?1",
            [&id],
        )?;
    }
    if draft.statement_id.is_some() {
        mark_override(ctx, &id, "statement_id")?;
    } else {
        ctx.tx.execute("DELETE FROM transaction_overrides WHERE transaction_id=?1 AND field_key='statement_id'",[&id])?;
    }
    let is_new = old.is_none();
    if let Some(old) = old.filter(|old| !old.sources.is_empty()) {
        for (key, changed) in [
            ("description", old.draft.description != draft.description),
            ("notes", old.draft.notes != draft.notes),
            (
                "category_id",
                old.draft.category_id != draft.category_id
                    || digest(&old.draft.splits)? != digest(&draft.splits)?,
            ),
            ("payee_id", old.draft.payee_id != draft.payee_id),
            (
                "reporting_date_override",
                old.draft.reporting_date_override != draft.reporting_date_override,
            ),
            (
                "effective_bill_date",
                old.draft.effective_bill_date != draft.effective_bill_date,
            ),
            ("ignored", old.draft.ignored != draft.ignored),
            (
                "exclude_from_pnl",
                old.draft.exclude_from_pnl != draft.exclude_from_pnl,
            ),
            ("flagged", old.draft.flagged != draft.flagged),
            (
                "flag_due_date",
                old.draft.flag_due_date != draft.flag_due_date,
            ),
            ("alias_id", old.draft.alias_id != draft.alias_id),
        ] {
            if changed {
                mark_override(ctx, &id, key)?;
            }
        }
    }
    project_posting(ctx, &id, is_new)?;
    get(ctx, &id)
}

pub(crate) fn mark_override(ctx: &Context<'_>, id: &str, field: &str) -> Result<()> {
    ctx.tx.execute(
        "INSERT OR IGNORE INTO transaction_overrides VALUES (?1,?2)",
        params![id, field],
    )?;
    Ok(())
}

pub fn post(ctx: &Context<'_>, target: &VersionedId, booking: Option<&str>) -> Result<TxDetail> {
    let mut detail = get(ctx, &target.id)?;
    check_version(&detail.row.version, target)?;
    if matches!(detail.row.status, TxStatus::Posted) || !detail.sources.is_empty() {
        return Err(conflict("posting_cannot_be_promoted"));
    }
    detail.draft.status = TxStatus::Posted;
    detail.draft.bank_date = booking.map_or_else(|| ctx.today.to_string(), str::to_owned);
    save(ctx, Some(target), &detail.draft)
}

pub fn rebuild_dates(ctx: &Context<'_>, account_id: &str) -> Result<()> {
    let (account, _) = accounts::raw(ctx, account_id)?;
    let mut stmt=ctx.tx.prepare("SELECT t.id,t.direction,t.bank_date,t.effective_bill_date,t.effective_date,t.applied_mode,t.statement_id,
        EXISTS(SELECT 1 FROM transaction_overrides o WHERE o.transaction_id=t.id AND field_key='statement_id')
        FROM transactions t WHERE account_id=?1 AND entry_kind='ordinary'")?;
    let records = stmt
        .query_map([account_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Direction::read(r.get_ref(1)?.as_str()?)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, bool>(7)?,
            ))
        })?
        .collect::<crate::sql::Result<Vec<_>>>()?;
    for (id, direction, bank, bill, old_effective, old_mode, old_statement, manual) in records {
        let (effective, mode, statement) = accounts::booking(
            ctx,
            account_id,
            &account,
            &direction,
            parse_date(&bank)?,
            date_option(bill.as_deref())?,
            if manual {
                old_statement.as_deref()
            } else {
                None
            },
        )?;
        if effective != old_effective || mode.as_str() != old_mode || statement != old_statement {
            ctx.tx.execute("UPDATE transactions SET effective_date=?2,applied_mode=?3,statement_id=?4,version=version+1 WHERE id=?1",
                params![id,effective,mode.as_str(),statement])?;
        }
    }
    Ok(())
}

pub fn delete(ctx: &Context<'_>, target: &VersionedId, confirm: bool) -> Result<Deleted> {
    if !confirm {
        return Err(invalid("confirm", "confirmation_required"));
    }
    let before = get(ctx, &target.id)?;
    check_version(&before.row.version, target)?;
    remove(ctx, &target.id, false)
}
pub(crate) fn check_delete(ctx: &Context<'_>, id: &str, opening: bool) -> Result<TxDetail> {
    check_removal(ctx, id, opening)
}

fn check_removal(ctx: &Context<'_>, id: &str, opening: bool) -> Result<TxDetail> {
    let detail = get(ctx, id)?;
    super::corrections::check_dependencies(ctx, id, true)?;
    if detail.row.transfer_id.is_some() || (!opening && detail.row.entry_kind == EntryKind::Opening)
    {
        return Err(conflict("protected_posting"));
    }
    let has_dependency:bool=ctx.tx.query_row("SELECT
       EXISTS(SELECT 1 FROM attachments WHERE transaction_id=?1) OR EXISTS(SELECT 1 FROM shared_expenses WHERE transaction_id=?1)
       OR EXISTS(SELECT 1 FROM shared_settlements WHERE transaction_id=?1) OR EXISTS(SELECT 1 FROM loan_payment_allocations WHERE transaction_id=?1)
       OR EXISTS(SELECT 1 FROM asset_transactions WHERE linked_ledger_tx_id=?1) OR EXISTS(SELECT 1 FROM recurring_occurrences WHERE transaction_id=?1)",
       [id],|r|r.get(0))?;
    if has_dependency {
        return Err(conflict("posting_has_dependencies"));
    }
    Ok(detail)
}
pub fn remove(ctx: &Context<'_>, id: &str, opening: bool) -> Result<Deleted> {
    let audited: bool = ctx.tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM posting_audit WHERE transaction_id=?1)",
        [id],
        |r| r.get(0),
    )?;
    if audited {
        return Err(conflict("posting_has_audited_dependencies"));
    }
    let detail = check_delete(ctx, id, opening)?;
    if !detail.draft.splits.is_empty() {
        ctx.tx
            .execute("UPDATE transactions SET assembly_state=0 WHERE id=?1", [id])?;
        ctx.tx.execute(
            "DELETE FROM transaction_splits WHERE transaction_id=?1",
            [id],
        )?;
    }
    let rowid = ctx
        .tx
        .query_row("SELECT rowid FROM transactions WHERE id=?1", [id], |r| {
            r.get(0)
        })?;
    if let Some(fts) = fts_rowid(ctx, id, rowid)? {
        ctx.tx
            .execute("DELETE FROM transaction_search WHERE rowid=?1", [fts])?;
    }
    ctx.tx
        .execute("DELETE FROM transactions WHERE id=?1", [id])?;
    Ok(Deleted { id: id.to_owned() })
}

pub(crate) fn hashtag_spans(notes: &str) -> Vec<(usize, usize, String)> {
    fn token_char(ch: char) -> bool {
        ch.is_alphanumeric() || ch == '_' || ch == '-' || is_combining_mark(ch)
    }
    let chars = notes.char_indices().collect::<Vec<_>>();
    let mut spans = Vec::new();
    for (index, &(start, ch)) in chars.iter().enumerate() {
        if ch != '#' || (index > 0 && (token_char(chars[index - 1].1) || chars[index - 1].1 == '#'))
        {
            continue;
        }
        let mut end = start + 1;
        for &(offset, ch) in chars
            .iter()
            .skip(index + 1)
            .take_while(|(_, ch)| token_char(*ch))
        {
            end = offset + ch.len_utf8();
        }
        if end > start + 1 {
            let tag = normalize(&notes[start + 1..end]);
            if !tag.is_empty() {
                spans.push((start, end, tag));
            }
        }
    }
    spans
}

pub fn project(ctx: &Context<'_>, id: &str) -> Result<()> {
    project_posting(ctx, id, false)
}
fn fts_rowid(ctx: &Context<'_>, id: &str, rowid: i64) -> Result<Option<i64>> {
    let current = ctx
        .tx
        .query_row(
            "SELECT rowid FROM transaction_search WHERE rowid=?1 AND transaction_id=?2",
            params![rowid, id],
            |r| r.get(0),
        )
        .optional()?;
    if current.is_some() {
        return Ok(current);
    }
    Ok(ctx
        .tx
        .query_row(
            "SELECT rowid FROM transaction_search WHERE transaction_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?)
}
pub(crate) fn project_posting(ctx: &Context<'_>, id: &str, is_new: bool) -> Result<()> {
    let (description,notes,payee,rowid)=ctx.tx.query_row("SELECT t.description,t.notes,p.name,t.rowid FROM transactions t LEFT JOIN payees p ON p.id=t.payee_id
        WHERE t.id=?1",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,i64>(3)?)))?;
    ctx.tx
        .execute("DELETE FROM transaction_tags WHERE transaction_id=?1", [id])?;
    for tag in hashtag_spans(&notes)
        .into_iter()
        .map(|(_, _, tag)| tag)
        .collect::<BTreeSet<_>>()
    {
        ctx.tx.execute(
            "INSERT INTO transaction_tags VALUES(?1,?2)",
            params![id, tag],
        )?;
    }
    // Preserve the FTS row identifier when it exists, always resolve it via the durable UUID.
    let existing = if is_new {
        None
    } else {
        fts_rowid(ctx, id, rowid)?
    };
    if let Some(key) = existing {
        ctx.tx.execute("UPDATE transaction_search SET description=?2,notes=?3,payee=?4 WHERE rowid=?1 AND transaction_id=?5",
            params![key,normalize(&description),normalize(&notes),payee.as_deref().map(normalize),id])?;
    } else {
        let available: bool = ctx.tx.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM transaction_search WHERE rowid=?1)",
            [rowid],
            |r| r.get(0),
        )?;
        ctx.tx.execute("INSERT INTO transaction_search(rowid,transaction_id,description,notes,payee) VALUES(?1,?2,?3,?4,?5)",
            params![available.then_some(rowid),id,normalize(&description),normalize(&notes),payee.as_deref().map(normalize)])?;
    }
    Ok(())
}
