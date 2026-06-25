// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Unix-domain socket binding helpers for portless ("embedded") mode.
//!
//! The REST (axum/hyper), gRPC (tonic), and Arrow Flight (tonic) surfaces all
//! reuse [`bind_unix_listener`] so the stale-socket + parent-dir handling is
//! identical across transports. See `proximadb_runtime::bootstrap_config::BindTarget`.

use std::path::Path;
use tokio::net::UnixListener;

/// Bind a [`UnixListener`] at `path`, preparing the filesystem first:
///
/// 1. Create the parent directory if it does not yet exist.
/// 2. Unlink any leftover socket file at `path` — a stale socket from a crashed
///    or `SIGKILL`ed prior run makes `bind()` fail with `EADDRINUSE`.
///
/// Returns the bound listener ready for `axum::serve` / `serve_with_incoming`.
pub fn bind_unix_listener(path: &Path) -> std::io::Result<UnixListener> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Remove a stale socket left behind by a previous process. Ignore "not
    // found" (the common, clean case); surface any other error (e.g. perms).
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    UnixListener::bind(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bind_creates_parent_and_unlinks_stale() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sock = dir.path().join("nested").join("a.sock");

        // First bind succeeds and creates the nested parent dir.
        let l1 = bind_unix_listener(&sock).expect("first bind");
        assert!(sock.exists(), "socket file should exist after bind");
        drop(l1);

        // A leftover socket file must not block a re-bind (EADDRINUSE).
        assert!(sock.exists(), "socket file persists after drop");
        let _l2 = bind_unix_listener(&sock).expect("re-bind over stale socket");
    }
}
