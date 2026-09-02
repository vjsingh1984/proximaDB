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

    /// Azure Blob Storage budget: 4 MiB target. This is the conservative
    /// planner policy being evaluated by TD-SEARCH-3, not a Blob billing
    /// quantum or a proven SDK range limit. Wire-level Azurite/Azure evidence
    /// must precede any larger default.
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
    /// Per-provider: Azure schemes → [`IopsBudget::AZURE`] (conservative 4 MiB
    /// evaluation policy); S3 / HTTP → [`IopsBudget::S3`] (8 MiB — no hard
    /// limit); GCS → [`IopsBudget::GCS`] (8 MiB); local / `file` / `minio` /
    /// bare paths → [`IopsBudget::LOCAL`]; unknown → [`IopsBudget::DEFAULT`].
    pub fn for_path(path: &str) -> Self {
        let scheme = path.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
        match scheme.as_deref() {
            // Azure: conservative 4 MiB policy pending TD-SEARCH-3 wire sweep.
            Some("azure" | "abfs" | "adls" | "az") => Self::AZURE,
            // S3 / HTTP: no hard range limit; 8 MiB sweet spot.
            Some("s3" | "http" | "https") => Self::S3,
            // GCS: same as S3.
            Some("gs" | "gcs") => Self::GCS,
            // Local / MinIO / bare path: no round-trip cost. ADR-073: an
            // HDD-backed location (500 IOPS, seek-bound) must coalesce like a
            // cloud store — the measured sweep puts the LOCAL profile at ~191
            // reads/query (≈2.6 QPS on HDD) vs ~41 with the CLOUD profile
            // (≈12 QPS). `PROXIMADB_DISK_CLASS=hdd` is the deploy-time hint
            // (per-location tags don't reach this leaf crate).
            Some("file" | "minio") | None => {
                if std::env::var("PROXIMADB_DISK_CLASS")
                    .map(|v| v.trim().eq_ignore_ascii_case("hdd"))
                    .unwrap_or(false)
                {
                    Self::CLOUD
                } else {
                    Self::LOCAL
                }
            }
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

/// The IOP target a **writer** should size blocks/cells against.
///
/// Read-side planning resolves the backend from the object URL, but the write
/// path stages segments to a LOCAL file and publishes afterwards, so at
/// clustering time it does not know its destination backend. Passing the staging
/// path here would resolve `LOCAL` and be actively wrong.
///
/// This exists so both sides share **one** definition of "the IOP target"
/// (TD-IVF-4: two independent constants is the defect, a single helper consulted
/// by both is the fix), and so the unknown-destination case is explicit in the
/// signature instead of hidden inside a hard-coded constant.
///
/// `None` yields the `CLOUD` target, which is what the write path used
/// implicitly before. Closing TD-IVF-4 fully means plumbing the destination URL
/// from the flush/compaction caller — which does know it — down to clustering.
pub fn write_target_block_bytes(destination_url: Option<&str>) -> u64 {
    match destination_url {
        Some(url) => IopsBudget::for_path(url).target_block_bytes(),
        None => IopsBudget::CLOUD.target_block_bytes(),
    }
}

/// Candidate merged-range caps for the adaptive **read** planner.
///
/// This is intentionally separate from [`IopsBudget::max`]. That field bounds
/// writer block geometry; raising it to improve reads would silently create
/// larger blocks at flush/compaction time. Read candidates instead begin at the
/// backend's conservative target and grow through a small deterministic set up
/// to a provider-bounded ceiling. The exact range planner scores the resulting
/// GET/byte plans for the current selected cells.
///
/// Azure's 24 MiB ceiling covers the measured 1M and 768-d knees without
/// admitting the 32 MiB point that exceeded the declared RSS guard. S3/GCS keep
/// their existing 16 MiB maximum; local storage remains bounded at 8 MiB.
pub fn read_range_cap_candidates(path: &str) -> Vec<u64> {
    let budget = IopsBudget::for_path(path);
    let scheme = path.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
    let ceiling = match scheme.as_deref() {
        Some("az" | "azure" | "adls" | "abfs") => 24 * 1024 * 1024,
        Some("s3" | "gs" | "gcs" | "http" | "https") => 16 * 1024 * 1024,
        Some("file" | "minio") | None => 8 * 1024 * 1024,
        Some(_) => 8 * 1024 * 1024,
    };

    let mut caps = Vec::with_capacity(6);
    for multiplier in [1_u64, 2, 3, 4, 6] {
        let cap = budget.target.saturating_mul(multiplier);
        if cap > 0 && cap <= ceiling && caps.last().copied() != Some(cap) {
            caps.push(cap);
        }
    }
    if caps.last().copied() != Some(ceiling) {
        caps.push(ceiling);
    }
    caps
}

#[cfg(test)]
mod tests {
    /// ADR-073: the HDD disk-class hint swaps LOCAL for the CLOUD profile on
    /// file:// paths; unset/other values keep LOCAL. (nextest = process-per-
    /// test, so the env mutation cannot leak.)
    #[test]
    fn disk_class_hdd_selects_cloud_profile_for_local_paths() {
        unsafe { std::env::set_var("PROXIMADB_DISK_CLASS", "hdd") };
        assert_eq!(
            super::IopsBudget::for_path("file:///tmp/x.pax").target,
            super::IopsBudget::CLOUD.target
        );
        unsafe { std::env::set_var("PROXIMADB_DISK_CLASS", "ssd") };
        assert_eq!(
            super::IopsBudget::for_path("file:///tmp/x.pax").target,
            super::IopsBudget::LOCAL.target
        );
        // Cloud schemes unaffected by the hint.
        assert_eq!(
            super::IopsBudget::for_path("s3://b/k").target,
            super::IopsBudget::S3.target
        );
    }

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
        assert_eq!(IopsBudget::for_path("s3://bucket/key"), IopsBudget::S3);
        assert_eq!(
            IopsBudget::for_path("abfs://container/path"),
            IopsBudget::AZURE
        );
        assert_eq!(IopsBudget::for_path("adls://acct/fs"), IopsBudget::AZURE);
        assert_eq!(IopsBudget::for_path("gs://b/o"), IopsBudget::GCS);
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

    #[test]
    fn adaptive_read_caps_cover_the_measured_azure_knees() {
        let mib = 1024 * 1024;
        let expected = vec![4 * mib, 8 * mib, 12 * mib, 16 * mib, 24 * mib];
        for path in ["az://c/k", "azure://c/k", "adls://c/k", "abfs://c/k"] {
            assert_eq!(read_range_cap_candidates(path), expected, "{path}");
        }
    }

    #[test]
    fn adaptive_read_caps_are_backend_bounded_and_monotone() {
        let mib = 1024 * 1024;
        assert_eq!(
            read_range_cap_candidates("s3://b/k"),
            vec![8 * mib, 16 * mib]
        );
        assert_eq!(
            read_range_cap_candidates("gs://b/k"),
            vec![8 * mib, 16 * mib]
        );
        assert_eq!(
            read_range_cap_candidates("gcs://b/k"),
            vec![8 * mib, 16 * mib]
        );
        assert_eq!(
            read_range_cap_candidates("file:///data/k"),
            vec![mib, 2 * mib, 3 * mib, 4 * mib, 6 * mib, 8 * mib]
        );
        for path in ["az://c/k", "s3://b/k", "file:///data/k", "redis://x/k"] {
            let caps = read_range_cap_candidates(path);
            assert!(!caps.is_empty(), "{path}");
            assert!(caps.windows(2).all(|pair| pair[0] < pair[1]), "{path}");
        }
    }
}
