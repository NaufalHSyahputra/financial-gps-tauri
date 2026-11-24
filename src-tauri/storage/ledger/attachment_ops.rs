use super::*;

impl LedgerManager {
    pub fn attachment_add(
        &self,
        window: &str,
        mutation: &Mutation,
        transaction: &str,
        input: &impl Serialize,
        grants: impl FnOnce() -> Result<Vec<ApprovedPath>>,
        publish: impl FnOnce() -> Result<()>,
    ) -> Result<Write<AttachmentWriteResult>> {
        self.with_vault(window, &mutation.scope, |db, vault, _| {
            let mut prepared = Vec::new();
            let result = db.write(|tx| {
                let ctx = Context::system(crate::storage::SqliteSession(tx), &mutation.scope)?;
                ctx.mutate(
                    "attachment_add_start",
                    mutation,
                    input,
                    &["attachments"],
                    |ctx| {
                        let grants = grants()?;
                        if grants.is_empty() || grants.len() > 10 {
                            return Err(invalid("grants", "attachment_count_limit"));
                        }
                        for grant in &grants {
                            prepared.push(Pending::read(grant)?);
                        }
                        attachments::insert(&ctx.tx, transaction, &prepared, ctx.now_ms)?;
                        for file in &prepared {
                            vault.prepare(file)?;
                        }
                        let rows = prepared
                            .iter()
                            .map(|file| attachments::get(&ctx.tx, &file.id))
                            .collect::<Result<_>>()?;
                        publish()?;
                        Ok(AttachmentWriteResult { attachments: rows })
                    },
                )
            });
            let mut result = match result {
                Ok(value) => value,
                Err(error) => {
                    for file in &prepared {
                        vault.abort(file);
                    }
                    return Err(error);
                }
            };
            if !prepared.is_empty() {
                for file in &prepared {
                    vault.promote(file)?;
                }
                db.write(|tx| attachments::mark_ready(tx, &prepared))?;
            } else if result
                .value
                .attachments
                .iter()
                .any(|row| matches!(row.state, attachments::AttachmentState::Staged))
            {
                db.write(|tx| vault.recover(tx))?;
            }
            result.value.attachments = db.read(|tx| {
                result
                    .value
                    .attachments
                    .iter()
                    .map(|row| attachments::get(&crate::storage::sqlite_view(tx), &row.id))
                    .collect()
            })?;
            Ok(result)
        })
    }

    pub fn attachment_delete(
        &self,
        window: &str,
        mutation: &Mutation,
        id: &str,
        input: &impl Serialize,
        confirmed: bool,
    ) -> Result<Write<Deleted>> {
        self.with_vault(window, &mutation.scope, |db, vault, _| {
            let result = db.write(|tx| {
                Context::system(crate::storage::SqliteSession(tx), &mutation.scope)?.mutate(
                    "attachment_delete",
                    mutation,
                    input,
                    &["attachments"],
                    |ctx| {
                        if !confirmed {
                            return Err(invalid("confirm", "confirmation_required"));
                        }
                        attachments::mark_deleting(&ctx.tx, id)?;
                        Ok(Deleted { id: id.to_owned() })
                    },
                )
            })?;
            db.write(|tx| {
                if tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM attachments WHERE id=?1 AND state='deleting')",
                        [id],
                        |r| r.get::<_, bool>(0),
                    )
                    .map_err(crate::storage::sql_error)?
                {
                    vault.delete_blob(tx, id)?;
                    tx.execute("DELETE FROM attachments WHERE id=?1", [id])
                        .map_err(crate::storage::sql_error)?;
                }
                Ok(())
            })?;
            Ok(result)
        })
    }

    pub fn attachment_save(
        &self,
        window: &str,
        scope: &Scope,
        id: &str,
        target: &ApprovedPath,
    ) -> Result<FileResult> {
        self.with_vault(window, scope, |db, vault, _| {
            db.read(|tx| {
                let (_, bytes) = vault.get_bytes(tx, id)?;
                Ok(FileResult {
                    file_name: attachments::save_as(target, &bytes)?,
                    row_count: "1".to_owned(),
                    revision: crate::ledger_engine::native_revision(tx)?,
                })
            })
        })
    }

    pub fn attachment_bytes<T>(
        &self,
        window: &str,
        scope: &Scope,
        id: &str,
        work: impl FnOnce(AttachmentSummary, Zeroizing<Vec<u8>>) -> Result<T>,
    ) -> Result<T> {
        self.with_vault(window, scope, |db, vault, _| {
            db.read(|tx| {
                let (summary, bytes) = vault.get_bytes(tx, id)?;
                work(summary, bytes)
            })
        })
    }
}

pub(super) fn attachment_root(entry: &Entry) -> PathBuf {
    match entry.storage_policy {
        StoragePolicy::Managed => entry
            .path
            .parent()
            .and_then(Path::parent)
            .expect("managed ledger path")
            .join("attachments")
            .join(entry.instance.to_string()),
        StoragePolicy::Portable => files::sibling(&entry.path, ".attachments"),
    }
}
pub(super) fn entry_vault<'a>(entry: &Entry, db: &'a Database) -> Vault<'a> {
    Vault {
        root: attachment_root(entry),
        uuid: &db.info.uuid,
        key: db.attachment_key.as_ref(),
    }
}
