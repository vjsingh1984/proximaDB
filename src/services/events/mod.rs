//! Event logging and persistence
//!
//! Handles event log for operations, recovery, and audit

pub mod log;
#[cfg(feature = "axis")]
pub mod persistence;

pub use log::{EventLogService as EventLog, EventLogStats as Stats};

#[cfg(feature = "axis")]
pub use persistence::EventLogWAL as WAL;
