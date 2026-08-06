//! Re-export shim — the ingestion facade (`ObservabilityIngester`, `buffer`,
//! `parser`) lives in `proximadb-observability-engine`. The format *adapters*
//! (OTLP HTTP/gRPC, Syslog, CEF/LEEF, OCSF, Fluent — the network ingress
//! servers) stay here in the root: they couple to the axum/tonic/tower web
//! stack, which does not belong in a modality *engine* crate. They reach the
//! facade via `crate::observability::ObservabilityService`.

pub mod adapters;
pub use proximadb_observability_engine::ingestion::*;
pub use proximadb_observability_engine::ingestion::{buffer, parser};
