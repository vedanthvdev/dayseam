//! An in-process [`SecretStore`] used by unit and integration tests.
//! Values live in a `Mutex<HashMap>` and are zeroed when the map is
//! dropped (via `Secret<String>`'s own `Drop`).

use std::collections::HashMap;
use std::sync::Mutex;

use crate::{Secret, SecretError, SecretResult, SecretStore};

#[derive(Default)]
pub struct InMemoryStore {
    inner: Mutex<HashMap<String, String>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for InMemoryStore {
    fn put(&self, key: &str, value: Secret<String>) -> SecretResult<()> {
        let mut guard = self.inner.lock().map_err(|e| SecretError::Backend {
            message: e.to_string(),
        })?;
        // `into_inner` hands us the raw String without running `Drop` —
        // the HashMap now owns it, and it'll be zeroed on the next `put`
        // (which overwrites) or on store drop when the HashMap itself is
        // cleared. For extra safety we explicitly zeroize the previous
        // value if one was there.
        let incoming = value.into_inner();
        if let Some(mut prev) = guard.insert(key.to_string(), incoming) {
            use zeroize::Zeroize;
            prev.zeroize();
        }
        Ok(())
    }

    fn get(&self, key: &str) -> SecretResult<Option<Secret<String>>> {
        let guard = self.inner.lock().map_err(|e| SecretError::Backend {
            message: e.to_string(),
        })?;
        Ok(guard.get(key).cloned().map(Secret::new))
    }

    fn delete(&self, key: &str) -> SecretResult<()> {
        let mut guard = self.inner.lock().map_err(|e| SecretError::Backend {
            message: e.to_string(),
        })?;
        if let Some(mut prev) = guard.remove(key) {
            use zeroize::Zeroize;
            prev.zeroize();
        }
        Ok(())
    }
}

impl Drop for InMemoryStore {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.inner.lock() {
            use zeroize::Zeroize;
            for (_, v) in guard.iter_mut() {
                v.zeroize();
            }
        }
    }
}
