//! Durable ordering tokens carried by object-store WAL objects (ADR-063 / TD-OBJSTORE-4).

use anyhow::{Result, anyhow, bail};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RecoveryToken {
    pub tenant_id: String,
    pub epoch: u64,
    pub sequence: u64,
}

#[derive(Debug)]
struct TokenState {
    tenant_id: String,
    epoch: u64,
    next_sequence: AtomicU64,
    expires_at_ms: AtomicI64,
}

#[derive(Debug, Default)]
pub struct RecoveryTokenProvider {
    states: DashMap<String, Arc<TokenState>>,
}

impl RecoveryTokenProvider {
    pub fn global() -> Arc<Self> {
        static PROVIDER: OnceLock<Arc<RecoveryTokenProvider>> = OnceLock::new();
        PROVIDER.get_or_init(|| Arc::new(Self::default())).clone()
    }

    /// Install/refresh the durable writer incarnation for a canonical collection UUID.
    pub fn register_incarnation(
        &self,
        tenant_id: &str,
        collection_id: &str,
        epoch: u64,
        expires_at_ms: i64,
    ) {
        if let Some(existing) = self.states.get(collection_id)
            && existing.tenant_id == tenant_id
            && existing.epoch == epoch
        {
            existing
                .expires_at_ms
                .store(expires_at_ms, Ordering::Release);
            return;
        }
        self.states.insert(
            collection_id.to_string(),
            Arc::new(TokenState {
                tenant_id: tenant_id.to_string(),
                epoch,
                next_sequence: AtomicU64::new(0),
                expires_at_ms: AtomicI64::new(expires_at_ms),
            }),
        );
    }

    pub fn allocate(&self, collection_id: &str, now_ms: i64) -> Result<RecoveryToken> {
        if let Some(state) = self.states.get(collection_id) {
            let expiry = state.expires_at_ms.load(Ordering::Acquire);
            if now_ms >= expiry {
                bail!(
                    "writer incarnation for collection {collection_id} expired at {expiry}; refusing WAL acknowledgement"
                );
            }
            return Ok(RecoveryToken {
                tenant_id: state.tenant_id.clone(),
                epoch: state.epoch,
                sequence: state.next_sequence.fetch_add(1, Ordering::AcqRel),
            });
        }

        if certified_mode() {
            bail!(
                "no durable writer incarnation for collection {collection_id} in certified object-store mode"
            );
        }

        // Embedded/R&D compatibility only. Certified deployments fail closed above.
        static FALLBACK_EPOCH: OnceLock<u64> = OnceLock::new();
        let epoch = *FALLBACK_EPOCH.get_or_init(|| {
            let micros = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros() as u64;
            micros ^ u64::from(std::process::id())
        });
        let state = Arc::new(TokenState {
            tenant_id: String::new(),
            epoch,
            next_sequence: AtomicU64::new(1),
            expires_at_ms: AtomicI64::new(i64::MAX),
        });
        match self.states.entry(collection_id.to_string()) {
            dashmap::mapref::entry::Entry::Occupied(entry) => {
                let state = entry.get();
                Ok(RecoveryToken {
                    tenant_id: state.tenant_id.clone(),
                    epoch: state.epoch,
                    sequence: state.next_sequence.fetch_add(1, Ordering::AcqRel),
                })
            }
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                entry.insert(state);
                Ok(RecoveryToken {
                    tenant_id: String::new(),
                    epoch,
                    sequence: 0,
                })
            }
        }
    }

    pub fn expire(&self, collection_id: &str) -> Result<()> {
        let state = self
            .states
            .get(collection_id)
            .ok_or_else(|| anyhow!("unknown writer incarnation for {collection_id}"))?;
        state.expires_at_ms.store(0, Ordering::Release);
        Ok(())
    }
}

pub fn certified_mode() -> bool {
    std::env::var("PROXIMADB_SERVERLESS_CERTIFIED")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_incarnation_allocates_monotonic_sequences_and_expires_closed() {
        let provider = RecoveryTokenProvider::default();
        provider.register_incarnation("tenant", "collection", 9, 100);
        assert_eq!(provider.allocate("collection", 1).unwrap().sequence, 0);
        assert_eq!(provider.allocate("collection", 2).unwrap().sequence, 1);
        assert!(provider.allocate("collection", 100).is_err());
    }
}
