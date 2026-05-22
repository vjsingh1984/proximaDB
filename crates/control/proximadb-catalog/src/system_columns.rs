//! Reserved ProximaDB system columns exposed through SQL/catalog facades.
//!
//! These names are SQL-visible aliases over canonical `ProximaRecord` fields,
//! row-version metadata, or xCatalog metadata. They are not application columns
//! and must remain reserved to avoid collisions with user schemas.

use serde::{Deserialize, Serialize};

use crate::CatalogDataType;

/// Reserved prefix for SQL-visible internal columns.
pub const SYSTEM_COLUMN_PREFIX: &str = "__proxima_";

pub const OID: &str = "__proxima_oid";
pub const TENANT_ID: &str = "__proxima_tenant_id";
pub const RECORD_VERSION: &str = "__proxima_record_version";
pub const CREATED_AT_NS: &str = "__proxima_created_at_ns";
pub const UPDATED_AT_NS: &str = "__proxima_updated_at_ns";
pub const VALID_FROM_NS: &str = "__proxima_valid_from_ns";
pub const VALID_TO_NS: &str = "__proxima_valid_to_ns";
pub const ACTOR: &str = "__proxima_actor";
pub const ORIGIN: &str = "__proxima_origin";
pub const DELETED: &str = "__proxima_deleted";
pub const BRANCH_ID: &str = "__proxima_branch_id";
pub const SCHEMA_VERSION: &str = "__proxima_schema_version";

/// Import/export aliases accepted for compatibility, but never canonical.
pub const LEGACY_DELETED_ALIAS: &str = "_deleted";
pub const LEGACY_VERSION_ALIAS: &str = "_version";

/// Stable identifier for a reserved system column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SystemColumnId {
    Oid,
    TenantId,
    RecordVersion,
    CreatedAtNs,
    UpdatedAtNs,
    ValidFromNs,
    ValidToNs,
    Actor,
    Origin,
    Deleted,
    BranchId,
    SchemaVersion,
}

/// SQL-visible metadata for a reserved system column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SystemColumn {
    pub id: SystemColumnId,
    pub name: &'static str,
    pub data_type: CatalogDataType,
    pub nullable: bool,
    /// Canonical source field or derived metadata path.
    pub source: &'static str,
}

/// Return true if `name` is reserved for ProximaDB internal/system metadata.
pub fn is_reserved_column_name(name: &str) -> bool {
    name.starts_with(SYSTEM_COLUMN_PREFIX)
}

/// Map an accepted compatibility alias to its canonical system column.
pub fn canonicalize_system_alias(name: &str) -> Option<&'static str> {
    match name {
        LEGACY_DELETED_ALIAS => Some(DELETED),
        LEGACY_VERSION_ALIAS => Some(RECORD_VERSION),
        OID => Some(OID),
        TENANT_ID => Some(TENANT_ID),
        RECORD_VERSION => Some(RECORD_VERSION),
        CREATED_AT_NS => Some(CREATED_AT_NS),
        UPDATED_AT_NS => Some(UPDATED_AT_NS),
        VALID_FROM_NS => Some(VALID_FROM_NS),
        VALID_TO_NS => Some(VALID_TO_NS),
        ACTOR => Some(ACTOR),
        ORIGIN => Some(ORIGIN),
        DELETED => Some(DELETED),
        BRANCH_ID => Some(BRANCH_ID),
        SCHEMA_VERSION => Some(SCHEMA_VERSION),
        _ => None,
    }
}

/// Ordered set of reserved SQL-visible system columns.
pub fn system_columns() -> Vec<SystemColumn> {
    vec![
        SystemColumn {
            id: SystemColumnId::Oid,
            name: OID,
            data_type: CatalogDataType::String,
            nullable: false,
            source: "ProximaRecord.oid",
        },
        SystemColumn {
            id: SystemColumnId::TenantId,
            name: TENANT_ID,
            data_type: CatalogDataType::String,
            nullable: false,
            source: "ProximaRecord.tenant_id",
        },
        SystemColumn {
            id: SystemColumnId::RecordVersion,
            name: RECORD_VERSION,
            data_type: CatalogDataType::Int64,
            nullable: false,
            source: "ProximaRecord.record_version",
        },
        SystemColumn {
            id: SystemColumnId::CreatedAtNs,
            name: CREATED_AT_NS,
            data_type: CatalogDataType::Int64,
            nullable: false,
            source: "ProximaRecord.created_at_ns",
        },
        SystemColumn {
            id: SystemColumnId::UpdatedAtNs,
            name: UPDATED_AT_NS,
            data_type: CatalogDataType::Int64,
            nullable: false,
            source: "ProximaRecord.updated_at_ns",
        },
        SystemColumn {
            id: SystemColumnId::ValidFromNs,
            name: VALID_FROM_NS,
            data_type: CatalogDataType::Int64,
            nullable: true,
            source: "ProximaRecord.valid_from_ns",
        },
        SystemColumn {
            id: SystemColumnId::ValidToNs,
            name: VALID_TO_NS,
            data_type: CatalogDataType::Int64,
            nullable: true,
            source: "ProximaRecord.valid_to_ns",
        },
        SystemColumn {
            id: SystemColumnId::Actor,
            name: ACTOR,
            data_type: CatalogDataType::String,
            nullable: true,
            source: "ProximaRecord.actor",
        },
        SystemColumn {
            id: SystemColumnId::Origin,
            name: ORIGIN,
            data_type: CatalogDataType::String,
            nullable: true,
            source: "ProximaRecord.origin",
        },
        SystemColumn {
            id: SystemColumnId::Deleted,
            name: DELETED,
            data_type: CatalogDataType::Boolean,
            nullable: false,
            source: "derived tombstone/visibility state",
        },
        SystemColumn {
            id: SystemColumnId::BranchId,
            name: BRANCH_ID,
            data_type: CatalogDataType::String,
            nullable: true,
            source: "branch/snapshot metadata",
        },
        SystemColumn {
            id: SystemColumnId::SchemaVersion,
            name: SCHEMA_VERSION,
            data_type: CatalogDataType::Int32,
            nullable: false,
            source: "xCatalog schema_version",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserves_double_underscore_prefix() {
        assert!(is_reserved_column_name("__proxima_deleted"));
        assert!(!is_reserved_column_name("_deleted"));
        assert!(!is_reserved_column_name("deleted"));
    }

    #[test]
    fn maps_legacy_aliases_to_canonical_columns() {
        assert_eq!(canonicalize_system_alias("_deleted"), Some(DELETED));
        assert_eq!(canonicalize_system_alias("_version"), Some(RECORD_VERSION));
    }

    #[test]
    fn canonicalizes_every_system_column_and_rejects_user_columns() {
        for name in [
            OID,
            TENANT_ID,
            RECORD_VERSION,
            CREATED_AT_NS,
            UPDATED_AT_NS,
            VALID_FROM_NS,
            VALID_TO_NS,
            ACTOR,
            ORIGIN,
            DELETED,
            BRANCH_ID,
            SCHEMA_VERSION,
        ] {
            assert_eq!(canonicalize_system_alias(name), Some(name));
            assert!(is_reserved_column_name(name));
        }

        assert_eq!(canonicalize_system_alias("user_column"), None);
        assert!(!is_reserved_column_name("proxima_oid"));
    }

    #[test]
    fn ordered_system_columns_describe_canonical_record_and_catalog_sources() {
        let columns = system_columns();
        assert_eq!(columns.len(), 12);
        assert_eq!(columns[0].id, SystemColumnId::Oid);
        assert_eq!(columns[0].name, OID);
        assert_eq!(columns[0].data_type, CatalogDataType::String);
        assert!(!columns[0].nullable);
        assert_eq!(columns[0].source, "ProximaRecord.oid");

        let tenant = columns
            .iter()
            .find(|column| column.id == SystemColumnId::TenantId)
            .unwrap();
        assert_eq!(tenant.source, "ProximaRecord.tenant_id");
        assert!(!tenant.nullable);

        let deleted = columns
            .iter()
            .find(|column| column.id == SystemColumnId::Deleted)
            .unwrap();
        assert_eq!(deleted.data_type, CatalogDataType::Boolean);
        assert_eq!(deleted.source, "derived tombstone/visibility state");

        let schema_version = columns
            .iter()
            .find(|column| column.id == SystemColumnId::SchemaVersion)
            .unwrap();
        assert_eq!(schema_version.data_type, CatalogDataType::Int32);
        assert_eq!(schema_version.source, "xCatalog schema_version");

        let serialized = serde_json::to_string(&SystemColumnId::RecordVersion).unwrap();
        assert_eq!(
            serde_json::from_str::<SystemColumnId>(&serialized).unwrap(),
            SystemColumnId::RecordVersion
        );
    }
}
