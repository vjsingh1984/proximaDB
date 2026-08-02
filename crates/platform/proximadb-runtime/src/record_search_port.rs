//! Canonical v2 record/vector-search read port (TD-FLIGHT-1).
//!
//! The Arrow Flight search paths (`do_get` single-query and `do_exchange`
//! `bulk_search`) consume canonical v2 search so they no longer run the
//! deprecated v1 contract. This port lets the Flight service depend on the
//! contract instead of a concrete root-crate service — the same seam the
//! write path already uses via [`crate::RecordOpsPort`].
//!
//! Implemented by the root crate's `RecordOpsService`
//! (`src/api_handlers/record_ops_service.rs`), which delegates to its
//! `handle_record_search_for_tenant` — the single canonical search authority
//! also called by REST v2 and gRPC v2. It owns typed-filter lowering, WAL
//! delta-merge, MVCC/tombstone filtering, Strong-freshness cache behavior,
//! and the tenant-collection-access check; no durable authority lives here.

use anyhow::Result;
use async_trait::async_trait;

use crate::rich_search::{RichSearchRequest, RichSearchResponse};
use crate::service_ports::PortIdentity;

#[async_trait]
pub trait RecordSearchPort: Send + Sync {
    /// Run a canonical v2 search for `request.collection_id` under
    /// `identity.tenant_id`; `identity.subject` drives ABAC at the shared seam
    /// (TD-ABAC-7) — `None` subject = passthrough.
    async fn search_record(
        &self,
        request: RichSearchRequest,
        identity: PortIdentity<'_>,
    ) -> Result<RichSearchResponse>;
}
