use super::*;

impl LedgerManager {
    pub fn change_protection(
        &self,
        window: &str,
        mutation: &Mutation,
        credentials: ProtectionCredentials,
        cancelled: &dyn Fn() -> bool,
        publish: impl FnOnce() -> Result<()>,
    ) -> Result<LedgerSession> {
        let ProtectionCredentials {
            protection,
            old,
            new,
            safety,
        } = credentials;
        let scope = &mutation.scope;
        let (current,local)=self.with_vault(window,scope,|db,_,_|{
            let active=self.inner()?.scoped(window,scope)?;
            if active.entry.protection==Protection::Encrypted {
                let credential=old.as_ref().ok_or_else(||invalid("oldCredential","credential_required"))?;
                let key=files::read_wrapper(&active.entry.path)?.unlock(credential)?;
                let derived=crate::crypto::attachment::derive(&key)?;
                if db.attachment_key.as_ref().is_none_or(|key|**key!=*derived) {return Err(bad_credential());}
            } else if old.is_some(){return Err(invalid("oldCredential","ledger_is_plain"));}
            if protection==Protection::Plain&&active.entry.protection==Protection::Plain {
                return Err(invalid("protection","ledger_is_plain"));
            }
            db.read(|tx|{
                let revision=crate::ledger_engine::native_revision(tx)?;
                if mutation.expected_revision.as_ref().is_some_and(|expected|expected!=&revision){
                    return Err(crate::ledger_engine::stale_revision());
                }
                let mut query=tx.prepare("SELECT key,value_json FROM ledger_settings WHERE key LIKE 'backup.%' OR key LIKE 'fx.%enabled'").map_err(crate::storage::sql_error)?;
                let rows=query.query_map([],|row|Ok((row.get(0)?,row.get(1)?))).map_err(crate::storage::sql_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(crate::storage::sql_error)?;
                Ok((revision,rows))
            })
        })?;
        let (image, objects) = self.backup_image(window, scope, cancelled)?;
        let archive = snapshot::materialize(&image, objects)?;
        if archive.manifest.source_revision != current {
            return Err(crate::ledger_engine::stale_revision());
        }
        let remembered = self
            .read_domain(window, scope, |tx| {
                self.scheduled_backup_password(&Context::system(
                    crate::storage::SqliteSession(tx),
                    scope,
                )?)
            })
            .ok();
        let fallback = safety
            .as_ref()
            .or(remembered.as_ref())
            .or(old.as_ref())
            .or(new.as_ref())
            .ok_or_else(|| invalid("backupPassphrase", "credential_required"))?;
        let output = new.as_ref().map(Credential::copy);
        self.restore_replace_impl(
            window,
            scope,
            ReplacementSource {
                expected_revision: &current,
                protection,
                credential: output,
                safety_password: fallback,
                archive,
            },
            cancelled,
            publish,
            &local,
        )
    }

    pub fn set_keychain(
        &self,
        window: &str,
        mutation: &Mutation,
        enabled: bool,
        credential: Option<&Credential>,
    ) -> Result<Write<ProtectionStatus>> {
        let mut inner = self.inner()?;
        let active = inner.scoped(window, &mutation.scope)?;
        let slot = active.database.try_read().ok_or_else(AppError::busy)?;
        let db = slot.as_ref().ok_or_else(stale)?;
        let old_entry = inner.registry.get(active.entry.instance)?;
        let mut updated = false;
        let mut result = db.write(|tx| {
            let ctx = Context::system(crate::storage::SqliteSession(tx), &mutation.scope)?;
            ctx.mutate(
                "ledger_keychain_set",
                mutation,
                &enabled,
                &["protection"],
                |_| {
                    if old_entry.protection != Protection::Encrypted {
                        return Err(invalid("enabled", "keychain_requires_encryption"));
                    }
                    if enabled {
                        let credential = credential
                            .ok_or_else(|| invalid("credential", "credential_required"))?;
                        let key = files::read_wrapper(&old_entry.path)?.unlock(credential)?;
                        let derived = crate::crypto::attachment::derive(&key)?;
                        if db
                            .attachment_key
                            .as_ref()
                            .is_none_or(|key| **key != *derived)
                        {
                            return Err(bad_credential());
                        }
                        self.keychain.remember(old_entry.instance, &key)?;
                    } else {
                        self.keychain.revoke(old_entry.instance)?;
                    }
                    updated = true;
                    Ok(ProtectionStatus {
                        protection: old_entry.protection,
                        quick_unlock: enabled,
                    })
                },
            )
        })?;
        if updated || old_entry.quick_unlock != enabled {
            let mut entry = old_entry;
            entry.quick_unlock = enabled;
            inner.registry.replace(entry)?;
        }
        result.value.quick_unlock = enabled;
        Ok(result)
    }
}
