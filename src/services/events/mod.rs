//! Event logging and persistence
//! 
//! Handles event log for operations, recovery, and audit

pub mod log;
pub mod persistence;

pub use log::{
    EventLogService as EventLog,
    EventLogStats as Stats,
};

pub use persistence::EventLogWAL as WAL;