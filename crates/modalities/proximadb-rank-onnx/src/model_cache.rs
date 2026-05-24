//! `OnnxModelCache` — refcounted, LRU-evictable session pool.
//!
//! `acquire(key) → ScorerToken` is the public read path. Tokens hold a
//! cloned `Arc<dyn ScorerSession>`; the cache holds the same `Arc`. LRU
//! eviction checks `Arc::strong_count` and *never* evicts a session
//! with outstanding tokens (matches Vespa's `OnnxModelCache::Token`).
//!
//! v1 has no built-in lazy load — `install(session)` is the caller's
//! responsibility. R-5b will add `acquire_with_loader(...)` that fetches
//! from object storage on miss.

use crate::descriptor::ModelKey;
use crate::scorer_session::ScorerSession;
use dashmap::DashMap;
use proximadb_rank_core::{RankError, RankResult};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// LRU eviction strategy.
#[derive(Debug, Clone)]
pub enum EvictionPolicy {
    /// Evict oldest-unused sessions until total memory ≤ `budget_bytes`.
    LruByMemory { budget_bytes: usize },
    /// Evict oldest-unused sessions until cache size ≤ `max_entries`.
    LruByCount { max_entries: usize },
    /// Per-tenant budget. Tracked separately so one tenant can't squeeze
    /// out another. R-5b will implement; v1 falls back to LruByMemory.
    Tenanted { per_tenant_budget_bytes: usize },
}

/// Refcounted handle to a loaded session. While at least one token
/// exists, the cache will not evict the underlying session.
#[derive(Clone)]
pub struct ScorerToken {
    session: Arc<dyn ScorerSession>,
}

impl ScorerToken {
    pub fn session(&self) -> &dyn ScorerSession {
        self.session.as_ref()
    }
    /// Number of outstanding tokens (including this one) plus the cache
    /// entry's reference. Exposed for tests.
    pub fn refcount(&self) -> usize {
        Arc::strong_count(&self.session)
    }
}

impl std::ops::Deref for ScorerToken {
    type Target = dyn ScorerSession;
    fn deref(&self) -> &Self::Target {
        self.session.as_ref()
    }
}

struct CacheEntry {
    session: Arc<dyn ScorerSession>,
    last_used_at_ms: AtomicI64,
}

impl CacheEntry {
    fn touch(&self) {
        self.last_used_at_ms.store(now_ms(), Ordering::Relaxed);
    }
    fn last_used(&self) -> i64 {
        self.last_used_at_ms.load(Ordering::Relaxed)
    }
}

pub struct OnnxModelCache {
    entries: DashMap<ModelKey, Arc<CacheEntry>>,
    policy: EvictionPolicy,
}

impl OnnxModelCache {
    pub fn new(policy: EvictionPolicy) -> Self {
        Self {
            entries: DashMap::new(),
            policy,
        }
    }

    /// Insert a loaded session and return a token holding the first
    /// outstanding reference. Replaces any prior entry for the same key
    /// (last-write-wins, matches the catalog's hot-reload semantics).
    /// Triggers an eviction pass after install.
    pub fn install(&self, session: Arc<dyn ScorerSession>) -> ScorerToken {
        let key = session.descriptor().key.clone();
        let entry = Arc::new(CacheEntry {
            session: session.clone(),
            last_used_at_ms: AtomicI64::new(now_ms()),
        });
        self.entries.insert(key, entry);
        let token = ScorerToken { session };
        let _ = self.evict_if_over_budget();
        token
    }

    /// Look up a loaded session by key. Updates last-used timestamp on
    /// hit. Returns `Err(ProfileNotFound)` on miss — R-5b will overload
    /// this with the loader path.
    pub fn acquire(&self, key: &ModelKey) -> RankResult<ScorerToken> {
        match self.entries.get(key) {
            Some(r) => {
                r.touch();
                Ok(ScorerToken {
                    session: r.session.clone(),
                })
            }
            None => Err(RankError::ProfileNotFound(format!(
                "model not loaded into cache: {key}"
            ))),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Sum of `memory_bytes()` across resident sessions.
    pub fn total_memory_bytes(&self) -> usize {
        self.entries
            .iter()
            .map(|r| r.value().session.memory_bytes())
            .sum()
    }

    /// Force an LRU pass. Returns bytes freed. Evicts in order of
    /// oldest `last_used_at`. Sessions with outstanding tokens
    /// (`Arc::strong_count > 1`) are NEVER evicted regardless of age
    /// or budget.
    pub fn evict_if_over_budget(&self) -> usize {
        let (budget_bytes, by_count_max) = match &self.policy {
            EvictionPolicy::LruByMemory { budget_bytes } => (Some(*budget_bytes), None),
            EvictionPolicy::LruByCount { max_entries } => (None, Some(*max_entries)),
            EvictionPolicy::Tenanted {
                per_tenant_budget_bytes,
            } => {
                // R-5b: per-tenant accounting. Fall back to a global
                // budget that's tenant_budget * 10 so things at least
                // don't grow unbounded.
                (Some(per_tenant_budget_bytes * 10), None)
            }
        };

        if budget_bytes.is_none() && by_count_max.is_none() {
            return 0;
        }

        let mut candidates: Vec<(ModelKey, i64, usize)> = self
            .entries
            .iter()
            .filter_map(|r| {
                // strong_count is at least 1 from CacheEntry.session +
                // however many tokens. We're evictable iff no token
                // holds a clone, which means strong_count == 1.
                if Arc::strong_count(&r.value().session) == 1 {
                    Some((
                        r.key().clone(),
                        r.value().last_used(),
                        r.value().session.memory_bytes(),
                    ))
                } else {
                    None
                }
            })
            .collect();
        candidates.sort_by_key(|(_, ts, _)| *ts); // oldest first

        let mut freed = 0;
        let mut current_bytes = self.total_memory_bytes();
        let mut current_count = self.entries.len();

        for (key, _, _) in candidates {
            let should_evict = match (budget_bytes, by_count_max) {
                (Some(b), _) if current_bytes > b => true,
                (_, Some(m)) if current_count > m => true,
                _ => false,
            };
            if !should_evict {
                break;
            }
            // Double-check refcount under the remove — another thread
            // may have acquired between our scan and now.
            if let Some((_, entry)) = self.entries.remove_if(&key, |_, v| {
                Arc::strong_count(&v.session) == 1
            }) {
                let b = entry.session.memory_bytes();
                freed += b;
                current_bytes = current_bytes.saturating_sub(b);
                current_count = current_count.saturating_sub(1);
            }
        }
        freed
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{DType, ModelDescriptor, ModelFramework};
    use crate::scorer_session::MockScorerSession;

    fn descriptor(id: &str, size_bytes: u64) -> ModelDescriptor {
        ModelDescriptor {
            key: ModelKey::new(id, "1"),
            tenant: None,
            uri: format!("file:///tmp/{id}.onnx"),
            sha256: [0; 32],
            size_bytes,
            framework: ModelFramework::Onnx,
            dtype: DType::Fp32,
            input_spec: vec![],
            output_spec: vec![],
            max_batch_size: 8,
            seq: 0,
            created_at_ms: 0,
        }
    }

    fn session(id: &str, size: u64) -> Arc<dyn ScorerSession> {
        Arc::new(MockScorerSession::zeros(descriptor(id, size)))
    }

    #[test]
    fn install_then_acquire_returns_token() {
        let cache = OnnxModelCache::new(EvictionPolicy::LruByMemory {
            budget_bytes: usize::MAX,
        });
        let t = cache.install(session("a", 100));
        drop(t); // release the install-time token
        let acquired = cache.acquire(&ModelKey::new("a", "1")).unwrap();
        assert_eq!(acquired.descriptor().key.model_id, "a");
    }

    #[test]
    fn acquire_unknown_key_errors() {
        let cache = OnnxModelCache::new(EvictionPolicy::LruByMemory {
            budget_bytes: usize::MAX,
        });
        match cache.acquire(&ModelKey::new("nope", "1")) {
            Err(RankError::ProfileNotFound(_)) => {}
            Err(_) => panic!("expected ProfileNotFound, got a different RankError"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[test]
    fn scorer_token_keeps_session_alive() {
        let cache = OnnxModelCache::new(EvictionPolicy::LruByMemory { budget_bytes: 50 });
        let t = cache.install(session("a", 200)); // way over budget
        // While token is alive, eviction must NOT remove the entry
        // (refcount > 1 because cache + token both hold the Arc).
        let freed = cache.evict_if_over_budget();
        assert_eq!(freed, 0, "in-use session must not be evicted");
        assert_eq!(cache.len(), 1);
        drop(t);
        let freed = cache.evict_if_over_budget();
        assert_eq!(freed, 200, "after drop, eviction must reclaim");
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn lru_evicts_oldest_first() {
        let cache = OnnxModelCache::new(EvictionPolicy::LruByMemory { budget_bytes: 250 });
        let t1 = cache.install(session("a", 100));
        drop(t1);
        // Sleep a tick so timestamps differ.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let t2 = cache.install(session("b", 100));
        drop(t2);
        std::thread::sleep(std::time::Duration::from_millis(2));
        let t3 = cache.install(session("c", 100));
        drop(t3);
        // total = 300, budget = 250 → evict the oldest (a)
        cache.evict_if_over_budget();
        assert!(cache.acquire(&ModelKey::new("a", "1")).is_err());
        assert!(cache.acquire(&ModelKey::new("b", "1")).is_ok());
        assert!(cache.acquire(&ModelKey::new("c", "1")).is_ok());
    }

    #[test]
    fn lru_eviction_respects_inuse_refcount_under_pressure() {
        let cache = OnnxModelCache::new(EvictionPolicy::LruByMemory { budget_bytes: 50 });
        // Install two sessions, keep a token on the OLDER one.
        let pinned = cache.install(session("a", 100));
        std::thread::sleep(std::time::Duration::from_millis(2));
        let throwaway = cache.install(session("b", 100));
        drop(throwaway);
        // The oldest is "a" but it's pinned by `pinned`. Eviction must
        // evict "b" instead even though it's newer.
        cache.evict_if_over_budget();
        assert!(
            cache.acquire(&ModelKey::new("a", "1")).is_ok(),
            "pinned session must survive eviction"
        );
        assert!(
            cache.acquire(&ModelKey::new("b", "1")).is_err(),
            "unpinned session must be evicted"
        );
        drop(pinned);
    }

    #[test]
    fn lru_by_count_caps_entries() {
        let cache = OnnxModelCache::new(EvictionPolicy::LruByCount { max_entries: 2 });
        let _ = cache.install(session("a", 1));
        std::thread::sleep(std::time::Duration::from_millis(2));
        let _ = cache.install(session("b", 1));
        std::thread::sleep(std::time::Duration::from_millis(2));
        let _ = cache.install(session("c", 1));
        // total entries should be at most 2 after eviction.
        cache.evict_if_over_budget();
        assert!(cache.len() <= 2);
    }

    #[test]
    fn install_replaces_existing_entry() {
        let cache = OnnxModelCache::new(EvictionPolicy::LruByMemory {
            budget_bytes: usize::MAX,
        });
        let _ = cache.install(session("a", 100));
        let _ = cache.install(session("a", 200));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.total_memory_bytes(), 200);
    }

    #[test]
    fn refcount_reflects_outstanding_tokens() {
        let cache = OnnxModelCache::new(EvictionPolicy::LruByMemory {
            budget_bytes: usize::MAX,
        });
        let t1 = cache.install(session("a", 100));
        // cache holds 1 (CacheEntry.session), t1 holds 1 → 2
        assert_eq!(t1.refcount(), 2);
        let t2 = cache.acquire(&ModelKey::new("a", "1")).unwrap();
        assert_eq!(t1.refcount(), 3);
        drop(t2);
        assert_eq!(t1.refcount(), 2);
    }

    #[test]
    fn evict_with_no_budget_set_is_noop() {
        // Tenanted policy with a budget that, when multiplied by 10,
        // still exceeds the load → no eviction.
        let cache = OnnxModelCache::new(EvictionPolicy::Tenanted {
            per_tenant_budget_bytes: 1_000_000,
        });
        let _ = cache.install(session("a", 100));
        let freed = cache.evict_if_over_budget();
        assert_eq!(freed, 0);
    }
}
