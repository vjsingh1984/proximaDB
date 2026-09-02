//! Exclusive ownership of every local writable root used by a server.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::Context;
use proximadb::core::Config;
use proximadb_runtime_common::{AccessMode, FileLockSet};

/// Holds all root locks for the server lifetime. Partial acquisition is safe:
/// already-acquired locks drop if a later overlapping root is unavailable.
pub(crate) struct StorageOwnershipLease {
    _locks: FileLockSet,
}

impl StorageOwnershipLease {
    pub(crate) fn acquire(config: &Config) -> anyhow::Result<Self> {
        let roots = local_writable_roots(config)?;
        let locks = FileLockSet::acquire(&roots, AccessMode::Exclusive).with_context(|| {
            "one or more local storage roots are already owned; connect to the existing ProximaDB process instead of opening its files again"
        })?;
        for root in locks.roots() {
            tracing::info!(root = %root.display(), "acquired local-storage ownership");
        }
        Ok(Self { _locks: locks })
    }
}

fn local_writable_roots(config: &Config) -> anyhow::Result<Vec<PathBuf>> {
    let mut candidates = vec![config.server.data_dir.clone()];
    push_local_path(&mut candidates, &config.storage.metadata_url);
    for location in &config.storage.storage_locations {
        push_local_path(&mut candidates, &location.url);
    }
    if config.storage.wal_config.enable_wal {
        push_local_path(
            &mut candidates,
            &config.storage.wal_config.write_buffer_directory,
        );
        if let Some(path) = config.storage.wal_config.wal_local_dir.as_deref() {
            push_local_path(&mut candidates, path);
        }
    }
    if let Some(sst) = config.storage.sst_config.as_ref() {
        push_local_path(&mut candidates, &sst.data_directory);
        if let Some(path) = sst.cache_local_disk_path.as_deref() {
            push_local_path(&mut candidates, path);
        }
    }
    if let Some(viper) = config.storage.viper_config.as_ref() {
        push_local_path(&mut candidates, &viper.data_directory);
    }

    let mut roots = BTreeSet::new();
    for candidate in candidates {
        std::fs::create_dir_all(&candidate)
            .with_context(|| format!("create local storage root {}", candidate.display()))?;
        roots.insert(
            candidate
                .canonicalize()
                .with_context(|| format!("canonicalize local root {}", candidate.display()))?,
        );
    }
    Ok(roots.into_iter().collect())
}

fn push_local_path(paths: &mut Vec<PathBuf>, value: &str) {
    if let Some(path) = value.strip_prefix("file://") {
        paths.push(PathBuf::from(path));
    } else if !value.contains("://") {
        paths.push(PathBuf::from(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_config(root: &std::path::Path) -> Config {
        let mut config = Config::default();
        config.server.data_dir = root.join("instance");
        config.storage.storage_locations[0].url =
            format!("file://{}", root.join("storage").display());
        config.storage.metadata_url = format!("file://{}", root.join("metadata").display());
        config.storage.wal_config.enable_wal = false;
        config.storage.sst_config = None;
        config.storage.viper_config = None;
        config
    }

    #[test]
    fn rejects_a_second_owner_for_the_same_roots() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = local_config(temp.path());
        let first = StorageOwnershipLease::acquire(&config).expect("first owner");
        assert!(StorageOwnershipLease::acquire(&config).is_err());
        drop(first);
        StorageOwnershipLease::acquire(&config).expect("released owner");
    }

    #[test]
    fn shared_root_conflicts_even_when_data_dirs_differ() {
        let temp = tempfile::tempdir().expect("tempdir");
        let shared = temp.path().join("shared");
        let mut first = local_config(&temp.path().join("first"));
        let mut second = local_config(&temp.path().join("second"));
        first.storage.storage_locations[0].url = format!("file://{}", shared.display());
        second.storage.storage_locations[0].url = format!("file://{}", shared.display());
        let _lease = StorageOwnershipLease::acquire(&first).expect("first owner");
        assert!(StorageOwnershipLease::acquire(&second).is_err());
    }

    #[test]
    fn remote_urls_are_not_treated_as_local_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = local_config(temp.path());
        config.storage.storage_locations[0].url = "s3://bucket/prefix".into();
        let roots = local_writable_roots(&config).expect("roots");
        assert!(roots.iter().all(|root| !root.ends_with("bucket/prefix")));
    }
}
