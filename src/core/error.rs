//! Compatibility shim for legacy core error imports.
//!
//! The foundational error contract now lives in the workspace leaf crate
//! `proximadb-kernel`. This module preserves existing imports such as
//! `crate::core::error::ProximaDBError` while keeping root-only bridge
//! conversions local to the root crate.

pub use proximadb_kernel::error::*;

impl From<crate::storage::persistence::filesystem::FilesystemError> for VectorDBError {
    fn from(err: crate::storage::persistence::filesystem::FilesystemError) -> Self {
        VectorDBError::Storage(StorageError::DiskIO(std::io::Error::other(err.to_string())))
    }
}

impl From<StorageError> for crate::core::errors::StorageError {
    fn from(err: StorageError) -> Self {
        match err {
            StorageError::SstEngine(msg) => {
                crate::core::errors::StorageError::DiskIO(format!("SST engine: {}", msg))
            }
            StorageError::Mmap(msg) => {
                crate::core::errors::StorageError::DiskIO(format!("MMAP: {}", msg))
            }
            StorageError::DiskIO(io_err) => {
                crate::core::errors::StorageError::DiskIO(io_err.to_string())
            }
            StorageError::Serialization(msg) | StorageError::SerializationError(msg) => {
                crate::core::errors::StorageError::DiskIO(format!("Serialization: {}", msg))
            }
            StorageError::Corruption(msg) => crate::core::errors::StorageError::Corruption(msg),
            StorageError::AlreadyExists(msg) => {
                crate::core::errors::StorageError::InvalidOperation(format!(
                    "Already exists: {}",
                    msg
                ))
            }
            StorageError::NotFound(msg) | StorageError::KeyNotFound(msg) => {
                crate::core::errors::StorageError::InvalidOperation(format!("Not found: {}", msg))
            }
            StorageError::IndexError(msg) => {
                crate::core::errors::StorageError::InvalidOperation(format!("Index: {}", msg))
            }
            StorageError::WalError(msg) => crate::core::errors::StorageError::WAL(msg),
            StorageError::MetadataError(err) => {
                crate::core::errors::StorageError::Metadata(err.to_string())
            }
            StorageError::CollectionNotFound(id) => {
                crate::core::errors::StorageError::InvalidOperation(format!(
                    "Collection not found: {}",
                    id
                ))
            }
            StorageError::InvalidDimension { expected, actual } => {
                crate::core::errors::StorageError::InvalidOperation(format!(
                    "Dimension mismatch: expected {}, got {}",
                    expected, actual
                ))
            }
            StorageError::TransactionCommitFailed(msg) => {
                crate::core::errors::StorageError::InvalidOperation(format!(
                    "Transaction commit failed: {}",
                    msg
                ))
            }
        }
    }
}

impl From<ProtocolError> for crate::core::errors::ProximaDBError {
    fn from(err: ProtocolError) -> Self {
        match err {
            ProtocolError::InvalidArgument { msg, .. } => {
                crate::core::errors::ProximaDBError::InvalidInput(msg)
            }
            ProtocolError::NotFound { resource, id } => {
                crate::core::errors::ProximaDBError::NotFound {
                    resource_type: resource,
                    id,
                }
            }
            ProtocolError::AlreadyExists { resource, id } => {
                crate::core::errors::ProximaDBError::AlreadyExists {
                    resource_type: resource,
                    id,
                }
            }
            ProtocolError::Internal { details } => {
                crate::core::errors::ProximaDBError::Internal(details)
            }
            ProtocolError::PermissionDenied { action } => {
                crate::core::errors::ProximaDBError::PermissionDenied(action)
            }
            ProtocolError::Timeout { duration_ms, .. } => {
                crate::core::errors::ProximaDBError::Timeout(duration_ms / 1000)
            }
            ProtocolError::ResourceExhausted { details } => {
                crate::core::errors::ProximaDBError::CapacityExceeded { message: details }
            }
            ProtocolError::PreconditionFailed { details } => {
                crate::core::errors::ProximaDBError::InvalidInput(details)
            }
        }
    }
}

impl From<VectorDBError> for crate::core::errors::ProximaDBError {
    fn from(err: VectorDBError) -> Self {
        match err {
            VectorDBError::Storage(e) => crate::core::errors::ProximaDBError::Storage(e.into()),
            VectorDBError::Config(msg) => {
                crate::core::errors::ProximaDBError::Internal(format!("Config: {}", msg))
            }
            VectorDBError::Internal(msg) => crate::core::errors::ProximaDBError::Internal(msg),
            VectorDBError::Quantization(msg) => {
                crate::core::errors::ProximaDBError::Quantization(msg)
            }
            VectorDBError::InvalidInput(msg) => {
                crate::core::errors::ProximaDBError::InvalidInput(msg)
            }
            VectorDBError::Filesystem(msg) => crate::core::errors::ProximaDBError::Io(msg),
            VectorDBError::NotImplemented(msg) => {
                crate::core::errors::ProximaDBError::Internal(format!("Not implemented: {}", msg))
            }
            VectorDBError::Consensus(e) => {
                crate::core::errors::ProximaDBError::Network(e.to_string())
            }
            VectorDBError::Network(e) => {
                crate::core::errors::ProximaDBError::Network(e.to_string())
            }
            VectorDBError::Query(e) => crate::core::errors::ProximaDBError::Internal(e.to_string()),
            VectorDBError::Schema(e) => {
                crate::core::errors::ProximaDBError::Internal(e.to_string())
            }
            VectorDBError::TransactionNotFound { id } => {
                crate::core::errors::ProximaDBError::TransactionNotFound { id }
            }
            VectorDBError::TransactionNotActive { id } => {
                crate::core::errors::ProximaDBError::TransactionNotActive { id }
            }
            VectorDBError::TransactionTimedOut { id } => {
                crate::core::errors::ProximaDBError::TransactionTimedOut { id }
            }
            VectorDBError::TransactionConflict {
                transaction,
                conflicting_with,
            } => crate::core::errors::ProximaDBError::TransactionConflict {
                transaction,
                conflicting_with,
            },
            VectorDBError::TooManyTransactions { max } => {
                crate::core::errors::ProximaDBError::TooManyTransactions { max }
            }
            VectorDBError::TransactionAborted(msg) => {
                crate::core::errors::ProximaDBError::Internal(format!(
                    "Transaction aborted: {}",
                    msg
                ))
            }
            VectorDBError::LockTimeout { resource } => {
                crate::core::errors::ProximaDBError::LockTimeout { resource }
            }
            VectorDBError::DeadlockDetected { transaction } => {
                crate::core::errors::ProximaDBError::DeadlockDetected { transaction }
            }
            VectorDBError::SavepointNotFound { name } => {
                crate::core::errors::ProximaDBError::SavepointNotFound { name }
            }
        }
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
