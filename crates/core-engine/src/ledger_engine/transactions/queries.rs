use super::*;

pub const LINK_ID: &str = "coalesce((SELECT l.id FROM transfer_links l WHERE l.debit_id=t.id),(SELECT l.id FROM transfer_links l WHERE l.credit_id=t.id))";
pub const NO_TRANSFER: &str =
    "NOT EXISTS(SELECT 1 FROM transfer_links l WHERE l.debit_id=t.id OR l.credit_id=t.id)";
pub const VISIBLE_CATEGORY: &str = "NOT EXISTS(SELECT 1 FROM categories c WHERE c.id=t.category_id AND (c.ignored=1 OR c.kind='transfer'))";
pub const VISIBLE_SPLIT: &str =
    "(NOT EXISTS(SELECT 1 FROM transaction_splits s WHERE s.transaction_id=t.id) OR
    EXISTS(SELECT 1 FROM transaction_splits s JOIN categories c ON c.id=s.category_id
           WHERE s.transaction_id=t.id AND c.ignored=0 AND c.kind<>'transfer'))";
pub const ROW_COLUMNS: &str = "t.id,CAST(t.version AS TEXT),t.account_id,t.currency_code,CAST(t.amount_minor AS TEXT),t.direction,
    t.bank_date,t.effective_date,t.reporting_date,t.status,t.entry_kind,t.description,t.payee_id,t.category_id,t.statement_id,
    t.ignored,t.exclude_from_pnl,t.flagged,t.flag_due_date";
pub const SOURCE: &str = " FROM transactions t JOIN accounts a ON a.id=t.account_id
    LEFT JOIN payees p ON p.id=t.payee_id LEFT JOIN categories cat ON cat.id=t.category_id WHERE 1=1";

pub fn read_row(r: &Row<'_>) -> crate::sql::Result<TxRow> {
    Ok(TxRow {
        id: r.get(0)?,
        version: r.get(1)?,
        account_id: r.get(2)?,
        amount: Money {
            currency: r.get(3)?,
            minor: r.get(4)?,
        },
        direction: Direction::read(r.get_ref(5)?.as_str()?)?,
        bank_date: r.get(6)?,
        effective_date: r.get(7)?,
        reporting_date: r.get(8)?,
        status: TxStatus::read(r.get_ref(9)?.as_str()?)?,
        entry_kind: EntryKind::read(r.get_ref(10)?.as_str()?)?,
        description: r.get(11)?,
        payee_id: r.get(12)?,
        category_id: r.get(13)?,
        category: None,
        statement_id: r.get(14)?,
        ignored: r.get(15)?,
        exclude_from_pnl: r.get(16)?,
        flagged: r.get(17)?,
        flag_due_date: r.get(18)?,
        transfer_id: r.get(19)?,
        running_balance: None,
        pnl_amount: None,
    })
}

fn read_category(row: &Row<'_>, start: usize) -> crate::sql::Result<Option<TransactionCategory>> {
    let Some(name) = row.get(start)? else {
        return Ok(None);
    };
    Ok(Some(TransactionCategory {
        name,
        kind: super::catalog::CategoryKind::read(row.get_ref(start + 1)?.as_str()?)?,
        icon: row.get(start + 2)?,
    }))
}

pub fn get(ctx: &Context<'_>, id: &str) -> Result<TxDetail> {
    let (row, notes, bill, override_date, alias, explicit_statement, installment, original, applied_mode) = ctx.tx.query_row(
        &format!("SELECT {ROW_COLUMNS},{LINK_ID},t.notes,t.effective_bill_date,t.reporting_date_override,t.alias_id,
         CASE WHEN EXISTS(SELECT 1 FROM transaction_overrides o WHERE o.transaction_id=t.id AND o.field_key='statement_id') THEN t.statement_id END,
         t.installment_series_id,t.installment_number,t.original_currency_code,CAST(t.original_amount_minor AS TEXT),
         t.account_fx_rate_decimal,t.account_fx_rate_date,t.account_fx_source,t.applied_mode,cat.name,cat.kind,cat.icon
         FROM transactions t LEFT JOIN categories cat ON cat.id=t.category_id WHERE t.id=?1"),
        [id], |r| {
            let installment = if let Some(series_id) = r.get::<_,Option<String>>(25)? {
                Some(InstallmentRef {series_id,number:r.get(26)?})
            } else {None};
            let original = if let Some(currency)=r.get::<_,Option<String>>(27)? {
                Some(OriginalAmount {amount:Money{currency,minor:r.get(28)?},rate_decimal:r.get(29)?,rate_date:r.get(30)?,rate_source:r.get(31)?})
            } else {None};
            let mut row = read_row(r)?;
            row.category = read_category(r, 33)?;
            Ok((row,r.get(20)?,r.get(21)?,r.get(22)?,r.get(23)?,r.get(24)?,installment,original,AccountingMode::read(r.get_ref(32)?.as_str()?)?))
        },
    ).optional()?.ok_or_else(|| missing("transactionId"))?;
    let splits = ctx.tx.prepare_cached("SELECT category_id,CAST(amount_minor AS TEXT) FROM transaction_splits WHERE transaction_id=?1 ORDER BY position")?
        .query_map([id],|r|Ok(SplitDraft{category_id:r.get(0)?,minor:r.get(1)?}))?.collect::<crate::sql::Result<_>>()?;
    let sources=ctx.tx.prepare_cached("SELECT provider_namespace,external_id,raw_bank_date,raw_currency,CAST(raw_amount_minor AS TEXT),raw_direction,raw_description
        FROM transaction_import_keys WHERE transaction_id=?1 ORDER BY id")?.query_map([id],|r|Ok(ImportSource {
            namespace:r.get(0)?,external_id:r.get(1)?,bank_date:r.get(2)?,amount:Money{currency:r.get(3)?,minor:r.get(4)?},
            direction:r.get(5)?,description:r.get(6)?,
        }))?.collect::<crate::sql::Result<_>>()?;
    let tags = accounts::strings(
        &ctx.tx,
        "SELECT token FROM transaction_tags WHERE transaction_id=?1 ORDER BY token",
        id,
    )?;
    let attachment_ids = accounts::strings(
        &ctx.tx,
        "SELECT id FROM attachments WHERE transaction_id=?1 AND state='ready' ORDER BY id",
        id,
    )?;
    let recurring_rule_id = ctx
        .tx
        .query_row(
            "SELECT rule_id FROM recurring_occurrences WHERE transaction_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    let shared_expense_id = ctx
        .tx
        .query_row(
            "SELECT id FROM shared_expenses WHERE transaction_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    let draft = TxDraft {
        account_id: row.account_id.clone(),
        bank_date: row.bank_date.clone(),
        amount: row.amount.clone(),
        direction: row.direction.clone(),
        status: row.status.clone(),
        description: row.description.clone(),
        notes,
        payee_id: row.payee_id.clone(),
        category_id: row.category_id.clone(),
        splits,
        effective_bill_date: bill,
        reporting_date_override: override_date,
        ignored: row.ignored,
        exclude_from_pnl: row.exclude_from_pnl,
        flagged: row.flagged,
        flag_due_date: row.flag_due_date.clone(),
        alias_id: alias,
        statement_id: explicit_statement,
        installment,
        original,
    };
    Ok(TxDetail {
        row,
        draft,
        applied_mode,
        sources,
        tags,
        recurring_rule_id,
        shared_expense_id,
        attachment_ids,
    })
}

pub fn search_terms(value: &str) -> Result<Option<String>> {
    if value.len() > 512 {
        return Err(invalid("text", "search_too_long"));
    }
    let value = normalize(value);
    let terms = value
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .take(16)
        .map(|term| format!("\"{term}\"*"))
        .collect::<Vec<_>>();
    Ok((!terms.is_empty()).then(|| terms.join(" AND ")))
}

pub struct Query {
    pub sql: String,
    pub args: Vec<Value>,
}
impl Query {
    pub fn new(sql: String) -> Self {
        Self {
            sql,
            args: Vec::new(),
        }
    }
    pub fn bind(&mut self, value: impl Into<Value>) -> String {
        self.args.push(value.into());
        format!("?{}", self.args.len())
    }
    pub fn and(&mut self, predicate: &str) {
        self.sql.push_str(" AND ");
        self.sql.push_str(predicate);
    }
    pub fn equals(&mut self, expr: &str, value: impl Into<Value>) {
        let param = self.bind(value);
        self.and(&format!("{expr}={param}"));
    }
    pub fn choices(&mut self, pattern: &str, values: &[String]) {
        if !values.is_empty() {
            let params = values
                .iter()
                .map(|id| self.bind(id.clone()))
                .collect::<Vec<_>>()
                .join(",");
            self.and(&pattern.replace("{values}", &params));
        }
    }
    pub fn limit(&mut self, limit: usize) {
        self.sql.push_str(&format!(" LIMIT {limit}"));
    }
}

pub fn pnl_query(query: &mut Query, today: NaiveDate) {
    let day = query.bind(today.to_string());
    query.and(&format!("t.status='posted' AND t.bank_date<={day} AND t.reporting_date<={day} AND t.ignored=0 AND t.exclude_from_pnl=0 AND {NO_TRANSFER} AND {VISIBLE_CATEGORY} AND {VISIBLE_SPLIT} AND NOT EXISTS(SELECT 1 FROM shared_settlements ss WHERE ss.transaction_id=t.id)"));
}
pub fn filter_query(ctx: &Context<'_>, query: &mut Query, filter: &LedgerFilter) -> Result<()> {
    for values in [
        &filter.account_ids,
        &filter.alias_ids,
        &filter.category_ids,
        &filter.payee_ids,
        &filter.group_ids,
        &filter.tags,
    ] {
        if values.len() > 100 {
            return Err(invalid("filter", "too_many_filter_values"));
        }
    }
    query.choices("t.account_id IN ({values})", &filter.account_ids);
    query.choices("t.alias_id IN ({values})", &filter.alias_ids);
    query.choices("(t.category_id IN ({values}) OR EXISTS(SELECT 1 FROM transaction_splits s WHERE s.transaction_id=t.id AND s.category_id IN ({values})))",&filter.category_ids);
    query.choices("t.payee_id IN ({values})", &filter.payee_ids);
    query.choices("EXISTS(SELECT 1 FROM shared_expenses e WHERE e.transaction_id=t.id AND e.group_id IN ({values}))",&filter.group_ids);
    if let Some(id) = &filter.statement_id {
        query.equals("t.statement_id", id.clone());
    } else {
        if filter.unbilled {
            query.and("a.kind='credit_card' AND t.statement_id IS NULL");
        }
        if let Some(range) = &filter.range {
            let (from, to) = date_range(range)?;
            let column = filter
                .basis
                .as_ref()
                .unwrap_or(&DateBasis::Reporting)
                .column();
            for (op, value) in [(">=", from), ("<=", to)] {
                if let Some(value) = value {
                    let p = query.bind(value);
                    query.and(&format!("t.{column}{op}{p}"));
                }
            }
        }
    }
    if let Some(text) = &filter.text
        && !text.trim().is_empty()
    {
        if let Some(terms) = search_terms(text)? {
            let param = query.bind(terms);
            query.and(&format!("t.id IN(SELECT transaction_id FROM transaction_search WHERE transaction_search MATCH {param})"));
        } else {
            query.and("0");
        }
    }
    if filter.uncategorized {
        query.and("t.category_id IS NULL AND NOT EXISTS(SELECT 1 FROM transaction_splits s WHERE s.transaction_id=t.id)");
    }
    let tags = filter
        .tags
        .iter()
        .map(|tag| normalize(tag.trim_start_matches('#')))
        .collect::<Vec<_>>();
    query.choices("t.id IN (SELECT tag.transaction_id FROM transaction_tags tag INDEXED BY tag_lookup WHERE tag.token IN ({values}))",&tags);
    if let Some(bounds) = &filter.native_amount {
        scale(&ctx.tx, &bounds.currency)?;
        let (min, max) = bounds_data(bounds)?;
        query.equals("t.currency_code", bounds.currency.clone());
        let lo = query.bind(min);
        let hi = query.bind(max);
        query.and(&format!("t.amount_minor BETWEEN {lo} AND {hi}"));
    }
    if let Some(kind) = &filter.entry_kind {
        query.equals("t.entry_kind", kind.as_str().to_owned());
    } else if filter.include_opening == Some(false) {
        query.and("t.entry_kind<>'opening'");
    }
    if let Some(status) = &filter.status {
        query.equals("t.status", status.as_str().to_owned());
    }
    if let Some(direction) = &filter.direction {
        query.equals("t.direction", direction.as_str().to_owned());
    }
    if let Some(ignored) = filter.ignored {
        query.equals("t.ignored", i64::from(ignored));
    }
    if let Some(transfer) = filter.transfer {
        query.and(if transfer {
            "EXISTS(SELECT 1 FROM transfer_links l WHERE l.debit_id=t.id OR l.credit_id=t.id)"
        } else {
            NO_TRANSFER
        });
    }
    if filter.pnl_only == Some(true) {
        pnl_query(query, ctx.today);
        query.choices("(t.category_id IN ({values}) OR EXISTS(SELECT 1 FROM transaction_splits s JOIN categories c ON c.id=s.category_id
            WHERE s.transaction_id=t.id AND s.category_id IN ({values}) AND c.ignored=0 AND c.kind<>'transfer'))",&filter.category_ids);
    }
    if let Some(flag) = &filter.flag_state {
        query.and("t.flagged=1");
        match flag {
            FlagState::Flagged => {}
            FlagState::Overdue => {
                let p = query.bind(ctx.today.to_string());
                query.and(&format!("t.flag_due_date<{p}"));
            }
            FlagState::DueWithinWeek => {
                let from = query.bind(ctx.today.to_string());
                let to = query.bind(
                    ctx.today
                        .checked_add_days(Days::new(7))
                        .ok_or_else(|| integrity("date_out_of_range"))?
                        .to_string(),
                );
                query.and(&format!("t.flag_due_date BETWEEN {from} AND {to}"));
            }
        }
    }
    Ok(())
}

pub fn bounds_data(bounds: &AmountBounds) -> Result<(i64, i64)> {
    let min = bounds
        .minimum
        .as_deref()
        .map(accounts::nonnegative)
        .transpose()?
        .unwrap_or(0);
    let max = bounds
        .maximum
        .as_deref()
        .map(accounts::nonnegative)
        .transpose()?
        .unwrap_or(i64::MAX);
    if min > max {
        return Err(invalid("amount", "invalid_amount_range"));
    }
    Ok((min, max))
}

pub fn sort_cursor(
    query: &mut Query,
    cursor: &ReadPage,
    sorter: &str,
    id_expr: &str,
    numeric: bool,
    ascending: bool,
) -> Result<()> {
    if let Some((value, id)) = &cursor.after {
        let value = if numeric {
            query.bind(minor(value)?)
        } else {
            query.bind(value.clone())
        };
        let id = query.bind(id.clone());
        let op = if ascending { ">" } else { "<" };
        query.and(&format!("({sorter},{id_expr}) {op} ({value},{id})"));
    }
    let dir = if ascending { "ASC" } else { "DESC" };
    query
        .sql
        .push_str(&format!(" ORDER BY {sorter} {dir},{id_expr} {dir}"));
    Ok(())
}

pub fn list(
    ctx: &Context<'_>,
    filter: &LedgerFilter,
    page: &PageReq,
    running: bool,
) -> Result<Page<TxRow>> {
    let paging = ReadPage::new(
        ctx,
        page,
        &("transactions", filter, running, ctx.today.to_string()),
    )?;
    let column = filter
        .basis
        .as_ref()
        .unwrap_or(&DateBasis::Reporting)
        .column();
    let (sorter, numeric) = match filter.sort.as_ref().map(|sort| &sort.field) {
        None | Some(SortField::Date) => (format!("t.{column}"), false),
        Some(field) => (
            match field {
                SortField::Amount => "t.amount_minor",
                SortField::Description => "t.description",
                SortField::Payee => "coalesce(p.normalized_name,'')",
                SortField::Category => "coalesce(cat.name,'')",
                SortField::Account => "a.name",
                SortField::Type => "t.direction",
                SortField::Status => "t.status",
                SortField::FlagDueDate => "coalesce(t.flag_due_date,'')",
                SortField::Date => unreachable!(),
            }
            .to_owned(),
            matches!(field, SortField::Amount),
        ),
    };
    let mut query = Query::new(format!(
        "SELECT {ROW_COLUMNS},{LINK_ID},CAST({sorter} AS TEXT),cat.name,cat.kind,cat.icon{SOURCE}"
    ));
    filter_query(ctx, &mut query, filter)?;
    sort_cursor(
        &mut query,
        &paging,
        &sorter,
        "t.id",
        numeric,
        filter.sort.as_ref().is_some_and(|s| s.ascending),
    )?;
    let display = filter
        .display_amount
        .as_ref()
        .map(|bounds| {
            scale(&ctx.tx, &bounds.currency)?;
            let (lo, hi) = bounds_data(bounds)?;
            Ok::<_, AppError>((&bounds.currency, lo, hi))
        })
        .transpose()?;
    if display.is_none() {
        query.limit(paging.limit + 1);
    }
    let mut stmt = ctx.tx.prepare(&query.sql)?;
    let mut rows = stmt.query(params_from_iter(query.args))?;
    let mut out = Vec::new();
    let mut conversions = currency::Resolver::new(ctx);
    while let Some(row) = rows.next()? {
        let mut item = read_row(&row)?;
        item.category = read_category(&row, 21)?;
        if let Some((currency, min, max)) = display {
            let result =
                conversions.convert(&item.amount, currency, parse_date(&item.reporting_date)?)?;
            let Some(value) = result.displayed else {
                continue;
            };
            if !(min..=max).contains(&minor(&value.minor)?) {
                continue;
            }
        }
        out.push((row.get::<_, String>(20)?, item));
        if out.len() > paging.limit {
            break;
        }
    }
    let mut result = paging.finish(out, |(key, row)| (key.clone(), row.id.clone()))?;
    if running {
        running_balance(ctx, &mut result.rows)?;
    }
    if filter.pnl_only == Some(true) {
        let mut sums=Query::new("SELECT sum(s.amount_minor) FROM transaction_splits s JOIN categories c ON c.id=s.category_id
            WHERE c.ignored=0 AND c.kind<>'transfer' AND s.transaction_id=?1".to_owned());
        sums.args.push(Value::Null);
        sums.choices("s.category_id IN ({values})", &filter.category_ids);
        let mut statement = ctx.tx.prepare(&sums.sql)?;
        for (_, row) in &mut result.rows {
            sums.args[0] = row.id.clone().into();
            let split_total = statement.query_row(params_from_iter(&sums.args), |row| {
                row.get::<_, Option<i64>>(0)
            })?;
            row.pnl_amount = Some(split_total.map_or_else(
                || row.amount.clone(),
                |minor| money(&row.amount.currency, minor),
            ));
        }
    }
    Ok(Page {
        rows: result.rows.into_iter().map(|(_, row)| row).collect(),
        next_cursor: result.next_cursor,
        revision: result.revision,
    })
}

fn running_balance(ctx: &Context<'_>, selected: &mut [(String, TxRow)]) -> Result<()> {
    let mut locations: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, (_, row)) in selected.iter().enumerate() {
        locations
            .entry(row.account_id.clone())
            .or_default()
            .push(index);
    }
    for (account, mut indices) in locations {
        let key = |index: usize| {
            let row = &selected[index].1;
            (row.bank_date.clone(), row.id.clone())
        };
        indices.sort_unstable_by_key(|index| key(*index));
        let floor = accounts::opening_floor(ctx, &account)?;
        if let Some(floor) = &floor {
            indices.retain(|index| selected[*index].1.bank_date >= *floor);
            if indices.is_empty() {
                continue;
            }
        }
        let (first, first_id) = key(indices[0]);
        let (last, last_id) = key(*indices.last().expect("one selected row"));
        let mut total = accounts::balance(ctx, &account, parse_date(&first)?, Some(&first_id))?;
        let snapshot = if floor.is_some() {
            "AND NOT (t.bank_date=?2 AND t.entry_kind='opening')"
        } else {
            ""
        };
        let mut stmt=ctx.tx.prepare_cached(&format!("SELECT t.bank_date,t.id,CASE t.direction WHEN 'credit' THEN t.amount_minor ELSE -t.amount_minor END
            FROM transactions t JOIN accounts a ON a.id=t.account_id WHERE t.account_id=?1 AND
            (t.bank_date,t.id)>(?2,?3) AND (t.bank_date,t.id)<=(?4,?5) AND {} {snapshot} ORDER BY t.bank_date,t.id",accounts::BALANCE_VISIBLE))?;
        let mut rows = stmt.query(params![account, first, first_id, last, last_id])?;
        let mut pending = rows.next()?.as_ref().map(movement).transpose()?;
        for position in indices {
            let row = &mut selected[position].1;
            while pending
                .as_ref()
                .is_some_and(|(date, id, _)| (date, id) <= (&row.bank_date, &row.id))
            {
                total += i128::from(pending.take().expect("pending movement").2);
                pending = rows.next()?.as_ref().map(movement).transpose()?;
            }
            row.running_balance = Some(money(&row.amount.currency, total));
        }
    }
    Ok(())
}
fn movement(row: &Row<'_>) -> crate::sql::Result<(String, String, i64)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}

pub fn weighted_split(amount: i64, weights: &[u64]) -> Result<Vec<i64>> {
    if !(1..=9_000_000_000_000).contains(&amount)
        || !(2..=1000).contains(&weights.len())
        || weights.contains(&0)
    {
        return Err(invalid("weights", "invalid_split_weights"));
    }
    let sum = weights
        .iter()
        .map(|weight| u128::from(*weight))
        .sum::<u128>();
    let mut shares = Vec::with_capacity(weights.len());
    let mut remainder = Vec::with_capacity(weights.len());
    for (index, weight) in weights.iter().enumerate() {
        let numerator = u128::from(amount as u64) * u128::from(*weight);
        shares.push((numerator / sum) as i64);
        remainder.push((numerator % sum, index));
    }
    remainder.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, index) in remainder
        .into_iter()
        .take((amount - shares.iter().sum::<i64>()) as usize)
    {
        shares[index] += 1;
    }
    if shares.contains(&0) {
        return Err(invalid("weights", "split_share_rounds_to_zero"));
    }
    Ok(shares)
}
