CREATE TABLE financial_calendar (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    timezone TEXT NOT NULL CHECK(timezone IN ('Asia/Jakarta','Asia/Makassar','Asia/Jayapura'))
) STRICT;

CREATE TABLE financial_calendar_revisions (
    effective_from TEXT PRIMARY KEY,
    anchor INTEGER NOT NULL CHECK(anchor BETWEEN 1 AND 31),
    weekday_policy TEXT NOT NULL CHECK(weekday_policy IN ('none','previousWorkingDay','nextWorkingDay'))
) STRICT;

CREATE TABLE cycle_budgets (
    id TEXT PRIMARY KEY,
    cycle_start TEXT NOT NULL,
    name TEXT NOT NULL CHECK(length(trim(name)) BETWEEN 1 AND 100),
    limit_minor INTEGER NOT NULL CHECK(limit_minor BETWEEN 0 AND 9000000000000),
    UNIQUE(cycle_start,name),
    UNIQUE(id,cycle_start)
) STRICT;

CREATE TABLE cycle_budget_categories (
    budget_id TEXT NOT NULL,
    cycle_start TEXT NOT NULL,
    category_id TEXT NOT NULL REFERENCES categories(id),
    PRIMARY KEY(cycle_start,category_id),
    FOREIGN KEY(budget_id,cycle_start) REFERENCES cycle_budgets(id,cycle_start) ON DELETE CASCADE
) STRICT;

CREATE INDEX tx_adjustment_date ON transactions(bank_date,id) WHERE entry_kind='adjustment';

CREATE TABLE posting_audit (
    id TEXT PRIMARY KEY,
    transaction_id TEXT NOT NULL,
    action TEXT NOT NULL CHECK(action IN ('void','correction','opening','refund','adjustment')),
    reason TEXT NOT NULL CHECK(length(trim(reason)) BETWEEN 1 AND 500),
    before_json TEXT NOT NULL CHECK(json_valid(before_json)),
    after_json TEXT NOT NULL CHECK(json_valid(after_json)),
    recorded_at_ms INTEGER NOT NULL,
    context_json TEXT NOT NULL CHECK(json_valid(context_json))
) STRICT;

CREATE INDEX posting_audit_subject ON posting_audit(transaction_id,recorded_at_ms,id);

CREATE TABLE posting_voids (
    transaction_id TEXT PRIMARY KEY REFERENCES transactions(id),
    audit_id TEXT NOT NULL UNIQUE REFERENCES posting_audit(id)
) STRICT;

CREATE TABLE posting_refunds (
    refund_id TEXT PRIMARY KEY REFERENCES transactions(id),
    original_id TEXT REFERENCES transactions(id),
    CHECK(original_id IS NULL OR original_id<>refund_id)
) STRICT;
CREATE INDEX posting_refunds_original ON posting_refunds(original_id);

CREATE TABLE cash_plan_entries (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL CHECK(length(trim(name)) BETWEEN 1 AND 100),
    due_on TEXT NOT NULL,
    amount_minor INTEGER NOT NULL CHECK(amount_minor BETWEEN 1 AND 9000000000000),
    kind TEXT NOT NULL CHECK(kind IN ('obligation','stableIncome','uncertainIncome')),
    transaction_id TEXT UNIQUE REFERENCES transactions(id)
) STRICT;
CREATE INDEX cash_plan_due ON cash_plan_entries(due_on,id);

CREATE TABLE transfer_fees (
    transfer_id TEXT PRIMARY KEY REFERENCES transfer_links(id),
    transaction_id TEXT NOT NULL UNIQUE REFERENCES transactions(id)
) STRICT;

CREATE TRIGGER protected_financial_posting_flags
BEFORE UPDATE OF ignored, exclude_from_pnl, status, amount_minor, direction, bank_date, account_id, currency_code ON transactions
WHEN (
  OLD.ignored<>NEW.ignored OR OLD.exclude_from_pnl<>NEW.exclude_from_pnl OR OLD.status<>NEW.status
  OR OLD.amount_minor<>NEW.amount_minor OR OLD.direction<>NEW.direction OR OLD.bank_date<>NEW.bank_date
  OR OLD.account_id<>NEW.account_id OR OLD.currency_code<>NEW.currency_code
) AND (
  EXISTS(SELECT 1 FROM posting_voids WHERE transaction_id=OLD.id)
  OR EXISTS(SELECT 1 FROM posting_refunds WHERE refund_id=OLD.id OR original_id=OLD.id)
  OR EXISTS(SELECT 1 FROM cash_plan_entries WHERE transaction_id=OLD.id)
  OR EXISTS(SELECT 1 FROM transfer_fees WHERE transaction_id=OLD.id)
)
BEGIN
  SELECT RAISE(ABORT,'posting_has_audited_dependencies');
END;

CREATE TABLE cash_plan_stages (
    rule_id TEXT NOT NULL REFERENCES recurring_rules(id) ON DELETE CASCADE,
    starts_on TEXT NOT NULL,
    ends_on TEXT CHECK(ends_on>=starts_on),
    amount_minor INTEGER NOT NULL CHECK(amount_minor BETWEEN 1 AND 9000000000000),
    PRIMARY KEY(rule_id,starts_on)
) STRICT;

CREATE TABLE account_opening_snapshots (
    account_id TEXT PRIMARY KEY REFERENCES accounts(id),
    effective_on TEXT NOT NULL
) STRICT;
