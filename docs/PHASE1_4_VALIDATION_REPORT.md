# Phases 1–4 validation report

**Backend Phase 5 gate ready for sign-off.** Both requested backend implementation gaps are fixed, the listed backend vectors pass, and the release workload meets the service-level targets. Phase 5 requires explicit approval.

## Baseline and scope

Reviewed the [frozen requirements](REQUIREMENTS_SPEC.md), the 24 product decisions, the scope exclusions and the [architecture](ARCHITECTURE_SPEC.md) against the Rust, SQLite and Tauri application. The coverage matrix lists every functional and nonfunctional requirement, including intentional exclusions. The production surface contains local ledger services and an opt-in Frankfurter FX endpoint. Checks of Rust sources, SQL, command registration and capabilities found no deferred invoicing, tax, AI, MCP, bank-sync, live market-data or executable-plugin surface and no permanent hosted authentication, tenant, remote storage, server-worker or public-sharing surface. The Tauri capability is local and assigned to the main webview.

The initial SQL differs from the embedded schema only in the seeded ISO currencies and the explicit unindexed transaction UUID in FTS5, both required by the implementation baseline. Reviewed SQLite strict typing, stored reporting-date precedence, deferred sealed split validation, transfer leg protection, and the configured WAL, foreign keys, full sync and five-second busy timeout. The lifecycle tests exercise generation rotation and a concurrent writer during lock. The financial calculation source scan found no binary floating-point arithmetic. The only floating-point conversion is for printing elapsed time in the benchmark fixture. The runtime source scan found no application stubs using Rust's unimplemented or todo macros and no fallible runtime unwrap calls. Internal invariant assertions and the executable startup error path still use expect.

All 141 commands in Section 7.2 are registered with Specta, allowed locally and present in the generated bindings. The existing account_delete and fx_convert commands bring the total to 143. The generated bindings also include ledger-job-progress.

## Remediated defects and added evidence

- Removed eight strict Clippy diagnostics by fixing the UTF-16 decoding iteration and CAMT argument, boxing a large import undo variant, simplifying an import row type, using an integer parity operation and grouping maintenance credentials and replacement inputs.
- A transaction now stays out of actual P&L and its drill-through until both bank date and reporting date have arrived. The regression keeps the booked account balance and posting ID intact while the future reporting date remains visible in ordinary search.
- Added a regression assertion for the specified statement-ID precedence over a supplied date range. Hashtag filtering starts from matching tokens in the existing tag index. The schema migration and checksum were preserved.
- Retroactive rules now bind the override policy to the confirmed preview. The default preserves manual overrides. Explicit overwrite requires a preview made with that policy and the rule's category overwrite setting.
- Import preview now tracks parsed row counts in job status and coalesces window-targeted progress events. Existing cancellation and atomic commit paths remain in use.
- Restore preview now returns typed warnings for new-ledger identity and excluded device credentials. The SQLCipher archive test restores under a separate app-data root using the authenticated archive.
- Added golden tests for a positive-rate 1,200-month Price and SAC loan, exact 5- and 10-percentage-point goal pace boundaries, localized IDR JSON and TOML template round trips, attachment signature and size/count limits, and atomic budget-copy rollback.

## Backend sign-off work

- The optional-module guards use the stored application visibility flags. Existing local modules retain their defaults. Frankfurter still requires explicit ledger-level opt-in. Disabling a module blocks its direct commands without modifying its records. Guards also cover asset import writes, bulk group assignment, recurring background generation and provider refresh.
- A scheduled backup can use a selected folder through a purpose-bound PathGrant. The service stores the canonical path locally, keeps archives in a ledger-specific subdirectory, and lists those archives through the existing backup commands. Unlock wakes the maintenance worker. Tests exercise the same scheduler with a deterministic clock and an in-memory secret-store adapter.
- Existing ledger open runs SQLite quick_check and verifies foreign-key enforcement on each connection. It skips the separate database-wide foreign_key_check on normal open. Creation and restore continue to validate foreign-key contents. Archive restore also runs full integrity_check.
- Added the targeted backend fixtures recorded in the matrix, including generated occurrence edits, all goal sources and asset intervals, rule packs, protection of imported fields, nullable keyset pagination, and linked cross-currency settlements.

## Test and benchmark summary

- `cargo clippy --all-targets -- -D warnings` passed.
- `cargo test --all-targets` passed **56** tests with zero failures and 1 ignored. The live Frankfurter smoke test remains opt-in.
- `npm run check` passed with zero errors and zero warnings. Generated IPC binding drift was checked by the Rust suite.

```sh
FINANCIAL_GPS_BENCHMARK=1 cargo test --release --test phase_three representative_250k_ledger_uses_composite_date_and_fts_indexes -- --nocapture
```

A fresh ledger manager, normal open and validation, and the first 200-row page completed in **678.523 ms**, below the 3,000 ms backend threshold. Each query met the 500 ms p95 threshold.

| Filter | Index evidence | Release p95 (ms, 100 observations) |
| --- | --- | ---: |
| account + date | tx_account_bank | 22.519 |
| reporting category | tx_reporting and split_category | 1.704 |
| status + date | tx_bank | 3.440 |
| statement | tx_statement | 0.552 |
| indexed tag | tag_lookup and transaction UUID lookup | 11.305 |

The deterministic fixture has 250,000 transactions across 50 accounts. It includes skewed activity, multiple currency scales, posted/pending/projected transactions, splits, a card statement, a transfer, and indexed tags and text. The optimized integration test runs on Linux 7.3.4-1-cachyos, Ryzen 7 5800H, 19 GiB RAM and NVMe Btrfs. OS caches were warm. Timing includes the Rust service call and result assembly. The account/date query includes running balances. The tag plan also uses a temporary sort after index lookup. A previous debug run with the prior open path took 3,051.387 ms. The build profile and validation path both changed before the release measurement.

## Remaining release validation

- Phase 5 owns the command palette, rendered privacy and FX badges, full finance screens, and keyboard and display-scale checks.
- The measured scope is the backend service. Packaged cold process startup, renderer dispatch, real OS keychain behavior, and broader fault injection remain separate release qualification work. The matrix distinguishes these checks from the requested backend vectors.

## Requirements coverage matrix

PASS records positive implementation evidence or a verified scope exclusion. FIXED records a defect remediated and retested. PENDING identifies an evidence gap. GAP identifies remaining implementation work. The Phase 5 UI stays outside the implementation scope.

| ID | Status | Implementing module or command | Test or verification evidence |
| --- | --- | --- | --- |
| FR-001 | PASS | ipc::app_bootstrap, storage::ledger | launcher_commands_round_trip_through_tauri_and_enforce_acl |
| FR-002 | PASS | storage::preferences, app_preferences_set | phase_three_commands_enforce_scope_and_round_trip_financial_strings |
| FR-003 | PASS | Context::new, Clock | ledger_midnight_and_changed_system_zone_are_resolved_per_context. The fixture injects two OS-zone results and crosses midnight in an IANA ledger override. |
| FR-004 | PASS | crypto, LedgerManager::unlock, ledger_keychain_set | encrypted_ledger_rejects_wrong_passphrase_and_rotates_scopes_on_unlock. Uses a mock keychain. |
| FR-005 | FIXED | storage::preferences, Tauri module guards | phase_three_commands_enforce_scope_and_round_trip_financial_strings verifies hidden modules reject calls and re-enable with their original records. |
| FR-006 | PASS | ledger_engine::fx, local-only capability | launcher_commands_round_trip_through_tauri_and_enforce_acl. fixed-host HTTPS FX client has no redirects. |
| FR-007 | PASS | storage::registry, ledger_switch | phase_three_commands_enforce_scope_and_round_trip_financial_strings. negative scope audit. |
| FR-008 | PASS | ledger_engine::accounts, transactions, accounts_list | balances_archives_pending_authorizations_and_running_balance_across_display_filters |
| FR-009 | PASS | ledger_engine::accounts, transactions, accounts_list | balances_archives_pending_authorizations_and_running_balance_across_display_filters |
| FR-010 | PASS | ledger_engine::accounts, transactions, accounts_list | balances_archives_pending_authorizations_and_running_balance_across_display_filters |
| FR-011 | PASS | ledger_engine::accounts, transactions, accounts_list | balances_archives_pending_authorizations_and_running_balance_across_display_filters |
| FR-012 | PASS | ledger_engine::accounts, credit card lines | shared_credit_lines_include_archived_debt_once_and_cap_available_credit. close_date_exclusion_and_rust_month_end_clamping |
| FR-013 | PASS | ledger_engine::accounts, transactions, accounts_list | balances_archives_pending_authorizations_and_running_balance_across_display_filters |
| FR-014 | PASS | ledger_engine::accounts, transactions, accounts_list | balances_archives_pending_authorizations_and_running_balance_across_display_filters |
| FR-015 | PASS | ledger_engine::accounts, transactions, accounts_list | balances_archives_pending_authorizations_and_running_balance_across_display_filters |
| FR-016 | PASS | transaction_save, transaction_get, transaction_delete | phase_three_commands_enforce_scope_and_round_trip_financial_strings. split_edit_rollback_unicode_tags_bulk_and_pnl_calculation |
| FR-017 | PASS | transactions::save_posting | csv_identity_review_and_card_reconciliation_preserve_manual_fields_and_exposure checks ordinary edits to date, amount, currency and direction. |
| FR-018 | PASS | transactions and generated reporting date | transfer_link_protects_leg_structure_and_reports_dates_independently |
| FR-019 | PASS | transactions and generated reporting date | transfer_link_protects_leg_structure_and_reports_dates_independently |
| FR-020 | PASS | accounts::booking, reports, budgets | booking_dates_rebuild_when_statement_closing_and_due_dates_change. calendar_and_flow_keep_booked_and_reporting_timing_separate |
| FR-021 | FIXED | transactions::pnl_query, transaction filters | future_reporting_override_leaves_balance_and_waits_for_actual_pnl |
| FR-022 | PASS | split assembly triggers, planning::allocations | split_assembly_requires_positive_matching_lines_and_a_sealed_commit. budgets_use_economic_share_and_goals_use_deadline_boundaries |
| FR-023 | PASS | split assembly triggers, planning::allocations | split_assembly_requires_positive_matching_lines_and_a_sealed_commit. budgets_use_economic_share_and_goals_use_deadline_boundaries |
| FR-024 | PASS | transactions::project_posting, workflows::bulk_apply | split_edit_rollback_unicode_tags_bulk_and_pnl_calculation |
| FR-025 | FIXED | transactions::filter_query | representative_250k_ledger_uses_composite_date_and_fts_indexes checks statement precedence over date and the indexed tag path. Exhaustive filter combinations remain pending. |
| FR-026 | PASS | transactions::sort_cursor, ReadPage | nullable_sort_keeps_tied_rows_stable_and_bulk_respects_scope_revision_and_confirmation checks a nullable sort column with a stable ID tie-breaker across pages. |
| FR-027 | PASS | transactions_bulk_preview, transactions_bulk_apply | nullable_sort_keeps_tied_rows_stable_and_bulk_respects_scope_revision_and_confirmation checks group assignment, flag, confirmed delete and stale previews. |
| FR-028 | PASS | transactions_export_start, ledger_engine::export | portable_csv_export_preserves_native_units_and_neutralizes_text_formulas |
| FR-029 | PASS | transactions::pnl_query, transfers | cross_currency_transfer_validation_pair_effects_and_unlinking |
| FR-030 | FIXED | transactions::pnl_query, ledger_engine::reports | future_reporting_override_leaves_balance_and_waits_for_actual_pnl |
| FR-031 | PASS | installment_preview, installment_apply | installment_schedule_preview_applies_only_the_selected_scope |
| FR-032 | PENDING | storage::preferences, Rust export | portable_csv_export_preserves_native_units_and_neutralizes_text_formulas. Render-level masking belongs to Phase 5. |
| FR-033 | PASS | transfer_save, ledger_engine::transfers | cross_currency_transfer_validation_pair_effects_and_unlinking |
| FR-034 | PASS | transfer_save, ledger_engine::transfers | cross_currency_transfer_validation_pair_effects_and_unlinking |
| FR-035 | PASS | transfer_save, ledger_engine::transfers | cross_currency_transfer_validation_pair_effects_and_unlinking |
| FR-036 | PASS | transfer_candidates, transfer_save | two_imported_postings_link_as_one_transfer_without_recreating_either_leg verifies reuse, provenance and P&L exclusion. |
| FR-037 | PASS | importers::parsers, import_preview_start | statement_formats_and_localized_csv. phase_three_commands_enforce_scope_and_round_trip_financial_strings. fx_csv_preview_is_read_only_and_undo_reverts_the_rate |
| FR-038 | PASS | importers::parsers, import_preview_start | statement_formats_and_localized_csv. phase_three_commands_enforce_scope_and_round_trip_financial_strings. fx_csv_preview_is_read_only_and_undo_reverts_the_rate |
| FR-039 | PASS | import_template_save, import_template_file | idr_template_export_import_is_equivalent_in_json_and_toml |
| FR-040 | PASS | importers::ledger, import fingerprint | csv_identity_review_and_card_reconciliation_preserve_manual_fields_and_exposure tests normalized fallback multiplicity and CSV opt-out. |
| FR-041 | PASS | transactions::reconcile_import | csv_identity_review_and_card_reconciliation_preserve_manual_fields_and_exposure verifies preservation of description, note, payee, category and reporting override. |
| FR-042 | PASS | import_batches_list, import_undo_preview, import_undo_apply | staged_import_persists_identity_and_blocks_undo_after_dependency. statement_reconciliation_promotes_and_undo_restores_a_generated_occurrence |
| FR-043 | PASS | importers::parsers, import_preview_start | statement_formats_and_localized_csv. phase_three_commands_enforce_scope_and_round_trip_financial_strings. fx_csv_preview_is_read_only_and_undo_reverts_the_rate |
| FR-044 | PASS | local import adapters and capabilities | Negative scope scan, IPC inventory and launcher_commands_round_trip_through_tauri_and_enforce_acl |
| FR-045 | PASS | local import adapters and capabilities | Negative scope scan, IPC inventory and launcher_commands_round_trip_through_tauri_and_enforce_acl |
| FR-046 | PASS | local import adapters and capabilities | Negative scope scan, IPC inventory and launcher_commands_round_trip_through_tauri_and_enforce_acl |
| FR-047 | PASS | importers::ledger, transactions::reconcile_import | csv_identity_review_and_card_reconciliation_preserve_manual_fields_and_exposure proves one posted row with unchanged balance, debt and available credit. |
| FR-048 | PASS | local import adapters and capabilities | Negative scope scan, IPC inventory and launcher_commands_round_trip_through_tauri_and_enforce_acl |
| FR-049 | PASS | local import adapters and capabilities | Negative scope scan, IPC inventory and launcher_commands_round_trip_through_tauri_and_enforce_acl |
| FR-050 | PASS | rules_reorder, rule_pack_transfer | rule_pack_collision_policies_and_priority_round_trip tests serialization, priority order, reject, skip and replace. |
| FR-051 | PASS | rules::CompiledRule | rule_ast_enforces_shape_and_regex_bounds. rule_preview_binds_manual_override_policy_and_existing_category_opt_in |
| FR-052 | PASS | rules::CompiledRule | rule_ast_enforces_shape_and_regex_bounds. rule_preview_binds_manual_override_policy_and_existing_category_opt_in |
| FR-053 | FIXED | rule_preview_start, rule_apply_start | rule_preview_binds_manual_override_policy_and_existing_category_opt_in |
| FR-054 | FIXED | rule_preview_start, rule_apply_start | rule_preview_binds_manual_override_policy_and_existing_category_opt_in |
| FR-055 | PASS | budgets::save, budgets::get | budgets_use_economic_share_and_goals_use_deadline_boundaries |
| FR-056 | PASS | budgets::Calculator, reports::report | budget_actual_matches_fx_pnl_and_projected_placeholders_count_once matches reliable P&L actuals and verifies projected and remaining totals. |
| FR-057 | PASS | budgets::save, budgets::get | budgets_use_economic_share_and_goals_use_deadline_boundaries |
| FR-058 | PASS | budgets_copy and immediate writer | budget_copy_rolls_back_on_collision_and_requires_positive_rounded_limits |
| FR-059 | PASS | recurring::dates | six_recurring_frequencies_skip_first_count_eligible_and_preserve_edit_floor exercises all six frequencies. |
| FR-060 | PASS | recurring::dates, calendar, account balances | calendar_clamps_then_shifts_and_amortization_conserves_principal. statement_reconciliation_promotes_and_undo_restores_a_generated_occurrence |
| FR-061 | PASS | recurring::dates, calendar, account balances | calendar_clamps_then_shifts_and_amortization_conserves_principal. statement_reconciliation_promotes_and_undo_restores_a_generated_occurrence |
| FR-062 | PASS | recurring::save, generation_floor | six_recurring_frequencies_skip_first_count_eligible_and_preserve_edit_floor verifies the old posting survives and historical virtual rows are suppressed after the edit. |
| FR-063 | PASS | loan_schedule, planning::loans | loan_contract_limit_and_positive_rate_conserve_all_minor_units. calendar_clamps_then_shifts_and_amortization_conserves_principal |
| FR-064 | PASS | loan_schedule, planning::loans | loan_contract_limit_and_positive_rate_conserve_all_minor_units. calendar_clamps_then_shifts_and_amortization_conserves_principal |
| FR-065 | PASS | goal_save, planning::goals | goal_sources_use_ledger_values_and_asset_growth_resets_on_new_valuation checks all five sources. |
| FR-066 | PASS | planning::goals | goal_sources_use_ledger_values_and_asset_growth_resets_on_new_valuation and goal_pace_classifies_exactly_five_and_ten_percentage_point_boundaries. |
| FR-067 | PASS | asset_valuation_save, assets::project_value | goal_sources_use_ledger_values_and_asset_growth_resets_on_new_valuation tests all four cadences and a new valuation anchor. |
| FR-068 | PASS | assets::replay, asset_group_save, asset_group_get | weighted_assets_apply_splits_first_and_allocate_final_residue. group implementation and binding inspection |
| FR-069 | PASS | local valuation, import and history | Negative scope scan and command inventory |
| FR-070 | PENDING | report_get, dashboard_get, layout_get, layout_set | calendar_and_flow_keep_booked_and_reporting_timing_separate. Visual workflows and layout round trip pending. |
| FR-071 | PENDING | report_get, dashboard_get, layout_get, layout_set | calendar_and_flow_keep_booked_and_reporting_timing_separate. Visual workflows and layout round trip pending. |
| FR-072 | PASS | calendar_get, reports::calendar | calendar_and_flow_keep_booked_and_reporting_timing_separate. statement_reconciliation_promotes_and_undo_restores_a_generated_occurrence |
| FR-073 | PASS | calendar_get, reports::calendar | calendar_and_flow_keep_booked_and_reporting_timing_separate. statement_reconciliation_promotes_and_undo_restores_a_generated_occurrence |
| FR-074 | PASS | ledger settings, fx_rate_save, structured FX import | currency_scales_exact_inverse_cross_and_missing_rates. fx_csv_preview_is_read_only_and_undo_reverts_the_rate |
| FR-075 | PASS | ledger settings, fx_rate_save, structured FX import | currency_scales_exact_inverse_cross_and_missing_rates. fx_csv_preview_is_read_only_and_undo_reverts_the_rate |
| FR-076 | PENDING | fx::Resolver, ConversionStatus | currency_scales_exact_inverse_cross_and_missing_rates. Visual badge belongs to Phase 5. |
| FR-077 | PASS | fx_provider_configure, fx_refresh_start | phase_three_commands_enforce_scope_and_round_trip_financial_strings verifies explicit opt-in. live_provider_smoke is opt-in. |
| FR-078 | PENDING | attachment commands and Vault | attachment_recovery_adopts_staging_and_finishes_deletions. Native viewer and full rename/save-as workflow need platform validation. |
| FR-079 | PASS | attachments::Pending::read, validate_signature, insert | attachment_count_byte_limit_and_signature_are_enforced |
| FR-080 | FIXED | backup_schedule_set, BackupDirectory PathGrant | scheduled_backup_catches_up_after_unlock_and_retains_ten_in_selected_directory verifies delivery with a scoped native directory grant and a ledger-specific archive directory. |
| FR-081 | PASS | storage::schedule, ledger_unlock, storage::backups | scheduled_backup_catches_up_after_unlock_and_retains_ten_in_selected_directory verifies catch-up, encrypted archive readback and ten-generation pruning with an injected secret store and clock. |
| FR-082 | PASS | crypto::archive, storage::snapshot | encrypted_archive_round_trip_preserves_postings_and_attachments |
| FR-083 | FIXED | restore_preview_start, RestorePreview warnings | Version and entity counts in encrypted_archive_round_trip_preserves_postings_and_attachments. Typed warnings covered by bindings_match_command_contract. |
| FR-084 | PASS | restore_apply_start, restore_confirm_replacement | plain_wal_backup_and_confirmed_replacement_keep_a_safety_archive. restart_rolls_back_a_partially_swapped_portable_ledger |
| FR-085 | PASS | local schedule, local capabilities | Negative scope scan and command inventory |
| FR-086 | PASS | local schedule, local capabilities | Negative scope scan and command inventory |
| FR-087 | PASS | local schedule, local capabilities | Negative scope scan and command inventory |
| FR-088 | PASS | local schedule, local capabilities | Negative scope scan and command inventory |
| FR-089 | PASS | local schedule, local capabilities | Negative scope scan and command inventory |
| FR-090 | PASS | local schedule, local capabilities | Negative scope scan and command inventory |
| FR-091 | PASS | local schedule, local capabilities | Negative scope scan and command inventory |
| FR-092 | PASS | planning::shared, planning::allocations | exact_and_percentage_shared_expenses_reconcile_cross_currency_only_with_provenance verifies exact shares, percentage rounding and native balances. |
| FR-093 | PASS | group_settlement_save, shared balances, FX | exact_and_percentage_shared_expenses_reconcile_cross_currency_only_with_provenance checks overpayment rejection, mandatory FX provenance and P&L exclusion. |
| FR-094 | PASS | application source, migration and capability boundary | Negative scope scan and command inventory |
| FR-095 | PASS | application source, migration and capability boundary | Negative scope scan and command inventory |
| FR-096 | PASS | application source, migration and capability boundary | Negative scope scan and command inventory |
| FR-097 | PASS | application source, migration and capability boundary | Negative scope scan and command inventory |
| FR-098 | PASS | search_global, scoped transactions_list | phase_three_commands_enforce_scope_and_round_trip_financial_strings. bounded_pool_and_lock_drain_keep_commit_bound_to_original_ledger |
| FR-099 | GAP | Phase 5 command palette and editing shortcuts | Outside the permitted Phase 1–4 implementation scope. |
| FR-100 | PENDING | storage::preferences, layout_get, layout_set | phase_three_commands_enforce_scope_and_round_trip_financial_strings covers preferences. Layout persistence and rendered privacy need separate verification. |
| NFR-001 | PASS | domain::money, currency registry, FX, loans | exact_money_and_gregorian_dates. migration_strict_currency_scales_and_connection_policy. currency_scales_exact_inverse_cross_and_missing_rates. Monetary source review. |
| NFR-002 | PASS | domain::money, currency registry, FX, loans | exact_money_and_gregorian_dates. migration_strict_currency_scales_and_connection_policy. currency_scales_exact_inverse_cross_and_missing_rates. Monetary source review. |
| NFR-003 | PENDING | local IPC and opt-in FX | Offline-capable core test fixtures. fresh packaged installation under explicit network isolation pending. |
| NFR-004 | PASS | storage::database and migration | migration_strict_currency_scales_and_connection_policy. bounded_pool_and_lock_drain_keep_commit_bound_to_original_ledger |
| NFR-005 | PASS | storage::snapshot and storage::backups | encrypted_archive_round_trip_preserves_postings_and_attachments. plain_wal_backup_and_confirmed_replacement_keep_a_safety_archive |
| NFR-006 | PENDING | single writer, deferred foreign keys, split and transfer triggers, attachment journal | split_assembly_requires_positive_matching_lines_and_a_sealed_commit. attachment_recovery_adopts_staging_and_finishes_deletions. Full cross-layer fault injection pending. |
| NFR-007 | PENDING | SQLCipher, KeyWrapper, OS keychain | encrypted_ledger_rejects_wrong_passphrase_and_rotates_scopes_on_unlock uses mock keychain. Real platform secret handling and log instrumentation pending. |
| NFR-008 | PASS | Tauri CSP and optional fixed-host FX | Negative scan and command inventory. Source inspection found no application telemetry client. |
| NFR-009 | PASS | session scope, JobStore, PathGrants, ledger lifecycle | bounded_pool_and_lock_drain_keep_commit_bound_to_original_ledger. grants_are_single_use_window_and_purpose_bound_and_aliases_are_rejected |
| NFR-010 | PENDING | optimized 250k/50 backend fixture | The backend measurements appear above. Packaged startup and renderer dispatch remain unmeasured. |
| NFR-011 | PENDING | optimized 250k/50 backend fixture | The backend measurements appear above. Packaged startup and renderer dispatch remain unmeasured. |
| NFR-012 | FIXED | ipc::jobs, atomic import staging | phase_three_commands_enforce_scope_and_round_trip_financial_strings verifies durable job row counts. Packaged large-file responsiveness remains unmeasured. |
| NFR-013 | PASS | versioned archive, manifest and SQLite bridge | encrypted_archive_round_trip_preserves_postings_and_attachments restores into a distinct app-data root. Real cross-platform installation coverage remains pending. |
| NFR-014 | PENDING | migration checksums, immediate transaction | migration_checksum_and_schema_version_are_validated_on_open. No irreversible upgrade fixture or generalized pre-upgrade backup workflow yet. |
| NFR-015 | PASS | storage::attachments two-phase install/remove | attachment_recovery_adopts_staging_and_finishes_deletions |
| NFR-016 | PENDING | launcher and Phase 5 views | launcher_commands_round_trip_through_tauri_and_enforce_acl verifies commands. Keyboard and display scaling qualification pending. |
| NFR-017 | PENDING | preferences and exact unmasked Rust services | portable_csv_export_preserves_native_units_and_neutralizes_text_formulas. Phase 5 visuals, chart/tooltip and notice masking pending. |
| NFR-018 | PASS | Rust domain/services and typed IPC | bindings_match_command_contract. domain and integration suites |
| NFR-019 | PASS | fx::fetch_cancellable, Tauri local capability | phase_three_commands_enforce_scope_and_round_trip_financial_strings verifies disabled provider. fixed HTTPS endpoint, no redirects or proxies. |
| NFR-020 | PASS | crypto::archive | encrypted_archive_round_trip_preserves_postings_and_attachments. plain_wal_backup_and_confirmed_replacement_keep_a_safety_archive |
