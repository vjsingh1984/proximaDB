//! Shared security, audit, and policy contracts.
//!
//! Runtime enforcement stays in the root/platform security modules during the
//! migration. This crate owns reusable security value contracts that lower
//! layers can depend on without pulling in root services.

pub mod audit;

pub use audit::{
    AuditEvent, AuditEventType, AuditResource, AuditResult, SecurityAlert, SecurityAlertSeverity,
    SecurityAlertType,
};
