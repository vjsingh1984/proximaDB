//! Network-layer adapter bridging the cluster `PartitionLeaseManager` to the
//! storage-layer [`StorageWriteFence`] trait.
//!
//! The flush path (storage layer) must not depend upward on `cluster`, and
//! `FlushParameters` is serializable so it cannot carry the lease manager. So the
//! coordinator holds an `Arc<dyn StorageWriteFence>`, and this adapter — built in
//! the `network` layer, which already depends on both — wraps the **same**
//! `PartitionLeaseManager` instance the write-gates use. See
//! `crate::storage::write_fence` for the A6 rationale.

use async_trait::async_trait;
use std::sync::Arc;

use crate::cluster::partition_lease::PartitionLeaseManager;
use crate::storage::write_fence::StorageWriteFence;

/// A6 storage-write fence backed by the durable partition-lease manager (#346).
pub struct LeaseStorageWriteFence {
    lease_manager: Arc<PartitionLeaseManager>,
}

impl LeaseStorageWriteFence {
    /// Wrap the shared lease manager as a storage-write fence.
    pub fn new(lease_manager: Arc<PartitionLeaseManager>) -> Self {
        Self { lease_manager }
    }
}

#[async_trait]
impl StorageWriteFence for LeaseStorageWriteFence {
    async fn is_fenced_out(&self, tenant_id: &str, collection_id: &str, now_ms: i64) -> bool {
        // Ground truth across pods: delegates to the durable lease read (#346).
        self.lease_manager
            .is_fenced_out(tenant_id, collection_id, now_ms)
            .await
    }
}
