use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Relational schema definition for ProximaDB
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationalSchema {
    /// Schema name
    pub name: String,
    /// Schema version for evolution
    pub version: String,
    /// Table definitions
    pub tables: HashMap<String, TableDefinition>,
    /// Foreign key constraints
    pub foreign_keys: Vec<ForeignKeyConstraint>,
    /// Optional schema description
    pub description: Option<String>,
}

/// Table definition in relational schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableDefinition {
    /// Table name
    pub name: String,
    /// Column definitions
    pub columns: HashMap<String, ColumnDefinition>,
    /// Primary key columns
    pub primary_key: Vec<String>,
    /// Unique constraints
    pub unique_constraints: Vec<UniqueConstraint>,
    /// Check constraints
    pub check_constraints: Vec<CheckConstraint>,
    /// Table indexes
    pub indexes: Vec<IndexDefinition>,
    /// Optional description
    pub description: Option<String>,
}

/// Column definition in table schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnDefinition {
    /// Column name
    pub name: String,
    /// Column data type
    pub data_type: SqlDataType,
    /// Whether column is nullable
    pub nullable: bool,
    /// Default value
    pub default_value: Option<String>,
    /// Column constraints
    pub constraints: Vec<ColumnConstraint>,
    /// Optional description
    pub description: Option<String>,
}

/// SQL data types supported in relational schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SqlDataType {
    Integer,
    BigInt,
    Real,
    Double,
    Decimal(u32, u32), // precision, scale
    VarChar(u32),      // max length
    Char(u32),         // fixed length
    Text,
    Boolean,
    Date,
    Time,
    Timestamp,
    TimestampTz,
    Json,
    Jsonb,
    Vector(u32), // Vector with dimension
    Uuid,
    Bytea,
}

/// Column constraints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ColumnConstraint {
    NotNull,
    Unique,
    PrimaryKey,
    Check(String), // Check expression
    References {
        table: String,
        column: String,
        on_delete: ReferentialAction,
        on_update: ReferentialAction,
    },
}

/// Referential actions for foreign keys
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReferentialAction {
    NoAction,
    Restrict,
    Cascade,
    SetNull,
    SetDefault,
}

/// Unique constraint definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniqueConstraint {
    pub name: String,
    pub columns: Vec<String>,
}

/// Check constraint definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckConstraint {
    pub name: String,
    pub expression: String,
}

/// Foreign key constraint definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForeignKeyConstraint {
    pub name: String,
    pub from_table: String,
    pub from_columns: Vec<String>,
    pub to_table: String,
    pub to_columns: Vec<String>,
    pub on_delete: ReferentialAction,
    pub on_update: ReferentialAction,
}

/// Index definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexDefinition {
    pub name: String,
    pub table: String,
    pub columns: Vec<String>,
    pub index_type: IndexType,
    pub unique: bool,
    pub where_clause: Option<String>, // Partial index condition
}

/// Index types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IndexType {
    BTree,
    Hash,
    Gin,    // For JSON, arrays
    Gist,   // For geometric data
    SpGist, // Space-partitioned GIST
    Brin,   // Block range indexes
    Vector, // For vector similarity
}

impl RelationalSchema {
    /// Create new relational schema
    pub fn new(name: String, version: String) -> Self {
        Self {
            name,
            version,
            tables: HashMap::new(),
            foreign_keys: Vec::new(),
            description: None,
        }
    }

    /// Add table to schema
    pub fn add_table(&mut self, table: TableDefinition) -> &mut Self {
        self.tables.insert(table.name.clone(), table);
        self
    }

    /// Add foreign key constraint
    pub fn add_foreign_key(&mut self, fk: ForeignKeyConstraint) -> &mut Self {
        self.foreign_keys.push(fk);
        self
    }

    /// Validate schema consistency
    pub fn validate(&self) -> Result<(), SchemaValidationError> {
        // Validate that all tables exist for foreign keys
        for fk in &self.foreign_keys {
            if !self.tables.contains_key(&fk.from_table) {
                return Err(SchemaValidationError::TableNotFound(fk.from_table.clone()));
            }
            if !self.tables.contains_key(&fk.to_table) {
                return Err(SchemaValidationError::TableNotFound(fk.to_table.clone()));
            }

            // Validate columns exist
            let from_table = &self.tables[&fk.from_table];
            let to_table = &self.tables[&fk.to_table];

            for col in &fk.from_columns {
                if !from_table.columns.contains_key(col) {
                    return Err(SchemaValidationError::ColumnNotFound(
                        fk.from_table.clone(),
                        col.clone(),
                    ));
                }
            }

            for col in &fk.to_columns {
                if !to_table.columns.contains_key(col) {
                    return Err(SchemaValidationError::ColumnNotFound(
                        fk.to_table.clone(),
                        col.clone(),
                    ));
                }
            }
        }

        // Validate primary keys exist
        for (table_name, table) in &self.tables {
            for pk_col in &table.primary_key {
                if !table.columns.contains_key(pk_col) {
                    return Err(SchemaValidationError::ColumnNotFound(
                        table_name.clone(),
                        pk_col.clone(),
                    ));
                }
            }
        }

        Ok(())
    }

    /// Generate CREATE TABLE SQL for a table
    pub fn generate_create_table_sql(&self, table_name: &str) -> Option<String> {
        if let Some(table) = self.tables.get(table_name) {
            let mut sql = format!("CREATE TABLE {} (", table_name);

            let mut column_defs = Vec::new();
            for (col_name, col_def) in &table.columns {
                let mut col_sql = format!(
                    "{} {}",
                    col_name,
                    self.sql_type_to_string(&col_def.data_type)
                );
                if !col_def.nullable {
                    col_sql.push_str(" NOT NULL");
                }
                if let Some(default) = &col_def.default_value {
                    col_sql.push_str(&format!(" DEFAULT {}", default));
                }
                column_defs.push(col_sql);
            }

            if !table.primary_key.is_empty() {
                column_defs.push(format!("PRIMARY KEY ({})", table.primary_key.join(", ")));
            }

            sql.push_str(&column_defs.join(", "));
            sql.push(')');

            Some(sql)
        } else {
            None
        }
    }

    fn sql_type_to_string(&self, data_type: &SqlDataType) -> String {
        match data_type {
            SqlDataType::Integer => "INTEGER".to_string(),
            SqlDataType::BigInt => "BIGINT".to_string(),
            SqlDataType::Real => "REAL".to_string(),
            SqlDataType::Double => "DOUBLE PRECISION".to_string(),
            SqlDataType::Decimal(p, s) => format!("DECIMAL({}, {})", p, s),
            SqlDataType::VarChar(len) => format!("VARCHAR({})", len),
            SqlDataType::Char(len) => format!("CHAR({})", len),
            SqlDataType::Text => "TEXT".to_string(),
            SqlDataType::Boolean => "BOOLEAN".to_string(),
            SqlDataType::Date => "DATE".to_string(),
            SqlDataType::Time => "TIME".to_string(),
            SqlDataType::Timestamp => "TIMESTAMP".to_string(),
            SqlDataType::TimestampTz => "TIMESTAMP WITH TIME ZONE".to_string(),
            SqlDataType::Json => "JSON".to_string(),
            SqlDataType::Jsonb => "JSONB".to_string(),
            SqlDataType::Vector(dim) => format!("VECTOR({})", dim),
            SqlDataType::Uuid => "UUID".to_string(),
            SqlDataType::Bytea => "BYTEA".to_string(),
        }
    }
}

impl TableDefinition {
    /// Create new table definition
    pub fn new(name: String) -> Self {
        Self {
            name,
            columns: HashMap::new(),
            primary_key: Vec::new(),
            unique_constraints: Vec::new(),
            check_constraints: Vec::new(),
            indexes: Vec::new(),
            description: None,
        }
    }

    /// Add column to table
    pub fn add_column(&mut self, column: ColumnDefinition) -> &mut Self {
        self.columns.insert(column.name.clone(), column);
        self
    }

    /// Set primary key
    pub fn set_primary_key(&mut self, columns: Vec<String>) -> &mut Self {
        self.primary_key = columns;
        self
    }
}

/// Schema validation errors
#[derive(Debug)]
pub enum SchemaValidationError {
    TableNotFound(String),
    ColumnNotFound(String, String),
    DuplicateTable(String),
    DuplicateColumn(String, String),
    InvalidConstraint(String),
}

impl std::fmt::Display for SchemaValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            SchemaValidationError::TableNotFound(table) => write!(f, "Table not found: {}", table),
            SchemaValidationError::ColumnNotFound(table, column) => {
                write!(f, "Column {} not found in table {}", column, table)
            }
            SchemaValidationError::DuplicateTable(table) => write!(f, "Duplicate table: {}", table),
            SchemaValidationError::DuplicateColumn(table, column) => {
                write!(f, "Duplicate column {} in table {}", column, table)
            }
            SchemaValidationError::InvalidConstraint(msg) => {
                write!(f, "Invalid constraint: {}", msg)
            }
        }
    }
}

impl std::error::Error for SchemaValidationError {}
