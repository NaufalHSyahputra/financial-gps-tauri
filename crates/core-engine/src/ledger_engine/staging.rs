#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
use std::{collections::HashMap, sync::Arc, time::Duration};
#[cfg(target_arch = "wasm32")]
use web_time::Instant;

use super::{Context, invalid, new_id, revision, stale_revision};
use crate::domain::{AppError, Result, Scope};
use parking_lot::Mutex;

struct Entry<T> {
    scope: Scope,
    revision: String,
    expires: Instant,
    value: Arc<T>,
}

pub struct Stage<T>(Mutex<HashMap<String, Entry<T>>>);

impl<T> Default for Stage<T> {
    fn default() -> Self {
        Self(Mutex::new(HashMap::new()))
    }
}

impl<T> Stage<T> {
    pub fn put(&self, ctx: &Context<'_>, value: T) -> Result<String> {
        let mut items = self.0.lock();
        let now = Instant::now();
        items.retain(|_, item| item.expires > now);
        if items.len() >= 8 {
            return Err(AppError::busy());
        }
        let token = new_id();
        items.insert(
            token.clone(),
            Entry {
                scope: ctx.scope.clone(),
                revision: revision(&ctx.tx)?,
                expires: now + Duration::from_secs(300),
                value: Arc::new(value),
            },
        );
        Ok(token)
    }

    pub fn get(&self, ctx: &Context<'_>, token: &str) -> Result<Arc<T>> {
        let items = self.0.lock();
        let item = items
            .get(token)
            .filter(|item| item.scope == *ctx.scope && Instant::now() < item.expires)
            .ok_or_else(|| invalid("previewToken", "invalid_or_expired_preview"))?;
        if item.revision != revision(&ctx.tx)? {
            return Err(stale_revision());
        }
        Ok(item.value.clone())
    }

    pub fn invalidate(&self, scope: &Scope) {
        self.0.lock().retain(|_, item| item.scope != *scope);
    }
}
