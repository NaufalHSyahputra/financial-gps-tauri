-- The adapters disable foreign keys before the transaction. The migration runner
-- checks every foreign key before committing and restores connection enforcement.
CREATE TABLE accounts_upgrade (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  institution TEXT,
  kind TEXT NOT NULL CHECK(kind IN ('checking','savings','credit_card','loan','cash')),
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
INSERT INTO accounts_upgrade(rowid,id,name,institution,kind,currency_code,archived_on,payment_method,close_day,due_day,card_limit_minor,credit_line_id,accounting_override)
SELECT rowid,id,name,institution,kind,currency_code,archived_on,payment_method,close_day,due_day,card_limit_minor,credit_line_id,accounting_override FROM accounts;
DROP TABLE accounts;
ALTER TABLE accounts_upgrade RENAME TO accounts;
CREATE INDEX accounts_line ON accounts(credit_line_id);
