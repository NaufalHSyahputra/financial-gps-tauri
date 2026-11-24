# V1 release sign-off

**APPROVED FOR V1 RELEASE.** All 106 applicable requirements passed. The other 14 were confirmed outside V1 scope. No requirements remain blocked.

Qualification ran on 22 November 2026. The reference host was CachyOS x86-64, Linux 7.3.4-1-cachyos, AMD Ryzen 7 5800H, 19 GiB RAM, Intel SSDPEKNW512GZL NVMe and Btrfs. Toolchain versions were Rust/Cargo 1.100.0, Node.js 26.13.0 and WebKitGTK 2.54.3.

## Artifacts and gates

| Artifact | Path | Bytes | SHA-256 |
| --- | --- | ---: | --- |
| Linux amd64 executable | [target/release/qualified/financial-gps](../target/release/qualified/financial-gps) | 47,932,352 | `2c06e1f89258e4c2694421e23e854c1d1152775b2c5a21c4d590227c5eb52647` |
| Linux amd64 Debian bundle | [Financial GPS_0.1.0_amd64.deb](../target/release/bundle/deb/Financial%20GPS_0.1.0_amd64.deb) | 12,301,102 | `3e5b9817ca12a3cd031d205de8fb5387f724531cbf596792edfa10da34e30376` |
| Static frontend | [build/index.html](../build/index.html) | — | — |

- `npm run tauri -- build --bundles deb` produced the native executable and package with zero compiler or bundler warnings. The qualified executable is a byte-identical copy extracted from the package. It is kept separately because Cargo test builds can replace its default output binary. GTK reported software rendering fallback on this host. The bundle contains one application executable.
- `npm run check` reported zero TypeScript and Svelte errors or warnings. `npm run build` emitted the adapter-static app as part of the full Tauri build.
- `cargo clippy --all-targets -- -D warnings` passed.
- `cargo test --release` passed 61 tests with zero failures. The live Frankfurter test remained ignored.
- ELF linkage on the qualification host includes `libcrypto.so.4`. This package was qualified on that host. Installation on an independent Debian system and macOS/Windows packaging were outside this run.

## Performance

The fixture inserts rows 1 through 250,000 deterministically across 50 accounts using the seed in `representative_250k_ledger_uses_composite_date_and_fts_indexes`. It includes skewed account activity, USD/IDR/KWD, pending and projected rows, a card statement, sealed splits, a linked transfer and indexed tags/text. Use `FINANCIAL_GPS_PACKAGED_FIXTURES=<directory>` with the benchmark to preserve equivalent plain and SQLCipher profiles for the package timing. The fixture passphrase is synthetic. SQLite 3.54.1/SQLCipher 4.20.0 used WAL, foreign keys on, full sync, a 5,000 ms busy timeout and a bounded one-writer/two-reader pool. Every open validated the schema, migration checksum, ledger identity, revision and connection settings before Ready. Full-database `PRAGMA quick_check` ran synchronously on an unclean reopen, when a receipt was invalid, and for migrations and restore. The benchmark's verified clean reopen moved the full scan to a session-scoped read-only worker. Restore retained `PRAGMA integrity_check` and foreign-key content validation.

| Startup measurement | Plain (ms) | SQLCipher (ms) | Gate |
| --- | ---: | ---: | --- |
| Fresh manager and clean reopen through first 200-row service page | 45.932 | — | Pass for the plain service fixture |
| New packaged process to interactive launcher | 928.859 | 402.918 | Pass |
| Packaged open or credential submit to populated dashboard | 443.650 | 418.389 | Pass |
| Packaged dashboard to first Transactions page | 77.460 | 80.086 | Pass |
| Packaged open or credential submit through first Transactions page | 575.623 | 552.661 | Pass |

Packaged timing used a new process and a fresh renderer profile with the original checkpointed ledger paths. Copying a database invalidates the inode-bound receipt. The process-to-launcher measurement starts at process spawn. The open interval starts at the Open click or credential submit and excludes typing. The debugger adapter exposed the packaged webview over localhost for observation. The shipping build kept the inspector disabled. OS caches were warm or uncontrolled after fixture preparation. The renderer loaded the normal 150-row first Transactions page. The controlled query measurements used 200 rows and measured dispatch to result delivery without rendering each response.

| Filter | Rust p95 (ms) | Packaged IPC plain p95 (ms) | Packaged IPC SQLCipher p95 (ms) | Rows | Payload (bytes) | Plan evidence |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Account + date | 40.864 | 17 | 51 | 200 | 103,109 | `tx_account_bank`, running balance on |
| Reporting category | 3.462 | 4 | 3 | 200 | 96,917 | `tx_reporting`, `split_category` |
| Status + date | 6.075 | 5 | 3 | 200 | 96,088 | `tx_bank` |
| Statement | 1.666 | 4 | 2 | 200 | 98,118 | `tx_statement` |
| Indexed tag | 28.643 | 16 | 47 | 200 | 94,871 | `tag_lookup`, UUID lookup, temporary sort |

Each p95 is the 95th ordered sample of 100 requests for that class and mode. Connections warmed naturally, and the measured IPC path used no UI page cache. FTS5 passed its virtual-index plan assertion. The protected startup had previously spent 3,904 ms in unlock and 173 ms in the dashboard IPC. SQLCipher already used a raw 32-byte key through `sqlite3_key`. The implementation now drops the zeroizing hex buffer immediately after that call, opens foreground readers on demand, and moves the full scan off a verified clean-open path. Each packaged trial confirmed verification was pending at first paint and completed before clean close. The release tests also damaged a non-schema B-tree page after a clean open and verified that the worker emitted a scoped integrity signal, blocked subsequent reads and writes, and withheld the next receipt. A crash or a missing, modified or copied receipt still takes the synchronous recovery path. Cold kernel-cache and concurrent checkpoint/import stress measurements are outside the timings above.

## Linux runtime qualification

- The package booted at `tauri://localhost`. The production webview reported enforced CSP violations for an inline script and an unapproved connection. The source review reconciled 146 generated commands with the local main-window capability. No V2 or SaaS commands, routes, or remote origins were found. The lone production network adapter makes explicit, fixed-host HTTPS Frankfurter requests.
- The extracted package ran inside `unshare -rn` with loopback enabled for the local test inspector and no external interface or route. Encrypted ledger creation, account and transaction CRUD, native-granted CSV and OFX preview/commit, calendar, exact USD 10.00 reporting, attachment add/rename/viewer dispatch/save-as and encrypted backup succeeded. An explicit opt-in FX attempt returned `Offline` while subsequent ledger reads succeeded. A separate 97,000-row preview exposed row progress and rendered frames before cancellation. It left the two imported rows intact.
- The rebuilt binary used host KWallet via `org.freedesktop.secrets` to enroll and retrieve a disposable SQLCipher key. Quick unlock worked without a passphrase. Revocation removed the item from D-Bus search, after which manual unlock worked. The app profile and test/application logs contained no plaintext passphrase sentinel. The key stays in the OS wallet using versioned text encoding compatible with the host service.
- The rebuilt package repeated clean-install restore in a second process with fresh XDG data/config/cache directories and no existing ledger registry. The external Argon2id and AES-256-GCM archive authenticated and restored two transactions and one attachment into a new protected ledger with a new identity. Previous native smoke also verified confirmed replacement creates a separate safety archive.
- The production migration runner validates version and checksums. The simulated irreversible upgrade exercised an exclusive writer, completed WAL checkpoint, synced pre-upgrade recovery copy and atomic SQL rollback after a deliberate failure. Both plain and SQLCipher fixtures retained data and the schema version. A successful simulated upgrade then advanced the version. The recovery copy preserves the source database encoding and wrapper. Keep the original attachment directory with it. V1 ships schema 1 and no destructive upgrade.
- Broader fault coverage comes from the release rollback and reopen tests, attachment recovery tests, wrong-key/corruption and scope-isolation tests. The runtime tests used disposable profiles and files. Coverage is scoped to the Linux package and reference workload above.

## Requirements matrix

**PASS** means verified implementation evidence from this qualification or the prior backend and frontend gates. **EXCLUDED** means the deferred or SaaS-only surface was checked for absence. **BLOCKED** means the release budget was missed. The matrix covers all 100 functional and 20 nonfunctional requirements. Earlier detailed test ownership is available in the [Phases 1–4 validation report](PHASE1_4_VALIDATION_REPORT.md).

| Requirement | Status | Qualification evidence |
| --- | --- | --- |
| FR-001 | PASS | Installed payload ran with only loopback. Ledger CRUD, imports, reports, encrypted backup and clean-profile restore passed. |
| FR-002 | PASS | phase_three_commands_enforce_scope_and_round_trip_financial_strings |
| FR-003 | PASS | ledger_midnight_and_changed_system_zone_are_resolved_per_context. The fixture injects two OS-zone results and crosses midnight in an IANA ledger override. |
| FR-004 | PASS | Packaged SQLCipher ledger enrolled and retrieved a disposable key through KWallet Secret Service. Revocation and manual unlock passed. |
| FR-005 | PASS | Fresh profile starts with all six optional modules disabled. The IPC test explicitly enables them, disables them and verifies the original rows on re-enable. |
| FR-006 | PASS | launcher_commands_round_trip_through_tauri_and_enforce_acl. fixed-host HTTPS FX client has no redirects. |
| FR-007 | PASS | phase_three_commands_enforce_scope_and_round_trip_financial_strings. negative scope audit. |
| FR-008 | PASS | balances_archives_pending_authorizations_and_running_balance_across_display_filters |
| FR-009 | PASS | balances_archives_pending_authorizations_and_running_balance_across_display_filters |
| FR-010 | PASS | balances_archives_pending_authorizations_and_running_balance_across_display_filters |
| FR-011 | PASS | balances_archives_pending_authorizations_and_running_balance_across_display_filters |
| FR-012 | PASS | shared_credit_lines_include_archived_debt_once_and_cap_available_credit. close_date_exclusion_and_rust_month_end_clamping |
| FR-013 | PASS | balances_archives_pending_authorizations_and_running_balance_across_display_filters |
| FR-014 | PASS | balances_archives_pending_authorizations_and_running_balance_across_display_filters |
| FR-015 | PASS | balances_archives_pending_authorizations_and_running_balance_across_display_filters |
| FR-016 | PASS | phase_three_commands_enforce_scope_and_round_trip_financial_strings. split_edit_rollback_unicode_tags_bulk_and_pnl_calculation |
| FR-017 | PASS | csv_identity_review_and_card_reconciliation_preserve_manual_fields_and_exposure checks ordinary edits to date, amount, currency and direction. |
| FR-018 | PASS | transfer_link_protects_leg_structure_and_reports_dates_independently |
| FR-019 | PASS | transfer_link_protects_leg_structure_and_reports_dates_independently |
| FR-020 | PASS | booking_dates_rebuild_when_statement_closing_and_due_dates_change. calendar_and_flow_keep_booked_and_reporting_timing_separate |
| FR-021 | PASS | future_reporting_override_leaves_balance_and_waits_for_actual_pnl |
| FR-022 | PASS | split_assembly_requires_positive_matching_lines_and_a_sealed_commit. budgets_use_economic_share_and_goals_use_deadline_boundaries |
| FR-023 | PASS | split_assembly_requires_positive_matching_lines_and_a_sealed_commit. budgets_use_economic_share_and_goals_use_deadline_boundaries |
| FR-024 | PASS | split_edit_rollback_unicode_tags_bulk_and_pnl_calculation |
| FR-025 | PASS | Generated filter contract and transaction filter panel reviewed. Release workload covers the indexed paths and statement-ID precedence. Other supported filters have targeted backend coverage. Exhaustive combinations were outside this run. |
| FR-026 | PASS | nullable_sort_keeps_tied_rows_stable_and_bulk_respects_scope_revision_and_confirmation checks a nullable sort column with a stable ID tie-breaker across pages. |
| FR-027 | PASS | nullable_sort_keeps_tied_rows_stable_and_bulk_respects_scope_revision_and_confirmation checks group assignment, flag, confirmed delete and stale previews. |
| FR-028 | PASS | portable_csv_export_preserves_native_units_and_neutralizes_text_formulas |
| FR-029 | PASS | cross_currency_transfer_validation_pair_effects_and_unlinking |
| FR-030 | PASS | future_reporting_override_leaves_balance_and_waits_for_actual_pnl |
| FR-031 | PASS | installment_schedule_preview_applies_only_the_selected_scope |
| FR-032 | PASS | Packaged privacy control masked a known balance, chart and accessibility labels. Phase 5 browser list, tooltip and export dispatch checks and exact backend CSV export cover unchanged raw amounts. |
| FR-033 | PASS | cross_currency_transfer_validation_pair_effects_and_unlinking |
| FR-034 | PASS | cross_currency_transfer_validation_pair_effects_and_unlinking |
| FR-035 | PASS | Transfer creation and scoped read round-trip the actual rate, date and provenance. The linked-leg panel exposes the saved quote in the desktop app. |
| FR-036 | PASS | two_imported_postings_link_as_one_transfer_without_recreating_either_leg verifies reuse, provenance and P&L exclusion. |
| FR-037 | PASS | statement_formats_and_localized_csv. phase_three_commands_enforce_scope_and_round_trip_financial_strings. fx_csv_preview_is_read_only_and_undo_reverts_the_rate |
| FR-038 | PASS | statement_formats_and_localized_csv. phase_three_commands_enforce_scope_and_round_trip_financial_strings. fx_csv_preview_is_read_only_and_undo_reverts_the_rate |
| FR-039 | PASS | idr_template_export_import_is_equivalent_in_json_and_toml |
| FR-040 | PASS | csv_identity_review_and_card_reconciliation_preserve_manual_fields_and_exposure tests normalized fallback multiplicity and CSV opt-out. |
| FR-041 | PASS | csv_identity_review_and_card_reconciliation_preserve_manual_fields_and_exposure verifies preservation of description, note, payee, category and reporting override. |
| FR-042 | PASS | staged_import_persists_identity_and_blocks_undo_after_dependency. statement_reconciliation_promotes_and_undo_restores_a_generated_occurrence |
| FR-043 | PASS | statement_formats_and_localized_csv. phase_three_commands_enforce_scope_and_round_trip_financial_strings. fx_csv_preview_is_read_only_and_undo_reverts_the_rate |
| FR-044 | PASS | Negative scope scan, IPC inventory and launcher_commands_round_trip_through_tauri_and_enforce_acl |
| FR-045 | EXCLUDED | Deferred to V2. Route, command and production endpoint inventory found no exposed surface. |
| FR-046 | EXCLUDED | Deferred to V2. Route, command and production endpoint inventory found no exposed surface. |
| FR-047 | PASS | csv_identity_review_and_card_reconciliation_preserve_manual_fields_and_exposure proves one posted row with unchanged balance, debt and available credit. |
| FR-048 | PASS | Negative scope scan, IPC inventory and launcher_commands_round_trip_through_tauri_and_enforce_acl |
| FR-049 | EXCLUDED | SaaS-only feature excluded from the desktop route, command and endpoint inventory. |
| FR-050 | PASS | rule_pack_collision_policies_and_priority_round_trip tests serialization, priority order, reject, skip and replace. |
| FR-051 | PASS | rule_ast_enforces_shape_and_regex_bounds. rule_preview_binds_manual_override_policy_and_existing_category_opt_in |
| FR-052 | PASS | rule_ast_enforces_shape_and_regex_bounds. rule_preview_binds_manual_override_policy_and_existing_category_opt_in |
| FR-053 | PASS | rule_preview_binds_manual_override_policy_and_existing_category_opt_in |
| FR-054 | PASS | rule_preview_binds_manual_override_policy_and_existing_category_opt_in |
| FR-055 | PASS | budgets_use_economic_share_and_goals_use_deadline_boundaries |
| FR-056 | PASS | budget_actual_matches_fx_pnl_and_projected_placeholders_count_once matches reliable P&L actuals and verifies projected and remaining totals. |
| FR-057 | PASS | budgets_use_economic_share_and_goals_use_deadline_boundaries |
| FR-058 | PASS | budget_copy_rolls_back_on_collision_and_requires_positive_rounded_limits |
| FR-059 | PASS | six_recurring_frequencies_skip_first_count_eligible_and_preserve_edit_floor exercises all six frequencies. |
| FR-060 | PASS | calendar_clamps_then_shifts_and_amortization_conserves_principal. statement_reconciliation_promotes_and_undo_restores_a_generated_occurrence |
| FR-061 | PASS | calendar_clamps_then_shifts_and_amortization_conserves_principal. statement_reconciliation_promotes_and_undo_restores_a_generated_occurrence |
| FR-062 | PASS | six_recurring_frequencies_skip_first_count_eligible_and_preserve_edit_floor verifies the old posting survives and historical virtual rows are suppressed after the edit. |
| FR-063 | PASS | loan_contract_limit_and_positive_rate_conserve_all_minor_units. calendar_clamps_then_shifts_and_amortization_conserves_principal |
| FR-064 | PASS | loan_contract_limit_and_positive_rate_conserve_all_minor_units. calendar_clamps_then_shifts_and_amortization_conserves_principal |
| FR-065 | PASS | goal_sources_use_ledger_values_and_asset_growth_resets_on_new_valuation checks all five sources. |
| FR-066 | PASS | goal_sources_use_ledger_values_and_asset_growth_resets_on_new_valuation and goal_pace_classifies_exactly_five_and_ten_percentage_point_boundaries. |
| FR-067 | PASS | goal_sources_use_ledger_values_and_asset_growth_resets_on_new_valuation tests all four cadences and a new valuation anchor. |
| FR-068 | PASS | weighted_assets_apply_splits_first_and_allocate_final_residue. group implementation and binding inspection |
| FR-069 | EXCLUDED | Deferred to V2. Route, command and production endpoint inventory found no exposed surface. |
| FR-070 | PASS | Packaged UI opened all four report kinds and navigated from a report period into Transactions. Backend reports verified exact amounts and inclusion. |
| FR-071 | PASS | Packaged UI saved a custom dashboard layout. Packaged IPC round-tripped layout independently of privacy. Report filters and dual calendar basis are implemented and exercised by report/calendar tests. |
| FR-072 | PASS | calendar_and_flow_keep_booked_and_reporting_timing_separate. statement_reconciliation_promotes_and_undo_restores_a_generated_occurrence |
| FR-073 | PASS | calendar_and_flow_keep_booked_and_reporting_timing_separate. statement_reconciliation_promotes_and_undo_restores_a_generated_occurrence |
| FR-074 | PASS | currency_scales_exact_inverse_cross_and_missing_rates. fx_csv_preview_is_read_only_and_undo_reverts_the_rate |
| FR-075 | PASS | currency_scales_exact_inverse_cross_and_missing_rates. fx_csv_preview_is_read_only_and_undo_reverts_the_rate |
| FR-076 | PASS | Packaged conversion rendered the 1:1 missing-rate badge. Rust tests verify native amounts and absence of an inserted synthetic rate. |
| FR-077 | PASS | phase_three_commands_enforce_scope_and_round_trip_financial_strings verifies explicit opt-in. live_provider_smoke is opt-in. |
| FR-078 | PASS | Packaged add, list, rename, byte-identical save-as and native viewer dispatch passed. Rust attachment lifecycle tests cover removal and interrupted publication. |
| FR-079 | PASS | attachment_count_byte_limit_and_signature_are_enforced |
| FR-080 | PASS | scheduled_backup_catches_up_after_unlock_and_retains_ten_in_selected_directory verifies delivery with a scoped native directory grant and a ledger-specific archive directory. |
| FR-081 | PASS | scheduled_backup_catches_up_after_unlock_and_retains_ten_in_selected_directory verifies catch-up, encrypted archive readback and ten-generation pruning with an injected secret store and clock. |
| FR-082 | PASS | encrypted_archive_round_trip_preserves_postings_and_attachments |
| FR-083 | PASS | Fresh-profile packaged restore preview authenticated the archive and verified format version, entity counts and warnings before confirmation. |
| FR-084 | PASS | plain_wal_backup_and_confirmed_replacement_keep_a_safety_archive. restart_rolls_back_a_partially_swapped_portable_ledger |
| FR-085 | PASS | Negative scope scan and command inventory |
| FR-086 | EXCLUDED | Deferred to V2. Route, command and production endpoint inventory found no exposed surface. |
| FR-087 | EXCLUDED | Deferred to V2. Route, command and production endpoint inventory found no exposed surface. |
| FR-088 | EXCLUDED | Deferred to V2. Route, command and production endpoint inventory found no exposed surface. |
| FR-089 | EXCLUDED | Deferred to V2. Route, command and production endpoint inventory found no exposed surface. |
| FR-090 | EXCLUDED | Deferred to V2. Route, command and production endpoint inventory found no exposed surface. |
| FR-091 | EXCLUDED | Deferred to V2. Route, command and production endpoint inventory found no exposed surface. |
| FR-092 | PASS | exact_and_percentage_shared_expenses_reconcile_cross_currency_only_with_provenance verifies exact shares, percentage rounding and native balances. |
| FR-093 | PASS | exact_and_percentage_shared_expenses_reconcile_cross_currency_only_with_provenance checks overpayment rejection, mandatory FX provenance and P&L exclusion. |
| FR-094 | EXCLUDED | Deferred to V2. Route, command and production endpoint inventory found no exposed surface. |
| FR-095 | EXCLUDED | Deferred to V2. Route, command and production endpoint inventory found no exposed surface. |
| FR-096 | EXCLUDED | Deferred to V2. Route, command and production endpoint inventory found no exposed surface. |
| FR-097 | EXCLUDED | SaaS-only feature excluded from the desktop route, command and endpoint inventory. |
| FR-098 | PASS | phase_three_commands_enforce_scope_and_round_trip_financial_strings. bounded_pool_and_lock_drain_keep_commit_bound_to_original_ledger |
| FR-099 | PASS | Packaged Ctrl+K opened the command palette and focused its search field. Phase 5 browser smoke also verified keyboard operation. |
| FR-100 | PASS | Packaged dashboard layout save and scoped readback passed. Changing app privacy retained the same ledger layout. |
| NFR-001 | PASS | exact_money_and_gregorian_dates. migration_strict_currency_scales_and_connection_policy. currency_scales_exact_inverse_cross_and_missing_rates. Monetary source review. |
| NFR-002 | PASS | exact_money_and_gregorian_dates. migration_strict_currency_scales_and_connection_policy. currency_scales_exact_inverse_cross_and_missing_rates. Monetary source review. |
| NFR-003 | PASS | Packaged network-namespace run used only loopback. Core flows completed, and an expressly enabled FX request returned Offline while reads remained available. |
| NFR-004 | PASS | migration_strict_currency_scales_and_connection_policy. bounded_pool_and_lock_drain_keep_commit_bound_to_original_ledger |
| NFR-005 | PASS | encrypted_archive_round_trip_preserves_postings_and_attachments. plain_wal_backup_and_confirmed_replacement_keep_a_safety_archive |
| NFR-006 | PASS | Release rollback fixtures cover split/transfer integrity, import dependency rollback, attachment staging and replacement recovery. SQLite storage-full injection verifies transaction rollback and retry. |
| NFR-007 | PASS | Host KWallet Secret Service enrollment, quick unlock, revocation and manual unlock passed. D-Bus search confirmed removal. Passphrase sentinel absent from the isolated app profile and its logs. |
| NFR-008 | PASS | Packaged CSP enforcement and the 145-command outbound/scope review passed. No automatic provider initialization or telemetry surfaced in the offline run. |
| NFR-009 | PASS | bounded_pool_and_lock_drain_keep_commit_bound_to_original_ledger. grants_are_single_use_window_and_purpose_bound_and_aliases_are_rejected |
| NFR-010 | PASS | Packaged process to interactive launcher took 928.859 ms plain and 402.918 ms protected. Clean open through the populated dashboard and first rendered Transactions page took 575.623 ms plain and 552.661 ms protected against the 3,000 ms budget. |
| NFR-011 | PASS | Each 200-row filter class stayed below 500 ms p95 over 100 requests in Rust and packaged IPC, including SQLCipher. File cap and job lifecycle tests passed. |
| NFR-012 | PASS | Packaged 97,000-row preview emitted progress, rendered frames while active, and cancelled without writes. Release import tests cover commit atomicity and undo. |
| NFR-013 | PASS | A second packaged process started with fresh XDG directories and no registry. It authenticated and restored a separate portable archive with postings and an authenticated attachment. |
| NFR-014 | PASS | The same pre-upgrade runner passed a simulated destructive v2 migration and a failure after DDL for plain and SQLCipher ledgers. Tests verified committed WAL data, rollback, version and checksum. |
| NFR-015 | PASS | attachment_recovery_adopts_staging_and_finishes_deletions |
| NFR-016 | PASS | Phase 5 browser smoke covered keyboard table focus, a 360 px layout and navigation. Packaged keyboard palette focus and native controls passed. |
| NFR-017 | PASS | Known balances, chart marks and accessible labels masked in rendered tests. BigInt format tests and Rust export tests preserved exact raw amounts. |
| NFR-018 | PASS | bindings_match_command_contract. domain and integration suites |
| NFR-019 | PASS | Packaged webview blocked a non-allowlisted connection. The Rust provider is opt-in and restricts requests to its fixed HTTPS host, with no redirects or proxy. |
| NFR-020 | PASS | encrypted_archive_round_trip_preserves_postings_and_attachments. plain_wal_backup_and_confirmed_replacement_keep_a_safety_archive |
