// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Paginated scan-cursor state, extracted from the root `services` module
//! (TD-DECOMP-50).
//!
//! [`scan_cursor`] carries the resumption state for paginated scans
//! ([`scan_cursor::ScanCursor`]) plus a base64 continuation-token codec
//! (`serde`-serializable). Depends only on `proximadb-records` + `serde` +
//! `base64`, keeping it a clean horizontal-tier leaf.

pub mod scan_cursor;
