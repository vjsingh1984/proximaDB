// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! WAL v2 segment header (serialization + integrity), extracted from the root
//! `storage/persistence/write_ahead_log` module (TD-DECOMP-27). Self-contained over
//! `anyhow`/`proximadb-records`/`serde`.

pub mod v2_segment_header;
