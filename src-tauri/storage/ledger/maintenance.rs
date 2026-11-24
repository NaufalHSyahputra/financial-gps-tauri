use super::*;

impl LedgerManager {
    pub(crate) fn shutdown(&self) {
        let _ = self.inner.lock().quiesce();
    }

    fn backup_root(&self, window: &str, scope: &Scope) -> Result<PathBuf> {
        let inner = self.inner()?;
        let active = inner.scoped(window, scope)?;
        let managed = inner.registry.managed_path(active.entry.instance);
        Ok(managed
            .parent()
            .and_then(Path::parent)
            .expect("managed registry root")
            .join("backups")
            .join(active.entry.instance.to_string()))
    }

    pub(super) fn custom_backup_root(path: &Path, scope: &Scope) -> Result<PathBuf> {
        let chosen = files::checked_directory(path)?;
        if chosen != path {
            return Err(invalid(
                "destinationGrant",
                "scheduled_backup_directory_changed",
            ));
        }
        Ok(chosen.join(format!("financial-gps-{}", scope.ledger_instance_id)))
    }

    fn backup_roots(&self, window: &str, ctx: &Context<'_>) -> Result<Vec<PathBuf>> {
        let mut roots = vec![self.backup_root(window, ctx.scope)?];
        if let Some(path) = crate::storage::schedule::destination_root(ctx)? {
            roots.push(Self::custom_backup_root(&path, ctx.scope)?);
        }
        Ok(roots)
    }

    fn backup_location(&self, window: &str, ctx: &Context<'_>, id: &str) -> Result<PathBuf> {
        for root in self.backup_roots(window, ctx)? {
            if crate::storage::backups::contains(&root, id)? {
                return Ok(root);
            }
        }
        Err(crate::ledger_engine::missing("backupId"))
    }

    pub fn backup_list(
        &self,
        window: &str,
        ctx: &Context<'_>,
        page: &crate::ledger_engine::PageReq,
    ) -> Result<crate::ledger_engine::Page<crate::storage::backups::BackupSummary>> {
        let cursor = crate::ledger_engine::ReadPage::new(ctx, page, &"backup_list")?;
        let after = cursor.after.as_ref().map(|(_, id)| id.as_str());
        let mut backups = Vec::new();
        for root in self.backup_roots(window, ctx)? {
            backups.extend(crate::storage::backups::list(&root)?);
        }
        backups.sort_unstable_by(|a, b| b.id.cmp(&a.id));
        backups.dedup_by(|a, b| a.id == b.id);
        let rows = backups
            .into_iter()
            .filter(|row| after.is_none_or(|after| row.id.as_str() < after))
            .take(cursor.limit + 1)
            .collect();
        cursor.finish(rows, |row| (row.id.clone(), row.id.clone()))
    }

    pub fn backup_remove(
        &self,
        window: &str,
        ctx: &Context<'_>,
        id: &str,
        confirmed: bool,
    ) -> Result<Deleted> {
        if !confirmed {
            return Err(invalid("confirm", "confirmation_required"));
        }
        let root = self.backup_location(window, ctx, id)?;
        crate::storage::backups::remove(&root, id)?;
        Ok(Deleted { id: id.to_owned() })
    }

    pub fn backup_file(&self, window: &str, scope: &Scope, id: &str) -> Result<std::fs::File> {
        self.read_domain(window, scope, |tx| {
            let ctx = Context::system(crate::storage::SqliteSession(tx), scope)?;
            crate::storage::backups::file(&self.backup_location(window, &ctx, id)?, id)
        })
    }

    pub fn backup_create(
        &self,
        window: &str,
        scope: &Scope,
        target: Option<&ApprovedPath>,
        password: &Credential,
        cancelled: &dyn Fn() -> bool,
        publish: impl FnOnce() -> Result<()>,
    ) -> Result<crate::storage::backups::BackupSummary> {
        let root = self.backup_root(window, scope)?;
        let (image, objects) = self.backup_image(window, scope, cancelled)?;
        crate::storage::backups::create(
            &root,
            target,
            &image,
            &objects,
            password,
            cancelled,
            || self.with_database(window, scope, |_, _| publish()),
        )
    }

    pub fn backup_image(
        &self,
        window: &str,
        scope: &Scope,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(Zeroizing<Vec<u8>>, Vec<snapshot::Object>)> {
        self.with_vault(window, scope, |db, vault, stopping| {
            db.write(|tx| vault.recover(tx))?;
            db.read(|tx| {
                let image = snapshot::snapshot(tx, db.attachment_key.is_some(), &|| {
                    stopping() || cancelled()
                })?;
                if stopping() || cancelled() {
                    return Err(AppError::new(ErrorCode::Cancelled, "operation_cancelled"));
                }
                Ok((image, vault.inventory(tx)?))
            })
        })
    }

    pub fn run_recurring_generation(&self, window: &str) -> Result<usize> {
        if !self.preferences()?.enables(preferences::Feature::Recurring) {
            return Ok(0);
        }
        let Some(scope) = self.session_scope(window)? else {
            return Ok(0);
        };
        self.write_domain(window, &scope, |tx| {
            crate::planning::recurring::generate_due(&Context::system(
                crate::storage::SqliteSession(tx),
                &scope,
            )?)
        })
    }

    pub fn set_backup_schedule(
        &self,
        window: &str,
        mutation: &Mutation,
        change: &crate::storage::schedule::ScheduleChange,
        grant_id: Option<&str>,
        destination: impl FnOnce() -> Result<Option<ApprovedPath>>,
        credential: Option<&Credential>,
    ) -> Result<Write<crate::storage::schedule::BackupSchedule>> {
        self.write_domain(window, &mutation.scope, |tx| {
            let ctx = Context::system(crate::storage::SqliteSession(tx), &mutation.scope)?;
            ctx.mutate(
                "backup_schedule_set",
                mutation,
                &(change, grant_id),
                &["backups"],
                |ctx| {
                    let destination = destination()?;
                    crate::storage::schedule::set(
                        ctx,
                        change,
                        credential,
                        self.backup_secrets.as_ref(),
                        destination.as_ref(),
                    )
                },
            )
        })
    }
    pub(crate) fn scheduled_backup_password(&self, ctx: &Context<'_>) -> Result<Credential> {
        crate::storage::schedule::password(ctx, self.backup_secrets.as_ref())
    }

    pub fn run_scheduled_backup(&self, window: &str) -> Result<()> {
        self.run_scheduled_backup_at(window, &crate::domain::SystemClock)
    }

    pub fn run_scheduled_backup_at(
        &self,
        window: &str,
        clock: &impl crate::domain::Clock,
    ) -> Result<()> {
        let _guard = self
            .scheduled_backup
            .try_lock()
            .ok_or_else(AppError::busy)?;
        let Some(scope) = self.session_scope(window)? else {
            return Ok(());
        };
        let run = self.read_domain(window, &scope, |tx| {
            crate::storage::schedule::due(&Context::new(
                crate::storage::SqliteSession(tx),
                &scope,
                clock,
            )?)
        })?;
        let Some(run) = run else {
            return Ok(());
        };
        let password = self.read_domain(window, &scope, |tx| {
            self.scheduled_backup_password(&Context::new(
                crate::storage::SqliteSession(tx),
                &scope,
                clock,
            )?)
        });
        let result = password.and_then(|password| {
            let root = match run.destination.as_deref() {
                Some(path) => Self::custom_backup_root(path, &scope)?,
                None => self.backup_root(window, &scope)?,
            };
            let (image, objects) = self.backup_image(window, &scope, &|| {
                self.session_scope(window).ok().flatten().as_ref() != Some(&scope)
            })?;
            let result = crate::storage::backups::create(
                &root,
                None,
                &image,
                &objects,
                &password,
                &|| self.session_scope(window).ok().flatten().as_ref() != Some(&scope),
                || {
                    self.read_domain(window, &scope, |tx| {
                        crate::storage::schedule::approve(
                            &Context::new(crate::storage::SqliteSession(tx), &scope, clock)?,
                            &run,
                        )
                    })
                },
            )?;
            if let Some(days) = run.retain_days {
                let cutoff = clock.now() - chrono::Duration::days(i64::from(days));
                for old in crate::storage::backups::list(&root)?
                    .into_iter()
                    .filter(|item| item.id != result.id)
                {
                    if chrono::DateTime::parse_from_rfc3339(&old.created_at)
                        .is_ok_and(|date| date < cutoff)
                    {
                        crate::storage::backups::remove(&root, &old.id)?;
                    }
                }
            }
            Ok(result)
        });
        self.write_domain(window, &scope, |tx| {
            crate::storage::schedule::finish(
                &Context::new(crate::storage::SqliteSession(tx), &scope, clock)?,
                &run,
                &result,
            )
        })?;
        result.map(|_| ())
    }

    pub fn validate_turso_credentials(url: &str, token: &SyncToken) -> Result<()> {
        SyncController::validate(url, token)
    }

    pub fn storage_mode_set(
        &self,
        window: &str,
        scope: &Scope,
        mode: SetStorageMode,
    ) -> Result<SyncStatus> {
        let managed =
            self.inner()?.scoped(window, scope)?.entry.storage_policy == StoragePolicy::Managed;
        self.sync.set(
            scope,
            managed,
            &ManagedReplica {
                manager: self,
                window,
                scope,
            },
            self.sync_vault.as_ref(),
            mode,
        )
    }

    pub fn sync_probe_saved(&self, window: &str, scope: &Scope) -> Result<()> {
        self.sync.probe_saved(
            scope,
            &ManagedReplica {
                manager: self,
                window,
                scope,
            },
            self.sync_vault.as_ref(),
        )
    }

    pub fn sync_status(&self, window: &str, scope: &Scope) -> Result<SyncStatus> {
        self.sync.status(
            scope,
            &ManagedReplica {
                manager: self,
                window,
                scope,
            },
        )
    }

    pub fn sync_now(&self, window: &str, scope: &Scope) -> Result<SyncStatus> {
        self.sync.run(
            scope,
            &ManagedReplica {
                manager: self,
                window,
                scope,
            },
            self.sync_vault.as_ref(),
        )
    }

    pub fn sync_publish_local(
        &self,
        window: &str,
        scope: &Scope,
        expected_generation: &str,
    ) -> Result<SyncStatus> {
        let expected = expected_generation
            .parse::<i64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| invalid("remoteGeneration", "invalid_sync_generation"))?;
        self.sync.publish_local(
            scope,
            &ManagedReplica {
                manager: self,
                window,
                scope,
            },
            self.sync_vault.as_ref(),
            expected,
        )
    }

    pub fn sync_accept_remote(
        &self,
        window: &str,
        scope: &Scope,
        generation: &str,
        local_version: &str,
    ) -> Result<SyncStatus> {
        let parse = |value: &str, field| {
            value
                .parse::<i64>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| invalid(field, "invalid_sync_generation"))
        };
        self.sync.accept_remote(
            scope,
            &ManagedReplica {
                manager: self,
                window,
                scope,
            },
            self.sync_vault.as_ref(),
            parse(generation, "remoteGeneration")?,
            parse(local_version, "localVersion")?,
        )
    }
}

pub(in crate::storage) struct ManagedReplica<'a> {
    pub(in crate::storage) manager: &'a LedgerManager,
    pub(in crate::storage) window: &'a str,
    pub(in crate::storage) scope: &'a Scope,
}

impl Replica for ManagedReplica<'_> {
    fn active(&self) -> Result<()> {
        self.manager
            .with_database(self.window, self.scope, |_, _| Ok(()))
    }

    fn load(&self) -> Result<Option<sync::Binding>> {
        self.manager
            .read_domain(self.window, self.scope, |tx| sync::load(tx))
    }

    fn update(&self, work: impl FnOnce(&rusqlite::Connection) -> Result<()>) -> Result<()> {
        self.manager
            .with_database(self.window, self.scope, |db, _| {
                db.write_sync_metadata(|tx| work(tx))
            })
    }

    fn capture(&self) -> Result<(sync::Binding, Zeroizing<Vec<u8>>)> {
        self.manager
            .with_vault(self.window, self.scope, |db, vault, stopping| {
                db.read(|tx| {
                    let state = sync::load(tx)?.ok_or_else(sync::corrupt)?;
                    Ok((state, sync::capture(tx, vault, stopping)?))
                })
            })
    }

    fn apply(&self, incoming: sync::Incoming, bytes: &[u8]) -> Result<()> {
        let archive = sync::decode(bytes)?;
        self.manager
            .with_vault(self.window, self.scope, |db, vault, stopping| {
                db.install_sync_image(&archive, vault, incoming, stopping)
            })
    }
}

impl Drop for LedgerManager {
    fn drop(&mut self) {
        let _ = self.inner.get_mut().quiesce();
    }
}
