//! Per-backend I/O round-trip budget (ADR-062 D6 / TD-RDSTRAT-6).
//!
//! On object storage the dominant vector-route cost term is **GET round-trips**
//! (`per_get=20 ≫ per_mib=5`, `route_cost_model`). The coalesced-RaBitQ layout
//! drives survivor-fetch block geometry off a per-backend **IOPS budget**: the
//! byte size a single ranged GET wants to move so that a survivor rerank touches
//! *few, large, coalesced* ranges rather than many tiny ones.
//!
//! `IopsBudget` is deliberately a **per-backend property** of the storage config —
//! it is NOT the [`crate::storage_profile::StorageProfile`] `{AppendBulk, Churn}`
//! *workload* enum (ADR-061 D1). The two are orthogonal: the workload enum selects
//! the read-projection *strategy*; this budget sizes the *physical* ranged GET.
//! Resolving a budget never changes a durability contract.

/// Fallback rows-per-block when a row's survivor byte cost is unknown (0), so the
/// geometry derivation stays total (no divide-by-zero). Matches the legacy
/// `SST_DEFAULT_VECTORS_PER_BLOCK=128` until the per-row cost is measured.
pub const FALLBACK_ROWS_PER_BLOCK: u64 = 128;

/// Per-backend ranged-GET byte budget for the coalesced-RaBitQ survivor fetch
/// (ADR-062 D6). `target` is the preferred block byte size; `min`/`max` bound the
/// self-derived rows-per-block.
///
/// * **Cloud object stores** (S3 / Azure Blob / ABFS / ADLS / GCS) lean to a
///   ~4 MiB target — large enough to amortise a 20-cost GET round-trip.
/// * **Local disk / MinIO** (no round-trip cost) lean to ~512 KiB–1 MiB.
/// * The generic [`IopsBudget::DEFAULT`] (~512 KiB / 2 MiB / 8 MiB) is used when
///   the backend cannot be inferred from the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IopsBudget {
    /// Smallest useful ranged GET (bytes). Blocks are never sized below this.
    pub min: u64,
    /// Preferred block byte size — the survivor-fetch ranged-GET target.
    pub target: u64,
    /// Largest single coalesced range (bytes). Blocks are never sized above this.
    pub max: u64,
}

impl IopsBudget {
    /// Cloud object store budget (S3 / Azure Blob / ABFS / ADLS / GCS):
    /// ~512 KiB min · 4 MiB target · 8 MiB max.
    pub const CLOUD: Self = Self {
        min: 512 * 1024,
        target: 4 * 1024 * 1024,
        max: 8 * 1024 * 1024,
    };

    /// Azure Blob Storage budget: 4 MiB target (the SDK chunks ranged reads at
    /// 4 MiB — a block > 4 MiB uncompressed + overhead could split into 2 GETs).
    /// zstd compression brings ~4 MiB uncompressed → ~2 MiB on-disk (safe).
    pub const AZURE: Self = Self {
        min: 512 * 1024,
        target: 4 * 1024 * 1024,
        max: 4 * 1024 * 1024,
    };

    /// AWS S3 budget: 8 MiB target (no hard range limit; sweet spot for
    /// throughput vs per-GET cost). Compression → ~4 MiB on-disk.
    pub const S3: Self = Self {
        min: 512 * 1024,
        target: 8 * 1024 * 1024,
        max: 16 * 1024 * 1024,
    };

    /// Google Cloud Storage budget: same as S3 (no hard range limit).
    pub const GCS: Self = Self {
        min: 512 * 1024,
        target: 8 * 1024 * 1024,
        max: 16 * 1024 * 1024,
    };

    /// Local disk / MinIO budget (no round-trip cost):
    /// ~256 KiB min · 1 MiB target · 8 MiB max.
    pub const LOCAL: Self = Self {
        min: 256 * 1024,
        target: 1024 * 1024,
        max: 8 * 1024 * 1024,
    };

    /// Generic default when the backend is unknown:
    /// ~512 KiB min · 2 MiB target · 8 MiB max.
    pub const DEFAULT: Self = Self {
        min: 512 * 1024,
        target: 2 * 1024 * 1024,
        max: 8 * 1024 * 1024,
    };

    /// Resolve the budget from a storage path's URL scheme (ADR-036 / ADR-062 D6).
    ///
    /// Per-provider: Azure schemes → [`IopsBudget::AZURE`] (4 MiB — SDK chunks
    /// ranged reads at 4 MiB); S3 / HTTP → [`IopsBudget::S3`] (8 MiB — no hard
    /// limit); GCS → [`IopsBudget::GCS`] (8 MiB); local / `file` / `minio` /
    /// bare paths → [`IopsBudget::LOCAL`]; unknown → [`IopsBudget::DEFAULT`].
    pub fn for_path(path: &str) -> Self {
        let scheme = path.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
        match scheme.as_deref() {
            // Azure: hard 4 MiB SDK chunk boundary for ranged reads.
            Some("azure" | "abfs" | "adls" | "az") => Self::AZURE,
            // S3 / HTTP: no hard range limit; 8 MiB sweet spot.
            Some("s3" | "http" | "https") => Self::S3,
            // GCS: same as S3.
            Some("gs") => Self::GCS,
            // Local / MinIO / bare path: no round-trip cost.
            Some("file" | "minio") | None => Self::LOCAL,
            Some(_) => Self::DEFAULT,
        }
    }

    /// The survivor-fetch block byte target — the ranged-GET size the writer aims
    /// for when cutting data blocks (replaces the static
    /// `SST_DEFAULT_VECTORS_PER_BLOCK` / `PROXIMADB_PAX_BLOCK_SIZE` default).
    pub fn target_block_bytes(self) -> u64 {
        self.target
    }

    /// Self-derived rows-per-block from a row's survivor-fetch byte cost
    /// (ADR-062 D6):
    ///
    /// ```text
    /// rows = clamp( target / per_row_bytes,
    ///               ceil(min / per_row_bytes),   // never below `min` bytes
    ///               floor(max / per_row_bytes) ) // never above `max` bytes
    /// ```
    ///
    /// `per_row_bytes = sq8(if present)·dim + fp32(if present)·4·dim + fixed_metadata`
    /// (RaBitQ excluded — it lives in the coalesced header region). A `0` cost
    /// (unknown composition) returns [`FALLBACK_ROWS_PER_BLOCK`]. Always ≥ 1.
    pub fn rows_per_block(self, per_row_bytes: u64) -> u64 {
        if per_row_bytes == 0 {
            return FALLBACK_ROWS_PER_BLOCK;
        }
        let want = self.target / per_row_bytes;
        let floor = self.min.div_ceil(per_row_bytes);
        let ceil = self.max / per_row_bytes;
        let lower = floor.max(1);
        let upper = ceil.max(lower);
        want.clamp(lower, upper)
    }
}

impl Default for IopsBudget {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_vs_local_budgets() {
        assert_eq!(IopsBudget::CLOUD.target, 4 * 1024 * 1024);
        assert_eq!(IopsBudget::LOCAL.target, 1024 * 1024);
        // min ≤ target ≤ max invariant for every preset.
        for b in [IopsBudget::CLOUD, IopsBudget::LOCAL, IopsBudget::DEFAULT] {
            assert!(b.min <= b.target && b.target <= b.max, "{b:?}");
        }
    }

    #[test]
    fn for_path_resolves_by_scheme() {
        assert_eq!(IopsBudget::for_path("s3://bucket/key"), IopsBudget::CLOUD);
        assert_eq!(
            IopsBudget::for_path("abfs://container/path"),
            IopsBudget::CLOUD
        );
        assert_eq!(IopsBudget::for_path("adls://acct/fs"), IopsBudget::CLOUD);
        assert_eq!(IopsBudget::for_path("gs://b/o"), IopsBudget::CLOUD);
        assert_eq!(IopsBudget::for_path("/var/data/seg.pax"), IopsBudget::LOCAL);
        assert_eq!(
            IopsBudget::for_path("file:///var/data/seg.pax"),
            IopsBudget::LOCAL
        );
        assert_eq!(
            IopsBudget::for_path("minio://bucket/key"),
            IopsBudget::LOCAL
        );
        // Unknown scheme → generic default.
        assert_eq!(IopsBudget::for_path("redis://x"), IopsBudget::DEFAULT);
    }

    #[test]
    fn rows_per_block_derives_from_per_row_cost() {
        let cloud = IopsBudget::CLOUD;
        // SQ8-only survivor: dim=128 → 128 B/row.
        let sq8_only = 128u64;
        let rows = cloud.rows_per_block(sq8_only);
        // 4 MiB / 128 B = 32768 rows — but capped by `max` (8 MiB / 128 = 65536),
        // so the target wins → 32768.
        assert_eq!(rows, (4 * 1024 * 1024) / 128);
        assert!(rows >= 1);
    }

    #[test]
    fn rows_per_block_floors_at_min_and_caps_at_max() {
        let b = IopsBudget::DEFAULT; // min 512KB, target 2MB, max 8MB
        // Huge per-row cost (1 MiB/row): target/per = 2, min ceil = 1, max floor = 8 → 2.
        assert_eq!(b.rows_per_block(1024 * 1024), 2);
        // Tiny per-row cost (1 B/row): target/per = 2M, but capped at max/per = 8M → 2M.
        assert_eq!(b.rows_per_block(1), 2 * 1024 * 1024);
    }

    #[test]
    fn rows_per_block_zero_cost_falls_back() {
        assert_eq!(
            IopsBudget::DEFAULT.rows_per_block(0),
            FALLBACK_ROWS_PER_BLOCK
        );
    }

    #[test]
    fn target_block_bytes_is_the_target() {
        assert_eq!(IopsBudget::CLOUD.target_block_bytes(), 4 * 1024 * 1024);
    }
}
