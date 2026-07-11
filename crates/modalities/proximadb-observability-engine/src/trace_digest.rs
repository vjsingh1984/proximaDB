// Trace digest — stable hash for dedup on the async metering sink.
//
// `metering_event::build_kru` produces one metering record per search.
// An upstream gateway POSTs the record to a metering-events collection
// (operator-configured; default name `proximadb_metering_events`)
// asynchronously (fire-and-forget) and the CDC fan-out downstream can
// replay on consumer restart. Without an idempotency key the sink ends
// up double-counting on replay.
//
// This module produces a stable 64-bit digest of the trace identity:
//
//   `(tenant_id, trace_id, occurred_at_bucket)`
//
// where `occurred_at_bucket` is the floor of the timestamp to a
// configurable bucket size. Bucketing handles the small clock skew
// between the data-plane recorder and the CDC sink's stamp — same
// trace replayed within the bucket hashes the same.
//
// Implementation is FNV-1a 64-bit:
//   - Allocation-free, branch-light, no external crate dependency.
//   - Distribution is good enough for dedup (cryptographic hashes are
//     overkill).
//   - Matches the hash used by `trace_sampling::trace_bucket` so the
//     two modules can share a hash domain when needed.

use std::time::Duration;

/// Inputs the digester consumes. Trace identity + the wall clock at
/// recording.
#[derive(Debug, Clone, Copy)]
pub struct DigestInputs<'a> {
    pub tenant_id: &'a str,
    pub trace_id: &'a str,
    /// Unix-epoch milliseconds — caller-supplied for determinism.
    pub occurred_at_ms: u64,
    /// Bucket size for the timestamp. Pass `Duration::ZERO` to bucket
    /// at 1 ms (effectively no bucketing). Defaults of 1s give the
    /// async sink ~1s of replay tolerance.
    pub bucket: Duration,
}

/// Compute the digest. Always returns a value — empty inputs are
/// allowed and produce a stable (non-zero) digest.
pub fn digest(inputs: &DigestInputs<'_>) -> u64 {
    let bucket_ms = inputs.bucket.as_millis().max(1) as u64;
    let bucketed = inputs.occurred_at_ms - (inputs.occurred_at_ms % bucket_ms);
    let mut h: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    for b in inputs.tenant_id.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h ^= b':' as u64;
    h = h.wrapping_mul(0x100000001b3);
    for b in inputs.trace_id.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h ^= b':' as u64;
    h = h.wrapping_mul(0x100000001b3);
    for byte in bucketed.to_le_bytes() {
        h ^= byte as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Hex-encode the digest as a 16-char lowercase string. Useful as an
/// idempotency-key field on the JSON billing record.
pub fn digest_hex(inputs: &DigestInputs<'_>) -> String {
    format!("{:016x}", digest(inputs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inp<'a>(tenant: &'a str, trace: &'a str, ms: u64, bucket: Duration) -> DigestInputs<'a> {
        DigestInputs {
            tenant_id: tenant,
            trace_id: trace,
            occurred_at_ms: ms,
            bucket,
        }
    }

    #[test]
    fn deterministic_for_identical_inputs() {
        let i = inp(
            "tenant-a",
            "trace-1",
            1_700_000_000_000,
            Duration::from_secs(1),
        );
        let a = digest(&i);
        let b = digest(&i);
        assert_eq!(a, b);
    }

    #[test]
    fn distinct_tenants_produce_distinct_digests() {
        let a = digest(&inp(
            "tenant-a",
            "trace-1",
            1_700_000_000_000,
            Duration::from_secs(1),
        ));
        let b = digest(&inp(
            "tenant-b",
            "trace-1",
            1_700_000_000_000,
            Duration::from_secs(1),
        ));
        assert_ne!(a, b);
    }

    #[test]
    fn distinct_trace_ids_produce_distinct_digests() {
        let a = digest(&inp(
            "tenant-a",
            "trace-1",
            1_700_000_000_000,
            Duration::from_secs(1),
        ));
        let b = digest(&inp(
            "tenant-a",
            "trace-2",
            1_700_000_000_000,
            Duration::from_secs(1),
        ));
        assert_ne!(a, b);
    }

    #[test]
    fn distinct_timestamps_across_buckets_distinct() {
        let a = digest(&inp(
            "t",
            "trace-1",
            1_700_000_000_000,
            Duration::from_secs(1),
        ));
        let b = digest(&inp(
            "t",
            "trace-1",
            1_700_000_002_000,
            Duration::from_secs(1),
        ));
        assert_ne!(a, b, "different seconds must differ");
    }

    #[test]
    fn timestamps_within_same_bucket_collapse() {
        // 1s bucket — two stamps in the same second hash the same.
        let bucket = Duration::from_secs(1);
        let a = digest(&inp("t", "trace-1", 1_700_000_000_001, bucket));
        let b = digest(&inp("t", "trace-1", 1_700_000_000_999, bucket));
        assert_eq!(a, b);
    }

    #[test]
    fn larger_bucket_collapses_more_aggressively() {
        let small = Duration::from_secs(1);
        let large = Duration::from_secs(60);
        let stamp_a = 1_700_000_000_000;
        let stamp_b = 1_700_000_030_000;
        // Across two seconds → distinct under 1s bucket.
        assert_ne!(
            digest(&inp("t", "trace-1", stamp_a, small)),
            digest(&inp("t", "trace-1", stamp_b, small))
        );
        // But same under a 60s bucket.
        assert_eq!(
            digest(&inp("t", "trace-1", stamp_a, large)),
            digest(&inp("t", "trace-1", stamp_b, large))
        );
    }

    #[test]
    fn zero_bucket_defaults_to_one_ms() {
        // Bucket::ZERO must not divide-by-zero; it floors to 1ms.
        let a = digest(&inp("t", "trace-1", 1_700_000_000_000, Duration::ZERO));
        let b = digest(&inp("t", "trace-1", 1_700_000_000_000, Duration::ZERO));
        assert_eq!(a, b);
        // 1 ms difference flips the digest at 1ms bucket.
        let c = digest(&inp("t", "trace-1", 1_700_000_000_001, Duration::ZERO));
        assert_ne!(a, c);
    }

    #[test]
    fn empty_inputs_produce_a_valid_nonzero_digest() {
        // FNV-1a offset basis is non-zero so an empty input still
        // hashes to a stable non-zero value. The dedup sink can use
        // this for malformed-row tracking.
        let d = digest(&inp("", "", 0, Duration::from_secs(1)));
        assert_ne!(d, 0);
    }

    #[test]
    fn hex_encoding_is_16_chars_lowercase() {
        let h = digest_hex(&inp(
            "tenant-a",
            "trace-1",
            1_700_000_000_000,
            Duration::from_secs(1),
        ));
        assert_eq!(h.len(), 16);
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn hex_round_trips_to_same_u64() {
        let i = inp(
            "tenant-a",
            "trace-1",
            1_700_000_000_000,
            Duration::from_secs(1),
        );
        let n = digest(&i);
        let h = digest_hex(&i);
        let back = u64::from_str_radix(&h, 16).unwrap();
        assert_eq!(n, back);
    }

    #[test]
    fn similar_tenant_strings_do_not_collide_easily() {
        // "tenant-a" vs "tenant-b" — single-char difference, FNV-1a's
        // multiplicative step propagates that across the rest of the
        // hash so we don't see adjacency-like collisions.
        let a = digest(&inp(
            "tenant-a",
            "trace-1",
            1_700_000_000_000,
            Duration::from_secs(1),
        ));
        let b = digest(&inp(
            "tenant-b",
            "trace-1",
            1_700_000_000_000,
            Duration::from_secs(1),
        ));
        let c = digest(&inp(
            "tenant-c",
            "trace-1",
            1_700_000_000_000,
            Duration::from_secs(1),
        ));
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn delimiter_byte_prevents_field_swap_collisions() {
        // Without a delimiter byte between tenant_id and trace_id, the
        // digests of ("ab", "c") and ("a", "bc") would collide. The
        // module inserts a `:` byte between fields so they don't.
        let ab_c = digest(&inp("ab", "c", 0, Duration::from_secs(1)));
        let a_bc = digest(&inp("a", "bc", 0, Duration::from_secs(1)));
        assert_ne!(ab_c, a_bc);
    }
}
