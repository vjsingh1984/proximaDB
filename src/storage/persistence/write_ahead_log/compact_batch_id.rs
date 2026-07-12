// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Re-export shim.
//!
//! `CompactBatchId` has been hoisted verbatim to the `proximadb-kernel` foundation
//! crate (`proximadb_kernel::batch_id`) as part of the root-crate decomposition
//! (Slice D / D2 link 1). This file re-exports it so the existing
//! `crate::storage::persistence::write_ahead_log::compact_batch_id::CompactBatchId`
//! path — and the `BatchId` alias in `mod.rs` — resolve unchanged.
//!
//! Round-trip / uniqueness tests now live alongside the type in the kernel crate.

pub use proximadb_kernel::batch_id::CompactBatchId;
