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
    pub columns: HashMap<String, RelationalColumnDefinition>,
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

/// Backwards-compat alias for [`RelationalColumnDefinition`].
pub type ColumnDefinition = RelationalColumnDefinition;

/// Column definition in table schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationalColumnDefinition {
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
    /// 32-bit signed integer
    Integer,
    /// 64-bit signed integer
    BigInt,
    /// Single-precision floating point
    Real,
    /// Double-precision floating point
    Double,
    /// Fixed-point decimal with precision and scale
    Decimal(u32, u32),
    /// Variable-length character string with maximum length
    VarChar(u32),
    /// Fixed-length character string
    Char(u32),
    /// Unlimited-length text
    Text,
    /// Boolean (true/false) value
    Boolean,
    /// Calendar date without time
    Date,
    /// Time of day without date
    Time,
    /// Date and time without time zone
    Timestamp,
    /// Date and time with time zone
    TimestampTz,
    /// JSON text value
    Json,
    /// Binary JSON value with indexing support
    Jsonb,
    /// Fixed-dimension vector with the given dimensionality
    Vector(u32),
    /// Universally unique identifier
    Uuid,
    /// Variable-length binary data
    Bytea,
}

/// Column constraints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ColumnConstraint {
    /// Column must not contain null values
    NotNull,
    /// Column values must be unique across the table
    Unique,
    /// Column is the primary key
    PrimaryKey,
    /// Custom check expression that values must satisfy
    Check(String),
    /// Foreign key reference to another table's column
    References {
        /// Referenced table name
        table: String,
        /// Referenced column name
        column: String,
        /// Action on referenced row deletion
        on_delete: ReferentialAction,
        /// Action on referenced row update
        on_update: ReferentialAction,
    },
}

/// Referential actions for foreign keys
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReferentialAction {
    /// Take no action; allow dangling references
    NoAction,
    /// Prevent deletion/update of referenced rows
    Restrict,
    /// Propagate deletion/update to referencing rows
    Cascade,
    /// Set referencing columns to null on deletion/update
    SetNull,
    /// Set referencing columns to their default values on deletion/update
    SetDefault,
}

/// Unique constraint definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniqueConstraint {
    /// Constraint name
    pub name: String,
    /// Columns that must be jointly unique
    pub columns: Vec<String>,
}

/// Check constraint definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckConstraint {
    /// Constraint name
    pub name: String,
    /// SQL expression that must evaluate to true
    pub expression: String,
}

/// Foreign key constraint definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForeignKeyConstraint {
    /// Constraint name
    pub name: String,
    /// Table containing the foreign key columns
    pub from_table: String,
    /// Foreign key columns in the source table
    pub from_columns: Vec<String>,
    /// Referenced (target) table name
    pub to_table: String,
    /// Referenced columns in the target table
    pub to_columns: Vec<String>,
    /// Action when a referenced row is deleted
    pub on_delete: ReferentialAction,
    /// Action when a referenced row is updated
    pub on_update: ReferentialAction,
}

/// Index definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexDefinition {
    /// Index name
    pub name: String,
    /// Table the index belongs to
    pub table: String,
    /// Indexed columns
    pub columns: Vec<String>,
    /// Type of index structure
    pub index_type: IndexType,
    /// Whether the index enforces uniqueness
    pub unique: bool,
    /// Optional WHERE clause for a partial index
    pub where_clause: Option<String>,
}

/// Index types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IndexType {
    /// Balanced tree index for range queries
    BTree,
    /// Hash index for equality lookups
    Hash,
    /// Generalized inverted index for JSON and arrays
    Gin,
    /// Generalized search tree for geometric data
    Gist,
    /// Space-partitioned GiST index
    SpGist,
    /// Block-range index for large sequential tables
    Brin,
    /// Vector similarity index
    Vector,
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

    /// Convert a SQL data type to its DDL string representation.
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
    pub fn add_column(&mut self, column: RelationalColumnDefinition) -> &mut Self {
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
    /// Referenced table does not exist in the schema
    TableNotFound(String),
    /// Referenced column does not exist in its table
    ColumnNotFound(String, String),
    /// A table with the same name already exists
    DuplicateTable(String),
    /// A column with the same name already exists in the table
    DuplicateColumn(String, String),
    /// A constraint definition is invalid
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
