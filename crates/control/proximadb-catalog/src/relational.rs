//! Canonical relational row contract for xCatalog tables.
//!
//! This module is intentionally storage-neutral. SQL, Arrow Flight, REST/gRPC,
//! and embedded loaders can validate/project rows here before they become
//! canonical `ProximaRecord` writes in the storage layer.

use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow};
use proximadb_data_model::ProximaValue;
use serde::{Deserialize, Serialize};

use crate::{CatalogColumn, CatalogDataType, CatalogTableSchema};

/// Schema enforcement mode for table writes and external/schema-on-read scans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelationalSchemaMode {
    /// Enforce catalog schema before accepting a row into canonical storage.
    SchemaOnWrite,
    /// Validate/project known columns but allow source-shaped rows to carry extras.
    SchemaOnRead,
}

impl Default for RelationalSchemaMode {
    fn default() -> Self {
        Self::SchemaOnWrite
    }
}

/// Validation profile shared by OLTP, OLAP, Arrow, SQL, and protocol surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationalWriteProfile {
    /// Schema-on-write for canonical durable tables; schema-on-read for external files/tables.
    pub mode: RelationalSchemaMode,
    /// Reject values for columns not cataloged in `CatalogTableSchema`.
    pub reject_unknown_columns: bool,
    /// Require all catalog columns to be present, even nullable/defaulted columns.
    pub require_all_columns: bool,
}

impl Default for RelationalWriteProfile {
    fn default() -> Self {
        Self {
            mode: RelationalSchemaMode::SchemaOnWrite,
            reject_unknown_columns: true,
            require_all_columns: false,
        }
    }
}

impl RelationalWriteProfile {
    /// A strict OLTP-style profile for schema-on-write table inserts.
    pub fn oltp() -> Self {
        Self::default()
    }

    /// A permissive profile for external/schema-on-read files and table scans.
    pub fn schema_on_read() -> Self {
        Self {
            mode: RelationalSchemaMode::SchemaOnRead,
            reject_unknown_columns: false,
            require_all_columns: false,
        }
    }
}

/// Catalog-validated row values keyed by logical column name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogRow {
    /// Catalog table name this row was validated against.
    pub table: String,
    /// Values for cataloged columns.
    pub values: HashMap<String, ProximaValue>,
}

impl CatalogRow {
    /// Validate raw values against a catalog schema and return a projected row.
    pub fn validate(
        schema: &CatalogTableSchema,
        values: HashMap<String, ProximaValue>,
        profile: &RelationalWriteProfile,
    ) -> Result<Self> {
        let column_names: HashSet<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();

        if profile.reject_unknown_columns {
            for key in values.keys() {
                if !column_names.contains(key.as_str()) {
                    return Err(anyhow!(
                        "Unknown column '{}' for table '{}'",
                        key,
                        schema.name
                    ));
                }
            }
        }

        let mut projected = HashMap::new();
        for column in &schema.columns {
            match values.get(&column.name) {
                Some(value) => {
                    validate_column_value(schema, column, value)?;
                    projected.insert(column.name.clone(), value.clone());
                }
                None if !column.nullable && column.default_value.is_none() => {
                    return Err(anyhow!(
                        "Missing required column '{}' for table '{}'",
                        column.name,
                        schema.name
                    ));
                }
                None if profile.require_all_columns => {
                    return Err(anyhow!(
                        "Missing column '{}' for table '{}'",
                        column.name,
                        schema.name
                    ));
                }
                None => {}
            }
        }

        let row = Self {
            table: schema.name.clone(),
            values: projected,
        };
        row.primary_key_values(schema)?;
        Ok(row)
    }

    /// Return primary-key values in catalog order.
    pub fn primary_key_values(&self, schema: &CatalogTableSchema) -> Result<Vec<ProximaValue>> {
        let primary_key = effective_primary_key(schema);
        let mut values = Vec::with_capacity(primary_key.len());
        for column in primary_key {
            match self.values.get(column) {
                Some(ProximaValue::Null) | None => {
                    return Err(anyhow!(
                        "Primary key column '{}' is missing or null for table '{}'",
                        column,
                        schema.name
                    ));
                }
                Some(value) => values.push(value.clone()),
            }
        }
        Ok(values)
    }

    /// Stable logical row key derived from primary-key values.
    pub fn primary_key_string(&self, schema: &CatalogTableSchema) -> Result<Option<String>> {
        let pk_values = self.primary_key_values(schema)?;
        if pk_values.is_empty() {
            return Ok(None);
        }

        let mut parts = Vec::with_capacity(pk_values.len());
        for value in pk_values {
            parts.push(stable_value_string(&value)?);
        }
        Ok(Some(parts.join("\u{1f}")))
    }
}

fn effective_primary_key(schema: &CatalogTableSchema) -> Vec<&str> {
    if !schema.relational_capabilities.primary_key.is_empty() {
        schema
            .relational_capabilities
            .primary_key
            .iter()
            .map(String::as_str)
            .collect()
    } else {
        schema.primary_key.iter().map(String::as_str).collect()
    }
}

fn validate_column_value(
    schema: &CatalogTableSchema,
    column: &CatalogColumn,
    value: &ProximaValue,
) -> Result<()> {
    if matches!(value, ProximaValue::Null) {
        if column.nullable {
            return Ok(());
        }
        return Err(anyhow!(
            "Column '{}' in table '{}' is not nullable",
            column.name,
            schema.name
        ));
    }

    if !matches_catalog_type(value, column.data_type) {
        return Err(anyhow!(
            "Column '{}' in table '{}' expects {:?}, got {}",
            column.name,
            schema.name,
            column.data_type,
            proxima_value_type_name(value)
        ));
    }

    Ok(())
}

fn matches_catalog_type(value: &ProximaValue, data_type: CatalogDataType) -> bool {
    match data_type {
        CatalogDataType::Boolean => matches!(value, ProximaValue::Boolean(_)),
        CatalogDataType::Int8 => matches!(value, ProximaValue::Int8(_)),
        CatalogDataType::Int16 => matches!(value, ProximaValue::Int8(_) | ProximaValue::Int16(_)),
        CatalogDataType::Int32 => matches!(
            value,
            ProximaValue::Int8(_) | ProximaValue::Int16(_) | ProximaValue::Int32(_)
        ),
        CatalogDataType::Int64 => matches!(
            value,
            ProximaValue::Int8(_)
                | ProximaValue::Int16(_)
                | ProximaValue::Int32(_)
                | ProximaValue::Int64(_)
        ),
        CatalogDataType::Float32 => {
            matches!(value, ProximaValue::Float16(_) | ProximaValue::Float32(_))
        }
        CatalogDataType::Float64 => matches!(
            value,
            ProximaValue::Float16(_) | ProximaValue::Float32(_) | ProximaValue::Float64(_)
        ),
        CatalogDataType::String => {
            matches!(value, ProximaValue::String(_) | ProximaValue::Symbol(_))
        }
        CatalogDataType::Binary => matches!(value, ProximaValue::Binary(_)),
        CatalogDataType::Date => matches!(value, ProximaValue::Date(_)),
        CatalogDataType::Time => matches!(value, ProximaValue::Time(_, _)),
        CatalogDataType::Timestamp => matches!(value, ProximaValue::Timestamp(_, _)),
        CatalogDataType::TimestampTz => matches!(value, ProximaValue::TimestampTz(_, _)),
        CatalogDataType::Decimal => matches!(value, ProximaValue::Decimal(_)),
        CatalogDataType::Uuid => matches!(value, ProximaValue::Uuid(_) | ProximaValue::String(_)),
        CatalogDataType::Json => matches!(
            value,
            ProximaValue::Json(_)
                | ProximaValue::Jsonb(_)
                | ProximaValue::Map(_)
                | ProximaValue::Struct(_)
        ),
        CatalogDataType::Vector => matches!(value, ProximaValue::DenseVector(_)),
        CatalogDataType::SparseVector => matches!(value, ProximaValue::SparseVector { .. }),
        CatalogDataType::BinaryVector => {
            matches!(
                value,
                ProximaValue::BinaryVector(_) | ProximaValue::Binary(_)
            )
        }
    }
}

fn stable_value_string(value: &ProximaValue) -> Result<String> {
    serde_json::to_string(value).map_err(|e| anyhow!("Failed to encode primary key value: {}", e))
}

fn proxima_value_type_name(value: &ProximaValue) -> &'static str {
    match value {
        ProximaValue::Boolean(_) => "boolean",
        ProximaValue::Int8(_) => "int8",
        ProximaValue::Int16(_) => "int16",
        ProximaValue::Int32(_) => "int32",
        ProximaValue::Int64(_) => "int64",
        ProximaValue::UInt8(_) => "uint8",
        ProximaValue::UInt16(_) => "uint16",
        ProximaValue::UInt32(_) => "uint32",
        ProximaValue::UInt64(_) => "uint64",
        ProximaValue::Float16(_) => "float16",
        ProximaValue::Float32(_) => "float32",
        ProximaValue::Float64(_) => "float64",
        ProximaValue::Decimal(_) => "decimal",
        ProximaValue::String(_) => "string",
        ProximaValue::Symbol(_) => "symbol",
        ProximaValue::Binary(_) => "binary",
        ProximaValue::Date(_) => "date",
        ProximaValue::Time(_, _) => "time",
        ProximaValue::Timestamp(_, _) => "timestamp",
        ProximaValue::TimestampTz(_, _) => "timestamptz",
        ProximaValue::Uuid(_) => "uuid",
        ProximaValue::ULID(_) => "ulid",
        ProximaValue::Json(_) => "json",
        ProximaValue::Jsonb(_) => "jsonb",
        ProximaValue::Array(_) => "array",
        ProximaValue::Map(_) => "map",
        ProximaValue::Struct(_) => "struct",
        ProximaValue::DenseVector(_) => "dense_vector",
        ProximaValue::SparseVector { .. } => "sparse_vector",
        ProximaValue::BinaryVector(_) => "binary_vector",
        ProximaValue::Null => "null",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CatalogColumn, RelationalCapabilities};

    fn users_schema() -> CatalogTableSchema {
        CatalogTableSchema::new("users")
            .with_column(CatalogColumn::new(1, "id", CatalogDataType::Int64).nullable(false))
            .with_column(CatalogColumn::new(2, "email", CatalogDataType::String).nullable(false))
            .with_column(CatalogColumn::new(3, "profile", CatalogDataType::Json))
            .with_column(CatalogColumn::new(4, "embedding", CatalogDataType::Vector))
            .with_relational_capabilities(RelationalCapabilities {
                primary_key: vec!["id".to_string()],
                ..Default::default()
            })
    }

    #[test]
    fn validates_schema_on_write_row() {
        let mut values = HashMap::new();
        values.insert("id".to_string(), ProximaValue::Int64(42));
        values.insert(
            "email".to_string(),
            ProximaValue::String("a@example.com".to_string()),
        );
        values.insert(
            "embedding".to_string(),
            ProximaValue::DenseVector(vec![0.1, 0.2]),
        );

        let row = CatalogRow::validate(&users_schema(), values, &RelationalWriteProfile::oltp())
            .expect("row should validate");
        assert_eq!(row.table, "users");
        assert_eq!(
            row.primary_key_string(&users_schema()).unwrap(),
            Some("{\"Int64\":42}".to_string())
        );
    }

    #[test]
    fn rejects_unknown_columns_for_schema_on_write() {
        let mut values = HashMap::new();
        values.insert("id".to_string(), ProximaValue::Int64(42));
        values.insert(
            "email".to_string(),
            ProximaValue::String("a@example.com".to_string()),
        );
        values.insert("extra".to_string(), ProximaValue::String("x".to_string()));

        let err = CatalogRow::validate(&users_schema(), values, &RelationalWriteProfile::oltp())
            .expect_err("unknown column should fail");
        assert!(err.to_string().contains("Unknown column"));
    }

    #[test]
    fn rejects_missing_required_column() {
        let mut values = HashMap::new();
        values.insert("id".to_string(), ProximaValue::Int64(42));

        let err = CatalogRow::validate(&users_schema(), values, &RelationalWriteProfile::oltp())
            .expect_err("missing required column should fail");
        assert!(err.to_string().contains("Missing required column 'email'"));
    }

    #[test]
    fn allows_unknown_columns_for_schema_on_read() {
        let mut values = HashMap::new();
        values.insert("id".to_string(), ProximaValue::Int64(42));
        values.insert(
            "email".to_string(),
            ProximaValue::String("a@example.com".to_string()),
        );
        values.insert("extra".to_string(), ProximaValue::String("x".to_string()));

        let row = CatalogRow::validate(
            &users_schema(),
            values,
            &RelationalWriteProfile::schema_on_read(),
        )
        .expect("schema-on-read row should validate projected columns");
        assert!(!row.values.contains_key("extra"));
    }

    #[test]
    fn rejects_type_mismatch() {
        let mut values = HashMap::new();
        values.insert(
            "id".to_string(),
            ProximaValue::String("not-an-int".to_string()),
        );
        values.insert(
            "email".to_string(),
            ProximaValue::String("a@example.com".to_string()),
        );

        let err = CatalogRow::validate(&users_schema(), values, &RelationalWriteProfile::oltp())
            .expect_err("type mismatch should fail");
        assert!(err.to_string().contains("expects Int64"));
    }
}
