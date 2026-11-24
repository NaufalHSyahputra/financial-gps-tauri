use super::*;

impl LedgerManager {
    pub fn create(&self, window: &str, draft: CreateLedger) -> Result<LedgerSession> {
        self.create_or_restore(window, draft, None)
    }

    pub fn restore_new(
        &self,
        window: &str,
        mut draft: CreateLedger,
        archive: RestoredArchive,
    ) -> Result<LedgerSession> {
        let (currency, timezone) = archive
            .database
            .query_row(
                "SELECT display_currency,timezone_override FROM ledger_meta",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(crate::storage::sql_error)?;
        draft.base_currency = currency;
        draft.timezone = timezone;
        self.create_or_restore(window, draft, Some(archive))
    }

    pub fn restore_replace(
        &self,
        window: &str,
        scope: &Scope,
        source: ReplacementSource<'_>,
        cancelled: &dyn Fn() -> bool,
        publish: impl FnOnce() -> Result<()>,
    ) -> Result<LedgerSession> {
        self.restore_replace_impl(window, scope, source, cancelled, publish, &[])
    }

    pub(super) fn restore_replace_impl(
        &self,
        window: &str,
        scope: &Scope,
        source: ReplacementSource<'_>,
        cancelled: &dyn Fn() -> bool,
        publish: impl FnOnce() -> Result<()>,
        local_settings: &[(String, String)],
    ) -> Result<LedgerSession> {
        let _sync_gate = self.sync.try_gate()?;
        let sync_binding = self.read_domain(window, scope, |tx| sync::load(tx))?;
        let ReplacementSource {
            expected_revision,
            protection,
            credential,
            safety_password,
            archive,
        } = source;
        if (protection == Protection::Encrypted) != credential.is_some() {
            return Err(invalid("outputCredential", "invalid_credential_options"));
        }
        let backup =
            self.backup_create(window, scope, None, safety_password, cancelled, || Ok(()))?;
        if backup.source_revision.as_deref() != Some(expected_revision) {
            return Err(crate::ledger_engine::stale_revision());
        }
        let mut inner = self.inner()?;
        let active = inner.scoped(window, scope)?;
        let entry = active.entry.clone();
        let uuid = &active.session.ledger_uuid;
        let (name, currency, timezone) = archive
            .database
            .query_row(
                "SELECT name,display_currency,timezone_override FROM ledger_meta",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .map_err(crate::storage::sql_error)?;
        if !valid_name(&name) {
            return Err(invalid("name", "invalid_ledger_name"));
        }
        let mut staged = replacement::Replacement::begin(
            &inner.registry,
            &entry,
            &attachment_root(&entry),
            protection,
            &name,
            !archive.objects.is_empty(),
        )?;
        let prepare = (|| {
            let path = staged.staged_db().to_owned();
            let mut file = NewLedger::reserve(path.clone())?;
            let lease = FileLease::ledger(&path)?;
            let key = if let Some(credential) = credential {
                let (wrapper, key) = KeyWrapper::enroll(&credential)?;
                file.write_wrapper(&wrapper)?;
                Some(key)
            } else {
                None
            };
            let db = Database::open_with(
                &path,
                lease,
                key.as_ref(),
                Some(LedgerMetadata {
                    uuid,
                    name: &name,
                    timezone: timezone.as_deref(),
                    currency: &currency,
                    created_at_ms: Utc::now().timestamp_millis(),
                }),
                |destination| {
                    snapshot::copy_tables(&archive.database, destination, Some(uuid), cancelled)?;
                    let tx = destination
                        .transaction()
                        .map_err(crate::storage::sql_error)?;
                    for (key, value) in local_settings {
                        tx.execute("INSERT INTO ledger_settings VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",
                            rusqlite::params![key,value]).map_err(crate::storage::sql_error)?;
                    }
                    if let Some(binding) = sync_binding.as_ref() {
                        sync::store(&tx, binding)?;
                        sync::dirty(&tx)?;
                    }
                    tx.commit().map_err(crate::storage::sql_error)?;
                    Ok(())
                },
            )?;
            let vault = Vault {
                root: staged.staged_vault().to_owned(),
                uuid,
                key: db.attachment_key.as_ref(),
            };
            db.read(|tx| vault.restore(tx, &archive.objects))?;
            db.close_staging();
            file.sync_parent()?;
            file.publish();
            Ok(key)
        })();
        let key = match prepare {
            Ok(key) => key,
            Err(error) => {
                staged.rollback()?;
                return Err(error);
            }
        };
        let generation = inner.next_generation()?;
        if let Err(error) = publish().and_then(|()| inner.quiesce_at(Some(expected_revision))) {
            staged.rollback()?;
            return Err(error);
        }
        inner.state = State::Opening;
        let installed = (|| {
            let _parked_lease = FileLease::ledger(&entry.path)?;
            if files::identity(&entry.path)? != *active.identity {
                return Err(crate::ledger_engine::conflict("ledger_file_changed"));
            }
            staged.swap()?;
            #[cfg(not(unix))]
            drop(_parked_lease); // Windows uses a path-based lease.
            let lease = FileLease::ledger(&entry.path)?;
            let db = Database::open(&entry.path, lease, key.as_ref(), None)?;
            if db.info.uuid != *uuid {
                return Err(crate::ledger_engine::integrity(
                    "restored_ledger_identity_mismatch",
                ));
            }
            let vault = entry_vault(&entry, &db);
            db.read(|tx| {
                for object in &archive.objects {
                    vault.get_bytes(tx, &object.id)?;
                }
                Ok(())
            })?;
            Ok(db)
        })();
        match installed {
            Ok(db) => {
                if let Err(error) = staged.commit(&mut inner.registry, &entry) {
                    db.close();
                    inner.state = State::Unselected;
                    return Err(error);
                }
                if entry.quick_unlock {
                    let _ = self.keychain.revoke(entry.instance);
                }
                let updated = inner.registry.get(entry.instance)?;
                Ok(self.ready(&mut inner, generation, updated, window, db))
            }
            Err(error) => {
                inner.state = State::Unselected;
                staged.rollback()?;
                if entry.protection == Protection::Encrypted {
                    inner.state = State::Locked(Selected::new(
                        entry.clone(),
                        true,
                        Arc::new(files::identity(&entry.path)?),
                        window,
                    ));
                } else {
                    let reopen = (|| {
                        let lease = FileLease::ledger(&entry.path)?;
                        Database::open(&entry.path, lease, None, None)
                    })();
                    if let Ok(db) = reopen {
                        self.ready(&mut inner, generation, entry, window, db);
                    }
                }
                Err(error)
            }
        }
    }

    fn create_or_restore(
        &self,
        window: &str,
        draft: CreateLedger,
        archive: Option<RestoredArchive>,
    ) -> Result<LedgerSession> {
        let name = draft.name.trim();
        if !valid_name(name) {
            return Err(AppError::validation("name", "invalid_ledger_name"));
        }
        let currency = CurrencyCode::parse(&draft.base_currency)?;
        if draft
            .timezone
            .as_ref()
            .is_some_and(|zone| zone.parse::<Tz>().is_err())
        {
            return Err(AppError::validation("timezone", "invalid_iana_timezone"));
        }
        if (draft.protection == Protection::Encrypted) != draft.credential.is_some()
            || (draft.protection == Protection::Plain && draft.remember_key)
        {
            return Err(AppError::validation(
                "credential",
                "invalid_credential_options",
            ));
        }

        let mut inner = self.inner()?;
        let instance = Uuid::new_v4();
        let (path, storage_policy) = match draft.destination {
            Some(ref grant) => (grant.revalidate()?.to_owned(), StoragePolicy::Portable),
            None => (
                inner.registry.managed_path(instance),
                StoragePolicy::Managed,
            ),
        };
        let generation = inner.next_generation()?;
        let mut file = NewLedger::reserve(path.clone())?;
        inner.quiesce()?;
        inner.state = State::Opening;
        let result = (|| {
            let lease = FileLease::ledger(&path)?;
            let key = if let Some(credential) = draft.credential {
                let (wrapper, key) = KeyWrapper::enroll(&credential)?;
                file.write_wrapper(&wrapper)?;
                Some(key)
            } else {
                None
            };
            let uuid = Uuid::new_v4().to_string();
            let db = Database::open_with(
                &path,
                lease,
                key.as_ref(),
                Some(LedgerMetadata {
                    uuid: &uuid,
                    name,
                    timezone: draft.timezone.as_deref(),
                    currency: currency.as_str(),
                    created_at_ms: Utc::now().timestamp_millis(),
                }),
                |writer| {
                    if let Some(source) = archive.as_ref() {
                        snapshot::copy_tables(&source.database, writer, Some(&uuid), &|| false)?;
                        writer
                            .execute("UPDATE ledger_meta SET name=?1 WHERE singleton=1", [name])
                            .map_err(crate::storage::sql_error)?;
                    }
                    Ok(())
                },
            )?;
            file.sync_parent()?;
            let entry = Entry {
                instance,
                path,
                label: name.to_owned(),
                protection: draft.protection,
                storage_policy,
                quick_unlock: draft.remember_key,
            };
            let vault = entry_vault(&entry, &db);
            if let Some(source) = archive.as_ref() {
                db.read(|tx| vault.restore(tx, &source.objects))?;
            }
            if draft.remember_key {
                self.keychain
                    .remember(instance, key.as_ref().expect("validated protection"))?;
            }
            if let Err(error) = inner.registry.add(entry.clone()) {
                if draft.remember_key {
                    let _ = self.keychain.revoke(instance);
                }
                return Err(error);
            }
            file.publish();
            Ok((entry, db))
        })();
        match result {
            Ok((entry, db)) => Ok(self.ready(&mut inner, generation, entry, window, db)),
            Err(error) => {
                inner.state = State::Unselected;
                Err(error)
            }
        }
    }

    pub fn close(&self, window: &str, scope: &Scope) -> Result<LifecycleState> {
        let mut inner = self.inner()?;
        inner.scoped(window, scope)?;
        inner.quiesce()?;
        Ok(LifecycleState::Unselected)
    }
}
