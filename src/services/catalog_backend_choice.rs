// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Which catalog backend serves as `default`, and **why** — decided once, as a
//! value.
//!
//! Boot used to pick the backend inline in a three-armed `if/else`. Two arms
//! logged a ✅ line; the third — the `NativeCatalog` fallback — logged
//! *nothing*, so the only signal an operator got that their deployment had
//! dropped off the WAL-backed catalog was the **absence** of a line they had no
//! reason to look for. That is the same conflation this program has been
//! closing everywhere else: "the intended thing happened" and "something else
//! happened quietly" were indistinguishable from outside.
//!
//! The choice is not cosmetic. `SystemCatalog` serves catalog reads from RAM
//! and persists each DDL as one fsync'd WAL append; `NativeCatalog` is
//! file-per-object and does a `read_dir` per `list_tables`. Falling back trades
//! a per-request round-trip class the operator never agreed to.
//!
//! Deciding in a pure function makes the reason a *value* the caller must
//! handle, and makes every arm unit-testable without a filesystem.

use std::path::{Path, PathBuf};

/// Why boot fell back to the file-per-object `NativeCatalog`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeFallbackReason {
    /// `PROXIMADB_DISABLE_SYSTEM_CATALOG` is set — the cutover kill-switch.
    KillSwitch,
    /// An object-store metadata URL, but no local `data_dir` to host the
    /// per-DDL WAL (some test/embedded paths pass no server config).
    ObjectStoreWithoutDataDir,
    /// The metadata URL is neither a `file://` URL nor an object-store URL —
    /// e.g. a bare local path.
    MetadataUrlNotFileScheme,
}

impl NativeFallbackReason {
    /// Operator-facing explanation, including what to change.
    pub fn explain(self) -> &'static str {
        match self {
            Self::KillSwitch => {
                "PROXIMADB_DISABLE_SYSTEM_CATALOG is set; unset it to restore the \
                 WAL-backed SystemCatalog"
            }
            Self::ObjectStoreWithoutDataDir => {
                "object-store metadata URL with no local data_dir for the catalog WAL; \
                 supply server.data_dir to use the SystemCatalog"
            }
            Self::MetadataUrlNotFileScheme => {
                "metadata URL is neither a file:// URL nor an object-store URL; \
                 use file://<path> to get the WAL-backed SystemCatalog"
            }
        }
    }
}

/// The backend that will serve the `default` catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultCatalogBackend {
    /// WAL-backed `SystemCatalog`, WAL + snapshot on a local volume.
    SystemCatalogLocal {
        /// Local root parsed out of the `file://` metadata URL.
        base: PathBuf,
    },
    /// WAL-backed `SystemCatalog`: per-DDL WAL on the local working volume,
    /// snapshot blob in the object store.
    SystemCatalogObjectStore {
        /// Local working volume hosting the WAL.
        data_dir: PathBuf,
    },
    /// File-per-object `NativeCatalog`.
    NativeFallback {
        /// What forced the fallback. Never discard this — it is the whole
        /// reason this type exists.
        reason: NativeFallbackReason,
    },
}

impl DefaultCatalogBackend {
    /// Short label for logs and health output.
    pub fn label(&self) -> &'static str {
        match self {
            Self::SystemCatalogLocal { .. } => "SystemCatalog (local WAL + snapshot)",
            Self::SystemCatalogObjectStore { .. } => {
                "SystemCatalog (local WAL, object-store snapshot)"
            }
            Self::NativeFallback { .. } => "NativeCatalog (file-per-object fallback)",
        }
    }
}

/// Decide the default catalog backend from boot inputs.
///
/// Pure: no environment reads, no I/O. `system_catalog_disabled` is the
/// already-read `PROXIMADB_DISABLE_SYSTEM_CATALOG` state and `local_data_dir`
/// the already-resolved `server.data_dir`, so every arm is reachable from a
/// test.
///
/// ANY non-`file://` scheme is an object store — the scheme list is
/// deliberately not enumerated here. `adls://` / `abfs://` / `azure://` /
/// `gcs://` are documented aliases (ADR-036) that once fell through *both*
/// branches onto NativeCatalog's non-durable temp cache, silently losing
/// catalog durability on Azure (TD-OBJSTORE-1, #960).
pub fn resolve_default_catalog_backend(
    metadata_url: &str,
    system_catalog_disabled: bool,
    local_data_dir: Option<&Path>,
) -> DefaultCatalogBackend {
    if system_catalog_disabled {
        return DefaultCatalogBackend::NativeFallback {
            reason: NativeFallbackReason::KillSwitch,
        };
    }

    if let Some(base) = metadata_url.strip_prefix("file://") {
        return DefaultCatalogBackend::SystemCatalogLocal {
            base: PathBuf::from(base),
        };
    }

    let is_objstore = metadata_url.contains("://");
    if is_objstore {
        return match local_data_dir {
            Some(dir) => DefaultCatalogBackend::SystemCatalogObjectStore {
                data_dir: dir.to_path_buf(),
            },
            None => DefaultCatalogBackend::NativeFallback {
                reason: NativeFallbackReason::ObjectStoreWithoutDataDir,
            },
        };
    }

    DefaultCatalogBackend::NativeFallback {
        reason: NativeFallbackReason::MetadataUrlNotFileScheme,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_url_selects_the_local_system_catalog() {
        assert_eq!(
            resolve_default_catalog_backend("file:///var/lib/proximadb", false, None),
            DefaultCatalogBackend::SystemCatalogLocal {
                base: PathBuf::from("/var/lib/proximadb")
            }
        );
    }

    #[test]
    fn object_store_url_with_a_data_dir_selects_the_snapshot_backed_system_catalog() {
        let dir = PathBuf::from("/data");
        assert_eq!(
            resolve_default_catalog_backend("s3://bucket/prefix", false, Some(&dir)),
            DefaultCatalogBackend::SystemCatalogObjectStore { data_dir: dir }
        );
    }

    /// ADR-036: the Azure aliases are object stores, not local paths. Enumerating
    /// schemes here is what let them fall through both branches onto the
    /// non-durable temp cache (TD-OBJSTORE-1).
    #[test]
    fn azure_and_gcs_aliases_are_object_stores_not_local_paths() {
        let dir = PathBuf::from("/data");
        for url in [
            "adls://acct/container",
            "abfs://acct/container",
            "azure://acct/container",
            "az://acct/container",
            "gcs://bucket/prefix",
        ] {
            assert_eq!(
                resolve_default_catalog_backend(url, false, Some(&dir)),
                DefaultCatalogBackend::SystemCatalogObjectStore {
                    data_dir: dir.clone()
                },
                "{url} must resolve to the object-store SystemCatalog"
            );
        }
    }

    /// Each fallback carries the reason that produced it. A fallback whose cause
    /// is not recoverable from the value would be the defect this type exists to
    /// remove.
    #[test]
    fn every_fallback_names_its_cause() {
        let dir = PathBuf::from("/data");
        assert_eq!(
            resolve_default_catalog_backend("file:///x", true, Some(&dir)),
            DefaultCatalogBackend::NativeFallback {
                reason: NativeFallbackReason::KillSwitch
            },
            "the kill-switch wins over an otherwise-valid file:// URL"
        );
        assert_eq!(
            resolve_default_catalog_backend("s3://bucket/prefix", false, None),
            DefaultCatalogBackend::NativeFallback {
                reason: NativeFallbackReason::ObjectStoreWithoutDataDir
            }
        );
        assert_eq!(
            resolve_default_catalog_backend("/var/lib/proximadb", false, Some(&dir)),
            DefaultCatalogBackend::NativeFallback {
                reason: NativeFallbackReason::MetadataUrlNotFileScheme
            },
            "a bare local path is not a file:// URL"
        );
    }
}
