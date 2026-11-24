# Securo Local-First Desktop Architecture Specification

**Status:** Proposed for Product Owner review. **Requirements baseline:** [Frozen V1 requirements](REQUIREMENTS_SPEC.md). **Reference:** [Feature inventory](FEATURE_INVENTORY.md) and checkout `dade60c8abc418e58dbb47c04818bcac00dddfea`.

## 1. Architecture decisions and reference trace

| Decision | Desktop design | Reference checked |
|---|---|---|
| Isolation | One active ledger connection family per selected file and session generation. Rust owns paths, connections and secrets. | ADR 0002, 0003, 0011 and `backend/app/core/workspace_context.py` |
| Money and dates | Exact native/account amounts, independent report attribution, explicit cash-flow timing. Indexed persisted derivations. | ADR 0008, 0009, 0017, 0018, 0027 and `backend/app/services/{account_service,credit_card_service,_query_filters,transaction_calendar_service}.py` |
| Import and automation | Format adapters, deterministic pipeline, local clock and scheduler. User-approved preview and reversible import effects. | `backend/app/services/{import_service,rule_engine,recurring_transaction_service,budget_service}.py` |
| Planning and groups | Local read-time projection, weighted-average holdings and personal IOUs. | `backend/app/services/{loan_schedule,goal_service,asset_transaction_service,balance_service,settlement_service}.py` |
| Storage | Local files, encrypted archives, per-ledger SQLCipher and device-local quick unlock. | ADR 0019 and `backend/app/services/{attachment_service,backup_service}.py` |
| Desktop shell | Typed local IPC, SvelteKit SPA and ledger-scoped caches. | `frontend/src/{App.tsx,contexts,hooks/use-privacy-mode.ts}` |

Preserve reference close-day exclusion, pending authorization treatment, exact category/share totals and reporting-date priority. V1 changes include zero-/three-decimal money, loan contracts up to 1,200 months (reference contract cap 600), local IOU settlements on either side excluded from P&L as debt clearance, and backup format replacement. V2 items enter through new adapters and additive migrations. Permanent SaaS exclusions are listed in the frozen requirements.

## 2. Process and module boundaries

| Location | Responsibilities |
|---|---|
| `src-tauri/domain/` | Pure IDs, money/scale, dates, statuses, typed commands, error codes and validated value objects. Interfaces `LedgerRead`, `LedgerWrite`, `Clock`, `CurrencyRegistry`, `FxRateSource`, `StatementParser`, `ValuationSource` and `AttachmentStore`. |
| `src-tauri/ledger_engine/` | Ledger unit of work, accounts/cards, postings, transfers, splits, installment series, reconciliation, balances, search and reports. Central eligibility predicates. |
| `src-tauri/importers/` | Byte detection/decoding, format adapters, declarative templates, staging, dedupe, dependency-aware undo and export mapping. |
| `src-tauri/rules/` | Versioned AST, compilation, deterministic evaluation, preview and replay. |
| `src-tauri/planning/` | Recurring occurrences, budgets, forecasts, loans, goals, assets and group IOUs. Scheduler dispatches to services. |
| `src-tauri/storage/` | SQLite repositories, migrations, connection lifecycle, registry, safe paths, attachment/restore journals and backup snapshot coordination. |
| `src-tauri/crypto/` | Credential enrollment, wrapping/Keychain, SQLCipher bootstrap, attachment keys and backup envelope. |
| `src-tauri/ipc/` | Tauri command registration, DTO mapping, authorization, job registry and progress. |
| `src/` | Static SvelteKit SPA, Svelte 5 Runes, Tailwind CSS, typed IPC client, routes and presentation. |

Use one Rust app crate initially with module visibility as the boundary. Domain ports depend on domain types, storage implements ports, IPC composes services. Repositories receive a scoped transaction handle or read snapshot. Ports are versioned DTO boundaries, so V2 ingestion can emit the same canonical statement records. Business invoices can later reference stable payee/posting IDs in separate tables. No renderer SQL, raw shell/filesystem, or general-purpose network permissions.

### 2.1 Ledger lifecycle

`Unselected → Resolving → Locked → Opening → Ready → Quiescing → Locked/Unselected`. Unencrypted ledgers skip the credential challenge. A failed open returns to Locked or Unselected with an actionable error. Restore/rekey use exclusive Maintenance mode and return to Ready only after validation. Keep only a sanitized registry outside ledger data (local file handle, instance ID, optional label and storage policy). Check canonical path and OS file identity, reject duplicate aliases and take an application-level interprocess lease. Open existing files without create flags. Reject unsupported schema versions and invalid SQLCipher keys before exposing records.

Every Ready transition issues a new opaque session generation bound to window and ledger instance. Switch/lock rejects new work, cancels read/staging jobs, waits for active commit or safe rollback, closes readers then writer, clears in-memory keys and renderer caches. If quiescing fails, report busy and preserve a consistent session. Long jobs hold a generation-bound lease and cannot commit against the next ledger. Keep an attachment/storage-instance ID distinct from the internal ledger UUID so restored copies receive independent directories and registry identities.

Use a single keyed writer with bounded reader workers. Each connection uses SQLCipher library and validates `PRAGMA foreign_keys=ON`, `busy_timeout=5000`, `synchronous=FULL`, memory temp storage and extended result codes. Apply key as the first codec operation, force a schema read to validate it, then apply ordinary DB settings. Enable and verify WAL on the writer. Initialize the two foreground reader slots on demand and make their connections query-only after setup. Writes use `BEGIN IMMEDIATE` and rollback on errors. Coalesce best-effort passive checkpoints during idle and run a bounded truncate checkpoint on clean drain when readers allow. Leave WAL sidecars in place on a busy checkpoint. Use local filesystems supporting WAL locking, atomic same-filesystem rename and directory durability.

Track migration version and checksum. Acquire the writer lease, take a safety backup before irreversible migration, run migrations atomically where SQLite permits, validate foreign keys and quick check, then publish Ready. Full integrity checks run for restore, support and recovery. Normal open also validates ledger identity, revision and migration checksums synchronously. On Unix, an app-owned, one-use clean-checkpoint receipt can defer the full SQLite quick check to a separate bounded query-only reader. The receipt binds the ledger UUID and revision to the device, inode, size, mtime and ctime after a drained truncate checkpoint. SQLCipher receipts use authenticated encryption under a domain-separated key. The app consumes the receipt before opening a connection. Missing or conflicting evidence, WAL residue, migrations, and restore or diagnostics keep validation synchronous. A failed background check sends a scoped integrity alert and blocks subsequent access. Closing before the check finishes or after it fails leaves no new receipt. JSON codecs have their own version and strict size/depth validation. Scheduled work runs on unlocked ledgers while the app is running, records missed due times, and catches up once per overdue backup schedule on unlock. No execution requires a server.

## 3. SQLite schema

The SQL below is the proposed complete `migrations/0001_init.sql`, embedded for review. The migration runner enables FK enforcement before the transaction, commits and records checksum/version. Use a pinned SQLCipher build with SQLite STRICT tables, generated columns, JSON support and FTS5. Type conventions are ISO `YYYY-MM-DD` text for validated Gregorian dates, UTC epoch milliseconds for audit timestamps, text UUIDs for durable identities, and `0/1` for booleans. Currency scales are forward-extendable and immutable once used. Seed ISO registry before creating ledger metadata, including IDR 0, USD/CNY/EUR/SGD 2 and KWD 3. Dates, canonical decimals, Unicode/search keys and polymorphic payload validation remain Rust responsibilities.

Amounts are signed where intrinsically net (opening balance, adjustment input, growth delta) and positive magnitudes plus debit/credit elsewhere. Store SQLite INTEGER minor units with an immutable currency reference (NFR-001/NFR-002). Posted transaction amounts and category shares use account currency. Optional original statement amount/currency preserves a second denomination. Exact rates and fractional units are canonical base-10 TEXT, parsed with checked decimal operations. Use checked integer/decimal operations and explicit range errors. Use i128 for aggregation beyond i64. One posting may contain at most 9,000,000,000,000 minor units and 1,000 split lines, with a matching split-line bound to keep SQL split sums inside i64. At reference scale even the maximum transaction magnitude sum fits i64. IPC exposes integer minor units and decimals as strings.

```sql
CREATE TABLE currencies (
  code TEXT PRIMARY KEY CHECK(length(code)=3 AND code GLOB '[A-Z][A-Z][A-Z]'),
  scale INTEGER NOT NULL CHECK(scale IN (0,2,3)),
  name TEXT NOT NULL
) STRICT;
CREATE TRIGGER currency_scale_immutable BEFORE UPDATE OF scale ON currencies
WHEN NEW.scale <> OLD.scale BEGIN
  SELECT RAISE(ABORT,'immutable currency scale');
END;

CREATE TABLE ledger_meta (
  singleton INTEGER PRIMARY KEY CHECK(singleton=1),
  ledger_uuid TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL,
  timezone_override TEXT,
  display_currency TEXT NOT NULL REFERENCES currencies(code),
  accounting_mode TEXT NOT NULL DEFAULT 'cash' CHECK(accounting_mode IN ('cash','accrual')),
  revision INTEGER NOT NULL DEFAULT 0 CHECK(revision>=0),
  created_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE ledger_settings (
  key TEXT PRIMARY KEY,
  value_json TEXT NOT NULL CHECK(json_valid(value_json))
) STRICT;

CREATE TABLE fx_rates (
  id TEXT PRIMARY KEY,
  base_code TEXT NOT NULL REFERENCES currencies(code),
  quote_code TEXT NOT NULL REFERENCES currencies(code),
  rate_date TEXT NOT NULL,
  rate_decimal TEXT NOT NULL,
  source_kind TEXT NOT NULL CHECK(source_kind IN ('manual','csv','provider')),
  source_key TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  CHECK(base_code<>quote_code),
  UNIQUE(base_code,quote_code,rate_date,source_kind,source_key)
) STRICT;
CREATE INDEX fx_lookup ON fx_rates(base_code,quote_code,rate_date DESC);

CREATE TABLE credit_card_lines (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  currency_code TEXT NOT NULL REFERENCES currencies(code),
  limit_minor INTEGER NOT NULL CHECK(limit_minor>=0),
  UNIQUE(id,currency_code)
) STRICT;

CREATE TABLE accounts (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  institution TEXT,
  kind TEXT NOT NULL CHECK(kind IN ('checking','savings','credit_card','loan')),
  currency_code TEXT NOT NULL REFERENCES currencies(code),
  archived_on TEXT,
  payment_method TEXT,
  close_day INTEGER CHECK(close_day BETWEEN 1 AND 31),
  due_day INTEGER CHECK(due_day BETWEEN 1 AND 31),
  card_limit_minor INTEGER CHECK(card_limit_minor>=0),
  credit_line_id TEXT,
  accounting_override TEXT CHECK(accounting_override IN ('cash','accrual')),
  UNIQUE(id,currency_code),
  FOREIGN KEY(credit_line_id,currency_code) REFERENCES credit_card_lines(id,currency_code),
  CHECK((close_day IS NULL)=(due_day IS NULL)),
  CHECK(credit_line_id IS NULL OR card_limit_minor IS NULL),
  CHECK(kind='credit_card' OR (close_day IS NULL AND card_limit_minor IS NULL
    AND credit_line_id IS NULL AND accounting_override IS NULL)),
  CHECK(payment_method IS NULL OR kind='checking')
) STRICT;
CREATE INDEX accounts_line ON accounts(credit_line_id);

CREATE TABLE account_aliases (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  label TEXT NOT NULL,
  last_four TEXT CHECK(last_four GLOB '[0-9][0-9][0-9][0-9]'),
  UNIQUE(id,account_id)
) STRICT;

CREATE TABLE card_statements (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  close_on TEXT NOT NULL,
  due_on TEXT NOT NULL CHECK(due_on>close_on),
  source TEXT NOT NULL CHECK(source IN ('computed','manual','statement_file')),
  UNIQUE(account_id,close_on),
  UNIQUE(id,account_id)
) STRICT;

CREATE TABLE category_groups (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  sort_order INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE TABLE categories (
  id TEXT PRIMARY KEY,
  group_id TEXT REFERENCES category_groups(id),
  name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN ('income','expense','transfer')),
  hidden INTEGER NOT NULL DEFAULT 0 CHECK(hidden IN (0,1)),
  ignored INTEGER NOT NULL DEFAULT 0 CHECK(ignored IN (0,1)),
  archived INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0,1))
) STRICT;
CREATE INDEX category_group_ref ON categories(group_id);

CREATE TABLE payees (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  normalized_name TEXT NOT NULL UNIQUE,
  archived INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0,1))
) STRICT;

CREATE TABLE import_templates (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  institution TEXT,
  format TEXT NOT NULL CHECK(format IN ('csv','ofx','qif','camt')),
  schema_version INTEGER NOT NULL CHECK(schema_version>0),
  mapping_json TEXT NOT NULL CHECK(json_valid(mapping_json)),
  updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE import_batches (
  id TEXT PRIMARY KEY,
  account_id TEXT REFERENCES accounts(id),
  kind TEXT NOT NULL CHECK(kind IN ('statement','asset','fx')),
  template_id TEXT REFERENCES import_templates(id) ON DELETE SET NULL,
  file_name TEXT NOT NULL,
  content_sha256 TEXT NOT NULL CHECK(length(content_sha256)=64),
  format TEXT NOT NULL CHECK(format IN ('csv','ofx','qif','camt')),
  committed_at_ms INTEGER NOT NULL,
  undone_at_ms INTEGER,
  mapping_snapshot_json TEXT CHECK(json_valid(mapping_snapshot_json)),
  summary_json TEXT NOT NULL CHECK(json_valid(summary_json)),
  CHECK((kind='statement')=(account_id IS NOT NULL))
) STRICT;
CREATE INDEX batch_history ON import_batches(kind,committed_at_ms DESC,id);

CREATE TABLE recurring_rules (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL,
  currency_code TEXT NOT NULL,
  payee_id TEXT REFERENCES payees(id),
  category_id TEXT REFERENCES categories(id),
  direction TEXT NOT NULL CHECK(direction IN ('debit','credit')),
  amount_minor INTEGER NOT NULL CHECK(amount_minor BETWEEN 1 AND 9000000000000),
  description TEXT NOT NULL,
  notes TEXT NOT NULL DEFAULT '' CHECK(length(notes)<=1000),
  frequency TEXT NOT NULL CHECK(frequency IN ('weekly','biweekly','monthly','quarterly','semiannual','yearly')),
  anchor_day INTEGER CHECK(anchor_day BETWEEN 1 AND 31),
  starts_on TEXT NOT NULL,
  weekend_policy TEXT NOT NULL CHECK(weekend_policy IN ('none','previous_friday','next_monday')),
  ends_on TEXT CHECK(ends_on>=starts_on),
  count_limit INTEGER CHECK(count_limit>0),
  skip_first INTEGER NOT NULL DEFAULT 0 CHECK(skip_first IN (0,1)),
  auto_generate INTEGER NOT NULL DEFAULT 0 CHECK(auto_generate IN (0,1)),
  active INTEGER NOT NULL DEFAULT 1 CHECK(active IN (0,1)),
  generated_through_nominal TEXT,
  version INTEGER NOT NULL DEFAULT 1 CHECK(version>0),
  FOREIGN KEY(account_id,currency_code) REFERENCES accounts(id,currency_code),
  CHECK(ends_on IS NULL OR count_limit IS NULL)
) STRICT;

CREATE TABLE recurring_rule_splits (
  rule_id TEXT NOT NULL REFERENCES recurring_rules(id) ON DELETE CASCADE,
  position INTEGER NOT NULL CHECK(position BETWEEN 1 AND 1000),
  category_id TEXT NOT NULL REFERENCES categories(id),
  amount_minor INTEGER NOT NULL CHECK(amount_minor BETWEEN 1 AND 9000000000000),
  PRIMARY KEY(rule_id,position)
) STRICT;

CREATE TABLE installment_series (
  id TEXT PRIMARY KEY,
  purchase_date TEXT NOT NULL,
  occurrence_count INTEGER NOT NULL CHECK(occurrence_count>0),
  fingerprint TEXT UNIQUE
) STRICT;

-- Only sealed parents may exist at commit. State 0 may exist inside a write.
CREATE TABLE valid_transaction_state (
  value INTEGER PRIMARY KEY CHECK(value=1)
) STRICT;
INSERT INTO valid_transaction_state VALUES(1);

CREATE TABLE transactions (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL,
  currency_code TEXT NOT NULL,
  original_currency_code TEXT REFERENCES currencies(code),
  original_amount_minor INTEGER CHECK(original_amount_minor>0),
  account_fx_rate_decimal TEXT,
  account_fx_rate_date TEXT,
  account_fx_source TEXT,
  amount_minor INTEGER NOT NULL CHECK(amount_minor BETWEEN 1 AND 9000000000000),
  direction TEXT NOT NULL CHECK(direction IN ('debit','credit')),
  bank_date TEXT NOT NULL,
  effective_date TEXT NOT NULL,
  effective_bill_date TEXT,
  reporting_date_override TEXT,
  applied_mode TEXT NOT NULL CHECK(applied_mode IN ('cash','accrual')),
  reporting_date TEXT GENERATED ALWAYS AS (
    coalesce(reporting_date_override,effective_bill_date,
      CASE WHEN applied_mode='accrual' THEN effective_date ELSE bank_date END)
  ) STORED,
  status TEXT NOT NULL CHECK(status IN ('posted','pending','projected')),
  entry_kind TEXT NOT NULL DEFAULT 'ordinary' CHECK(entry_kind IN ('ordinary','opening','adjustment')),
  description TEXT NOT NULL,
  notes TEXT NOT NULL DEFAULT '' CHECK(length(notes)<=1000),
  payee_id TEXT REFERENCES payees(id),
  category_id TEXT REFERENCES categories(id),
  alias_id TEXT,
  statement_id TEXT,
  ignored INTEGER NOT NULL DEFAULT 0 CHECK(ignored IN (0,1)),
  exclude_from_pnl INTEGER NOT NULL DEFAULT 0 CHECK(exclude_from_pnl IN (0,1)),
  flagged INTEGER NOT NULL DEFAULT 0 CHECK(flagged IN (0,1)),
  flag_due_date TEXT,
  installment_series_id TEXT REFERENCES installment_series(id),
  installment_number INTEGER CHECK(installment_number>=1),
  assembly_state INTEGER NOT NULL CHECK(assembly_state IN (0,1)),
  created_at_ms INTEGER NOT NULL,
  version INTEGER NOT NULL DEFAULT 1 CHECK(version>0),
  UNIQUE(installment_series_id,installment_number),
  FOREIGN KEY(account_id,currency_code) REFERENCES accounts(id,currency_code),
  FOREIGN KEY(alias_id,account_id) REFERENCES account_aliases(id,account_id),
  FOREIGN KEY(statement_id,account_id) REFERENCES card_statements(id,account_id),
  FOREIGN KEY(assembly_state) REFERENCES valid_transaction_state(value)
    DEFERRABLE INITIALLY DEFERRED,
  CHECK((original_currency_code IS NULL)=(original_amount_minor IS NULL)),
  CHECK(original_currency_code IS NULL OR
    (original_currency_code<>currency_code AND account_fx_rate_decimal IS NOT NULL
      AND account_fx_rate_date IS NOT NULL AND account_fx_source IS NOT NULL)),
  CHECK((installment_series_id IS NULL)=(installment_number IS NULL)),
  CHECK(flag_due_date IS NULL OR flagged=1),
  CHECK(entry_kind='ordinary' OR
    (status='posted' AND category_id IS NULL AND exclude_from_pnl=1))
) STRICT;

CREATE UNIQUE INDEX one_account_opening ON transactions(account_id) WHERE entry_kind='opening';
CREATE INDEX tx_bank ON transactions(bank_date DESC,id DESC);
CREATE INDEX tx_reporting ON transactions(reporting_date DESC,id DESC);
CREATE INDEX tx_flow ON transactions(effective_date,id);
CREATE INDEX tx_account_bank ON transactions(account_id,bank_date DESC,id DESC);
CREATE INDEX tx_balance ON transactions(account_id,bank_date,id,status,direction,amount_minor)
  WHERE ignored=0 AND status IN ('posted','pending');
CREATE INDEX tx_account_report ON transactions(account_id,reporting_date DESC,id DESC);
CREATE INDEX tx_pnl_report ON transactions(reporting_date,account_id,category_id,id)
  WHERE status='posted' AND ignored=0 AND exclude_from_pnl=0;
CREATE INDEX tx_category_report ON transactions(category_id,reporting_date DESC,id DESC);
CREATE INDEX tx_payee_report ON transactions(payee_id,reporting_date DESC,id DESC);
CREATE INDEX tx_statement ON transactions(statement_id,bank_date,id);
CREATE INDEX tx_alias ON transactions(alias_id,bank_date,id);
CREATE INDEX tx_amount ON transactions(currency_code,amount_minor,id);
CREATE INDEX tx_flag_due ON transactions(flag_due_date,id) WHERE flagged=1;

CREATE TABLE transaction_splits (
  transaction_id TEXT NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
  position INTEGER NOT NULL CHECK(position BETWEEN 1 AND 1000),
  category_id TEXT NOT NULL REFERENCES categories(id),
  amount_minor INTEGER NOT NULL CHECK(amount_minor BETWEEN 1 AND 9000000000000),
  PRIMARY KEY(transaction_id,position)
) STRICT;
CREATE INDEX split_category ON transaction_splits(category_id,transaction_id);

CREATE TABLE transfer_links (
  id TEXT PRIMARY KEY,
  debit_id TEXT NOT NULL UNIQUE REFERENCES transactions(id),
  credit_id TEXT NOT NULL UNIQUE REFERENCES transactions(id),
  applied_rate_decimal TEXT,
  rate_date TEXT,
  rate_source TEXT,
  created_at_ms INTEGER NOT NULL,
  CHECK(debit_id<>credit_id),
  CHECK((applied_rate_decimal IS NULL)=(rate_date IS NULL)
    AND (rate_date IS NULL)=(rate_source IS NULL))
) STRICT;

CREATE TRIGGER tx_assemble_before_insert BEFORE INSERT ON transactions
WHEN NEW.assembly_state<>0 BEGIN
  SELECT RAISE(ABORT,'insert transaction in assembly state');
END;
CREATE TRIGGER tx_validate_seal BEFORE UPDATE ON transactions
WHEN NEW.assembly_state=1 AND EXISTS(
  SELECT 1 FROM transaction_splits WHERE transaction_id=OLD.id
) BEGIN
  SELECT RAISE(ABORT,'invalid split parent')
  WHERE NEW.category_id IS NOT NULL OR NEW.entry_kind<>'ordinary'
     OR (SELECT count(*) FROM transaction_splits WHERE transaction_id=OLD.id)<2
     OR (SELECT sum(amount_minor) FROM transaction_splits WHERE transaction_id=OLD.id)<>NEW.amount_minor;
END;
CREATE TRIGGER split_insert_gate BEFORE INSERT ON transaction_splits BEGIN
  SELECT RAISE(ABORT,'split requires assembling a non-transfer transaction')
  WHERE NOT EXISTS(SELECT 1 FROM transactions WHERE id=NEW.transaction_id AND assembly_state=0)
     OR EXISTS(SELECT 1 FROM transfer_links WHERE debit_id=NEW.transaction_id OR credit_id=NEW.transaction_id);
END;
CREATE TRIGGER split_update_gate BEFORE UPDATE ON transaction_splits BEGIN
  SELECT RAISE(ABORT,'replace split lines within assembly')
  WHERE OLD.transaction_id<>NEW.transaction_id OR NOT EXISTS(
    SELECT 1 FROM transactions WHERE id=OLD.transaction_id AND assembly_state=0);
END;
CREATE TRIGGER split_delete_gate BEFORE DELETE ON transaction_splits
WHEN EXISTS(SELECT 1 FROM transactions WHERE id=OLD.transaction_id AND assembly_state=1) BEGIN
  SELECT RAISE(ABORT,'unseal parent before editing splits');
END;

CREATE TRIGGER transfer_validate_insert BEFORE INSERT ON transfer_links BEGIN
  SELECT RAISE(ABORT,'invalid transfer legs')
  WHERE NOT EXISTS(
    SELECT 1 FROM transactions d JOIN transactions c ON c.id=NEW.credit_id
    WHERE d.id=NEW.debit_id AND d.direction='debit' AND c.direction='credit'
      AND d.account_id<>c.account_id AND d.assembly_state=1 AND c.assembly_state=1
      AND d.entry_kind='ordinary' AND c.entry_kind='ordinary'
      AND d.ignored=0 AND c.ignored=0 AND d.category_id IS NULL AND c.category_id IS NULL
      AND ((d.currency_code=c.currency_code AND d.amount_minor=c.amount_minor
           AND NEW.applied_rate_decimal IS NULL)
        OR (d.currency_code<>c.currency_code AND NEW.applied_rate_decimal IS NOT NULL))
  ) OR EXISTS(
    SELECT 1 FROM transaction_splits WHERE transaction_id IN (NEW.debit_id,NEW.credit_id)
  );
END;
CREATE TRIGGER transfer_link_immutable BEFORE UPDATE ON transfer_links BEGIN
  SELECT RAISE(ABORT,'replace transfer link in one unit of work');
END;
CREATE TRIGGER transfer_leg_protect BEFORE UPDATE OF id,account_id,currency_code,amount_minor,
  direction,entry_kind,ignored,category_id,assembly_state ON transactions
WHEN EXISTS(SELECT 1 FROM transfer_links WHERE debit_id=OLD.id OR credit_id=OLD.id)
  AND (NEW.id IS NOT OLD.id OR NEW.account_id IS NOT OLD.account_id
    OR NEW.currency_code IS NOT OLD.currency_code OR NEW.amount_minor IS NOT OLD.amount_minor
    OR NEW.direction IS NOT OLD.direction OR NEW.entry_kind IS NOT OLD.entry_kind
    OR NEW.ignored IS NOT OLD.ignored OR NEW.category_id IS NOT OLD.category_id
    OR NEW.assembly_state IS NOT OLD.assembly_state)
BEGIN
  SELECT RAISE(ABORT,'unlink before changing transfer leg structure');
END;

CREATE TABLE transaction_import_keys (
  id INTEGER PRIMARY KEY,
  transaction_id TEXT NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
  batch_id TEXT NOT NULL REFERENCES import_batches(id),
  provider_namespace TEXT,
  external_id TEXT,
  external_key_hash TEXT,
  fingerprint TEXT NOT NULL CHECK(length(fingerprint)=64),
  raw_description TEXT NOT NULL,
  provider_category TEXT,
  raw_bank_date TEXT NOT NULL,
  raw_amount_minor INTEGER NOT NULL CHECK(raw_amount_minor>0),
  raw_currency TEXT NOT NULL REFERENCES currencies(code),
  raw_direction TEXT NOT NULL CHECK(raw_direction IN ('debit','credit')),
  CHECK((provider_namespace IS NULL)=(external_id IS NULL)
    AND (external_id IS NULL)=(external_key_hash IS NULL))
) STRICT;
CREATE UNIQUE INDEX import_external_identity ON transaction_import_keys(external_key_hash)
  WHERE external_key_hash IS NOT NULL;
CREATE INDEX import_fingerprint ON transaction_import_keys(fingerprint,transaction_id);
CREATE INDEX import_keys_transaction ON transaction_import_keys(transaction_id,batch_id);
CREATE INDEX import_keys_batch ON transaction_import_keys(batch_id);

CREATE TABLE transaction_overrides (
  transaction_id TEXT NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
  field_key TEXT NOT NULL,
  PRIMARY KEY(transaction_id,field_key)
) STRICT;

CREATE TABLE transaction_tags (
  transaction_id TEXT NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
  token TEXT NOT NULL,
  PRIMARY KEY(transaction_id,token)
) STRICT;
CREATE INDEX tag_lookup ON transaction_tags(token,transaction_id);

-- Derived, rebuildable search cache. rowid maps to transaction rowid.
CREATE VIRTUAL TABLE transaction_search USING fts5(
  description, notes, payee, tokenize='unicode61 remove_diacritics 2', prefix='2 3'
);

CREATE TABLE recurring_occurrences (
  rule_id TEXT NOT NULL REFERENCES recurring_rules(id),
  nominal_date TEXT NOT NULL,
  scheduled_date TEXT NOT NULL,
  transaction_id TEXT UNIQUE REFERENCES transactions(id),
  state TEXT NOT NULL CHECK(state IN ('generated','matched','skipped')),
  rule_version INTEGER NOT NULL,
  PRIMARY KEY(rule_id,nominal_date),
  CHECK((state='skipped')=(transaction_id IS NULL))
) STRICT;
CREATE INDEX recurring_occurrence_date ON recurring_occurrences(scheduled_date,rule_id);

CREATE TABLE budget_defaults (
  category_id TEXT PRIMARY KEY REFERENCES categories(id),
  currency_code TEXT NOT NULL REFERENCES currencies(code),
  amount_minor INTEGER NOT NULL CHECK(amount_minor>0)
) STRICT;
CREATE TABLE budgets (
  category_id TEXT NOT NULL REFERENCES categories(id),
  month_start TEXT NOT NULL CHECK(substr(month_start,9,2)='01'),
  currency_code TEXT NOT NULL REFERENCES currencies(code),
  amount_minor INTEGER NOT NULL CHECK(amount_minor>0),
  PRIMARY KEY(category_id,month_start)
) STRICT;
CREATE INDEX budget_by_month ON budgets(month_start,category_id);

CREATE TABLE categorization_rules (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  priority INTEGER NOT NULL,
  enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
  apply_existing INTEGER NOT NULL CHECK(apply_existing IN (0,1)),
  overwrite_existing_category INTEGER NOT NULL CHECK(overwrite_existing_category IN (0,1)),
  schema_version INTEGER NOT NULL CHECK(schema_version>0),
  conditions_json TEXT NOT NULL CHECK(json_valid(conditions_json)),
  actions_json TEXT NOT NULL CHECK(json_valid(actions_json)),
  version INTEGER NOT NULL DEFAULT 1 CHECK(version>0)
) STRICT;
CREATE INDEX rules_execution_order ON categorization_rules(enabled,priority,id);

CREATE TABLE rule_applications (
  rule_id TEXT NOT NULL REFERENCES categorization_rules(id) ON DELETE CASCADE,
  rule_version INTEGER NOT NULL,
  transaction_id TEXT NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
  PRIMARY KEY(rule_id,rule_version,transaction_id)
) STRICT;
CREATE INDEX rule_applications_transaction ON rule_applications(transaction_id);

CREATE TABLE loans (
  account_id TEXT PRIMARY KEY,
  currency_code TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN ('mortgage','auto','personal','student','other')),
  principal_minor INTEGER NOT NULL CHECK(principal_minor>0),
  rate_decimal TEXT NOT NULL,
  rate_period TEXT NOT NULL CHECK(rate_period IN ('monthly','yearly_nominal','yearly_effective')),
  amortization TEXT NOT NULL CHECK(amortization IN ('price','sac')),
  term_months INTEGER NOT NULL CHECK(term_months BETWEEN 1 AND 1200),
  first_payment_date TEXT NOT NULL,
  FOREIGN KEY(account_id,currency_code) REFERENCES accounts(id,currency_code)
) STRICT;

CREATE TABLE loan_payment_allocations (
  transaction_id TEXT PRIMARY KEY REFERENCES transactions(id),
  loan_account_id TEXT NOT NULL REFERENCES loans(account_id),
  principal_minor INTEGER NOT NULL CHECK(principal_minor>=0),
  interest_minor INTEGER NOT NULL CHECK(interest_minor>=0),
  CHECK(principal_minor>0 OR interest_minor>0)
) STRICT;
CREATE INDEX loan_payment_by_account ON loan_payment_allocations(loan_account_id);

CREATE TABLE asset_groups (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  archived INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0,1))
) STRICT;

CREATE TABLE assets (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN ('real_estate','vehicle','valuable','investment','other')),
  currency_code TEXT NOT NULL REFERENCES currencies(code),
  valuation_mode TEXT NOT NULL CHECK(valuation_mode IN ('manual','growth')),
  archived_on TEXT,
  UNIQUE(id,currency_code)
) STRICT;
CREATE TABLE asset_group_members (
  group_id TEXT NOT NULL REFERENCES asset_groups(id) ON DELETE CASCADE,
  asset_id TEXT NOT NULL REFERENCES assets(id),
  PRIMARY KEY(group_id,asset_id)
) STRICT;
CREATE INDEX asset_groups_by_asset ON asset_group_members(asset_id,group_id);

CREATE TABLE asset_growth_rules (
  asset_id TEXT NOT NULL REFERENCES assets(id),
  starts_on TEXT NOT NULL,
  interval_kind TEXT NOT NULL CHECK(interval_kind IN ('daily','weekly','monthly','yearly')),
  growth_kind TEXT NOT NULL CHECK(growth_kind IN ('percentage','fixed')),
  percent_decimal TEXT,
  delta_minor INTEGER,
  PRIMARY KEY(asset_id,starts_on),
  CHECK((growth_kind='percentage' AND percent_decimal IS NOT NULL AND delta_minor IS NULL)
     OR (growth_kind='fixed' AND delta_minor IS NOT NULL AND percent_decimal IS NULL))
) STRICT;

CREATE TABLE asset_valuations (
  id TEXT PRIMARY KEY,
  asset_id TEXT NOT NULL REFERENCES assets(id),
  valued_on TEXT NOT NULL,
  value_minor INTEGER NOT NULL CHECK(value_minor>=0),
  source TEXT NOT NULL CHECK(source IN ('manual','import')),
  created_at_ms INTEGER NOT NULL,
  UNIQUE(asset_id,valued_on)
) STRICT;

CREATE TABLE asset_transactions (
  id TEXT PRIMARY KEY,
  asset_id TEXT NOT NULL REFERENCES assets(id),
  trade_date TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN ('buy','sell','split')),
  units_decimal TEXT,
  gross_minor INTEGER CHECK(gross_minor>0),
  fees_minor INTEGER NOT NULL DEFAULT 0 CHECK(fees_minor>=0),
  split_old_units INTEGER CHECK(split_old_units>0),
  split_new_units INTEGER CHECK(split_new_units>0),
  linked_ledger_tx_id TEXT UNIQUE REFERENCES transactions(id),
  created_at_ms INTEGER NOT NULL,
  CHECK((kind='split' AND units_decimal IS NULL AND gross_minor IS NULL AND fees_minor=0
      AND split_old_units IS NOT NULL AND split_new_units IS NOT NULL)
    OR (kind<>'split' AND units_decimal IS NOT NULL AND gross_minor IS NOT NULL
      AND split_old_units IS NULL AND split_new_units IS NULL))
) STRICT;
CREATE INDEX asset_trades_history ON asset_transactions(asset_id,trade_date,created_at_ms,id);

CREATE TABLE goals (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  target_minor INTEGER NOT NULL,
  initial_minor INTEGER NOT NULL DEFAULT 0,
  currency_code TEXT NOT NULL REFERENCES currencies(code),
  starts_on TEXT NOT NULL,
  target_on TEXT NOT NULL CHECK(target_on>=starts_on),
  source TEXT NOT NULL CHECK(source IN ('manual','account','asset','asset_group','net_worth')),
  manual_minor INTEGER,
  account_id TEXT REFERENCES accounts(id),
  asset_id TEXT REFERENCES assets(id),
  asset_group_id TEXT REFERENCES asset_groups(id),
  state TEXT NOT NULL CHECK(state IN ('active','completed','paused','archived')),
  CHECK((source='manual')=(manual_minor IS NOT NULL)),
  CHECK((source='account')=(account_id IS NOT NULL)),
  CHECK((source='asset')=(asset_id IS NOT NULL)),
  CHECK((source='asset_group')=(asset_group_id IS NOT NULL))
) STRICT;

CREATE TABLE shared_groups (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN ('social','trip','cost_center','project','client','other')),
  archived INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0,1))
) STRICT;
CREATE TABLE shared_group_members (
  group_id TEXT NOT NULL REFERENCES shared_groups(id),
  id TEXT NOT NULL,
  label TEXT NOT NULL,
  is_self INTEGER NOT NULL CHECK(is_self IN (0,1)),
  PRIMARY KEY(group_id,id)
) STRICT;
CREATE UNIQUE INDEX shared_one_self ON shared_group_members(group_id) WHERE is_self=1;

CREATE TABLE shared_expenses (
  id TEXT PRIMARY KEY,
  group_id TEXT NOT NULL REFERENCES shared_groups(id),
  payer_id TEXT NOT NULL,
  transaction_id TEXT UNIQUE REFERENCES transactions(id),
  expense_on TEXT NOT NULL,
  description TEXT NOT NULL,
  currency_code TEXT NOT NULL REFERENCES currencies(code),
  amount_minor INTEGER NOT NULL CHECK(amount_minor BETWEEN 1 AND 9000000000000),
  allocation_method TEXT NOT NULL CHECK(allocation_method IN ('exact','percentage','equal')),
  FOREIGN KEY(group_id,payer_id) REFERENCES shared_group_members(group_id,id),
  UNIQUE(id,group_id)
) STRICT;
CREATE INDEX shared_expenses_by_group ON shared_expenses(group_id,expense_on,id);

CREATE TABLE shared_allocations (
  expense_id TEXT NOT NULL,
  group_id TEXT NOT NULL,
  member_id TEXT NOT NULL,
  share_minor INTEGER NOT NULL CHECK(share_minor>=0),
  percentage_decimal TEXT,
  position INTEGER NOT NULL CHECK(position>0),
  PRIMARY KEY(expense_id,member_id),
  UNIQUE(expense_id,position),
  FOREIGN KEY(expense_id,group_id) REFERENCES shared_expenses(id,group_id) ON DELETE CASCADE,
  FOREIGN KEY(group_id,member_id) REFERENCES shared_group_members(group_id,id)
) STRICT;
CREATE INDEX shared_allocations_member ON shared_allocations(group_id,member_id);

CREATE TABLE shared_settlements (
  id TEXT PRIMARY KEY,
  group_id TEXT NOT NULL REFERENCES shared_groups(id),
  from_member_id TEXT NOT NULL,
  to_member_id TEXT NOT NULL,
  settled_on TEXT NOT NULL,
  currency_code TEXT NOT NULL REFERENCES currencies(code),
  amount_minor INTEGER NOT NULL CHECK(amount_minor>0),
  transaction_id TEXT UNIQUE REFERENCES transactions(id),
  notes TEXT NOT NULL DEFAULT '',
  FOREIGN KEY(group_id,from_member_id) REFERENCES shared_group_members(group_id,id),
  FOREIGN KEY(group_id,to_member_id) REFERENCES shared_group_members(group_id,id),
  CHECK(from_member_id<>to_member_id)
) STRICT;
CREATE INDEX shared_settlements_group ON shared_settlements(group_id,settled_on,id);

CREATE TABLE attachments (
  id TEXT PRIMARY KEY,
  transaction_id TEXT NOT NULL REFERENCES transactions(id),
  storage_key TEXT NOT NULL UNIQUE,
  display_name TEXT NOT NULL,
  media_type TEXT NOT NULL,
  size_bytes INTEGER NOT NULL CHECK(size_bytes BETWEEN 1 AND 10000000),
  sha256 TEXT NOT NULL CHECK(length(sha256)=64),
  state TEXT NOT NULL CHECK(state IN ('staged','ready','deleting','missing')),
  created_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX attachments_transaction ON attachments(transaction_id,state);

-- Import effects retain history even if an entity is subsequently deleted.
CREATE TABLE import_effects (
  batch_id TEXT NOT NULL REFERENCES import_batches(id),
  ordinal INTEGER NOT NULL CHECK(ordinal>0),
  entity_kind TEXT NOT NULL CHECK(entity_kind IN ('transaction','fx_rate','asset_valuation','asset_transaction')),
  entity_id TEXT NOT NULL,
  operation TEXT NOT NULL CHECK(operation IN ('insert','update')),
  before_json TEXT CHECK(json_valid(before_json)),
  after_hash TEXT NOT NULL CHECK(length(after_hash)=64),
  PRIMARY KEY(batch_id,ordinal),
  CHECK((operation='update')=(before_json IS NOT NULL))
) STRICT;
CREATE INDEX import_effect_entity ON import_effects(entity_kind,entity_id,batch_id);

CREATE TABLE operation_receipts (
  request_id TEXT PRIMARY KEY,
  command TEXT NOT NULL,
  input_hash TEXT NOT NULL CHECK(length(input_hash)=64),
  result_json TEXT NOT NULL CHECK(json_valid(result_json)),
  committed_at_ms INTEGER NOT NULL
) STRICT;
```

Every posting creation inserts in assembly state 0, adds splits if any, then updates state to 1. Split editing reopens the parent and reseals before commit. Deferred FK rejects incomplete writes. Linked legs are sealed first; insert/check the link in the same SQL transaction. A leg belongs to at most one pair since roles are unique and fixed by validated direction. Delete linked records through a service: unlink then delete the selected pair or preserve explicit unmatched leg(s). External date/amount immutability, usable categories, account-kind-specific guards, original FX correctness, installment bounds, and group/loan/recurring allocation invariants are validated under the same writer transaction. For undo and parent deletion, check dependent objects before removing or explicitly detach them in the selected workflow.

Use keyset pagination with stable ID tie-breakers. Sort/filter keys are allowlisted and typed. Date/account indexes drive main paths. Compute partial range seed and paged running totals over the full balance stream before applying display-only filters. Additional exact sums switch to checked Rust accumulation if the deployment exceeds the proven SQLite SUM bound. Maintain FTS and normalized tag projections transactionally for create/edit/merge/delete. Rebuild FTS after snapshot restore or operations that can renumber SQLite rowids. Query by bound terms and compose filter predicates with EXISTS to avoid split/label fan-out. Run `ANALYZE` on representative volumes and evaluate query plans before adding indexes.

## 4. Financial algorithms and invariants

### 4.1 Dates, card cycles and balances

Use a validated local date, an injected Clock and an IANA ledger zone (OS-derived if unset). Capture today once for each command/read snapshot. Re-evaluate projections, due flags and day-based caches at local midnight, on resume, and on OS time-zone changes when the ledger follows the OS. Logging and audit timestamps use UTC. Preserve date-only statement fields as supplied. When a format supplies an instant, normalize using the source offset and the selected ledger/import-zone policy, and display the resolved date in preview.

| Concept | Computation and use |
|---|---|
| Bank date | User/provider booking date. Booked balance, historical FX stamping and import identity use this date. |
| Effective date | Cash-flow forecast date. For card purchases use manual effective bill date, then linked statement-file/manual due date, then computed cycle due date. For ordinary non-card entries and explicit card payments use actual bank date unless the entry has an applicable bill-date override. |
| Reporting date | First non-null of reporting_date_override, effective_bill_date and effective_date when the applied mode is accrual, otherwise bank_date. applied_mode = card override or ledger default for card entries, cash for ordinary non-card entries. Report, budget and calendar attribution use it. |

A cycle uses the first clamped close date strictly after purchase and the first clamped due date strictly after close. Cycle booking window is inclusive of the previous close and exclusive of the current close. For close 24, due 18, a July 23 purchase closes July 24 and is due August 18; July 24 closes August 24 and is due September 18. Close 10 and due 25 can both fall in the same month. Month arithmetic reuses original day and clamps each month separately. Missing cycle config falls back to bank date. A manual bill date maps to the unique adjacent due window `(previous_due, due]` if the neighboring date sequence distinguishes cycles. Use bank-date cycle assignment when clamping produces duplicate due dates and no exact statement ID is supplied. Statement IDs are authoritative for membership. The UI can show the next due and its associated close separately from the next future close. Rebuild derived dates and applied modes atomically on applicable account or ledger changes. Preserve manual overrides, actual statement dates and imported keys.

For each sorted `(bank_date, id)` row, credit adds and debit subtracts account minor units. The opening posting is ordinary signed opening equity for balance math and exists zero or one times per account. A zero opening amount removes it. Adjustments are signed dated delta postings with a reason in the notes. Current balance uses bank_date on or before today, excludes ignored rows and rows with an ignored parent category, and includes posted rows and pending financial authorizations for credit_card accounts. Ordinary pending and future projected entries remain outside current balance. A historical as-of query uses the same rule with its own cutoff, and reflects current data revisions rather than reconstructing past authorization states. Archiving preserves historical balances and disables new posting targets. Active-account dashboard totals omit archived accounts.

Card debt is max(0, negative signed balance). Shared line debt and available credit are derived once per line from the sum of distinct eligible same-currency card balances, including pending authorizations. An ungrouped card uses its own limit. Archived cards remain in the active line calculation until outstanding linked balances are cleared. Availability may go negative and credit balances cannot increase availability above the configured limit. Show future installment holds and capacity after holds separately. An unmatched authorization cannot be auto-posted. Confirmed pending-to-posted replacement retains identity and overrides and recalculates exposure once. A forecast may show due-date timing for a card charge already booked. Its projected closing balance must exclude any charge already in the starting position and exclude mirrored card payment cash flows when a predicted bill is replaced by its confirmed payment.

Use the same eligibility for all report aggregations and drill-downs: posted, bank_date no later than today, visible to arithmetic, P&L-enabled, and outside linked transfers or transfer categories. A parent with split categories expands into contribution lines only for allocation. Filter out ignored and transfer-classified lines individually and do not add parent value again. Actual budget buckets also cap reporting_date at today. Future reporting buckets appear as projections until reached. Synthetic openings and balance adjustments are excluded from P&L. Report series distinguish native amounts, reliable converted totals and estimated/missing components.

### 4.2 Transfers, FX, rounding and category shares

Link sealed debit and credit postings from different accounts in one unit of work. Same-currency magnitudes match exactly. Cross-currency legs store their own amounts and one positive implied or explicit rate with date and provenance. Rate means target major units per source major unit. If both amounts are supplied, they are authoritative and a conflicting optional quoted rate produces a validation error or proceeds through the explicit fee workflow. If one is derived, round once to the target currency scale using midpoint away from zero (half-up for positive magnitudes) and reject zero results. Record fees as separate categorized postings. Match existing legs in preview using amount/currency/date windows and safe metadata similarity. Confirm a unique candidate or choose manually. A match reuses the existing row. Report unmatched rows clearly and protect immutable import data on either side. Unlink/delete selections show which rows retain balances and which regain ordinary P&L classification.

FX chooses direct or inverse applicable pair, or a deterministic common-base cross rate for compatible dated legs. Source order at equal date is manual, CSV, provider, then stable stored ID. Prefer exact applicable date (use today as the upper bound for future projections), then latest prior rate. On-demand refresh is an explicit IPC request and consults a Rust allowlisted provider. Cross-rate provenance carries both dates and sources. Missing conversions preserve native amount and return a labeled 1:1 display estimate with a missing flag. Exclude unsupported values from reliable converted totals and present any estimate separately. Historical account conversions require real, explicit rates; reporting estimates cannot post to the ledger.

For weighted category splits, compute exact shares, floor at currency minor units and distribute remaining units by descending fractional remainder, ties in stable user order. Explicit category split amounts must already sum exactly and at least two shares must be positive. People allocations use exact entries or positive normalized weights, and round all but the final share in stable order with a running cap at remaining minor units. Final share absorbs residue. Exact allocations must already sum to total and percentage weights must sum exactly to 100%. Validate all caps before applying. Keep category and people allocations independent. Budget attribution uses the owner’s economic share. For split rows with people allocations, distribute that share over the category proportions once, preserving totals.

### 4.3 Recurring, budgeting, loans, goals, assets and groups

- **Recurring.** Weekly and biweekly add 7 or 14 days to the original anchor. Monthly/quarterly/semiannual/yearly advance 1/3/6/12 months and reapply the original day. A custom day 1–31 uses the first eligible on-or-after-start date. Compare the nominal occurrence to an inclusive end date or max count, then shift Saturday/Sunday by the selected previous-Friday/next-Monday policy. Skip-first advances the nominal sequence before generating eligible occurrences. Use `(rule_id, nominal_date)` as the identity even when shifted dates collide. Auto-generation inserts an idempotent projected placeholder; due placeholders may be marked posted or matched to real activity. Reuse/promote records on a confirmed import match. Persist generation floor and matched/skipped occurrences on rule edits; future virtual projections are regenerated from the current version. Schedules disabled for auto-generate still project, and each occurrence contributes at most once.
- **Budgets.** Read monthly override first, then category default. Projected spend is actual spend plus eligible future activity, with matched placeholders and virtual rows deduped. Remaining = limit minus projected spend. Carryover indicator is `remaining > 0`. No amount rolls forward. Validate 1–200 categories in atomic bulk operations, duplicate keys, copy adjustment range (−100% to +1000%), positive rounded amounts and explicit overwrite mode. Estimate suggested budgets from 1–24 complete prior months. Use category-currency conversion per reporting date and retain warnings.
- **Loans.** Validate positive principal, nonnegative decimal rate, monthly payment cadence, 1–1,200 months, and a valid first payment date. Normalize a rate entered as a percentage by dividing by 100. Monthly rate r is quoted monthly rate, nominal APR divided by 12, or twelfth root of (1 + effective annual rate) minus 1. Calculate roots via bounded decimal bisection/Newton at documented working precision, with interval refinement and a precision error if minor-unit rounding remains ambiguous. Price payment = P×r/[1−(1+r)^−n], or P/n at r=0. SAC principal = P/n. Round period interest and the fixed amount half-up to currency scale. Principal each period is balance-capped Price payment less interest or rounded fixed SAC principal. Increase rounded Price payment to at least first interest plus one minor unit when necessary for positive amortization, exposing the adjustment. Permit zero principal in early SAC rows when the obligation is smaller than term in minor units. In the last installment consume exactly the remaining principal. Due dates reuse the anchor day. Actual balance stays independent of the schedule. Payoff simulation stops by 1,200 months or when payment stops amortizing, and identifies the reference tolerance of one major unit as a projection indicator. Today is due, future dates are upcoming, and dates before today are overdue if unpaid. Principal/interest allocation is explicit and validates amount, currency and linked loan.
- **Goals.** Compute actual values from selected source, using native totals and explicit FX status. Enforce one valid target source on changes. Completion uses current ≥ target. For target ≤ 0 progress is 100% only if current > 0, else 0%. Monthly contribution is remaining divided by calendar-month difference between today and deadline, minimum one month, floored to zero when due/past due or funded. Active progress compares against elapsed linear expectation relative to the initial amount captured at creation, with 5 percentage-point on-track tolerance and 10 percentage-point ahead threshold. Compare deadline strictly against today. Status labels distinguish paused, archived, completed and overdue.
- **Assets.** Replay validated buy/sell/split events by date, split first on the same date, then creation time and ID. Buy adds quantity and gross cost plus fees. Sale removes weighted-average cost for quantity sold and realizes net proceeds less removed cost. A full exit consumes the remaining basis. Splits multiply quantity by new/old and preserve total basis. Validate prospective replay on past edits/deletes; reject oversell, impossible splits and overflow. Unrealized = current value minus held basis. Latest manual valuation on/before date anchors growth. Apply fixed or compound percent at completed day/week/month/year steps from the configured anchor until a newer observation/revision; maintain decimal precision and round at the output boundary. Clamp negative valuations at zero, validate growth ≥ −100%, and expose valuation origin and age. Groups aggregate assets once per query, and optional linked account postings must represent the cash leg only.
- **Shared IOUs.** Each group has one self member and zero or more local counterparties. Posting-linked expenses reuse the recorded amount/currency, while off-ledger expenses can identify another payer. Create payer credit and share debits in group balance projections. Settlement reduces existing same-currency obligation in the selected direction, optionally links an existing or newly recorded local movement if self participates, and recalculates remaining by member and currency. Foreign-currency paid amounts require a linked posting whose original amount/currency matches the settlement debt units and whose account amount has real FX provenance. Settlements clear IOUs and stay outside V1 income/expense. Show outstanding balances by native currency and convert display summaries only. Loan, group and schedule sum/type checks and their dependencies run under one writer transaction.

### 4.4 Reports and navigation

Net worth sums eligible historical bank-date account balances and asset values as-of each bucket. Income/expense and category trends aggregate the same reporting-date contribution lines used by drill-through. Cash flow separates booked bank-date data and effective-date forecasts. The calendar supports bank and reporting modes, Sunday-start and adjacent-month cells, account filter, forecast markers and comparable balance overlays. Event buckets carry independent movement/forecast dates. Account detail exposes opening/nonopening, running, native, statement, unbilled and card-alias views. Global search returns active-ledger matches for account, category, payee, transaction, recurring schedule, asset, goal and group.

## 5. Import, matching and categorization

### 5.1 Parser and preview contract

`StatementParser` exposes sniff(peek, extension) → format/confidence and parse(reader, validated configuration, cancellation, emit) → ordered row results plus safe summary. `CsvParser`, `OfxParser`, `QifParser`, `CamtParser` emit a shared `ParsedStatementRow` with source row key, raw bank date, optional explicit time zone, signed direction and magnitude, currency, raw description, optional provider ID/namespace/category, and status. Keep row ordinal and field location in safe diagnostics. Bound bytes/row length, records, XML depth, decoded text and regex work. Disable CAMT external entities and network access, handle SGML/XML OFX and QFX, and reject malformed/mismatched inputs with row or file errors. Unknown format detection tries OFX, QIF, CAMT, CSV. OFX may fall back to Latin-1 after UTF-8, filters balance summary rows and never treats synthesized IDs as external identities. QIF infers one file-wide date style, signals ambiguous dates and allows explicit strict override.

CSV supports BOM/encoding, delimiter and quoting, decimal/group separators, identifier/date/description/payee/category/notes mappings, signed column or distinct debit/credit columns, inversion, skip-header and row skip configuration. Per-bank templates serialize a version, format, structured columns and parser options, strictly reject unknown instructions and preview all changes. IDR mappings handle localized numeric separators while the UI stays English. Ambiguous separators or mixed signs are errors until resolved. Templates specify target currency or use account default, and never silently truncate sub-minor amounts.

Preview uploads use approved local-file handles. Bind staged bytes, account, hash, parser options, ledger generation and ledger revision to a single-use preview token. Parse and canonicalize in a background worker, apply normalization, detect duplicates and matching candidates, then evaluate rules on rows that would insert/merge. Store staged data in bounded memory or authenticated, keyed scratch with short TTL. Return pages of row results `ready|error|duplicate|ambiguous|pending_match`, candidate IDs, before/after diffs, counts, and actionable issue codes. Renderer never uploads a user-edited normalized row as authoritative data. At confirm, validate per-row decisions, re-evaluate revision and candidate row versions under the writer and require refreshed preview for material differences. Commit ready/explicitly approved effects in one transaction and capture import batch, provenance and before/after effect state. Cancel before the transaction commits. Losing the response after commit is handled through the operation receipt keyed by request UUID.

### 5.2 Deduplication, re-import and undo

The versioned identity hashes are SHA-256 over length-prefixed canonical UTF-8 fields. External key = provider namespace, external ID, account ID, original booking date, original native currency and amount minor, and direction/type. The fallback fingerprint = account, original date, native amount and currency, type and normalized original description. Use Unicode casefold plus NFKD and combining-mark removal, trim and collapse whitespace for description. Normalization version is part of key framing. Currency is included as an amount discriminator. Do not use reporting date, payee overrides or category in identity. External key with differing date/amount is a changed-record or pending match candidate and requires review. Identical CSV rows can represent genuine purchases. Unambiguous external identity skips/reconciles, repeated fallback hits and near-matches use candidate previews so a user may keep a distinct row. CSV duplicates toggle defaults on, disables fallback skip/candidate checks when off and preserves any hard external identity constraint. Use multiplicity and row ordinals for same-file candidates. Non-CSV dedupe stays enabled.

Keep original statement provenance separate from resolved fields. Edits mark explicit overrides including a cleared nullable field. Re-import merges only permitted fields and never replays rules over manual overrides or duplicates note append side effects. Confirmed pending match permits updating protected status/date/amount via import service and preserves the posting UUID, user annotations and dependent links. Revalidate dependent splits, transfer and allocation invariants before merge, returning an unresolved candidate if the source change requires user action.

Batch undo simulates rollback in reverse effect order and shows blocked rows for post-import edits, later imports referencing the same target, transfer/shared/recurring/asset/loan links, attachments or changed revision fingerprints. An eligible inserted row is deleted; an eligible updated row is restored from its before snapshot, including original provenance and generated/matched occurrence state. Rebuild tags and search projections. Apply the selected safe subset in one writer transaction, record resolved effects and mark import fully undone only when no effects remain. Represent the resolved effect ordinals in `import_batches.summary_json`. Re-run dependency checks under lock and refuse stale previews. FX/asset imports use the same preview/commit/undo framework with dedicated row validators.

### 5.3 Rule AST

Version 1 payload has one root `Group {op: And|Or, children: [Predicate|LeafGroup]}`. LeafGroup contains only predicates. Field enum includes description, payee, notes, signed amount, type, account ID, payee ID, bank date, status and provider category. Type-check contains/not_contains/equal/not_equal/prefix/suffix/regex on text, ordered comparisons on typed dates and numbers, and equality on identifier/enum/bool fields. Monetary operands specify currency and exact decimal; keep zero/false valid. Reject empty groups, extra depth, blank text, unsupported operators, invalid regex and regex matching the empty string. Compile Rust regex with finite source/compiled limits, apply to normalized text, document that regex syntax stays intact and literal user patterns must target the normalized form. Budget evaluation with bounded row samples.

Order enabled rules by priority then ID, evaluate against an immutable per-row input snapshot, and apply matched actions in that same order. The last eligible value wins for scalar actions; note appends are ordered and idempotent per rule version. Ignore marks a row. Category actions skip split rows and invalid or nonassignable categories. Existing categories require the overwrite switch for retroactive application. Manual override fields stay protected except for a user-selected explicit retroactive overwrite confirmed in preview. Expose would-change count, sample results and skip reasons. Rule pack import/export uses a versioned securo-categorization-rules envelope with key resolution and explicit collision policy. Keep enabled/apply-existing independent. Preview and apply share the same evaluator, and apply rechecks preview revision before writing.

## 6. Files, encryption and recovery

### 6.1 Storage and lock protection

Use Tauri `app_data_dir` subdirectories for ledgers, scheduled archives and attachment sets by default. Resolve chosen roots through native dialog handles. Portable mode uses a sibling directory named from the ledger basename, with a ledger-specific ownership marker; keep key-wrapping metadata inside its protected key sidecar or sibling bundle so the user can move both. Paths are canonical and operation-scoped. Reject path traversal, symlink/reparse-point redirection, unauthorized roots, existing nonledger input and read-only files when opening writable. Use restrictive OS permissions and prevent secrets in logs, diagnostics and frontend persistence.

For encrypted ledgers, generate an independent random SQLCipher key. Store a portable authenticated credential wrapper (salt, bounded KDF parameters and encrypted random database key) adjacent to the .db. A passphrase-derived or PIN-derived wrapping key uses Argon2id; PIN enrollment offers a stronger passphrase, as short PIN entropy remains limited for offline guessing. Sensitive key entry uses an isolated local dialog, ephemeral command argument and immediate clearing. Quick unlock stores the random key under per-ledger OS Keychain scope with user consent and may be disabled/revoked. Automatic backup credentials are independent, opt-in OS Keychain secrets; if unavailable or ledger is locked, defer scheduled work and show status on next unlock. Never silently downgrade encryption. Plain ledgers skip wrapping. SQLCipher covers persisted ledger pages and journals when enabled. Encrypt attachments in encrypted ledgers with a separate derived key and per-object random nonce, authenticating attachment ID and format version. Rekey/enable encryption uses an exclusive safety snapshot and staged transformed database plus attachment tree with a resumable publish journal. No keys in the archive payload. Threat boundary is data at rest, with OS controls for running sessions, page/swap exposure and quick unlock.

### 6.2 Attachment state machine

Allow jpg/jpeg/png/webp/gif/heic/pdf, validate signatures and type, default maximum 10 files per transaction and 10,000,000 bytes per attachment. Generate safe random storage keys, preserve safe display names and extensions, and never execute attachments inside the app.

1. Under a ledger file-operation lease, stream source bytes to a same-directory temporary file, compute size/digest, encrypt if needed, then fsync file and directory. Record a `staged` DB row under the single writer after validating owner transaction and quota.
2. Rename temp to final atomically, fsync directory and set row `ready` in a second DB commit. Only ready rows may open/save or enter a backup. Return the attachment UUID after the state is durable.
3. Delete by committing `deleting`, then remove the blob and fsync directory before deleting metadata. Keep retryable states on IO failure.
4. On open, reconcile staged rows using verified temp/final bytes. Finalize matching blobs, mark irrecoverable rows `missing` with a visible issue, or finish interrupted deletions. Sweep only files owned by this ledger without metadata, after an age threshold and when no active lease could own them. Preserve unknown files. Native open uses a controlled temporary materialization and cleanup, and save uses approved output handles.

File operation and backup share a coordination fence. The service settles or reports any unresolved attachment states before snapshotting. Transaction deletion with attachments requires a durable removal plan and does not leave a live row silently pointing to missing bytes.

### 6.3 Backup archive and restore

Take a bounded consistent snapshot holding the writer/attachment mutation fence until the database and immutable attachment inventory are protected. Readers may continue. Plain ledgers use `sqlite3_backup` from an active consistent source to a private in-memory destination. SQLCipher rejects the online backup API when a source or destination has a key. For an encrypted ledger use a keyed consistent read transaction to copy schema-controlled data to a private unkeyed in-memory bridge. Copy explicitly named base columns, preserve rowids when required, rebuild FTS, and activate triggers after bulk copy and validation (validate posting and transfer semantics before accepting the bridge). Then run the online backup API from that in-memory bridge into an unkeyed in-memory snapshot. Detach/release and zero secret buffers when feasible. A pinned SQLCipher integration fixture must prove coherent handling of generated columns, STRICT tables, deferred constraints, WAL and FTS. Fail visibly with a bounded-memory or storage error instead of spilling cleartext staging data to ordinary disk. Stream protected attachment generations after releasing the database writer fence while blob leases prevent deletion. SQLite and SQLCipher support references are listed in Section 12.

Envelope V1 consists of a fixed magic/version, KDF algorithm and parameters, 16-byte random salt, an 8-byte random nonce prefix, and authenticated frames. Derive 32 bytes via Argon2id v1.3 (default 64 MiB, 3 passes, 4 lanes with bounded accepted parameters). AES-256-GCM uses the nonce prefix concatenated with a checked 32-bit frame counter, a 16-byte tag, and AAD containing the canonical header, frame counter, payload length and final-frame flag. Use bounded frames (maximum 1 MiB) and reject nonce exhaustion, reordering, truncation or trailing frames. Retrying starts with fresh salt and prefix. Authenticated payload streams a length-framed `manifest.json`, `ledger.db` snapshot and listed attachment objects. Manifest contains schema and archive versions, ledger/source revision, creation time, counts, attachment IDs, relative controlled names, sizes and SHA-256 digests. Include profile/layout settings and local templates. Exclude absolute paths, temporary files, job state, keychain IDs/secrets and provider credentials. Backup snapshot uses portable SQLite page representation inside the encrypted stream. No ZIP encryption. Protect incomplete output with a temporary name, fsync and authenticate the completed archive before atomic publish. Rotate 10 successful generations by default and prune old generations only after publish. Schedule supports daily/weekly at a selected local time with optional age retention. Provide retryable status for low disk, locked source, credentials or interruption.

Restore preview authenticates all frames, validates version and schema allowlist, checks paths/digests and size bounds before opening any snapshot, and reports counts, issues and attachment coverage. Disable risky schema behavior and validate foreign keys/integrity. For encrypted output, materialize the authenticated SQLite data in memory and copy the validated schema/data into a fresh SQLCipher file using the controlled logical bridge in the opposite direction. For plain output, publish validated data to a fresh database and warn via the explicit target protection selection. Re-encrypt attached bytes to the chosen target policy. Default restore creates a fresh ledger instance and sibling attachment set, retaining entity UUIDs and relationships and assigning a new ledger UUID. It never merges. For in-place replacement require a target-bound confirmation and verify a new encrypted safety snapshot. Obtain its passphrase from the independently configured backup secret or the explicit replacement request. Close all handles, stage new DB/key wrapper/tree under one filesystem, and persist a fsynced restore journal outside swapped paths. Move old paths out, publish new paths, verify reopen, then commit registry state. Recovery finishes the validated replacement or rolls back from original paths using that journal. If failure occurs before publish the old ledger remains available. Old WAL sidecars must be checkpointed/closed and cannot accompany the new database.

### 6.4 Transaction CSV

Stream RFC 4180 CSV in UTF-8 for the selected filters/IDs and explicit date basis, including original/native currency and amount, direction, bank date, effective date, bill date, computed reporting date and override, status and chosen annotations. Quote and neutralize spreadsheet formula-like text fields while preserving financial cells. Export stable snapshot and row count with overwrite chosen in native save dialog. Privacy mask does not change the exported data. No arbitrary absolute renderer paths.

## 7. IPC contract

Every name below represents one Tauri v2 command registered with tauri-specta. The listed signatures define serialized payloads. All commands return a typed `Result<T, AppError>`. Requests and results use serde tagged enums, camelCase fields, and generated TypeScript bindings checked for drift. Money, signed minor units, percent/rate values, revisions and other potentially large integers that cross JSON use decimal strings. Dates are ISO strings. API timestamps are RFC 3339 UTC. Values with small bounded ranges such as day of month and page size use JSON numbers.

### 7.1 Shared DTOs and concurrency

- `Scope {sessionId, ledgerInstanceId}` is mandatory for each session-scoped call. `Mutation {scope, requestId, expectedRevision?}` wraps writes. Edit commands carry `VersionedId {id, version}` where declared. Use the entity row version when persisted and the ledger revision for all other resources. `PageReq {cursor?, limit}` is bounded to 200. `Page<T> {rows, nextCursor?, revision}` cursors include filter hash and session generation. Stale cursors return actionable refresh.
- `Money {currency, minor}`. `Conversion {native, displayed?, estimate?, status: exact|prior|missing, legs[]}`. `DateRange {from?, to?}` uses inclusive bounds. `DateBasis = bank|reporting|effective`, with calendar restricted to bank/reporting. `AppError {code, messageKey, fields?, retryable, correlationId}` covers `Validation, Locked, StaleSession, StaleRevision, Conflict, Duplicate, UnsupportedFormat, BadCredentialOrCorrupt, Busy, Io, Integrity, InsufficientSpace, Cancelled, UnsupportedSchema, Offline`. Logs contain correlation IDs and redacted codes.
- `PathGrant {id, purpose, displayName, expiresAt}` is created through native dialogs, bound to window and purpose, and revalidated in Rust when consumed. `ConfirmToken` is expiring and binds exact action, target, preview and revision. `Write<T> {value, revision, affected[]}` invalidates corresponding frontend reads. `JobTicket<T> {id, kind, state}` identifies a job whose terminal result type is T.
- `LedgerFilter` includes account, alias, category, payee and group ID lists, statement ID or unbilled filter, text, uncategorized toggle, tags (any), native/display amount bounds with currency, entry kind, status, direction, opening visibility, transfer/pnl/ignored flags, flag due state, date basis/range and allowlisted sort. Statement ID takes precedence over date range. `TxDraft` includes account, bank date, Money and direction, status, description/notes, payee/category or category splits, optional bill and reporting overrides, flags, alias, statement and installment data. `TxDetail` includes immutable source metadata, editable fields, computed dates, linked transfer/recurring/share info, and attachment summaries.
- Callback/file inputs are opaque handles. Mutations are idempotent per ledger through operation receipts that retain request hash and a redacted result reference. Preview submissions identify staged decisions through opaque IDs. Foreground file/CPU blocking work is off the WebView and Tauri dispatch thread. Commands may run in an exclusive maintenance actor when specified.

### 7.2 Command register

DTO names with `Draft` contain the writable fields of the corresponding schema entity and exclude generated IDs/revisions. `Patch` uses explicit keep/set/clear semantics. `Detail` returns identity, version, configured fields, and the derived aggregates described in Section 4. Grouped names in a cell each represent a separately registered command and use their own resource-typed fields.

| Commands | Request | Response |
|---|---|---|
| `app_bootstrap`, `ledger_list` | Empty | `Bootstrap {session?, prefs, schemaRange}`, `LedgerDescriptor[]` respectively |
| `file_choose` | `{purpose, multiple?}` | `PathGrant[]` |
| `ledger_create` | `{name, destination?, baseCurrency, timezone?, protection, credential?, rememberKey}` | `LedgerSession` |
| `ledger_open`, `ledger_switch` | `{ledgerInstanceId or openGrant, requestId}` | `OpenResult {state, session?, unlockChallenge?}` |
| `ledger_unlock` | `{unlockChallenge, credential?}` | `LedgerSession` |
| `ledger_lock`, `ledger_close` | `Scope` | `LifecycleState` |
| `ledger_integrity_status` | `Scope` | Pending or verified boolean, or an `Integrity` error |
| `ledger_settings_get`, `ledger_settings_set` | `Scope` or `Mutation + {patch: LedgerSettingsPatch}` | `LedgerSettings` or `Write<LedgerSettings>` |
| `ledger_change_protection`, `ledger_keychain_set` | `Mutation + {protection, oldCredential?, newCredential?}` or `Mutation + {enabled, credential?}` | `JobTicket<LedgerSession>` or `Write<ProtectionStatus>` |
| `app_preferences_get`, `app_preferences_set` | Empty or `{patch: AppPreferencesPatch}` | `AppPreferences` |
| `layout_get`, `layout_set` | `Scope + {view}` or `Mutation + {view, layout: LayoutSpec}` | `LayoutSpec` or `Write<LayoutSpec>` |
| `accounts_list`, `account_get` | `Scope + {includeArchived?, asOf?}` or `Scope + {id, asOf?}` | `AccountSummary[]` or `AccountDetail` |
| `account_save`, `account_archive` | `Mutation + {target?: VersionedId, draft: AccountDraft}` or `Mutation + {target, archivedOn?}` | `Write<AccountDetail>` |
| `account_set_opening`, `account_adjust_balance` | `Mutation + {accountId, signedMoney, date, reason?}` | `Write<AccountDetail>` |
| `card_lines_list`, `card_line_save`, `card_line_delete` | `Scope`, `Mutation + {target?, draft: CardLineDraft}`, or `Mutation + {id}` | `CardLineDetail[]`, `Write<CardLineDetail>`, or `Write<Deleted>` |
| `card_statements_list`, `card_statement_save`, `card_statement_delete` | `Scope + {accountId, range}`, `Mutation + {target?, draft: StatementDraft}`, or `Mutation + {id}` | `StatementSummary[]`, `Write<StatementSummary>`, or `Write<Deleted>` |
| `transactions_list`, `transaction_get` | `Scope + {filter, page, runningBalance?}` or `Scope + {id}` | `Page<TxRow>` or `TxDetail` |
| `transaction_save`, `transaction_post` | `Mutation + {target?, draft: TxDraft}` or `Mutation + {target, bookingDate?}` | `Write<TxDetail>` |
| `transaction_delete`, `transactions_bulk_preview`, `transactions_bulk_apply` | `Mutation + {target, confirm}`, `Scope + {selection: IdsOrFilter, action: BulkAction}`, or `Mutation + {previewToken, confirm?}` | `Write<Deleted>`, `MutationPreview`, or `Write<BulkResult>` |
| `installment_preview`, `installment_apply` | `Scope + {series?, draft, scope: this\|future\|all}` or `Mutation + {previewToken}` | `InstallmentPreview` or `Write<InstallmentDetail>` |
| `transfer_candidates`, `transfer_save`, `transfer_unlink`, `transfer_delete` | `Scope + {anchorId, criteria, page}`, `Mutation + {linkId?, source: ExistingOrDraft, destination: ExistingOrDraft, rate?}`, `Mutation + {linkId}`, or `Mutation + {linkId, disposition, confirm}` | `Page<TransferCandidate>`, `Write<TransferDetail>`, `Write<TxPair>`, or `Write<Deleted>` |
| `category_groups_list`, `categories_list`, `payees_list` | `Scope + {query?, includeArchived?, page}` | Corresponding `Page<CategoryGroup\|Category\|Payee>` |
| `category_group_save`, `category_save`, `payee_save` | `Mutation + {target?, draft: CategoryGroupDraft/CategoryDraft/PayeeDraft}` | Corresponding typed `Write<Detail>` |
| `category_group_delete`, `category_delete`, `payee_delete`, `payee_merge` | `Mutation + {id, replacementId?, confirm?}`; merge takes `{fromIds, intoId, confirm}` | `Write<ReferenceChange>` |
| `import_templates_list`, `import_template_save`, `import_template_delete`, `import_template_file` | `Scope + {format?}`, `Mutation + {target?, draft: ImportTemplateDraft}`, `Mutation + {id}`, or `Mutation + {direction, grant, templateId?, format}` | `ImportTemplate[]`, `Write<ImportTemplate>`, `Write<Deleted>`, or `TemplateFileResult` |
| `import_preview_start`, `import_preview_page`, `import_commit_start` | `Scope + {fileGrant, kind, accountId?, format?, templateId?, parserOptions?, detectDuplicates?}`, `Scope + {previewToken, page, status?}`, or `Mutation + {previewToken, rowDecisions[]}` | `JobTicket<ImportPreview>`, `Page<ImportPreviewRow>`, or `JobTicket<Write<ImportBatchDetail>>` |
| `import_batches_list`, `import_batch_get`, `import_undo_preview`, `import_undo_apply` | `Scope + {page, kind?}`, `Scope + {batchId}`, `Scope + {batchId, selection?}`, or `Mutation + {previewToken, selectedOrdinals}` | `Page<ImportBatchSummary>`, `ImportBatchDetail`, `UndoPreview`, or `Write<UndoResult>` |
| `rules_list`, `rule_get`, `rule_save`, `rule_delete`, `rules_reorder` | `Scope`, `Scope + {id}`, `Mutation + {target?, draft: RuleDraft}`, `Mutation + {id}`, or `Mutation + {orderedIds}` | `RuleSummary[]`, `RuleDetail`, `Write<RuleDetail>`, `Write<Deleted>`, or `Write<RuleSummary[]>` |
| `rule_preview_start`, `rule_apply_start`, `rule_pack_transfer` | `Scope + {draftOrIds, filter, sampleLimit}`, `Mutation + {previewToken, overridePolicy}`, or `Mutation + {direction, grant, ruleIds?, collisionPolicy?}` | `JobTicket<RulePreview>`, `JobTicket<Write<RuleApplyResult>>`, or `RulePackResult` |
| `recurring_list`, `recurring_get`, `recurring_save`, `recurring_archive` | `Scope + {range?, page}`, `Scope + {id}`, `Mutation + {target?, draft: RecurringDraft}`, or `Mutation + {id}` | `Page<RecurringSummary>`, `RecurringDetail`, or `Write<RecurringDetail>` for either write |
| `recurring_occurrences`, `recurring_occurrence_apply` | `Scope + {ruleId?, range, page}` or `Mutation + {ruleId, nominalDate, action: generate\|skip\|match\|unmatch\|post, transactionId?}` | `Page<Occurrence>` or `Write<Occurrence>` |
| `budgets_get`, `budgets_save`, `budgets_delete`, `budgets_copy`, `budget_suggestions` | `Scope + {month}`, `Mutation + {entries[], overwrite}`, `Mutation + {keys[]}`, `Mutation + {fromMonth,toMonth,adjustPercent,overwrite}`, or `Scope + {months, categoryIds?}` | `BudgetView`, `Write<BudgetView>` for three writes, or `BudgetSuggestion[]` |
| `loan_get`, `loan_save`, `loan_schedule`, `loan_payment_allocate` | `Scope + {accountId}`, `Mutation + {target?, terms: LoanTerms}`, `Scope + {accountId, asOf, page}`, or `Mutation + {transactionId, accountId, principal, interest}` | `LoanDetail`, `Write<LoanDetail>`, `LoanSchedulePage`, or `Write<LoanPaymentDetail>` |
| `goals_list`, `goal_get`, `goal_save`, `goal_delete` | `Scope + {state?,asOf?,page}`, `Scope + {id,asOf?}`, `Mutation + {target?, draft: GoalDraft}`, or `Mutation + {id,confirm?}` | `Page<GoalSummary>`, `GoalDetail`, `Write<GoalDetail>`, or `Write<Deleted>` |
| `assets_list`, `asset_get`, `asset_save`, `asset_archive`, `asset_delete` | `Scope + {groupId?,asOf?,page}`, `Scope + {id,asOf?}`, `Mutation + {target?, draft: AssetDraft}`, `Mutation + {id,archivedOn?}`, or `Mutation + {id,confirm}` | `Page<AssetSummary>`, `AssetDetail`, `Write<AssetDetail>` for save/archive, or `Write<Deleted>` |
| `asset_groups_list`, `asset_group_get`, `asset_group_save`, `asset_group_delete` | `Scope + {page}`, `Scope + {id,asOf?}`, `Mutation + {target?, draft: AssetGroupDraft, assetIds[]}`, or `Mutation + {id,confirm?}` | `Page<AssetGroupSummary>`, `AssetGroupDetail`, `Write<AssetGroupDetail>`, or `Write<Deleted>` |
| `asset_history`, `asset_valuation_save`, `asset_valuation_delete`, `asset_trade_save`, `asset_trade_delete` | `Scope + {assetId,range,page,view}`, `Mutation + {target?,draft: ValuationDraft}`, `Mutation + {id,confirm?}`, `Mutation + {target?,draft: AssetTradeDraft}`, or `Mutation + {id,confirm}` | `Page<AssetHistoryItem>` or `Write<AssetDetail>` |
| `groups_list`, `group_get`, `group_save`, `group_archive`, `group_member_save`, `group_member_remove` | `Scope + {page}`, `Scope + {id,asOf?}`, `Mutation + {target?,draft: GroupDraft}`, `Mutation + {id,archived}`, `Mutation + {groupId,target?,draft: MemberDraft}`, or `Mutation + {groupId,memberId}` | `Page<GroupSummary>`, `GroupDetail`, or `Write<GroupDetail>` |
| `group_expense_save`, `group_expense_remove`, `group_settlement_save`, `group_settlement_remove` | `Mutation + {target?,draft: SharedExpenseDraft}`, `Mutation + {id,confirm}`, `Mutation + {target?,draft: SettlementDraft}`, or `Mutation + {id,confirm}` | `Write<GroupDetail>` |
| `dashboard_get`, `report_get`, `calendar_get`, `search_global` | `Scope + {asOf?,accountIds?,widgets}`, `Scope + {kind,filter,granularity,currency}`, `Scope + {range,basis,accountIds?,includeProjection}`, or `Scope + {query,types?,page}` | `DashboardView`, `ReportView`, `CalendarView`, or `Page<SearchHit>` |
| `currencies_list`, `fx_rates_list`, `fx_rate_save`, `fx_rate_delete`, `fx_refresh_start`, `fx_provider_configure` | `Scope`, `Scope + {pair?,range?,page}`, `Mutation + {target?,draft: FxRateDraft}`, `Mutation + {id}`, `Mutation + {provider,pairs[],date}`, or `Mutation + {provider,enabled,credential?}` | `Currency[]`, `Page<FxRate>`, `Write<FxRate>`, `Write<Deleted>`, `JobTicket<Write<FxRefreshResult>>`, or `ProviderStatus` |
| `attachments_list`, `attachment_add_start`, `attachment_rename`, `attachment_open`, `attachment_save_as`, `attachment_delete` | `Scope + {transactionId}`, `Mutation + {transactionId,grants[]}`, `Mutation + {id,displayName}`, `Scope + {id}`, `Scope + {id,saveGrant}`, or `Mutation + {id,confirm?}` | `AttachmentSummary[]`, `JobTicket<Write<AttachmentWriteResult>>`, `Write<AttachmentSummary>`, `AttachmentOpenResult`, `FileResult`, or `Write<Deleted>` |
| `backup_list`, `backup_schedule_get`, `backup_schedule_set`, `backup_create_start`, `backup_remove` | `Scope + {page}`, `Scope`, `Mutation + {schedule,rememberSecret?,backupPassphrase?}`, `Scope + {requestId,destination?,backupPassphrase}`, or `Mutation + {id,confirm}` | `Page<BackupSummary>`, `BackupSchedule`, `Write<BackupSchedule>`, `JobTicket<BackupSummary>`, or `Write<Deleted>` |
| `restore_preview_start`, `restore_confirm_replacement`, `restore_apply_start` | `{fileGrant or scopedBackupId, backupPassphrase}`, `Scope + {previewToken,targetId}`, or `{requestId,previewToken,target: NewOrReplace,confirm?,protection,outputCredential?,safetyBackupPassphrase?}` | `JobTicket<RestorePreview>`, `ConfirmToken`, or `JobTicket<RestoreResult>` |
| `transactions_export_start` | `Scope + {selection,filter,columns,saveGrant}` | `JobTicket<FileResult>` |
| `job_status`, `job_cancel` | `{id,scope?}` bound to originating window | `JobSnapshot` or `CancelResult` |

No public HTTP listener. Tauri capabilities allow only declared commands to the main local app WebView, prohibit remote origin access and delegate platform dialogs to scoped wrappers. Use a restrictive CSP, local assets and allowlisted optional FX endpoints in Rust. Unknown DTO fields, mismatched resources, stale scope and unauthorized grants fail closed. Distinguish destructive UI actions with linked dependency summaries. Commands that edit protected provider fields accept changes only through explicit reconciler types.

### 7.3 Background events

Emit the window-targeted `ledger-integrity-alert` with the active scope and a sanitized integrity error when deferred validation fails. Clients can recover the outcome through `ledger_integrity_status`.

Emit window-targeted `ledger-job-progress` events with `{jobId, sessionId?, seq, kind, phase, done?, total?, state, cancellable}`. Applicable kinds cover import preview/commit, undo if long-running, rules, attachments, CSV export, backup/restore, FX and long read work. Stream checkpoints are coalesced. Phases include queued, parsing, validating, applying, snapshotting, encrypting, verifying and publishing. Terminal events carry only status and safe error codes. Clients recover typed results through `job_status` after dropped events and ignore stale generation or sequence. Use a typed Channel for bounded high-frequency data if a later measured path requires it. No credentials, financial rows or file paths in broadcasts. Cancellation is cooperative at safe boundaries. Commit/publish is noncancellable once started and reports the durable outcome.

## 8. SvelteKit SPA

Use adapter-static with an SPA fallback, SSR disabled in the root layout, bundled assets and Svelte 5 Runes. Tailwind tokens carry theme, spacing and contrast. Keep financial computation in Rust. Format minor-unit strings without converting to lossy JavaScript numbers. Chart coordinates may be approximated for drawing while labels, tooltips and exports use the exact result. Keyboard interactions and virtualized list focus remain accessible.

| Route | Views and state |
|---|---|
| `/`, `/unlock` | Ledger launcher, create/open, keychain/manual unlock, recovery. |
| `/dashboard`, `/reports/[kind]`, `/calendar` | Widgets, layouts, filters, series and drill-down. |
| `/accounts`, `/accounts/[id]` | Balances, account configuration, card lines/statements and loan schedules. |
| `/transactions`, `/transactions/[id]` | Virtualized data grid, grouped filters, cursor pages, detail/edit, splits, attachments, installments, link/undo. |
| `/import`, `/import/history`, `/rules` | File preview/errors/mapping, candidate resolution, batch undo, template and rule packs. |
| `/categories`, `/payees`, `/budgets`, `/recurring`, `/goals` | Categorization, setup, schedules and planning. |
| `/assets`, `/assets/[id]`, `/assets/groups/[id]`, `/groups`, `/groups/[id]` | Holdings, growth, groups and settlements. |
| `/settings`, `/settings/currencies`, `/settings/backups` | Preferences, storage/security, currencies/FX, backup and restore. |

Use `$state` for session, dialog, selection and job state and `$derived` for presentation. Cache read pages by ledger generation, normalized filter hash and revision. Store filter/sort state in route parameters without putting credentials or sensitive transaction search text into persistent URLs. Virtualize rows by stable ID with bounded prefetch and overscan. Cancel obsolete page reads and reset stale cursors. Preserve keyboard selection and focus as the virtual window moves, expose counts, and provide screen reader labels. Use transactional optimistic updates only where conflict rollback is clear. Invalidate affected contexts from Rust receipts and background job completion.

The privacy-mask store is a presentation preference applied at every visual monetary render, chart scale, tooltip, aria label and notification. Amounts stay within in-memory scoped data and Rust IPC. Clear all ledger caches and preview/job state on lock, switch or failed reauthorization. The app shell owns the ledger selector and keyboard-aware Ctrl/Cmd+K command palette. Persist window/theme preferences locally and scope report/layout preferences to the ledger. English text uses stable message keys.

## 9. Performance and failure budgets

Benchmark clean packaged release builds on a defined mid-range reference machine, initially a four-core or better laptop-class CPU, 16 GiB RAM and local NVMe SSD. Record machine, operating system, SQLite/SQLCipher settings and dataset seed. Fixture includes 250,000 transactions across 50 accounts, skewed active accounts, card cycles, 0/2/3-decimal currencies, categories/splits, tags, pending rows and transfer pairs. Measure cold launch from process start to interactive first screen and post-credential time to interactive dashboard for protected ledgers. Exclude time spent waiting for user input while including database open, validation and the first bounded reads. Require <3 seconds cold start and p95 <500 ms for common pages over at least 100 observations per filter class (account + date, reporting category, status/date, statement and indexed text or tag). Measure UI dispatch through result delivery and report cache state. Measure SQL plans, payload size and cursor stability during background imports/checkpoints. Keep migration/recovery work visible instead of blocking without status. Profile SQLite scan/seed paths before adding persisted balance checkpoints. Any cache must be rebuildable and scoped to ledger revision.

Use a failure-injection harness for interrupted boot, busy lock, disk full, corruption, wrong keys, journal replay, interrupted import, graph dependency failure and restore publish. On every durable write verify atomicity and recovery of post-commit events. Audit both unencrypted and SQLCipher paths using real target builds. Verify data and permissions remain scoped under concurrent switch, lock and background operations.

## 10. V2 migration boundaries

Keep V1 optional providers at their narrow ports. External bank adapters may later feed canonical source rows and add connection tables. Executable plugins would require a new trust boundary over those same data contracts. Live market prices can extend asset valuation provenance and implement `ValuationSource`. Invoice documents/schedules/catalog/tax IDs use new bounded modules referencing stable payee, currency and allocation IDs. AI agents/MCP may later use typed ledger read/write ports behind a reviewed policy boundary. Versioned migrations, fixtures from V1 and archive compatibility checks are required for each extension. Core ledgers remain standalone files.

## 11. Requirements verification map

IDs listed below omit the FR prefix. Deferred IDs 045–046, 069, 086–091 and 094–096, and permanent exclusions under 007, 049, 085 and 097 are also covered through availability and negative verification.

| FR | UI flow | Verification |
|---|---|---|
| 001–007 | Launch, preferences, ledger selector and unlock | Offline create/open; independent files; time-zone midnight; incorrect key; strict consent; feature toggle retains rows; no tenant onboarding. |
| 008–015 | Accounts, cards, adjustments | Native account types and archive; opening once; signs/order; pending debt vs ordinary pending; shared limit once; future excluded; ignore vs P&L-only flag. |
| 016–023 | Edit transaction, reporting mode, split editor | Manual CRUD; provider edits blocked; reporting precedence and calendar movement; valid split totals and immutable matching/bank identity. |
| 024–032 | Ledger list, selection/export and installments | Exact tag tokens, sort/filter matrix and stable cursor; split rows skipped in bulk categorize; CSV dual dates; P&L and transfer exclusions; invalid/edited series; UI-only masking. |
| 033–036 | Transfer composer and match | Same-/cross-currency amounts, provenance, linking existing rows, unlink P&L change and duplicate pair prevention. |
| 037–043 | Import, template, preview, history | Four parsers with QFX alias; repeatable bank template; per-row errors; no preview writes; CSV toggle; provider key and ambiguous review; override/re-import/undo. |
| 044–049 | Import settings and pending reconciliation | Offline file import with no provider credentials; JSON/TOML only; confirmed pending match once; no sync/worker/extension runtime. |
| 050–054 | Rule editor, pack transfer and batch actions | One nested level, normalized text, invalid regex, zero/false inputs, overwrite controls and side-effect-free preview. |
| 055–062 | Budgets and schedules | Overrides/defaults; same reporting date/P&L; positive remaining indicator; copy rollback; clamping, leap dates, weekend shifts, generation idempotency and edit floor. |
| 063–069 | Loan detail, goals and asset history | Price/SAC and exact final payoff; cap and overdue boundary; goal edge targets; growth cadence and split basis; no live quote calls. |
| 070–077 | Dashboard, reports, calendar and FX | Shared inclusion and drill-down totals; layouts/scopes/date basis; native amounts; dated FX imports and explicit refresh; badged fallback stays out of real rates. |
| 078–085 | Transaction attachments and backup/restore | File size and per-owner caps; portable roots; staged write recovery; encrypted WAL snapshot; retention, wrong password, preview counts and safe new/replace restore. |
| 086–091 | Navigation and command registration | AI/LLM/MCP routes, capabilities and runtime absent. |
| 092–097 | Shared group and settlement | Exact/percentage/equal shares, currency balances, manual settlements, no linked users or invoice routes/public sharing. |
| 098–100 | Search, command palette, layout and privacy | Active file scoping; standard shortcuts; layout persistence and lock/switch cache clearing. |

NFR release suites cover decimal golden vectors (001–002), offline/network allowlist (003, 008, 019), SQLite recovery/commit failure (004–006, 014–015), threat/secret scope testing (007, 009, 017, 020), workload and concurrency (010–012), clean-install restore (013), keyboard/visual scaling (016), and pure Rust domain vectors with contract binding checks (018).

## 12. Technical references

- SQLite WAL, online backup and deferred FK semantics. [WAL](https://sqlite.org/wal.html) · [Backup API](https://sqlite.org/backup.html) · [Foreign keys](https://sqlite.org/foreignkeys.html) · [STRICT tables](https://sqlite.org/stricttables.html)
- SQLCipher key handling and encrypted-backup limitation. [SQLCipher API](https://www.zetetic.net/sqlcipher/sqlcipher-api/index.html) · [SQLCipher backup implementation](https://github.com/sqlcipher/sqlcipher/blob/master/src/backup.c)
- Desktop renderer and IPC. [Tauri frontend](https://v2.tauri.app/start/frontend/) · [Tauri SvelteKit](https://v2.tauri.app/start/frontend/sveltekit/) · [Tauri events and channels](https://v2.tauri.app/develop/calling-frontend/) · [Tauri capabilities](https://v2.tauri.app/security/capabilities/)
- Archive cryptographic primitives. [RFC 9106 Argon2](https://www.rfc-editor.org/rfc/rfc9106.html) · [NIST SP 800-38D GCM](https://csrc.nist.gov/pubs/sp/800/38/d/final)
