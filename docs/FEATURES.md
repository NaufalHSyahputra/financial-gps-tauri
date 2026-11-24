# Feature guide

[Documentation home](README.md) · [Debugging](DEBUGGING.md) · [Build and release](BUILD_AND_RELEASE.md)

Start by opening your activity. Use the workspace sidebar to reach the sections below. A **code** line links to the UI route and the Rust implementation. Storage and file operations also link to their runtime adapters because they use platform services.

## Activity and protection

An activity file keeps one set of financial records separate from the rest. Open one activity file at a time. The app ties each action to the activity you opened.

1. On the first visit, choose your **Primary currency**. **IDR (Indonesian Rupiah)** is selected by default.
2. Choose **Cash**, **Bank account**, **Digital wallet**, or another account type and give it a name.
3. Enter a starting balance or leave it blank for zero, then select **Start Tracking**. To choose Cloud Sync, open existing activity, or set up protection from the start, select **More setup options**. Native Local Only activity can also use a portable file.
4. In **Your activity**, open or create a separate activity file when you need to keep records apart.
5. To protect the activity, select **Encrypt this ledger** and supply a passphrase or PIN. Select the quick-unlock checkbox only on a device you trust. Native encrypted files use SQLCipher. The browser uses an encrypted database image in its origin-private storage.
6. Use the **Open activity** selector or return to **Your activity** and open a saved ledger. Select **Lock** to end the session. Reopen an encrypted ledger and enter the credential or select **Use secure vault** if quick unlock is available. **Close activity** returns to the launcher without an active session.
7. Use **Settings → Activity protection** to change the credential, change protection, or manage quick unlock. Keep a separate backup passphrase available if the operation asks for a safety backup.
8. Use **Open file…** to register an existing native Financial GPS database. A native portable ledger keeps its `.db.keys` sidecar when encrypted and its adjacent `.db.attachments` folder if present. Web imports a compatible plain database into browser storage or unlocks an encrypted `.fgpsbackup` as an imported copy. Use the encrypted archive format to move protected ledgers between runtimes.

The shared lifecycle names are `unselected`, `resolving`, `locked`, `opening`, `ready`, and `quiescing`. Open and unlock lead to **ready**. The native ledger manager drains in-flight work as part of locking or closing. The secure vault stores a quick-unlock key only when enabled.

**Code** · [Account setup](../src/lib/views/AccountSetup.svelte) · [Launcher](../src/routes/+page.svelte) · [Unlock](../src/routes/unlock/+page.svelte) · [Settings](../src/routes/settings/+page.svelte) · [Lifecycle](../crates/core-engine/src/domain/ledger.rs) · [Native ledger manager](../src-tauri/storage/ledger.rs) · [Native vault](../src-tauri/crypto/native_secret_vault.rs) · [Web worker](../src/lib/drivers/web/db.worker.ts) · [Vault contract](../src/lib/drivers/contracts.ts)

## Storage and Cloud Sync

Cloud Sync shares your activity through your own Turso database. Writes commit locally first. The outbox records durable local and acknowledged versions, and sync publishes or pulls a generation of ledger data. Edits on both sides can require a choice between snapshots. Use one Turso database per activity file.

1. Choose **Local Only** for device-only storage. For cloud enrollment, select **Cloud Sync**, enter your `libsql://` or `https://` Turso URL and auth token, and select **Test Connection**. Creation also runs a live `SELECT 1` probe before making a cloud ledger.
2. For existing activity saved in the app, open **Settings → Storage & Sync → Switch Mode** and confirm enrollment. Enroll the other device with the same database. A fresh, empty activity file can pull the current generation. An activity file that already has records may require conflict review.
3. Check the masked endpoint, sync status, last successful sync, and pending change count. Use **Sync Now** when you want to run sync immediately. The app also checks periodically while an enrolled ledger is open. **Test Connection** uses the saved credential via the secret vault.
4. Use **Update credentials** to enter a replacement token. The form starts empty and the vault stores the replacement. To disconnect this device, choose **Switch Mode → Confirm Local Only**. This keeps the local records and revokes the locally stored token. Remote data and other enrolled devices remain in place.

If **Review sync conflict** appears, open it to compare the local edit version, remote generation, and last shared generation.

1. Enter a separate backup passphrase and choose **Export encrypted safety backup** to keep the local snapshot.
2. Choose **Keep Local Version** to publish the local snapshot against the displayed remote generation.
3. Or check the replacement acknowledgment and choose **Accept Remote Version**. The app checks both versions, replaces the pending local data, and rebuilds transaction search. Review again if either version changes before resolution.

**Code** · [Onboarding](../src/routes/+page.svelte) · [Settings route](../src/routes/settings/+page.svelte) · [Sync controls](../src/lib/views/StorageSync.svelte) · [Shared schema](../crates/core-engine/migrations/0001_init.sql) · [Cash migration](../crates/core-engine/migrations/0002_cash_accounts.sql) · [Native sync](../src-tauri/storage/sync/mod.rs) · [Web sync](../src/lib/drivers/web/sync.ts)

## Accounts, categories, shared groups, and transfers

### Accounts and credit

Accounts track balances in a fixed currency. Money uses integer minor units and exact decimal calculations. For example, USD 12.34 is 1,234 minor units, while JPY 100 is 100 minor units. Currency scales cannot be edited.

1. Open **Accounts → New account**, choose an account type, and enter a name and starting balance. New accounts use your primary currency. Cash has its own group for physical money. Use **Archived accounts** to revisit archived accounts. Open an account to archive or unarchive it.
2. Open an account to set an opening balance or make a dated balance correction with a reason. The app records these as separate posting types.
3. For a credit card, set closing and due days, an individual limit or shared credit line, and an optional accounting override. Use **Statement cycles → Add statement** to record or revise a cycle. Review the card balance and the amount still available.
4. Use **Shared credit limits** on the account list when multiple cards draw against one limit. Pending charges reduce available credit. Confirmed charges count toward the recorded balance.

**Code** · [Accounts](../src/routes/accounts/+page.svelte) · [Account detail](../src/routes/accounts/%5Bid%5D/+page.svelte) · [Account engine](../crates/core-engine/src/ledger_engine/accounts.rs) · [Money types](../crates/core-engine/src/domain/money.rs)

### Categories, people, and stores

Categories describe what the entry represents. People and stores identify who you paid or received money from. Category groups organize similar categories together.

1. Open **Categories**. Create a category group if needed, then add an expense, income, or transfer category. Choose a group where appropriate.
2. Open **People & stores** to add a name. Use the edit and merge controls to consolidate duplicate names.
3. Assign a category and a person or store in the transaction editor. Review replacement options when deleting a referenced catalog item. Categories can be hidden or ignored in calculations.

**Code** · [Categories](../src/routes/categories/+page.svelte) · [People & stores](../src/routes/payees/+page.svelte) · [Catalog editor](../src/lib/views/Catalog.svelte) · [Catalog engine](../crates/core-engine/src/ledger_engine/catalog.rs)

### Split expenses and pay people back

Shared groups track who owes whom for an expense. Their balances reflect each person’s share and what they have paid back. They also let you tag transactions with a shared group for filtering.

1. Open **Shared groups → New group**, choose a group type, then add members.
2. Open the group and select **Split expense**. Choose equal shares, percentages, or exact amounts. Use **Balances** to check the result.
3. Select **Pay back**, choose the members and amount, and review the group's balance. Balance rows keep each currency separate. Overpayment remains visible.
4. Assign rows to a group using **Transactions → Assign group** in the bulk action menu.

**Code** · [Groups](../src/routes/groups/+page.svelte) · [Group detail](../src/routes/groups/%5Bid%5D/+page.svelte) · [Shared groups engine](../crates/core-engine/src/planning/shared.rs) · [Allocation math](../crates/core-engine/src/planning/allocations.rs)

### Linked transfers

A transfer links the amount leaving one account with the amount entering the other so an internal movement is kept together and excluded from income and expense totals. New transfers use accounts in the same currency. Existing transfers keep their original amounts.

1. Open **Transactions → New transaction → Transfer**. Pick the transaction type and amount, then choose the two accounts. Add an optional date and description.
2. To link transactions already recorded or imported, open one side, choose **Find transfer match**, then select the matching row and save the link.
3. Use the linked transaction's detail view to inspect the other side or unlink. After unlinking, you can edit each transaction separately.

**Code** · [Transaction detail](../src/routes/transactions/%5Bid%5D/+page.svelte) · [Transaction editor](../src/lib/views/TransactionEditor.svelte) · [Transfers engine](../crates/core-engine/src/ledger_engine/transfers.rs)

## Transactions, review, and search

### Add a transaction in three steps

1. Open **Transactions → New transaction**. Choose **Expense**, **Income**, or **Transfer**. Enter a positive amount by keyboard or keypad. IDR uses whole units and offers a **000** key. Currencies with decimal places offer a decimal key. Toggle **Calculator** for plus, minus, multiply, divide, and equals. The calculator uses standard precedence and rounds the result only once, to the currency’s precision.
2. Choose an account. Pick an optional category using the searchable buttons, or choose a second account for a transfer. Transfers use two accounts in the same currency. Choose **Save Transaction** to use today’s date and skip details.
3. Choose **Add details** to change the date, add a description or notes, split between categories, or assign an expense to a shared group. The split needs at least two positive amounts that total the transaction amount. Review the summary and select **Save Transaction**.

Open an existing transaction to edit it, flag it for follow-up, or attach a receipt. Some details in historical imported entries are protected. For entries with dates used in reports, the app uses a date override first, then the bill date if entered, then the date based on the card settings.

**Code** · [Transaction route](../src/routes/transactions/%5Bid%5D/+page.svelte) · [Wizard](../src/lib/views/NewTransaction.svelte) · [Calculator](../src/lib/calculator.ts) · [Transaction editor](../src/lib/views/TransactionEditor.svelte) · [Transaction rules](../crates/core-engine/src/ledger_engine/transactions.rs) · [Split and group workflows](../crates/core-engine/src/ledger_engine/workflows.rs) · [Attachments](../src-tauri/storage/attachments.rs)

### Search, filters, bulk actions, and export

The transaction list pages through matching rows and renders only the visible part of the table. SQLite FTS5 indexes transaction text, and `financial_normalize` helps global search match equivalent case and accented text in names.

1. Use **Search** for descriptions, notes, and payees. Use the filters for date basis and range, account, category, payee, group, status, direction, posting kind, tags, statement, linked card alias, flags, transfer state, exclusions, and native or converted amounts.
2. Choose sort order and **Apply filters**. Use **Running balance** for a running view or **Export CSV** to save filtered data.
3. Select rows, choose Categorize, Add tag, Remove tag, Flag, Assign group, or Delete, then select **Preview action** and review before confirming. If your activity changed since the preview, make a new preview.
4. Use the [command palette](#workspace-settings-dashboards-and-shortcuts) to search across the open activity. From a focused transaction row control, use Up or Down to move to the corresponding control on the adjacent row.

Bulk editing and rule application each have a review step.

**Code** · [Transaction list](../src/routes/transactions/+page.svelte) · [Search and reports](../crates/core-engine/src/ledger_engine/reports.rs) · [Transactions](../crates/core-engine/src/ledger_engine/transactions.rs) · [Bulk workflows](../crates/core-engine/src/ledger_engine/workflows.rs) · [Staging](../crates/core-engine/src/ledger_engine/staging.rs) · [CSV export](../crates/core-engine/src/ledger_engine/export.rs)

### Posting history

Open a posted transaction to review a financial correction or void. Provide a reason, review the change, and confirm it. To record a refund, open an uncategorized posted credit and optionally select the original purchase. Purchases and refunds stay traceable, and the combined linked refunds cannot exceed the purchase. Select **Set starting balance** on a liquid account to review an opening snapshot. The snapshot starts balance history on its effective date while earlier postings remain in activity. [Posting corrections](../crates/core-engine/src/ledger_engine/corrections.rs)

## Planning

| Feature | Purpose and key rules | How to use it | Code |
| --- | --- | --- | --- |
| Payday calendar | Use a financial timezone and an adjustable payday to define planning cycles. Changes leave the active cycle intact. | Open **Settings → Payday calendar**. Select a timezone, payday, and weekend policy. A scheduled change can be replaced or cancelled before it takes effect. | [Calendar engine](../crates/core-engine/src/planning/calendar.rs) |
| Payday-cycle budgets | Track IDR spending by cycle, including category splits and linked refunds. Copy a previous plan to a new cycle with no rollover of money or balances. | Open **Budgets → Payday cycle**. Add a named limit and assign expense categories. Review remaining spending, pace, and the projection from day three. | [Cycle budgets](../crates/core-engine/src/planning/cycle_budgets.rs) |
| Monthly budgets | Compare category limits with actual and projected expenses. Monthly overrides take precedence over default limits. Amounts include spending in the budget’s currency. | Open **Budgets → Monthly** to set limits, copy a month, or review historical suggestions. | [Monthly budgets](../crates/core-engine/src/planning/budgets.rs) |
| Cash plan | Compare liquid IDR cash with obligations, stable expected income, and a scenario using selected uncertain income. Expected income on a due date affects the closing forecast after due payments. | Open **Cash plan**. Add due dates and amounts, link eligible posted transactions when they clear, and use **Recurring amounts** for staged changes. | [Cash plan engine](../crates/core-engine/src/planning/cash_plan.rs) |
| Goals | Track a positive target and optional deadline. Progress can come from Manual, Account, Asset, Asset group, or Net worth. | 1. Open **Goals → New goal**. 2. Set the amount, currency, tracking source, and optional deadline. 3. Review progress and pace. Edit the state as the goal changes. | [Goals route](../src/routes/goals/+page.svelte) · [Goals engine](../crates/core-engine/src/planning/goals.rs) |
| Loans | Show amortization by principal, interest, payment, and remaining balance. Terms support Price and SAC, with effective annual, nominal annual, or monthly rates. | 1. Create a **Loan** account. 2. Open it and select **Set loan terms**. 3. Enter principal, rate, term and first payment date. Save and review installments under **Loan payment plan**. | [Account detail](../src/routes/accounts/%5Bid%5D/+page.svelte) · [Account editor](../src/lib/views/AccountEditor.svelte) · [Loan engine](../crates/core-engine/src/planning/loans.rs) |
| Assets and asset groups | Track holdings, trades and valuation history, and project percentage or fixed growth on daily, weekly, monthly, or yearly intervals. A new valuation can reset the projection anchor. | 1. Open **Assets** and add an asset with its kind and currency. 2. Use **Add valuation** or **Record trade** for a buy, sell, or split. Add growth rules on the asset form. 3. Open or create an asset group to review combined holdings and value. | [Assets](../src/routes/assets/+page.svelte) · [Asset detail](../src/routes/assets/%5Bid%5D/+page.svelte) · [Asset group](../src/routes/assets/groups/%5Bid%5D/+page.svelte) · [Assets engine](../crates/core-engine/src/planning/assets.rs) |
| Recurring entries | A schedule can run weekly, biweekly, monthly, quarterly, semiannually, or yearly. Rules support skip-first, count limits, month-end clamping, and weekend adjustments. | 1. Open **Recurring → New schedule**. 2. Set the account, amount, dates, frequency, and optional generation rules. 3. Use **Occurrences** to review upcoming rows and generate, post, skip, match, or unmatch eligible occurrences. Archive a schedule when it ends. | [Recurring route](../src/routes/recurring/+page.svelte) · [Recurring engine](../crates/core-engine/src/planning/recurring.rs) |
| Financial calendar | See booked activity and optional projections in a monthly view. Bank and reporting dates give different views of the same entries. | 1. Open **Calendar** and choose a month and date basis. 2. Pick an account or show all accounts. 3. Turn on **Include expected transactions** when planning ahead. | [Calendar route](../src/routes/calendar/+page.svelte) · [Calendar calculation](../crates/core-engine/src/ledger_engine/reports.rs) |

## Rules

### Categorization rules

Rules run in priority order and can set a category or payee, replace a description, append a note, or mark an entry ignored. Tags are managed by transaction bulk actions.

1. Open **Rules → New rule**. Choose the conditions and actions. Combine conditions with All or Any and add a nested condition group if needed.
2. Set whether the rule may apply to existing records or overwrite a category. Use **Dry run** or **Preview apply** to review candidates and manual edits, then **Apply preview**.
3. Use the up and down buttons to reorder rules. Export a rule pack for reuse, or import one using reject, skip matching names, or replace matching rules.

**Code** · [Rules route](../src/routes/rules/+page.svelte) · [Rule conditions and actions](../crates/core-engine/src/rules/mod.rs) · [Rule storage and previews](../crates/core-engine/src/rules/storage.rs)

## Reports

| Report | What it answers |
| --- | --- |
| Net worth | What you own minus your debts and loans |
| Income vs. expense | How much money came in and went out |
| Cash flow | How recorded and expected transactions affect your money |
| Category trends | How spending and income change by category |

1. Open **Reports** and choose a view. Set the date range, account, category, or grouping where available. Use **Show chart** or **Hide chart** to change the display.
2. Totals use the activity’s **Primary currency**. Open **Settings → Activity settings** to review it. Existing accounts in other currencies still show their original amounts. Those amounts are left out of primary-currency totals.
3. Export filtered rows as CSV from **Transactions**. Statement import and exchange-rate tools are paused. Previously imported activity and exchange-rate records remain in existing databases.

**Code** · [Report route](../src/routes/reports/%5Bkind%5D/+page.svelte) · [Activity settings](../src/routes/settings/+page.svelte) · [Reports engine](../crates/core-engine/src/ledger_engine/reports.rs)

## Backups and restore

Backups are `.fgpsbackup` archives with a separate passphrase. Argon2id derives the key and AES-256-GCM encrypts and authenticates the archive. Each archive contains a database snapshot and eligible attachments. Credentials stored in the vault stay with the installation.

1. Open **Backups → Create backup**. Enter a backup passphrase. Choose app storage or **Choose file…** to export a copy.
2. Open **Edit schedule** to choose daily or weekly backups with a remembered backup passphrase. Scheduled jobs run while the app is open and the ledger is unlocked. Native scheduled backups keep up to ten successful generations and can use an approved directory. Web keeps up to ten managed archives per ledger and can also copy scheduled backups to an approved directory when the browser supports directory picking. A schedule can specify an age limit.
3. Use **Your activity → Restore from backup** or a managed archive's Restore action. Choose the archive, enter its passphrase, and select **Verify backup**. Review what will be restored.
4. Restore as new activity or choose **Replace the open activity** when available. Confirm replacement and provide the safety backup credential when requested. Set the restored ledger's protection and new credential if you choose encryption.

**Code** · [Backups route](../src/routes/settings/backups/+page.svelte) · [Launcher](../src/routes/+page.svelte) · [Restore dialog](../src/lib/views/RestoreFlow.svelte) · [Archive format](../crates/core-engine/src/archive.rs) · [Native backup and schedule](../src-tauri/storage/backups.rs) · [Native scheduling](../src-tauri/storage/schedule.rs) · [Web archive adapter](../src/lib/drivers/web/backup.ts)

## Workspace settings, dashboards, and shortcuts

| Feature | Purpose and usage | Code |
| --- | --- | --- |
| Dashboard | Choose an as-of date and account. Review account balances, available credit, net worth, income vs. expense, and cash-flow widgets. Select **Customize** to add, remove, or reorder widgets, then **Save layout**. | [Dashboard](../src/routes/dashboard/+page.svelte) · [Dashboard data](../crates/core-engine/src/ledger_engine/reports.rs) · [Layout preferences](../crates/core-engine/src/preferences.rs) |
| Preferences | Under **Settings**, set activity name, primary currency, an IANA timezone override, and default card accounting. Set theme, date and number formats, or optional features. Disabling an optional module preserves its records. | [Settings](../src/routes/settings/+page.svelte) · [App preferences](../crates/core-engine/src/preferences.rs) · [Ledger settings](../crates/core-engine/src/ledger_engine/accounts.rs) |
| Privacy mode | Select **Hide amounts** in the workspace header when sharing your screen. It masks monetary displays. Select **Amounts hidden** to reveal them again. Activity protection is managed separately. | [Workspace layout](../src/routes/+layout.svelte) · [Privacy store](../src/lib/stores/privacy.svelte.ts) · [Preferences](../crates/core-engine/src/preferences.rs) |
| Command palette | Open **Search** or press **Ctrl+K** on Windows/Linux or **Cmd+K** on macOS. Type to find actions, ledgers, or records. Use Up or Down and Enter to choose. Use Escape to close. | [Workspace layout](../src/routes/+layout.svelte) · [Palette](../src/lib/components/Palette.svelte) · [Global search](../crates/core-engine/src/ledger_engine/reports.rs) |
| Background jobs | Watch the job indicator for long-running exports, backup and restore, and other jobs. Choose Cancel when a job is still cancellable. | [Workspace layout](../src/routes/+layout.svelte) · [Job store](../src/lib/stores/jobs.svelte.ts) · [Core command dispatch](../crates/core-engine/src/commands.rs) · [Native jobs](../src-tauri/ipc/jobs.rs) · [Web jobs](../src/lib/drivers/web/jobs.ts) |

## Driver operations for contributors

The driver contract also exposes operations used directly by integrations or tests. The related routes below provide the user-facing workflows.

| Operation | Where to start |
| --- | --- |
| Delete an account or shared credit line | The UI manages accounts and credit lines under [Accounts](../src/routes/accounts/+page.svelte). Check the constraints in [accounts.rs](../crates/core-engine/src/ledger_engine/accounts.rs) before using `accountDelete` or `cardLineDelete`. |
| Delete a linked transfer pair | The [transaction editor](../src/routes/transactions/%5Bid%5D/+page.svelte) exposes unlinking. Pair deletion is a separate driver operation implemented by the [transfer engine](../crates/core-engine/src/ledger_engine/transfers.rs). |
| Allocate a posted transaction to a loan payment | The [account detail route](../src/routes/accounts/%5Bid%5D/+page.svelte) shows loan terms and the amortization schedule. Payment allocation is available through `loanPaymentAllocate` in the [loan engine](../crates/core-engine/src/planning/loans.rs). |

Use the shared [driver contract](../src/lib/drivers/contracts.ts) for serialized inputs and outputs.
