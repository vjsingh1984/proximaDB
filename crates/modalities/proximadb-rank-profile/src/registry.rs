//! `ProfileRegistry` — lock-free in-memory cache of compiled profiles
//! with RCU (Arc-swap) hot-reload.
//!
//! Per spec §4.5.5, each query hands out a cheap `Arc<CompiledRankProfile>`
//! by calling [`ProfileRegistry::get`]. When a catalog watcher fires,
//! [`ProfileRegistry::install`] atomically swaps in a new compiled profile.
//! In-flight queries continue with their captured Arc; the old version is
//! dropped only after the last in-flight reference goes away. No locks on
//! the hot path.

use crate::compiled::CompiledRankProfile;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::sync::Arc;

#[derive(Default)]
pub struct ProfileRegistry {
    inner: DashMap<String, Arc<ArcSwap<CompiledRankProfile>>>,
}

impl ProfileRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up the current version of a profile. O(1) DashMap lookup +
    /// O(1) ArcSwap load. Returns `None` if no profile with that name
    /// has ever been installed.
    pub fn get(&self, name: &str) -> Option<Arc<CompiledRankProfile>> {
        self.inner.get(name).map(|r| r.load_full())
    }

    /// Atomically replace (or insert) the compiled profile keyed by
    /// `profile.spec.name`. In-flight queries holding the prior `Arc`
    /// continue to see the old version until they drop it.
    pub fn install(&self, profile: CompiledRankProfile) {
        let key = profile.spec.name.clone();
        let new_arc = Arc::new(profile);
        // Use DashMap entry API to avoid a write race between two installers
        // for the same name.
        match self.inner.entry(key) {
            dashmap::Entry::Vacant(e) => {
                e.insert(Arc::new(ArcSwap::from(new_arc)));
            }
            dashmap::Entry::Occupied(e) => {
                e.get().store(new_arc);
            }
        }
    }

    pub fn remove(&self, name: &str) -> bool {
        self.inner.remove(name).is_some()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn registered_names(&self) -> Vec<String> {
        self.inner.iter().map(|r| r.key().clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{PhaseSpec, RankProfileSpec};
    use proximadb_rank_core::BlueprintFactory;
    use proximadb_rank_features::register_builtins;
    use std::collections::HashSet;

    fn factory() -> Arc<BlueprintFactory> {
        let f = Arc::new(BlueprintFactory::new());
        register_builtins(&f);
        f
    }

    fn make_profile(name: &str, version: u32) -> CompiledRankProfile {
        let mut spec = RankProfileSpec::new(name);
        spec.first_phase = Some(PhaseSpec {
            expression: format!("{}", version as f64),
            heap_size: Some(10),
            rerank_count: None,
            batch_size: None,
        });
        spec.version = version;
        CompiledRankProfile::compile(spec, factory()).unwrap()
    }

    #[test]
    fn install_then_get_returns_latest() {
        let reg = ProfileRegistry::new();
        reg.install(make_profile("a", 1));
        let p = reg.get("a").unwrap();
        assert_eq!(p.spec.version, 1);
    }

    #[test]
    fn install_replaces_via_arc_swap() {
        let reg = ProfileRegistry::new();
        reg.install(make_profile("a", 1));
        reg.install(make_profile("a", 2));
        let p = reg.get("a").unwrap();
        assert_eq!(p.spec.version, 2);
        assert_eq!(reg.len(), 1, "install must not duplicate the entry");
    }

    #[test]
    fn get_unknown_returns_none() {
        let reg = ProfileRegistry::new();
        assert!(reg.get("ghost").is_none());
    }

    #[test]
    fn remove_drops_entry() {
        let reg = ProfileRegistry::new();
        reg.install(make_profile("a", 1));
        assert!(reg.remove("a"));
        assert!(reg.get("a").is_none());
        assert!(!reg.remove("a"));
    }

    #[test]
    fn registered_names_round_trip() {
        let reg = ProfileRegistry::new();
        reg.install(make_profile("a", 1));
        reg.install(make_profile("b", 1));
        let mut names = reg.registered_names();
        names.sort();
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
    }

    #[tokio::test]
    async fn hot_reload_atomic_swap_under_concurrent_reads() {
        // Spawn a reader that loops getting "test"; concurrently install
        // many versions. The reader must always observe a *complete*
        // profile (no torn read) and should observe multiple versions
        // over the run.
        let reg = Arc::new(ProfileRegistry::new());
        reg.install(make_profile("test", 1));

        let reader_reg = reg.clone();
        let reader = tokio::spawn(async move {
            let mut seen: HashSet<u32> = HashSet::new();
            for _ in 0..1000 {
                if let Some(p) = reader_reg.get("test") {
                    // Touch the version to make sure the read is real.
                    seen.insert(p.spec.version);
                }
                tokio::task::yield_now().await;
            }
            seen
        });

        for v in 2..50 {
            reg.install(make_profile("test", v));
            tokio::task::yield_now().await;
        }

        let seen = reader.await.unwrap();
        assert!(
            seen.len() > 1,
            "reader should observe multiple versions during hot-reload, saw: {seen:?}"
        );
    }

    #[tokio::test]
    async fn concurrent_installs_do_not_duplicate() {
        let reg = Arc::new(ProfileRegistry::new());
        let mut handles = Vec::new();
        for v in 1..=10u32 {
            let reg = reg.clone();
            handles.push(tokio::spawn(async move {
                reg.install(make_profile("shared", v));
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(reg.len(), 1, "concurrent installs must collapse to one entry");
        assert!(reg.get("shared").is_some());
    }
}
