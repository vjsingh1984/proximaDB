//! TD-099(3d) WAL scan-index observability.
//!
//! Exposes `proximadb_wal_unflushed_distinct_oids{collection}` — the number of
//! distinct object IDs in a collection's *deduped unflushed scan index*
//! (`CollectionPartition::scan_index`, the BTreeMap built by
//! `rebuild_scan_index`). This is the index's entry count, which is also the
//! dominant term in its resident-memory cost, so operators use it to size the
//! per-collection scan-index footprint and to tune any future push-down
//! enable/disable threshold.
//!
//! Wiring: this module owns a *private* Prometheus registry (no boot
//! initialization required — unlike the precision/rank families), and
//! [`scrape_text`] is appended to the `/metrics/prometheus` exposition. The
//! gauge is emitted from the memtable layer on every scan-index rebuild via
//! [`set_unflushed_distinct_oids`]. The default Prometheus registry is NOT
//! scraped by that endpoint, so a private-registry + `scrape_text` family is the
//! pattern that actually reaches the wire (mirrors
//! `observability::precision_metrics` / `rank_metrics`).

use lazy_static::lazy_static;
use prometheus::{Encoder, IntGaugeVec, Opts, Registry, TextEncoder};
use tracing::error;

const METRIC_NAME: &str = "proximadb_wal_unflushed_distinct_oids";
const METRIC_HELP: &str = "Distinct object IDs in a collection's deduped unflushed scan index \
     (TD-099(3d)); sizes the per-collection index memory for push-down tuning";

lazy_static! {
    static ref REGISTRY: Registry = Registry::new();
    static ref UNFLUSHED_DISTINCT_OIDS: IntGaugeVec = build_and_register();
}

/// Build the gauge and register it into this module's private registry.
/// Never panics: on the (descriptor-invalid) error path it logs and returns a
/// fallback handle, matching the `*_safe` convention in `collection_pin_metrics`.
fn build_and_register() -> IntGaugeVec {
    let gauge = match IntGaugeVec::new(Opts::new(METRIC_NAME, METRIC_HELP), &["collection"]) {
        Ok(g) => g,
        Err(e) => {
            error!("failed to build {METRIC_NAME}: {e}");
            return IntGaugeVec::new(
                Opts::new(format!("{METRIC_NAME}_fallback"), METRIC_HELP),
                &["collection"],
            )
            .unwrap_or_else(|_| unreachable!("valid gauge descriptor"));
        }
    };
    if let Err(e) = REGISTRY.register(Box::new(gauge.clone())) {
        error!("failed to register {METRIC_NAME}: {e}");
    }
    gauge
}

/// Set the distinct-indexed-oid count for a collection. Called when the
/// TD-099(3d) scan index is rebuilt (`ordered.len()`).
pub fn set_unflushed_distinct_oids(collection: &str, count: i64) {
    UNFLUSHED_DISTINCT_OIDS
        .with_label_values(&[collection])
        .set(count);
}

/// Prometheus text exposition for this family. Empty until the first emit
/// (so binaries that never scan stay valid), appended to `/metrics/prometheus`.
pub fn scrape_text() -> String {
    let mut buf = Vec::new();
    let encoder = TextEncoder::new();
    if encoder.encode(&REGISTRY.gather(), &mut buf).is_err() {
        return String::new();
    }
    String::from_utf8(buf).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_then_scrape_exposes_value() {
        set_unflushed_distinct_oids("col_metrics_test", 7);
        assert_eq!(
            UNFLUSHED_DISTINCT_OIDS
                .with_label_values(&["col_metrics_test"])
                .get(),
            7
        );
        let text = scrape_text();
        assert!(
            text.contains(METRIC_NAME),
            "scrape must contain the metric name: {text}"
        );
        assert!(
            text.contains("col_metrics_test"),
            "scrape must contain the collection label: {text}"
        );
    }
}
