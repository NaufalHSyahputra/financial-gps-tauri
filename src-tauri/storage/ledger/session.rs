use super::*;

impl LedgerManager {
    pub fn session_scope(&self, window: &str) -> Result<Option<Scope>> {
        let inner = self.inner()?;
        Ok(match &inner.state {
            State::Ready(active) if active.window == window => Some(active.session.scope.clone()),
            _ => None,
        })
    }

    pub fn session(&self, window: &str) -> Result<Option<LedgerSession>> {
        let session = {
            let inner = self.inner()?;
            match &inner.state {
                State::Ready(active) if active.window == window => Some(active.session.clone()),
                _ => None,
            }
        };
        let Some(mut session) = session else {
            return Ok(None);
        };
        self.read(window,&session.scope,|tx| {
            (session.name,session.base_currency,session.revision)=tx.query_row(
                "SELECT name,display_currency,CAST(revision AS TEXT) FROM ledger_meta WHERE singleton=1",
                [],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
            )?;
            Ok(())
        })?;
        Ok(Some(session))
    }

    pub fn state(&self) -> Result<LifecycleState> {
        Ok(match self.inner()?.state {
            State::Unselected => LifecycleState::Unselected,
            State::Resolving => LifecycleState::Resolving,
            State::Locked(_) => LifecycleState::Locked,
            State::Opening => LifecycleState::Opening,
            State::Ready(_) => LifecycleState::Ready,
            State::Quiescing => LifecycleState::Quiescing,
        })
    }

    pub fn open(
        &self,
        window: &str,
        request: Uuid,
        target_ref: String,
        target: impl FnOnce() -> Result<OpenTarget>,
    ) -> Result<OpenResult> {
        let mut inner = self.inner()?;
        if let Some(receipt) = inner
            .receipts
            .iter()
            .find(|r| r.window == window && r.request == request)
        {
            if receipt.target != target_ref {
                return Err(AppError::new(ErrorCode::Conflict, "request_id_reused"));
            }
            return inner
                .current_open(window)
                .filter(|current| *current == receipt.result)
                .ok_or_else(stale);
        }

        let selected = match target()? {
            OpenTarget::Registered(instance) => {
                let entry = inner.registry.get(instance)?;
                let path = files::checked_path(&entry.path, true)?;
                let known = inner.registry.find(&path)?;
                if known.as_ref().map(|entry| entry.instance) != Some(instance) {
                    return Err(AppError::new(ErrorCode::Conflict, "ledger_file_changed"));
                }
                Self::resolve(window, &path, Some(entry), inner.active_at(&path))?
            }
            OpenTarget::Chosen(grant) => {
                let path = grant.revalidate()?;
                Self::resolve(
                    window,
                    path,
                    inner.registry.find(path)?,
                    inner.active_at(path),
                )?
            }
        };
        let generation = inner.next_generation()?;
        inner.quiesce()?;
        inner.state = State::Resolving;

        if selected.entry.protection == Protection::Encrypted {
            inner.state = State::Locked(selected);
        } else {
            inner.state = State::Opening;
            let result = (|| {
                selected.check_identity()?;
                let lease = FileLease::ledger(&selected.entry.path)?;
                selected.check_identity()?;
                let db = Database::reopen(&selected.entry.path, lease, None)?;
                db.write(|tx| entry_vault(&selected.entry, &db).recover(tx))?;
                let entry = inner.register(&selected, &db)?;
                Ok((entry, db))
            })();
            match result {
                Ok((entry, db)) => {
                    self.ready(&mut inner, generation, entry, window, db);
                }
                Err(error) => {
                    inner.state = State::Unselected;
                    return Err(error);
                }
            }
        }
        let result = inner
            .current_open(window)
            .expect("open returned Ready or Locked");
        if inner.receipts.len() == 32 {
            inner.receipts.pop_front();
        }
        inner.receipts.push_back(OpenReceipt {
            window: window.to_owned(),
            request,
            target: target_ref,
            result: result.clone(),
        });
        Ok(result)
    }

    fn resolve(
        window: &str,
        path: &Path,
        registered: Option<Entry>,
        active: Option<&Active>,
    ) -> Result<Selected> {
        if let Some(active) = active {
            let selected =
                Selected::new(active.entry.clone(), true, active.identity.clone(), window);
            selected.check_identity()?;
            return Ok(selected);
        }

        let _lease = FileLease::ledger(path)?;
        let identity = Arc::new(files::identity(path)?);
        let protection = match (files::is_plain(path)?, files::has_wrapper(path)?) {
            (true, false) => Protection::Plain,
            (false, true) => Protection::Encrypted,
            (true, true) | (false, false) => return Err(bad_credential()),
        };
        if registered
            .as_ref()
            .is_some_and(|entry| entry.protection != protection)
        {
            return Err(bad_credential());
        }
        let known = registered.is_some();
        let entry = registered.unwrap_or_else(|| Entry {
            instance: Uuid::new_v4(),
            path: path.to_owned(),
            label: path
                .file_name()
                .expect("validated file path")
                .to_string_lossy()
                .chars()
                .filter(|ch| !ch.is_control())
                .take(64)
                .collect(),
            protection,
            storage_policy: StoragePolicy::Portable,
            quick_unlock: false,
        });
        let selected = Selected::new(entry, known, identity, window);
        selected.check_identity()?;
        Ok(selected)
    }

    pub fn unlock(
        &self,
        window: &str,
        challenge: &str,
        credential: Option<Credential>,
    ) -> Result<LedgerSession> {
        let mut inner = self.inner()?;
        let State::Locked(selected) = &inner.state else {
            return Err(stale());
        };
        if selected.window != window
            || Uuid::parse_str(challenge).ok() != Some(selected.challenge)
            || selected.expires <= Instant::now()
        {
            return Err(stale());
        }
        let generation = inner.next_generation()?;
        let State::Locked(selected) = std::mem::replace(&mut inner.state, State::Opening) else {
            unreachable!("state validated under the same mutex");
        };
        let result = (|| {
            selected.check_identity()?;
            let lease = FileLease::ledger(&selected.entry.path)?;
            selected.check_identity()?;
            let wrapper = files::read_wrapper(&selected.entry.path)?;
            let key = if let Some(credential) = credential {
                wrapper.unlock(&credential)?
            } else if selected.entry.quick_unlock {
                self.keychain.retrieve(selected.entry.instance)?
            } else {
                return Err(AppError::validation("credential", "credential_required"));
            };
            let db = Database::reopen(&selected.entry.path, lease, Some(&key))?;
            db.write(|tx| entry_vault(&selected.entry, &db).recover(tx))?;
            let entry = inner.register(&selected, &db)?;
            Ok((entry, db))
        })();
        match result {
            Ok((entry, db)) => Ok(self.ready(&mut inner, generation, entry, window, db)),
            Err(error) => {
                inner.state = State::Locked(selected);
                Err(error)
            }
        }
    }

    pub(super) fn ready(
        &self,
        inner: &mut Inner,
        generation: u64,
        entry: Entry,
        window: &str,
        mut db: Database,
    ) -> LedgerSession {
        let mut generator = Sha256::new();
        generator.update(self.epoch.as_bytes());
        generator.update(generation.to_be_bytes());
        let mut opaque_id = [0; 16];
        opaque_id.copy_from_slice(&generator.finalize()[..16]);
        let scope = Scope {
            session_id: Uuid::from_bytes(opaque_id).to_string(),
            ledger_instance_id: entry.instance.to_string(),
        };
        let session = LedgerSession {
            scope,
            ledger_uuid: db.info.uuid.clone(),
            name: db.info.name.clone(),
            base_currency: db.info.base_currency.clone(),
            revision: db.info.revision.clone(),
            protection: entry.protection,
        };
        let alerts = self.integrity_alerts.clone();
        let alert_scope = session.scope.clone();
        db.start_verification(move |error| {
            if let Some(alerts) = &alerts {
                let _ = alerts.try_send((alert_scope.clone(), error));
            }
        });
        inner.state = State::Ready(Arc::new(Active {
            entry,
            identity: db.identity.clone(),
            window: window.to_owned(),
            session: session.clone(),
            accepting: AtomicBool::new(true),
            database: RwLock::new(Some(db)),
        }));
        session
    }

    pub fn lock(&self, window: &str, scope: &Scope) -> Result<LifecycleState> {
        let mut inner = self.inner()?;
        let active = inner.scoped(window, scope)?;
        let selected = Selected::new(
            inner.registry.get(active.entry.instance)?,
            true,
            active.identity.clone(),
            window,
        );
        inner.quiesce()?;
        inner.state = State::Locked(selected);
        Ok(LifecycleState::Locked)
    }

    pub fn integrity_verified(&self, window: &str, scope: &Scope) -> Result<bool> {
        self.with_database(window, scope, |database, _| database.integrity_verified())
    }

    pub fn read<T>(
        &self,
        window: &str,
        scope: &Scope,
        work: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<T>,
    ) -> Result<T> {
        self.with_database(window, scope, |database, _| {
            database.read(|tx| work(tx).map_err(crate::storage::sql_error))
        })
    }

    pub fn write<T>(
        &self,
        window: &str,
        scope: &Scope,
        work: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<T>,
    ) -> Result<T> {
        self.with_database(window, scope, |database, _| {
            database.write(|tx| work(tx).map_err(crate::storage::sql_error))
        })
    }

    pub fn read_domain<T>(
        &self,
        window: &str,
        scope: &Scope,
        work: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        self.with_database(window, scope, |database, _| database.read(work))
    }

    pub fn write_domain<T>(
        &self,
        window: &str,
        scope: &Scope,
        work: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        self.with_database(window, scope, |database, _| database.write(work))
    }

    pub fn read_cancellable<T>(
        &self,
        window: &str,
        scope: &Scope,
        work: impl FnOnce(&Transaction<'_>, &dyn Fn() -> bool) -> Result<T>,
    ) -> Result<T> {
        self.with_database(window, scope, |database, accepting| {
            database.read(|tx| work(tx, &|| !accepting.load(Ordering::Acquire)))
        })
    }

    pub(super) fn with_vault<T>(
        &self,
        window: &str,
        scope: &Scope,
        work: impl FnOnce(&Database, &Vault<'_>, &dyn Fn() -> bool) -> Result<T>,
    ) -> Result<T> {
        let active = self.inner()?.scoped(window, scope)?;
        let slot = active.database.try_read().ok_or_else(AppError::busy)?;
        if !active.accepting.load(Ordering::Acquire) {
            return Err(stale());
        }
        let db = slot.as_ref().ok_or_else(stale)?;
        let _fence = db.file_fence.try_lock().ok_or_else(AppError::busy)?;
        work(db, &entry_vault(&active.entry, db), &|| {
            !active.accepting.load(Ordering::Acquire)
        })
    }

    pub(super) fn with_database<T>(
        &self,
        window: &str,
        scope: &Scope,
        work: impl FnOnce(&Database, &AtomicBool) -> Result<T>,
    ) -> Result<T> {
        let active = self.inner()?.scoped(window, scope)?;
        let slot = active.database.try_read().ok_or_else(AppError::busy)?;
        if !active.accepting.load(Ordering::Acquire) {
            return Err(stale());
        }
        work(slot.as_ref().ok_or_else(stale)?, &active.accepting)
    }
}

impl Inner {
    pub(super) fn active_at(&self, path: &Path) -> Option<&Active> {
        match &self.state {
            State::Ready(active) if active.entry.path == path => Some(active),
            _ => None,
        }
    }

    pub(super) fn next_generation(&mut self) -> Result<u64> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| AppError::new(ErrorCode::Integrity, "session_generation_exhausted"))?;
        Ok(self.generation)
    }

    pub(super) fn register(&mut self, selected: &Selected, database: &Database) -> Result<Entry> {
        let mut entry = selected.entry.clone();
        if !selected.registered {
            if !valid_name(&database.info.name) {
                return Err(AppError::new(ErrorCode::Integrity, "ledger_name_invalid"));
            }
            entry.label = database.info.name.clone();
            self.registry.add(entry.clone())?;
        }
        Ok(entry)
    }

    pub(super) fn current_open(&self, window: &str) -> Option<OpenResult> {
        match &self.state {
            State::Ready(active) if active.window == window => Some(OpenResult {
                state: LifecycleState::Ready,
                session: Some(active.session.clone()),
                unlock_challenge: None,
            }),
            State::Locked(selected)
                if selected.window == window && selected.expires > Instant::now() =>
            {
                Some(OpenResult {
                    state: LifecycleState::Locked,
                    session: None,
                    unlock_challenge: Some(selected.descriptor()),
                })
            }
            _ => None,
        }
    }

    pub(super) fn scoped(&self, window: &str, scope: &Scope) -> Result<Arc<Active>> {
        match &self.state {
            State::Ready(active) if active.window == window && active.session.scope == *scope => {
                Ok(active.clone())
            }
            State::Ready(_) => Err(stale()),
            State::Locked(_) | State::Unselected => Err(unavailable()),
            _ => Err(AppError::busy()),
        }
    }

    pub(super) fn quiesce(&mut self) -> Result<()> {
        self.quiesce_at(None)
    }

    pub(super) fn quiesce_at(&mut self, expected_revision: Option<&str>) -> Result<()> {
        let previous = std::mem::replace(&mut self.state, State::Quiescing);
        if let State::Ready(active) = previous {
            active.accepting.store(false, Ordering::Release);
            if let Some(slot) = active.database.try_read()
                && let Some(db) = slot.as_ref()
            {
                db.interrupt_readers();
            }
            let drained = match active.database.try_write_for(Duration::from_secs(5)) {
                Some(mut slot) => (|| {
                    if let Some(expected) = expected_revision {
                        let actual = slot
                            .as_ref()
                            .ok_or_else(stale)?
                            .read(crate::ledger_engine::native_revision)?;
                        if actual != expected {
                            return Err(crate::ledger_engine::stale_revision());
                        }
                    }
                    if let Some(db) = slot.take() {
                        db.close();
                    }
                    Ok(())
                })(),
                None => Err(AppError::busy()),
            };
            if let Err(error) = drained {
                active.accepting.store(true, Ordering::Release);
                self.state = State::Ready(active);
                return Err(error);
            }
        }
        self.state = State::Unselected;
        Ok(())
    }
}
