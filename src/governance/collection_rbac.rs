/*
 * Copyright 2026 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Per-repo RBAC enforcement for data-plane operations
//!
//! This module provides collection-level access control based on the
//! `permitted_principals` field in CollectionConfig. It enforces fail-closed
//! access control at the data-plane handler layer (REST/gRPC/pgwire).

use anyhow::Result;
use proximadb_proto::proximadb_v1::CollectionConfig;

/// Error type for RBAC violations
#[derive(Debug, Clone, thiserror::Error)]
pub enum RbacError {
    #[error(
        "Access denied: principal '{principal}' is not permitted to access collection '{collection}'"
    )]
    AccessDenied {
        collection: String,
        principal: String,
    },

    #[error("RBAC check failed: {0}")]
    Internal(String),
}

/// Check if a principal is permitted to access a collection based on its
/// `permitted_principals` field.
///
/// # Arguments
/// * `collection` - The collection configuration to check
/// * `principal_id` - The ID of the principal (user/service account) attempting access
///
/// # Returns
/// * `Ok(())` if access is permitted
/// * `Err(RbacError::AccessDenied)` if the principal is not in the permitted list
///
/// # Behavior
/// * If `permitted_principals` is empty or not set, access is unrestricted (returns Ok)
/// * If `permitted_principals` is set, the principal must be in the list to access the collection
/// * This is a fail-closed check: explicit deny if not explicitly allowed
pub fn check_collection_access(
    collection: &CollectionConfig,
    principal_id: &str,
) -> Result<(), RbacError> {
    // If no permitted_principals are set, access is unrestricted
    if collection.permitted_principals.is_empty() {
        return Ok(());
    }

    // Check if the principal is in the permitted list
    if !collection
        .permitted_principals
        .contains(&principal_id.to_string())
    {
        return Err(RbacError::AccessDenied {
            collection: collection.name.clone(),
            principal: principal_id.to_string(),
        });
    }

    Ok(())
}

/// Extension trait for convenient RBAC checking on CollectionConfig
pub trait CollectionRbacExt {
    /// Check if a principal is permitted to access this collection
    fn check_principal_access(&self, principal_id: &str) -> Result<(), RbacError>;
}

impl CollectionRbacExt for CollectionConfig {
    fn check_principal_access(&self, principal_id: &str) -> Result<(), RbacError> {
        check_collection_access(self, principal_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_proto::proximadb_v1::{CollectionConfig, DistanceMetric, StorageEngine};

    fn test_collection() -> CollectionConfig {
        CollectionConfig {
            name: "test_collection".to_string(),
            dimension: 384,
            distance_metric: Some(DistanceMetric::Cosine as i32),
            storage_engine: Some(StorageEngine::Sst as i32),
            ..Default::default()
        }
    }

    #[test]
    fn empty_permitted_principals_allows_all_access() {
        let collection = test_collection();
        assert!(check_collection_access(&collection, "user1").is_ok());
        assert!(check_collection_access(&collection, "user2").is_ok());
        assert!(check_collection_access(&collection, "").is_ok());
    }

    #[test]
    fn permitted_principals_restricts_access() {
        let mut collection = test_collection();
        collection.permitted_principals = vec!["alice".to_string(), "bob".to_string()];

        // Users in the list are allowed
        assert!(check_collection_access(&collection, "alice").is_ok());
        assert!(check_collection_access(&collection, "bob").is_ok());

        // Users not in the list are denied
        assert!(check_collection_access(&collection, "charlie").is_err());
        assert!(check_collection_access(&collection, "eve").is_err());
    }

    #[test]
    fn extension_trait_works() {
        let mut collection = test_collection();
        collection.permitted_principals = vec!["admin".to_string()];

        assert!(collection.check_principal_access("admin").is_ok());
        assert!(collection.check_principal_access("user").is_err());
    }

    #[test]
    fn rbac_error_contains_details() {
        let mut collection = test_collection();
        collection.permitted_principals = vec!["alice".to_string()];
        collection.name = "secret_collection".to_string();

        let err = check_collection_access(&collection, "bob").unwrap_err();
        match err {
            RbacError::AccessDenied {
                collection,
                principal,
            } => {
                assert_eq!(collection, "secret_collection");
                assert_eq!(principal, "bob");
            }
            _ => panic!("Expected AccessDenied error"),
        }
    }
}
