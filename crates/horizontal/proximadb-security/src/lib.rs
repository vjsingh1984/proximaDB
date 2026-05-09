//! Shared security, audit, and policy contracts.
//!
//! Runtime enforcement stays in the root/platform security modules during the
//! migration. This crate owns reusable security value contracts that lower
//! layers can depend on without pulling in root services.

pub mod audit;
pub mod audit_config;
pub mod audit_storage;

pub use audit::{
    AuditEvent, AuditEventType, AuditResource, AuditResult, SecurityAlert, SecurityAlertSeverity,
    SecurityAlertType,
};
pub use audit_config::{AuditConfig, AuditStorageBackend, EmailConfig, EncryptionAlgorithm};
pub use audit_storage::{AuditStatistics, AuditStorage};
