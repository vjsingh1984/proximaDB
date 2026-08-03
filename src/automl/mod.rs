//! AutoML module — extracted subsystems in `proximadb-automl`; service stays root-side
//! (depends on control-tier metrics types).

pub use proximadb_automl::{optimization, prediction, tuning, workload};
pub mod service;
