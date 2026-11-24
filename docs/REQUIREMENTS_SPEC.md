# Securo Local-First Desktop Requirements Specification

## 1. Purpose and target

This Software Requirements Specification translates the source-confirmed Securo behavior documented in FEATURE_INVENTORY.md into a personal, local-first desktop product using Rust, Tauri v2, SQLite, and Svelte 5. The reference is a multi-user web SaaS; this target is a desktop ledger that must work without a network connection. **Status: Frozen V1 baseline.** The Product Owner approved all 24 decisions in Section 6.

“Shall” statements define V1 acceptance unless marked [DEFERRED-V2] or [DROP-SAAS]. IDs SEC-01 through SEC-17 refer to the feature inventory.

## 2. Scope triage matrix

| Feature ID | Tag | Porting rationale |
|---|---|---|
| SEC-01 Workspaces/profiles/access | [ADAPT-LOCAL] | Keep one-user profile, preferences, time-zone and feature gates. Replace SaaS membership/admin/OIDC assumptions with a optional SQLCipher unlock. One switchable SQLite file per ledger. |
| SEC-02 Accounts/balances | [KEEP] | Core account and exact running-balance capability, adapted to SQLite/local persistence. |
| SEC-03 Transactions/dual dates | [KEEP] | Core ledger, classification, filtering, reporting date and override behavior. |
| SEC-04 Internal transfers | [KEEP] | Core linked double-entry transfer behavior, including exact cross-currency leg amounts. |
| SEC-05 Imports/dedup | [KEEP] | Core offline statement import, mapping, preview, dedup and undo. Preserve manual overrides on re-import. |
| SEC-06 Bank sync/plugins | [ADAPT-LOCAL] / [DEFERRED-V2] | Keep declarative JSON/TOML file mappings. Defer external sync and executable extensions. |
| SEC-07 Categorization rules | [KEEP] | Core deterministic rule engine and preview/retroactive workflow. |
| SEC-08 Budgets/recurring | [KEEP] | Core planning, projections and schedules. Replace Celery/Redis with an in-process scheduler; do not require it for calculation. |
| SEC-09 Loans/goals/assets | [KEEP] | Core local tracking and calculations. Live quote feeds are [DEFERRED-V2]. |
| SEC-10 Reports/calendar/analytics | [KEEP] | Core financial analysis; ensure offline calculations and date-basis distinction. |
| SEC-11 FX | [ADAPT-LOCAL] | Preserve native/base values and history; provider fetch is optional. Support manual/imported rates and visible offline/missing-rate behavior. |
| SEC-12 Attachments/backup | [ADAPT-LOCAL] | Replace S3/server storage with app_data_dir or user-selected folder; preserve portable backup/restore with authenticated encryption by default. |
| SEC-13 AI/MCP | [DEFERRED-V2] | Defer AI agents, local/remote LLM chat and MCP. |
| SEC-14 Shared groups | [ADAPT-LOCAL] | Keep single-user tracking of counterparties/projects and settlement history; simplify membership/auth visibility that assumes multiple Securo users. |
| SEC-15 Invoices/schedules/products | [DEFERRED-V2] / [DROP-SAAS] | Defer invoices, recurring invoices, products and tax IDs. Permanently exclude vendor-hosted public URLs. |
| SEC-16 Search/navigation | [KEEP] | Local global and in-module search, shortcuts and privacy masking remain useful. |
| SEC-17 Operational configuration | [ADAPT-LOCAL] | Replace server environment/deployment configuration with desktop preferences, secure secret storage, optional integration settings, and local task scheduling. |

[DROP-SAAS] permanently excludes multi-user tenants, roles and invitations, OIDC SSO, WebAuthn passkeys, TOTP 2FA, API access tokens, S3 cloud storage, Redis/Celery workers and vendor-hosted public share URLs. V2 additions use additive migrations and stable Rust domain interfaces for ingestion, lookup and ledger commands. The V1 schema, IPC and UI contain no deferred modules.

## 3. Functional requirements

### 3.1 Profile, security and local data (SEC-01, SEC-17)

**FR-001** The application shall open and permit core ledger operations without network access after installation.

**FR-002** The application shall maintain local profile preferences for date and number formats, display currency, theme, privacy mask and feature visibility. V1 UI language shall be English.

**FR-003** The ledger shall default to the OS local time zone and support an IANA time-zone override. Financial dates shall use local calendar dates and audit timestamps shall use UTC.

**FR-004** The application shall offer optional per-ledger SQLCipher encryption with a PIN or passphrase. Users may enter their credential at launch or explicitly enable OS Keychain quick unlock. Secrets shall never be stored in plaintext.

**FR-005** The application shall keep optional modules disabled or unavailable until explicitly enabled, and disabling a module shall not delete its stored records.

**FR-006** The application shall not transmit ledger data, diagnostics, or telemetry unless the user explicitly enables a named integration that requires transmission.

**FR-007 [ADAPT-LOCAL]** The desktop product shall use one primary user per ledger. Users shall create, open and switch independent SQLite .db files in-app. SaaS workspace membership and authentication are [DROP-SAAS].

### 3.2 Accounts and balance engine (SEC-02)

**FR-008** The user shall create, edit, archive and inspect checking, savings, credit_card and loan accounts in native currency. Immediate-debit cards, QRIS and e-wallets use checking accounts with optional payment-method metadata. Credit Cards and PayLater/BNPL use credit_card accounts.

**FR-009** The user shall be able to set an opening amount and effective date; the application shall include that opening amount exactly once in account balance calculations.

**FR-010** The application shall calculate a running account balance from opening balance and eligible ledger movements using exact decimal arithmetic.

**FR-011** The application shall distinguish posted, pending, and future transactions when calculating current balances; pending credit-card authorizations shall affect current card debt according to the reference rule.

**FR-012** The user shall configure credit-card and PayLater close day, due day, limit and optional shared credit-line grouping. Pending authorizations shall count toward current debt and available credit. Shared credit availability shall count each card movement once.

**FR-013** The application shall exclude future scheduled activity from current balance and shall present it separately as projected activity.

**FR-014** The user shall be able to record a balance adjustment with date, amount, and reason; the adjustment shall be represented in ledger history and recalculate the balance.

**FR-015** The application shall distinguish transactions ignored from transactions merely excluded from P&L; ignored transactions shall not affect balances or reports, while P&L-excluded transactions shall retain their balance effect.

### 3.3 Transactions and dual-date ledger (SEC-03)

**FR-016** The user shall be able to create, inspect, edit, and delete manual transactions with account, date, positive amount, debit/credit type, description, payee, category, notes, status, and currency.

**FR-017** The application shall protect imported provider booking dates and amounts from ordinary edits while supporting user metadata overrides.

**FR-018** The application shall store bank date, effective date, effective bill date and reporting-date override as distinct values.

**FR-019** The application shall derive reporting date in this order: explicit reporting-date override; explicit effective bill date; effective date in accrual mode; otherwise bank date.

**FR-020** The application shall use a ledger cash/accrual default with a per-card override. Computed reporting date shall drive report, dashboard and budget attribution. Account balances shall follow bank booking dates. Cash-flow forecasts may use effective dates independently.

**FR-021** The application shall allow a reporting-date override to move a transaction between report periods without changing its bank date, account balance, sync identity, or matching identity.

**FR-022** The user shall be able to split a transaction across categories; split parts shall be positive, at least two, sum exactly to parent amount, and retain currency precision.

**FR-023** The application shall keep category splits distinct from people/group shares and shall not assign a single category to a split parent.

**FR-024** The user shall be able to add notes and hashtag tags; tag addition/removal shall operate on exact tokens and preserve unrelated note text.

**FR-025** The user shall be able to filter transactions by account, linked card, category, payee, date range, bill, group, unbilled/statement view, text, uncategorized, type, status, tag, amount, flagged/due, transfer status, P&L eligibility, and ignored status.

**FR-026** The user shall be able to sort and paginate transaction results by date, amount, description, payee, category, account, type, status, and flag due date.

**FR-027** The user shall be able to bulk categorize eligible non-split transactions, add/remove tags, flag, add to group, and delete, with a preview or confirmation appropriate to destructive actions.

**FR-028** The user shall export selected transactions to CSV including native amounts, bank date, computed reporting date and reporting-date override when present. Privacy masking affects UI display only.

**FR-029** The application shall exclude transfer rows and transfer-classified categories from income/expense totals.

**FR-030** The application shall calculate P&L only from posted, nonfuture, nonignored transactions not excluded from P&L.

**FR-031** The application shall reject incomplete or invalid installment metadata and support editing a series at the selected this/future/all scope when series identity is available.

**FR-032** The application shall make privacy masking affect displayed monetary values only, never persisted values or calculations.

### 3.4 Internal transfers (SEC-04)

**FR-033** The user shall be able to create a transfer between two accounts as linked debit and credit ledger legs.

**FR-034** The application shall update each account’s balance from its transfer leg and exclude both legs from income/expense reports.

**FR-035** The user shall be able to provide different native amounts/currencies for cross-currency transfer legs and inspect the applied rate and provenance.

**FR-036** The application shall allow transfer legs to be linked, unlinked, or matched to imported rows without creating a second movement.

### 3.5 Imports and deduplication (SEC-05)

**FR-037** The user shall preview and import CSV and OFX/QFX as primary workflows, with QIF and CAMT XML also supported.

**FR-038** The import preview shall not mutate the ledger and shall show parsed fields, row-level errors, detected duplicates, and expected import counts before confirmation.

**FR-039** The user shall save reusable per-bank templates with column mappings, date and number formats, separate inflow/outflow columns and amount inversion. Templates shall accommodate Indonesian and international bank exports.

**FR-040** The application shall match provider/external ID plus account, bank date, exact amount and type when present, otherwise a deterministic fingerprint of account, bank date, amount, type and normalized description. CSV deduplication shall default on and be toggleable. Likely duplicates with ambiguous identity shall require explicit resolution in preview.

**FR-041** The application shall preserve manual category, payee, description, note, reporting date and other supported overrides when re-importing a matching transaction.

**FR-042** The user shall be able to inspect import history and undo an import; the application shall report rows that cannot safely be undone because of later dependencies.

**FR-043** The application shall support row-specific parser errors and shall not silently discard an unparseable row.

### 3.6 File-import extensibility and deferred bank sync (SEC-06)

**FR-044** The user shall be able to operate the ledger without bank-sync credentials or an external plugin.

**FR-045 [DEFERRED-V2]** External bank providers and account linking are deferred.

**FR-046 [DEFERRED-V2]** Provider status, last successful sync and provider errors belong to the future sync workflow.

**FR-047** The import workflow shall reconcile a confirmed pending-to-posted match without double-counting movement and retain user overrides.

**FR-048** V1 extension support shall accept declarative JSON/TOML import mapping templates only. Executable extensions are [DEFERRED-V2].

**FR-049 [DROP-SAAS]** The desktop product shall not require a hosted plugin marketplace, server worker, Redis, or Celery to run local import and ledger workflows.

### 3.7 Rules, budgets and recurring activity (SEC-07, SEC-08)

**FR-050** The user shall be able to create, edit, enable/disable, prioritize, preview, apply, export, and import categorization rules.

**FR-051** The rules engine shall support the documented fields, string/numeric/date operators, case/accent-insensitive matching, AND/OR conditions, and one level of mixed nested condition groups.

**FR-052** The rules engine shall reject invalid regular expressions, regexes matching an empty string, blank conditions, and empty/nested condition groups.

**FR-053** Rule actions shall support category, payee, description, appended note, and ignore; existing category overwrite shall follow an explicit rule setting.

**FR-054** Rule preview shall show match count and sample rows without changing transactions.

**FR-055** The user shall be able to set monthly category budgets and recurring defaults/period overrides; budget amounts shall be positive.

**FR-056** The application shall calculate actual, projected, and remaining budget amounts using the same transaction inclusion and reporting-date rules as financial reports.

**FR-057** The application shall display carryover eligibility only when remaining is positive. Unused funds and overspending shall not automatically transfer across budget months.

**FR-058** The user shall be able to copy a budget month with a permitted adjustment and optional overwrite; batch failure shall not leave a partially copied month.

**FR-059** The user shall define frequency, start date, weekend policy, end condition, first-occurrence skip and auto-generation setting. Schedules shall evaluate on the ledger’s local wall-clock dates.

**FR-060** Monthly day-of-month schedules shall clamp to valid dates for short months and return to the intended day in later months.

**FR-061** The application shall distinguish virtual recurring projections from posted ledger transactions and shall not include virtual rows in current balance.

**FR-062** Schedule edits shall not silently backfill already generated or matched periods.

### 3.8 Loans, goals and assets (SEC-09)

**FR-063** The user shall define loan terms and calculate Price or SAC schedules of up to 1,200 months.

**FR-064** Loan calculations shall use decimal arithmetic, round interest to currency precision using the documented rounding mode, and absorb final principal rounding remainder in the last payment.

**FR-065** The user shall be able to define a goal with target value/date and manual, account, asset, asset-group, or net-worth tracking.

**FR-066** The application shall calculate current goal value, percentage, monthly contribution estimate, pace state, completion, and overdue status using the reference boundary rules.

**FR-067** The user shall enter manual valuations and configure percentage or fixed growth at daily, weekly, monthly or yearly intervals. Valuation history shall expose source and effective date.

**FR-068** The application shall support asset groups and buy/sell/split events with cost-basis and realized/unrealized calculations.

**FR-069 [DEFERRED-V2]** Live online market quote feeds are deferred.

### 3.9 Reports, calendar and FX (SEC-10, SEC-11)

**FR-070** The application shall provide net-worth, income-versus-expense, cash-flow, category trend, and transaction drill-down views.

**FR-071** The user shall be able to select date range, account/category scope, calendar date basis, dashboard widgets, and report layout.

**FR-072** The calendar shall support bank-date and reporting-date views and shall show actual transactions separately from projected/recurring activity.

**FR-073** In reporting-date calendar mode, activity shall appear on reporting date while booked balance movements remain on bank date and cash-flow overlays use the applicable effective date.

**FR-074** The user shall be able to select a display currency while every account and transaction retains its native amount/currency.

**FR-075** The application shall support manual and CSV-imported FX rates and retain rate date and source. It shall prefer the applicable exact-date rate and use the closest prior rate when offline.

**FR-076** When no historical rate is available, the application shall preserve native amount, visibly badge any 1:1 display estimate and never persist the estimate as a real FX rate.

**FR-077** FX may be fetched on demand through an explicitly enabled provider. Ledger operations shall proceed offline and V1 shall perform no scheduled online market quote refresh.

### 3.10 Attachments, backup and restore (SEC-12)

**FR-078** The user shall attach, list, rename, open/save and remove supported transaction receipt/document files.

**FR-079** The application shall enforce supported file types, configured per-record count and a 10 MB per-file limit. Stored filenames shall be sanitized or generated.

**FR-080** Ledgers, attachments and scheduled backups shall default to Tauri app_data_dir with user-selected alternatives. Portable .db files shall use a relative sibling attachment directory.

**FR-081** The user shall create manual and scheduled WAL-safe full backups with ledger data, configuration and attachments. Retention defaults to 10 successful rolling generations.

**FR-082** Backups shall default to Argon2id plus AES-256-GCM authenticated encryption. Wrong credentials or damaged archives shall produce a recoverable failure.

**FR-083** The application shall preview backup contents, version, warnings, and entity counts before restore.

**FR-084** Restore shall create a new ledger by default. Replacing an active ledger shall require explicit destructive confirmation and a successful automatic safety snapshot.

**FR-085 [ADAPT-LOCAL]** Scheduled backups shall run locally. Server schedulers and cloud storage are [DROP-SAAS].

### 3.11 Deferred AI and MCP (SEC-13)

**FR-086 [DEFERRED-V2]** AI agent features are deferred.

**FR-087 [DEFERRED-V2]** Local and remote LLM chat are deferred.

**FR-088 [DEFERRED-V2]** Agent access to financial reads is deferred.

**FR-089 [DEFERRED-V2]** Agent write proposals and approval UI are deferred.

**FR-090 [DEFERRED-V2]** Agent knowledge documents and derived indexes are deferred.

**FR-091 [DEFERRED-V2]** MCP server is deferred.

### 3.12 Shared expenses, product utilities and deferred invoices (SEC-14, SEC-15, SEC-16)

**FR-092** The primary user shall track local project/trip IOUs with counterparties and exact, percentage or equal allocations that sum to the expense. Counterparties are local records without linked users.

**FR-093** The user shall manually record settlements and inspect outstanding balances by original currency and display currency.

**FR-094 [DEFERRED-V2]** Business invoicing, products, recurring invoices and tax IDs are deferred.

**FR-095 [DEFERRED-V2]** Issuance snapshots and invoice lifecycle are deferred.

**FR-096 [DEFERRED-V2]** Invoice amounts and jurisdiction-specific tax rules are deferred.

**FR-097 [DROP-SAAS]** Vendor-hosted public share URLs are permanently excluded.

**FR-098** The application shall scope global search and transaction filtering to the active ledger.

**FR-099** The application shall provide the Ctrl/Cmd+K command palette and preserve standard editing shortcuts. The chat shortcut is deferred with chat.

**FR-100** The application shall persist dashboard/report layout and privacy display preferences separately from financial data meaning.

## 4. Non-functional requirements

**NFR-001 — Exact financial arithmetic.** All money inputs, persisted amounts, comparisons, allocations, interest, conversion results, and totals shall use an exact decimal or integer-minor-unit representation. Binary floating-point shall not be used for financial arithmetic. FX rates may use a separately defined high-precision decimal scale.

**NFR-002 — Currency precision.** A central scale registry shall support 0-decimal IDR, 2-decimal USD/CNY/EUR/SGD and 3-decimal currencies. Persist monetary values as exact integer minor units and use rust_decimal or checked integer arithmetic for calculations.

**NFR-003 — Offline-first.** Installation, startup, account/transaction CRUD, imports, rules, budgets, schedules, reports, calendar, local backup, and restore shall work without internet access. Optional network features shall fail closed or show an actionable offline state without blocking core use.

**NFR-004 — SQLite durability.** SQLite shall use transactional ACID writes and WAL mode for ordinary application access. Foreign-key enforcement, busy timeout, journal/synchronous policy, and checkpoint behavior shall be explicitly configured and validated in architecture.

**NFR-005 — Safe live backup.** Backups shall use SQLite’s online backup API on a compatible consistent snapshot, including committed WAL contents. For SQLCipher, the encrypted source shall first be exported by a supported, consistent bridge. Copying the main database file alone is prohibited.

**NFR-006 — Data integrity.** A transaction, its split lines, transfer pair, attachments metadata, and related records shall either commit consistently or roll back as one logical operation.

**NFR-007 — Local security.** Secrets shall stay out of plaintext configuration and logs. Optional SQLCipher encryption provides at-rest protection, with manual entry or opt-in OS Keychain quick unlock.

**NFR-008 — No implicit telemetry.** The application shall make no analytics/telemetry calls without clear, affirmative opt-in.

**NFR-009 — Data isolation.** Each read, write, job and export shall be isolated to one ledger file. Switching shall never mix records or results.

**NFR-010 — Responsive startup.** Cold start shall be under 3 seconds on a mid-range desktop at the V1 reference workload of 250,000 transactions and 50 accounts, excluding time spent waiting for user input.

**NFR-011 — Query responsiveness.** Common filtered queries shall have p95 latency below 500 ms at 250,000 transactions and 50 accounts on mid-range hardware. Attachment size is capped at 10 MB. Long-running operations shall support progress and cancellation where safe.

**NFR-012 — Import responsiveness.** Import preview shall be cancellable and provide progress for large files; the UI shall remain responsive while parsing. Import shall be atomic or have explicit recoverable partial-result semantics.

**NFR-013 — Backup portability.** Backup format shall be versioned, include a manifest and integrity checks, document included/excluded data, and be restorable on a clean install without the original machine’s absolute paths.

**NFR-014 — Migration compatibility.** Schema/data migrations shall be versioned, transactional where possible, backed up before irreversible changes, and report actionable recovery instructions on failure.

**NFR-015 — Attachment durability.** Attachment writes/deletes and metadata changes shall be coordinated so an interrupted operation does not leave silent broken references; cleanup of orphaned files shall be recoverable.

**NFR-016 — Accessibility and usability.** Core workflows shall be keyboard-accessible, provide labeled controls and validation feedback, and remain usable at supported desktop window sizes and display scaling.

**NFR-017 — Privacy.** Privacy masking shall apply strictly to UI display, including chart/tooltips and in-app notices. Calculations, persisted data and exports shall remain unmasked.

**NFR-018 — Maintainability.** Financial business rules shall reside in the Rust domain/service layer and be independently testable; Svelte components shall not be the sole enforcement point for money/date/permission rules.

**NFR-019 — Safe integrations.** Outbound providers shall be optional, visibly configured, and restricted to the required endpoint/capability. Local-only operation shall not initialize remote integrations.

**NFR-020 — Backup confidentiality.** New backups shall default to Argon2id and AES-256-GCM with versioned authenticated metadata. Legacy ZIP encryption is excluded.

## 5. Acceptance coverage

Section 11 of ARCHITECTURE_SPEC.md maps the V1 FRs to UI flows and verification cases. Boundary cases to retain include: leap years/month-end schedules; timezone midnight rollovers; transaction on card close day; due day in same month as close day; missing historical FX; zero/negative target values; one-cent split remainder; pending-to-posted bank match; re-import after manual override; restoring into a new ledger; cancel/undo after partial filesystem failure; and reporting-date movement without balance movement.

## 6. Resolved Product Decisions Log (V1 Baseline)

| ID | Decision | Locked V1 baseline |
|---|---|---|
| Q1 | Ledger container | Multi-file SQLite, one .db per ledger, switchable in-app. |
| Q2 | Local access | Optional SQLCipher PIN/passphrase. Manual launch unlock or optional OS Keychain quick unlock. |
| Q3 | Workspace semantics | One primary user per ledger. SaaS tenants, roles and invitations are excluded permanently. |
| Q4 | Backups | Manual and scheduled WAL-safe snapshots. Argon2id + AES-256-GCM by default. Keep 10 successful rolling generations by default. |
| Q5 | Statement import | CSV with reusable per-bank templates and OFX/QFX prioritized. QIF and CAMT supported. |
| Q6 | Deduplication | Provider and external ID plus account/date/amount/type, or deterministic fallback with normalized description. Default-on CSV dedup with opt-out and preview resolution of ambiguous matches. |
| Q7 | Bank connections | External bank sync providers deferred to V2. Local statement files are the V1 ingestion channel. |
| Q8 | Payment channels | Instant debit, QRIS and e-wallet channels use checking with optional payment-method metadata. Credit cards and PayLater/BNPL use credit_card with cycle, limit and shared line support. |
| Q9 | Card accounting | Ledger cash/accrual default with per-card override. Keep effective bill date and reporting override. Computed reporting date drives budgets. |
| Q10 | Time zone | OS local default, IANA ledger override, schedules on local wall-clock dates. |
| Q11 | Budgets | Show eligibility when remaining exceeds zero. No automatic monthly transfer. |
| Q12 | Currency precision | Central scales of 0, 2 and 3 minor digits. Exact scaled integer and Rust decimal arithmetic. |
| Q13 | FX | Manual entry, CSV import and optional on-demand fetch. Use closest prior rate offline, otherwise show a visibly labeled 1:1 estimate without storing it as a rate. |
| Q14 | AI | AI agents, local/remote LLM chat and MCP deferred to V2. |
| Q15 | Groups | Personal IOUs and trips/projects with local counterparties, exact/percentage/equal shares and manual settlements. |
| Q16 | Business features | Invoicing, products and recurring invoice schedules deferred to V2. Vendor-hosted invoice sharing permanently excluded. |
| Q17 | Assets | Manual valuation, fixed/percentage growth, groups and buy/sell/split cost basis. Live online quotes deferred to V2. |
| Q18 | Extensions | Declarative JSON/TOML import mappings. Executable plugins deferred to V2. |
| Q19 | Storage | Tauri app_data_dir default, user-selected folders supported, portable .db with sibling attachments. |
| Q20 | Performance | 250,000 transactions, 50 accounts, 10 MB per attachment. Cold start < 3 s and p95 filtered query < 500 ms on mid-range desktop. |
| Q21 | Restore | New ledger by default. Active replacement requires explicit destructive confirmation and automatic safety snapshot. |
| Q22 | Exports | Transaction CSV includes bank and computed reporting dates. Full encrypted backup archives. UI privacy masking never changes export data. |
| Q23 | Auth migration | OIDC, passkeys, TOTP and API access tokens permanently excluded. SQLCipher and optional OS Keychain cover local access. |
| Q24 | Tax | Tax IDs and jurisdiction-specific invoice calculations deferred with business invoicing. |

## 7. Exclusion and migration boundary

The permanent exclusions in Section 2 apply in every deployment. Deferred capabilities require new, forward-compatible migrations and new opt-in adapters. Existing ledgers shall open without installing or configuring these adapters. V1 data and exports shall retain stable identifiers and native financial values across migrations.
