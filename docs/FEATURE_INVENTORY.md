# Securo Feature Inventory

## Purpose and evidence boundary

This document catalogs behavior found in the Securo reference repository, including application UI, APIs, domain services/models, configuration, documentation, and tests. It describes the reference product rather than asserting that every optional integration is enabled in a deployment. Where an implementation detail is not established by source, it is marked as an open question.

The reference is a multi-user web application: React/TypeScript frontend, FastAPI/Python backend, SQLAlchemy/Alembic persistence, PostgreSQL, and optional Celery/Redis background services. The requested target is a single-user, local-first desktop application using Rust, Tauri v2, SQLite, and Svelte 5. That target is treated as a migration destination, not as existing Securo behavior.

### Evidence map

- Frontend routes/pages include dashboard; transaction list/detail; accounts/list/detail; imports; categorization rules and reconciliation; categories; payees; collections; budgets; goals; recurring transactions; assets and asset groups; reports; shared groups and group details; invoices and invoice details; invoice schedules; products; workspace and admin settings; plugins and plugin detail; agents, agent detail and connections; authentication/setup/callback; and backups.
- Shared UI includes sidebar/workspace switcher, header/user menu, collection selector, command palette, global chat, onboarding tour, update banner, privacy masking, configurable dashboard/report layouts, dialogs/drawers, responsive/mobile views, and translated labels.
- Backend route families include accounts, transactions, import/export, rules, categories, payees, collections, budgets, goals, recurring, assets/asset groups, reports/dashboard/calendar/forecast, currencies/FX, reconciliation, attachments, backups, groups, invoices/schedules/products, plugins, agents/connections/knowledge/MCP, settings, workspace/member/admin, OIDC/custom auth/passkeys/TOTP, access tokens, setup, timezone, search, and UI layouts.
- Repository documentation and ADRs define workspace isolation and role gates, financial date semantics, plugin boundaries, accounting modes, data retention and storage, and optional capabilities.

## Cross-cutting behavior and configuration

- All financial records are scoped to a workspace. The reference has three workspace roles: owner, editor, viewer, plus a virtual manager role in the global user/admin layer. Owner/editor/manager can write; viewer is read-only. Workspace rename/archive/member management are owner-controlled. Backend services—not only UI controls—enforce write access.
- Product modules govern whether a feature exists in a workspace, not whether a user has permission. Transactions, accounts, imports, reports, assets, budgets, goals, recurring, categories, payees, split groups, and rules are enabled by default. Invoices are enabled for business workspaces and disabled for personal workspaces by default. Workspace kind cannot be changed after creation.
- Deployment and workspace options are distinct. Optional bank sync, FX rate provider, object storage, auth extras, agents/MCP, and code plugins can be absent or disabled. No telemetry is intended; outbound calls are limited to explicitly configured integrations.
- Dates are stored/handled using a global timezone and optional workspace timezone. App-local “today” and midnight rollover use the applicable timezone. Persisted timestamps are UTC. Timezone boundaries affect date-based schedules, overdue status, and “today” calculations.
- Financial values use exact decimal arithmetic; money is not represented by binary floating point. Amounts retain account/native currency and may also have a primary/display-currency amount.
- Visibility, permissions, hidden/ignored state, and module availability are separate concerns. Workspace and entity scoping is applied to reads and mutations.
- Optional UI configuration includes locale/language, theme/accent, date and number formats, privacy masking, dashboard/report layout, and onboarding state. Keyboard shortcuts include Ctrl/Cmd+K for command palette and Ctrl/Cmd+J for global chat.

## SEC-01 — Workspaces, profiles and access control

**User story / purpose.** A household, business, or other workspace keeps its financial ledger, configuration, and members isolated. Each user can access only workspaces to which they belong.

**UI touchpoints and flow.** First-run setup creates the initial administrative user/workspace. Users sign in through the configured auth mode, select a workspace, and manage profile/preferences, members, workspace settings, module settings, and integrations from settings. Member management exposes role assignment. Global admin pages manage user state and access. Auth pages cover password login, OIDC callback/account linking, passkey registration/login, and TOTP setup/challenge when configured.

**Business rules and edges.** Roles are owner/editor/viewer; manager is a virtual elevated admin role. Owner/editor/manager may mutate financial records; viewer may read only. Sensitive workspace administration is owner-gated. Modules are not role grants. User-level timezone and workspace timezone coexist, with workspace date interpretation used for financial “today” where applicable. UTC timestamps are converted for display and date logic. Auth extras are optional; OIDC can be configured with different registration/linking policies. Passkeys require secure browser context; TOTP is a second factor. API access tokens are scoped and closed by default. Exact behavior for several auth deployment flags depends on configuration.

**Inputs and outputs.** Inputs: user identity/credentials, OIDC claims, passkey assertion, TOTP code, workspace name/kind/timezone, member/role, preference values, module toggles. Outputs: authenticated identity, workspace membership and role, effective access gates, localized UI preferences, audit-relevant ownership state.

## SEC-02 — Accounts and balance engine

**User story / purpose.** Maintain account ledgers and show current balances, credit exposure, and account-specific activity.

**UI touchpoints and flow.** Accounts list shows name, institution, type, currency, current balance, connection/sync status, and account actions. Account detail includes transaction history, running balances, account settings, linked cards/aliases, loan terms where applicable, and close/archive actions. Opening-balance and balance-adjustment forms create or alter ledger entries. Account selectors are reused in transaction, transfer, import, goal, and report flows.

**Business rules and edges.** Reference account types are checking, savings, credit card, and loan; immediate-debit cards are not a separate ledger type. A positive manually entered checking/savings opening balance is represented by a synthetic opening-balance transaction; credit card/loan debt uses the opposite transaction direction. Zero opening balance removes the synthetic row. Manual account balance edits change that opening row or create an explicit adjustment as appropriate. Synced balances are reconciled to provider balance by an opening offset; disconnecting can preserve the displayed balance by adjusting that offset. Current balance includes posted, nonfuture ledger activity; pending credit-card authorizations are included in current card debt, but other pending transactions do not become posted income/expense. Future scheduled activity is excluded from current balance. Ignored transactions are excluded. Provider-reserved funds are displayed separately rather than added to total. Available credit is credit limit minus current card debt; future scheduled card purchases may hold available credit without being current debt. Shared credit lines can surface under multiple cards, so account details avoid double-counting line debt in sidebar aggregates. Currency and conversion rules apply separately to native balance and primary display totals.

**Inputs and outputs.** Inputs: account name/type/currency, institution, opening amount/date, credit limit, statement close and due days, card aliases/last-four, loan terms, connection state, adjustments. Outputs: native and primary balances, running balances, available credit, pending/future activity summaries, sync status.

## SEC-03 — Transactions and dual-date ledger

**User story / purpose.** Capture and review income, expenses, transfers, and adjustments while keeping bank booking date distinct from period-reporting attribution.

**UI touchpoints and flow.** Transaction list supports free-text search, account/category/payee/type/status/date/bill/group/tag/amount filters, sort, pagination, status/flag indicators, category and split editing, notes, tags, attachments, and detail drawers. Forms support manual create/edit, split categories, people/share splits where relevant, recurring linkage, card bill date, and separate reporting-date override. Bulk actions categorize, add/remove tags, flag, add to group, or delete. CSV export is available. Reconciliation and search surfaces link back to the matching ledger row.

**Business rules and edges.**
- Transaction amount is positive with a debit/credit type; rule-engine signed amount is negative for debit and positive for credit. Exact decimals are used.
- date is the provider/bank booking date. effective_date is cash-flow timing, chiefly the card bill due date; reporting_date is separately computed. Reporting-date precedence is explicit reporting_date_override, then effective_bill_date, then effective_date in accrual mode, otherwise bank date.
- Changing reporting_date_override never changes provider booking date, balance movement, sync matching, or FX calculation. Synced amount/date cannot be changed. Unchanged edits may retain the existing FX stamp. Manual transfer edits cannot alter synced counterpart amount/date/FX/effective date. “Post today” is not allowed for a synced anchor.
- Instance-wide credit_card_accounting_mode defaults to cash. Cash P&L/report/budget attribution uses bank date; accrual uses effective due date for card expenses. Balances always follow ledger/bank date. A posted expense reported in the future is not counted twice in projected balance. Pending card rows can retain bill-date forecast timing even in cash mode.
- posted/pending, ignored, exclude_from_pnl, flagged/reminder, and future-date state are independent. P&L requires posted and nonfuture. ignored removes row from all totals including balances. exclude_from_pnl preserves balance effect but removes P&L effect. Flag/due affects review queues, not arithmetic. Transfers and categories marked as transfer are excluded from income/expense totals.
- Category splits require at least two positive parts summing exactly to the absolute parent amount; cent remainder goes to the largest fractional part. A split parent has no single category. Categories must belong to the same workspace and be assignable. Recurring templates may carry category lines. People/share splits are separate from category splits; their amounts are materialized in row currency and rounding residue goes to the final share.
- Notes are limited to 1,000 characters. Tags are hashtag tokens embedded in notes, not a separate tag table. Adding normalizes a leading # and avoids duplicate tokens; removal matches exact tokens; multiple tag filters use ANY/OR semantics with token boundaries.
- Filters include account(s), linked card(s), category(ies), payee, date range, bill ID (which takes precedence over from/to), group, unbilled, statement view, text, uncategorized/shared-as-uncategorized, type, status, opening-balance inclusion, running-balance mode, exclude transfers, P&L-only, ignored, flagged/overdue/due-within-week, tags, min/max absolute primary amount, sorting fields and direction.
- Bulk categorization skips split rows. Bulk tag operations preserve other note content. Bulk deletion is a distinct destructive action.
- Installment series require complete metadata, index >= 1 and <= total, positive total, and purchase date <= transaction date. Editing scope may be this/future/all; a series without its identifying fingerprint cannot be edited as a series.

**Inputs and outputs.** Inputs: account, date, amount/type, description/payee, category or splits, notes/tags, status, flags, group, attachments, installment metadata, bill date override, reporting-date override, P&L/ignore flags. Outputs: ledger entry, computed reporting/effective dates, account running balance, P&L inclusion, filtered/exported rows.

## SEC-04 — Internal transfers

**User story / purpose.** Move value between accounts without treating the movement as income or expense.

**UI touchpoints and flow.** Transfer creation is available from transaction/account flows. The UI identifies linked debit/credit legs, shows counterpart account, supports candidate matching/linking and unlinking, and allows transfer review in reconciliation. Transfers may be entered across currencies.

**Business rules and edges.** A transfer is represented by linked double-entry transaction legs. Each leg affects its own account balance in its native currency; both legs are excluded from income/expense totals. Cross-currency movements retain each account-side amount and FX information; conversion differences are not income/expense merely by virtue of transfer. Sync matching attempts to link provider transactions and avoid phantom duplicates. A manually edited transfer cannot silently mutate a synced counterpart’s protected provider fields. Exact rounding treatment for independently supplied amounts is determined by transfer/FX inputs and is a product decision for the port.

**Inputs and outputs.** Inputs: source/destination accounts, each-side amount/currency, dates, FX rate or conversion, description, matching candidates. Outputs: linked debit/credit rows, account balance movements, transfer-excluded report treatment.

## SEC-05 — File imports and deduplication

**User story / purpose.** Bring bank history into an account from common statement files without creating duplicate ledger entries.

**UI touchpoints and flow.** Import page selects account and file, previews detected format/columns, accepts CSV mapping/date format/amount direction, shows row errors and duplicate/skipped counts, then imports after the user confirms. Import history shows file, format, credits/debits/counts and supports undo. CSV category hints may be suggested. Preview does not mutate the ledger.

**Business rules and edges.** Supported parsers are OFX/QFX, QIF, CAMT XML, and CSV. Extensionless/unknown files are tried in OFX, QIF, CAMT, CSV order. CSV mapping can set date format, flip amount, separate inflow/outflow columns, or map fields by JSON mapping. Parser errors are row-specific. OFX can be decoded UTF-8 then Latin-1; absent FITID is synthesized for parsing but is not persisted as a provider ID. OFX balance-summary rows such as prior balance are filtered. QIF date format is inferred from the whole file, with historical US-first interpretation where ambiguous; explicit format is strict. CAMT follows parser-specific bank fields.
- Deduplication is not a universal content hash. With external IDs, matching uses account + external_id + date + amount + type. Without ID, it uses account + date + amount + type and description/original_description; a unique shared match against sync rows may also be used. CSV’s detect_duplicates option can disable its duplicate check; non-CSV formats always deduplicate. Provider sync uses stronger provider-specific identifiers and pending-to-posted matching.
- Re-import/sync preserves user categorization/payee edits, reporting-date override, and other manual overrides tracked separately from raw provider data. Provider-synced amount/date remain immutable.
- Import can be undone through its import log; undo behavior must consider subsequent edits/links and is exposed as a distinct action.

**Inputs and outputs.** Inputs: file bytes/name, target account, parser options, column mapping, duplicate setting, confirmation. Outputs: preview rows/errors, imported transactions, skipped/duplicate counts, import log and undo result.

## SEC-06 — Bank sync and extensibility

**User story / purpose.** Connect external account-data providers and extend data import without giving arbitrary code access to the finance store.

**UI touchpoints and flow.** Connections page starts provider authorization, selects/links accounts, shows status and last sync, and supports refresh/disconnect. Integration/plugin pages list installed integrations and configuration. Provider-specific OAuth/consent screens may be external. Plugin detail exposes capabilities and setup.

**Business rules and edges.** Providers documented include Pluggy (Brazil), Enable Banking (EU, around 2,500 institutions), and SimpleFIN (US and some international institutions). Connection records are workspace-scoped; credentials and provider IDs are sensitive. Sync merges by provider identity, handles pending-to-posted transitions, rejects provider-date/amount edits, preserves user overrides, and tries to clean phantom duplicate pending rows. Availability depends on credentials/provider geography. Data plugins use a constrained manifest/API for declared importers, reports, and other data features. Code plugins are a different trust boundary, disabled unless explicitly enabled and run with restricted host access/optional deployment profiles. Data plugin availability is separately controlled; PLUGINS_ENABLED governs data plugins. Code plugin installation is owner-controlled by ADR; docs contain setup language that should be reconciled. Neither plugin type is required for core ledger use.

**Inputs and outputs.** Inputs: provider credentials/consent, provider account mapping, sync request, plugin manifest/configuration, data file. Outputs: connection status, imported/updated provider rows, plugin-provided data/report capabilities, sync errors.

## SEC-07 — Auto-categorization rules engine

**User story / purpose.** Apply repeatable categorization and cleanup rules to imported or existing transactions.

**UI touchpoints and flow.** Rules page lists active/order/count state; create/edit form selects conditions and actions; preview/dry-run presents match count and newest samples with pagination; user can apply to existing rows or change priority/active status. Rule packs can be imported/exported. Rule changes can be previewed before application.

**Business rules and edges.** Conditions may inspect description, payee, notes, amount, type, account ID, payee ID, date, status, and provider category. Operators: contains, not_contains, equals, not_equals, starts_with, ends_with, regex, greater/greater-equal/less/less-equal. Text matching is case/accent-insensitive after normalization. Regex that matches empty and invalid regex are rejected. Empty values are invalid; false and zero remain valid. Amount condition is signed by type; date uses ISO form. Conditions support top-level AND or OR and one nested group level for mixed logic; groups cannot be empty or nested. Priority numeric order, active state, and apply-to-existing gate execution; preview exposes would-apply. Actions set category/payee/description, append notes, or ignore. Existing split-category lines are not changed by category rules. Invalid, hidden, unassignable, or stale category actions are discarded. Apply-to-existing does not overwrite an existing category unless overwrite_existing_categories is enabled. Built-in rule packs do not require plugin toggle; portable package identifier is securo-categorization-rules v1 and supports overwrite behavior.

**Inputs and outputs.** Inputs: conditions/logic, priority, actions, active/apply-existing/overwrite flags, target rows. Outputs: preview count/sample, rule-applied transaction fields, applied count, exported/imported rule pack.

## SEC-08 — Budgets and recurring transactions

**User story / purpose.** Set category spending limits, understand actual and projected spend, and model scheduled income/outflows.

**UI touchpoints and flow.** Budget page selects month, category, amount, recurring/default amount, copy-from-month, optional adjustment/overwrite, and displays actual/projected/remaining progress. Recurring page lists schedules and next occurrences; forms edit cadence, weekend behavior, end condition, amount, account/category/splits, and automatic materialization. Calendar shows virtual recurring occurrences; forecast includes future projected cash flows.

**Business rules and edges.**
- Budgets are monthly and keyed to first day of month/category. Amount must be positive. Batch create allows 1–200 and rejects duplicate categories atomically. A recurring category default may be overridden for a particular month. Copy supports adjustment from -100% through +1000%, optional overwrite; month overrides are handled separately. Actual/projected spend uses P&L rules and reporting-date attribution, posted actuals and future recurring plans; shared expense shares are accounted for. Suggestions use 1–24 prior complete months, not incomplete current month. Remaining equals budget minus projected spend. Carryover eligibility may be shown when remaining > 0; the code does not automatically post a carried-forward balance. Budget is an analysis limit, not a cash-account envelope.
- Recurring frequencies include weekly, biweekly, monthly, quarterly, semiannual, yearly, and day-of-month 1–31. Weekend policy is none, previous Friday, or next Monday. A day such as 31 is clamped for short months and restored in longer months. End conditions include date/count/none; end-date evaluation is on nominal occurrence, while weekend movement may cross month boundary. skip_first advances one interval. auto_generate controls materialized placeholder creation; disabled still supports projection until actual transaction arrives. Virtual schedule items in calendar are forecasts, not ledger balance entries. Generated placeholders can match incoming transactions. Updating a schedule preserves a generation pointer floor and does not backfill already generated/matched history.

**Inputs and outputs.** Inputs: budget month/category/amount/default/override/copy options; recurring amount, account, category/splits, cadence, weekend policy, start/end/skip-first/auto-generate. Outputs: budget actual/projected/remaining, eligible carryover indicator, next occurrence, virtual/materialized scheduled rows, projected cash flow.

## SEC-09 — Loans, goals and assets

**User story / purpose.** Track liabilities, planned savings outcomes, and owned assets alongside ordinary account balances.

**UI touchpoints and flow.** Loan account details include loan terms and amortization schedule. Goals page creates target, due date and tracking source. Assets page creates/edit assets, manual valuations, growth rules, market-priced holdings, buy/sell/split transactions, and asset groups. Asset and group details show valuation history, allocation, and archive/delete controls.

**Business rules and edges.**
- Loans support mortgage, auto, personal, student, or other, payment cadence, nominal/effective annual rate, and price or SAC amortization. Schedule is computed on read and compared with actual debt; it is not itself posted as a ledger. Monthly rates derive from selected annual convention; interest is rounded to cents half-up; final principal absorbs remainder. Payment due “today” is not overdue; overdue means strictly earlier. Payoff projection is capped at 1,200 months; payoff tolerance is $1 in source currency. Balance adjustment creates an adjustment transaction and may allocate interest. Mortgage allocation can split principal/interest.
- Goals support manual, account, asset, asset-group, or net-worth tracking and active/completed/paused/archived states. Linked sources must be in the same workspace. Changing source clears incompatible links. Linked value is dynamic. Percentage is 100% for nonpositive target only if current value is positive, otherwise 0%; monthly contribution is remaining target divided by approximate calendar months. “On track” compares progress against elapsed-time linear expectation, with 5% tolerance and “ahead” at 10% above expected. Completion is current >= target; overdue is strictly after target date. A goal due today can show zero monthly contribution.
- Assets include real estate, vehicle, valuable, investment, and other. Values can be manual history, growth rule (percent or absolute, daily/weekly/monthly/yearly), or market quote. Market prices may be stale/unavailable. Buy/sell/split history computes weighted average cost and realized/unrealized amounts. CSV import has preview. Asset groups are portfolio-like collections. Archive retains history; delete is separate. Provider/market source availability varies; Brazil Tesouro support is documented.

**Inputs and outputs.** Inputs: loan principal/rate/term/method/payment dates; goal target/currency/date/source; asset type/value/currency/growth rule/ticker/history, asset transaction, group membership. Outputs: amortization forecast, current/goal progress and pace, valuation history, allocation, cost basis and realized/unrealized figures.

## SEC-10 — Reports, calendar and analytics

**User story / purpose.** Review financial position, trends, activity, and forecast across selected date windows.

**UI touchpoints and flow.** Dashboard has configurable widgets and privacy masking. Reports library includes net worth, income vs. expenses, and cash-flow/report views with date-range controls, charts, category breakdowns/sparklines, and drill-through to transactions. Calendar uses month/week/day navigation and date-basis selector. Search is global. Saved report/dashboard layout is user-configurable.

**Business rules and edges.** Reports apply workspace filters, P&L inclusion, transfer exclusion, accounting mode, reporting-date rules, and currency conversion. Net worth combines assets and liabilities; income/expense charts and category sparklines drill into ledger data. Calendar offers reporting-date or bank-date basis (default reporting), Sunday-start grid and adjacent-month spillover days. It can filter by account and distinguishes actual from projected/recurring items. In reporting-date view, transaction activity appears on reporting date, while daily account balance movement remains on effective/bank flow date. Ignored rows are omitted; closed accounts may remain in historical calendar though excluded from current balance. Plugin query reports share P&L rules but may omit group shares and recurring projections; code reports have limited input scopes and sandbox limits.

**Inputs and outputs.** Inputs: date range, account/category/group, date basis, report layout/widgets, privacy toggle. Outputs: balances, net-worth series, income/expense totals, category trends/sparklines, projected cash flow, calendar events and drill-down row sets.

## SEC-11 — Multi-currency and FX conversion

**User story / purpose.** Keep native account/transaction values while presenting consolidated values in a selected display currency.

**UI touchpoints and flow.** User preferences set base/display currency. Account and transaction forms accept native currency; FX settings configure Open Exchange Rates credentials and retrieval mode. FX displays can indicate rate source/fallback. Supported currency list includes IDR.

**Business rules and edges.** Account-side amount and primary amount are separate. Rates are stored as USD-base-per-date; cross rates are calculated as USD-to-target divided by USD-to-source. Resolution checks exact date, can fetch on demand, then uses closest prior rate for historical dates; future dates use today’s rate. If no rate is available, read views may use 1:1 with warning; persisted conversion stays null rather than saving a fake 1:1. OER may be configured for on-demand or scheduled retrieval. Native amount is retained. FX fields do not override provider-protected amounts or date semantics.

**Inputs and outputs.** Inputs: native amount/currency, display currency, date, provider/API key, retrieval mode, override rate/source. Outputs: native/account balance, primary equivalent, rate provenance/date, missing-rate warning.

## SEC-12 — Attachments, backup and restore

**User story / purpose.** Keep receipts/documents with transactions and invoices, and move or recover a complete workspace.

**UI touchpoints and flow.** Transaction/invoice detail has attachment upload/list/rename/delete; settings configure local or S3 storage. Backup page supports download now, stored backup list, schedule/retention settings, upload/preview, password prompt, restore target selection, and confirmation for restoring over the current workspace.

**Business rules and edges.**
- Allowed attachment extensions: jpg, jpeg, png, webp, gif, heic, pdf. Maximum 10 MB each; max 10 per transaction and 20 per invoice. Filename is sanitized; content type is derived from extension; storage keys are workspace-prefixed. Rename preserves extension; file deletion is best-effort with database deletion. Reference supports local filesystem default and optional S3-compatible storage.
- Backup is versioned ZIP JSON (currently format 1.1), with optional AES-256 encrypted ZIP password protection, content/configuration/data modes, manual download and stored/scheduled backups. Schedule may be daily/weekly; retention count 1–200 (default 10), retention days 1–3650 optional. Preview inspects upload/stored archive and reports warnings/entity counts. Restore can create a new workspace (default) or replace current workspace after explicit confirmation. It remaps entity IDs and references, restores included attachment files, supports password retries, and reports/rolls back some storage changes on failure. Provider bank credentials/connections are excluded. Configuration and financial data entity sets are selected by backup mode. Stored backup directory is server-side in reference and must map to app data storage in desktop target.

**Inputs and outputs.** Inputs: files, storage config, backup mode/password, schedule/retention, archive, restore target/confirmation. Outputs: attached file references, portable archive, backup status/list, preview warnings/entity counts, restored workspace and attachments.

## SEC-13 — AI agents and MCP server

**User story / purpose.** Let an optional assistant answer questions over finance data and propose structured changes subject to explicit approval.

**UI touchpoints and flow.** Global chat and Agents pages support agent creation/configuration, conversation history, connections, per-agent knowledge uploads/search/pinning/deletion, and tool proposal review/approval. MCP settings expose token creation and agent/external client connection. Chat provider/model settings are per agent.

**Business rules and edges.** Feature is optional and gated by AGENTS_ENABLED; a Compose agents profile starts an MCP server, normally bound to localhost port 8765. MCP requires JWT; CORS is disabled unless configured. Providers include Anthropic, OpenAI, OpenAI-compatible endpoints, and Ollama. Ollama is local; remote providers receive prompts/context sent to them. Agents can use finance read tools and structured proposal tools. Writes are presented as proposals requiring user approval before mutation. Tool domains include accounts, transactions, categories, payees, transfers, rules, budgets, groups, assets, goals, forecast, reports, global search, and knowledge. Knowledge is per agent and uses vector embeddings. External MCP tokens are scoped. Actual endpoint/tool availability depends on configuration.

**Inputs and outputs.** Inputs: agent prompt/model/provider, API key or local endpoint, conversation, knowledge documents, MCP token/client, tool proposal approval. Outputs: assistant response, retrieved knowledge, finance tool results, pending/approved/rejected structured actions.

## SEC-14 — Shared groups and expense sharing

**User story / purpose.** Attribute portions of shared transactions to members, projects, clients, or cost centers and track settlement.

**UI touchpoints and flow.** Groups list/detail manages group kind, members, member links, transaction membership and share allocation; balances and settlement actions show outstanding amounts across currencies. Transaction flows can add a row to a group and set equal/exact/percentage shares. Group detail can drill into shared entries.

**Business rules and edges.** Group kinds include social, cost_center, project, client, other. Members may link to a user or be represented by email/name. Shares must total exactly to the transaction amount after cent rounding. Linked users can see their own share across workspaces but not other members’ private entries. Only payer/owner or relevant linked member can settle from permitted membership; settlement credits the receiver’s first eligible nonclosed checking/savings account when available, otherwise requires reconciliation. Payer-side settlement debit is excluded from P&L; receiver-side credit counts as income. Group balances retain source currencies and can also be converted to default currency. This feature’s multi-user semantics require adaptation for a single-user desktop.

**Inputs and outputs.** Inputs: group/member, transaction, share type and amounts, settlement parties/date/account. Outputs: member balances, settlement ledger legs, currency-wise and converted totals.

## SEC-15 — Invoices, schedules and products

**User story / purpose.** Support business-workspace billing, recurring invoices, product catalog, payment allocation, and customer-facing invoice viewing.

**UI touchpoints and flow.** Business workspace navigation exposes invoice list/detail, schedule list/detail, products, issuer settings/tax IDs, invoice create/edit/issue, PDF/document output, payment/deduction allocation, share-link generation/revocation, and public invoice route by token. Invoice schedule forms manage cadence, terms, pause/resume/end, link/unlink invoices, and due generation.

**Business rules and edges.** Invoices are a module, default on for business workspace and off for personal. Invoice lifecycle includes draft, issued, paid/partial, void, and uncollectible; derived status depends on allocated/deducted totals, due dates and current timezone-aware date. Issuing snapshots issuer/tax settings and line values. Lines use quantity × unit price with exact-decimal totals; products can have versioned price history and jurisdiction-specific fiscal fields. Allocations, deductions, installments and payment terms determine outstanding balance and overdue installments. Share tokens provide public read access until revoked. Recurring invoice schedules have period sequence/bounds, terms that may vary by period, end date/count, pause/resume/end, generation and invoice link/unlink. Editing schedule terms may be restricted after an invoice is linked to that period. Generated invoice dates/terms follow the schedule period. Tax behavior is jurisdiction/config dependent; do not infer a universal tax formula.

**Inputs and outputs.** Inputs: payee, line/product, quantity, price, discount/tax configuration, invoice/due dates, installment/payment allocation, issuer profile/tax IDs, schedule cadence/terms, share token. Outputs: invoice totals/status/balance/aging, PDF/document, payment allocations, public invoice view, generated scheduled invoices.

## SEC-16 — Search, navigation and interaction utilities

**User story / purpose.** Reach records and common actions quickly across a responsive desktop/web UI.

**UI touchpoints and flow.** Global search searches finance records. Command palette opens with Ctrl/Cmd+K; global chat with Ctrl/Cmd+J. Sidebar/workspace switcher, header menu, dashboard widgets, privacy toggle, onboarding tour, update notice, mobile navigation and layout controls are shared across pages. Transaction list has its own query/search/filter/sort; report and calendar layouts are configurable.

**Business rules and edges.** Search respects workspace scope and authorization. UI layout/preferences are persisted separately from ledger meaning. Privacy mode masks displayed monetary values but must not mutate stored amounts or calculations. Shortcut behavior depends on platform and focused control. No source-established guarantee exists that every page action is available through the command palette.

**Inputs and outputs.** Inputs: query text, shortcut, workspace, layout/preferences. Outputs: scoped result links, action palette, chat launch, personalized UI state.

## SEC-17 — Operational configuration and deployment controls

**User story / purpose.** Let an administrator enable only required integrations and control storage, authentication, background work and network exposure.

**UI touchpoints and flow.** Most deployment flags are environment configuration rather than end-user screens; supported workspace/user settings are surfaced in setup/settings. Admin configures provider credentials and optional services before enabling corresponding UI.

**Business rules and edges.** Options include timezone defaults/overrides, auth mode/OIDC settings, registration/link policy, passkey/TOTP availability, bank provider credentials, OER key and on-demand/scheduled FX, local/S3 attachment storage, agents enablement/provider, MCP host/port/JWT/CORS, data plugin enablement, code plugin profile/network hosts, task workers, backup schedule/storage, and module defaults. Core ledger operation does not require integrations. Secrets must remain deployment-managed and should not be exported in backups. Version-specific environment variable names are documented in deployment guides and must be checked when implementing; this inventory does not replace configuration reference pages.

**Inputs and outputs.** Inputs: deployment environment/configuration, credentials, workspace flags and module selections. Outputs: enabled capability set, service/network behavior, integration status, scheduled task behavior.

