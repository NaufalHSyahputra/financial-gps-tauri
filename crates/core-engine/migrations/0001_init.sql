CREATE TABLE currencies (
  code TEXT PRIMARY KEY CHECK(length(code)=3 AND code GLOB '[A-Z][A-Z][A-Z]'),
  scale INTEGER NOT NULL CHECK(scale IN (0,2,3)),
  name TEXT NOT NULL
) STRICT;
CREATE TRIGGER currency_scale_immutable BEFORE UPDATE OF scale ON currencies
WHEN NEW.scale <> OLD.scale BEGIN
  SELECT RAISE(ABORT,'immutable currency scale');
END;

INSERT INTO currencies (code, scale, name) VALUES
  ('IDR',0,'Indonesian Rupiah'),
  ('JPY',0,'Japanese Yen'),
  ('KRW',0,'South Korean Won'),
  ('USD',2,'US Dollar'),
  ('CNY',2,'Chinese Yuan'),
  ('EUR',2,'Euro'),
  ('SGD',2,'Singapore Dollar'),
  ('GBP',2,'Pound Sterling'),
  ('AUD',2,'Australian Dollar'),
  ('KWD',3,'Kuwaiti Dinar'),
  ('BHD',3,'Bahraini Dinar'),
  ('OMR',3,'Omani Rial');

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

-- Derived, rebuildable search cache. Join by the durable transaction UUID.
CREATE VIRTUAL TABLE transaction_search USING fts5(
  transaction_id UNINDEXED, description, notes, payee, tokenize='unicode61 remove_diacritics 2', prefix='2 3'
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
