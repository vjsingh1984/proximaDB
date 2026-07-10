// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Process-global [`DocumentService`] handle for the DataFusion `documents(collection)` table
//! function (ADR-055 P-DFSource). The cross-modal UDTF is registered per `SessionContext` but the
//! live document service is constructed once at bootstrap, so — exactly like
//! [`crate::services::timeseries_service`] — we stash it in a process singleton the UDTF reads,
//! rather than threading an `Arc<DocumentService>` through every session construction.

use std::sync::{Arc, OnceLock};

use crate::storage::document::DocumentService;

static DOCUMENT_SERVICE: OnceLock<Arc<DocumentService>> = OnceLock::new();

/// Register the process-global document service (idempotent — first wins). Called once at server
/// bootstrap after the shared `DocumentService` is constructed.
pub fn set_document_service(service: Arc<DocumentService>) {
    let _ = DOCUMENT_SERVICE.set(service);
}

/// The process-global document service, if registered.
pub fn document_service() -> Option<Arc<DocumentService>> {
    DOCUMENT_SERVICE.get().cloned()
}
