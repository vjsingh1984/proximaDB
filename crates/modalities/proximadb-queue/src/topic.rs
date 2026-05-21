//! Topic / partition routing.

use std::hash::Hasher;

pub type PartitionId = u32;

/// Deterministic partition selector. Same `tenant_id` always lands on the
/// same partition for a given `partition_count`, preserving per-tenant FIFO.
///
/// Uses `xxhash3` (`twox-hash` crate) modulo partition count. xxhash gives
/// uniform distribution on string inputs with negligible per-call cost.
pub fn partition_for(tenant_id: &str, partition_count: u32) -> PartitionId {
    debug_assert!(partition_count > 0, "partition_count must be > 0");
    let mut hasher = twox_hash::XxHash64::with_seed(0);
    hasher.write(tenant_id.as_bytes());
    (hasher.finish() % partition_count as u64) as PartitionId
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_tenant_always_lands_on_same_partition() {
        for n in [1, 4, 16, 64] {
            let a = partition_for("tenant-acme", n);
            let b = partition_for("tenant-acme", n);
            assert_eq!(a, b);
        }
    }

    #[test]
    fn distribution_spreads_reasonably_across_partitions() {
        // 1024 distinct tenants over 16 partitions — every partition should
        // see at least one tenant. (Uniform distribution would give ~64 each
        // by pigeonhole; we just guard against pathological hashing.)
        let pc = 16u32;
        let mut counts = vec![0usize; pc as usize];
        for i in 0..1024u32 {
            let t = format!("tenant-{i}");
            counts[partition_for(&t, pc) as usize] += 1;
        }
        for (idx, c) in counts.iter().enumerate() {
            assert!(*c > 0, "partition {idx} got no tenants — hash distribution broken");
        }
    }
}
