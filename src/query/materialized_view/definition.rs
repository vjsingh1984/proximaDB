//! # Materialized View Definition
//!
//! Core types for materialized view definitions and metadata.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use arrow_schema::{DataType as ArrowDataType, Field};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::refresh::RefreshStrategy;
use crate::catalog::internal::{
    CatalogObject, ModelProperties, ObjectSchema, ObjectType, SchemaEnforcementMode,
};
use crate::catalog::types::{CatalogColumn, CatalogDataType};
use crate::query::federated::{ExecutionResult, FederatedQuery, QueryPlan};

/// Unique identifier for a materialized view
pub type MaterializedViewId = String;

/// Error types for materialized view operations
#[derive(Debug, Error)]
pub enum MaterializedViewError {
    /// Materialized view not found
    #[error("Materialized view not found: {0}")]
    NotFound(MaterializedViewId),

    /// Materialized view already exists
    #[error("Materialized view already exists: {0}")]
    AlreadyExists(MaterializedViewId),

    /// Failed to parse the query
    #[error("Failed to parse query: {0}")]
    ParseError(String),

    /// Failed to execute the query
    #[error("Failed to execute query: {0}")]
    ExecutionError(String),

    /// Failed to refresh the view
    #[error("Failed to refresh view: {0}")]
    RefreshError(String),

    /// Invalid refresh strategy
    #[error("Invalid refresh strategy: {0}")]
    InvalidRefreshStrategy(String),

    /// Schema inference failed
    #[error("Failed to infer schema: {0}")]
    SchemaInferenceError(String),

    /// Catalog error
    #[error("Catalog error: {0}")]
    CatalogError(String),

    /// Invalid view name
    #[error("Invalid view name: {0}")]
    InvalidName(String),

    /// View is currently being refreshed
    #[error("View is currently being refreshed: {0}")]
    RefreshInProgress(MaterializedViewId),

    /// Internal error
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Result type for materialized view operations
pub type MaterializedViewResult<T> = Result<T, MaterializedViewError>;

/// Configuration for materialized view manager
#[derive(Debug, Clone)]
pub struct MaterializedViewConfig {
    /// Maximum number of materialized views (default: 1000)
    pub max_views: usize,
    /// Maximum data size per view in bytes (default: 1GB)
    pub max_view_size_bytes: usize,
    /// Enable automatic schema inference (default: true)
    pub enable_schema_inference: bool,
    /// Enable refresh scheduling (default: true)
    pub enable_scheduling: bool,
    /// Default refresh timeout (default: 5 minutes)
    pub default_refresh_timeout: Duration,
}

impl Default for MaterializedViewConfig {
    fn default() -> Self {
        Self {
            max_views: 1000,
            max_view_size_bytes: 1024 * 1024 * 1024, // 1GB
            enable_schema_inference: true,
            enable_scheduling: true,
            default_refresh_timeout: Duration::from_secs(300),
        }
    }
}

/// Column definition for materialized view schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnDef {
    /// Column name
    pub name: String,
    /// Column data type (SQL type string)
    pub data_type: String,
    /// Is nullable
    pub nullable: bool,
    /// Column comment
    pub comment: Option<String>,
}

impl ColumnDef {
    /// Create a new column definition
    pub fn new(name: impl Into<String>, data_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            data_type: data_type.into(),
            nullable: true,
            comment: None,
        }
    }

    /// Set nullable
    pub fn nullable(mut self, nullable: bool) -> Self {
        self.nullable = nullable;
        self
    }

    /// Set comment
    pub fn with_comment(mut self, comment: impl Into<String>) -> Self {
        self.comment = Some(comment.into());
        self
    }

    /// Convert to catalog column
    pub fn to_catalog_column(&self, id: i32) -> CatalogColumn {
        let data_type = match self.data_type.to_lowercase().as_str() {
            "boolean" | "bool" => CatalogDataType::Boolean,
            "int8" | "tinyint" => CatalogDataType::Int8,
            "int16" | "smallint" => CatalogDataType::Int16,
            "int32" | "int" | "integer" => CatalogDataType::Int32,
            "int64" | "bigint" => CatalogDataType::Int64,
            "float32" | "float" | "real" => CatalogDataType::Float32,
            "float64" | "double" => CatalogDataType::Float64,
            "string" | "text" | "varchar" => CatalogDataType::String,
            "binary" | "blob" => CatalogDataType::Binary,
            "date" => CatalogDataType::Date,
            "time" => CatalogDataType::Time,
            "timestamp" => CatalogDataType::Timestamp,
            "timestamptz" | "timestamp with time zone" => CatalogDataType::TimestampTz,
            "decimal" | "numeric" => CatalogDataType::Decimal,
            "uuid" => CatalogDataType::Uuid,
            "json" | "jsonb" => CatalogDataType::Json,
            "vector" => CatalogDataType::Vector,
            _ => CatalogDataType::String,
        };

        let mut col = CatalogColumn::new(id, &self.name, data_type).nullable(self.nullable);
        if let Some(ref comment) = self.comment {
            col = col.with_comment(comment);
        }
        col
    }

    /// Create from Arrow field
    pub fn from_arrow_field(field: &Field) -> Self {
        let data_type = match field.data_type() {
            ArrowDataType::Boolean => "boolean",
            ArrowDataType::Int8 => "int8",
            ArrowDataType::Int16 => "int16",
            ArrowDataType::Int32 => "int32",
            ArrowDataType::Int64 => "int64",
            ArrowDataType::Float32 => "float32",
            ArrowDataType::Float64 => "float64",
            ArrowDataType::Utf8 | ArrowDataType::LargeUtf8 => "string",
            ArrowDataType::Binary | ArrowDataType::LargeBinary => "binary",
            ArrowDataType::Date32 | ArrowDataType::Date64 => "date",
            ArrowDataType::Time32(_) | ArrowDataType::Time64(_) => "time",
            ArrowDataType::Timestamp(_, None) => "timestamp",
            ArrowDataType::Timestamp(_, Some(_)) => "timestamptz",
            ArrowDataType::Decimal128(_, _) | ArrowDataType::Decimal256(_, _) => "decimal",
            ArrowDataType::FixedSizeList(_, _) => "vector",
            _ => "string",
        };

        Self {
            name: field.name().clone(),
            data_type: data_type.to_string(),
            nullable: field.is_nullable(),
            comment: None,
        }
    }
}

/// State of a materialized view
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaterializedViewState {
    /// View is created but has no data yet
    Created,
    /// View is currently being refreshed
    Refreshing,
    /// View has data and is ready for queries
    Ready,
    /// View refresh failed
    Failed,
    /// View is being dropped
    Dropping,
}

impl std::fmt::Display for MaterializedViewState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MaterializedViewState::Created => write!(f, "CREATED"),
            MaterializedViewState::Refreshing => write!(f, "REFRESHING"),
            MaterializedViewState::Ready => write!(f, "READY"),
            MaterializedViewState::Failed => write!(f, "FAILED"),
            MaterializedViewState::Dropping => write!(f, "DROPPING"),
        }
    }
}

/// Materialized view definition (input for creating a view)
#[derive(Debug, Clone)]
pub struct MaterializedViewDefinition {
    /// View name
    pub name: String,
    /// SQL query that defines the view
    pub query: String,
    /// Refresh strategy
    pub refresh_strategy: RefreshStrategy,
    /// Explicit schema (if not inferred)
    pub schema: Option<Vec<ColumnDef>>,
    /// View properties
    pub properties: HashMap<String, String>,
    /// Comment/description
    pub comment: Option<String>,
}

impl MaterializedViewDefinition {
    /// Create a new materialized view definition
    pub fn new(name: impl Into<String>, query: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            query: query.into(),
            refresh_strategy: RefreshStrategy::Manual,
            schema: None,
            properties: HashMap::new(),
            comment: None,
        }
    }

    /// Set the refresh strategy
    pub fn with_refresh_strategy(mut self, strategy: RefreshStrategy) -> Self {
        self.refresh_strategy = strategy;
        self
    }

    /// Set an explicit schema
    pub fn with_schema(mut self, schema: Vec<ColumnDef>) -> Self {
        self.schema = Some(schema);
        self
    }

    /// Add a property
    pub fn with_property(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.properties.insert(key.into(), value.into());
        self
    }

    /// Set comment
    pub fn with_comment(mut self, comment: impl Into<String>) -> Self {
        self.comment = Some(comment.into());
        self
    }

    /// Validate the definition
    pub fn validate(&self) -> MaterializedViewResult<()> {
        // Validate name
        if self.name.is_empty() {
            return Err(MaterializedViewError::InvalidName(
                "View name cannot be empty".to_string(),
            ));
        }

        if !self.name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(MaterializedViewError::InvalidName(
                "View name must be alphanumeric with underscores".to_string(),
            ));
        }

        // Validate query
        if self.query.trim().is_empty() {
            return Err(MaterializedViewError::ParseError(
                "Query cannot be empty".to_string(),
            ));
        }

        // Validate refresh strategy
        self.refresh_strategy.validate()?;

        Ok(())
    }
}

/// Materialized view with metadata and cached data
pub struct MaterializedView {
    /// View name (unique identifier)
    pub name: String,
    /// SQL query that defines the view
    pub query: String,
    /// Refresh strategy
    pub refresh_strategy: RefreshStrategy,
    /// Last successful refresh timestamp
    pub last_refresh: Option<DateTime<Utc>>,
    /// Schema of the view (column definitions)
    pub schema: Vec<ColumnDef>,
    /// Current state
    pub state: MaterializedViewState,
    /// Cached execution result
    pub cached_result: Option<Arc<ExecutionResult>>,
    /// Parsed query (cached)
    pub parsed_query: Option<Arc<FederatedQuery>>,
    /// Optimized query plan (cached)
    pub query_plan: Option<Arc<QueryPlan>>,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
    /// Last updated timestamp
    pub updated_at: DateTime<Utc>,
    /// View properties
    pub properties: HashMap<String, String>,
    /// Comment/description
    pub comment: Option<String>,
    /// Statistics
    pub stats: MaterializedViewStats,
    /// Dependencies (collections/tables this view depends on)
    pub dependencies: Vec<String>,
    /// Last error message (if state is Failed)
    pub last_error: Option<String>,
}

impl MaterializedView {
    /// Create a new materialized view from a definition
    pub fn from_definition(definition: MaterializedViewDefinition) -> Self {
        let now = Utc::now();
        Self {
            name: definition.name,
            query: definition.query,
            refresh_strategy: definition.refresh_strategy,
            last_refresh: None,
            schema: definition.schema.unwrap_or_default(),
            state: MaterializedViewState::Created,
            cached_result: None,
            parsed_query: None,
            query_plan: None,
            created_at: now,
            updated_at: now,
            properties: definition.properties,
            comment: definition.comment,
            stats: MaterializedViewStats::default(),
            dependencies: Vec::new(),
            last_error: None,
        }
    }

    /// Check if the view needs refresh based on the strategy
    pub fn needs_refresh(&self) -> bool {
        match self.state {
            MaterializedViewState::Created => true,
            MaterializedViewState::Failed => true,
            MaterializedViewState::Refreshing => false,
            MaterializedViewState::Dropping => false,
            MaterializedViewState::Ready => {
                if let Some(last_refresh) = self.last_refresh {
                    match &self.refresh_strategy {
                        RefreshStrategy::Manual => false,
                        RefreshStrategy::Periodic { interval } => {
                            let elapsed = Utc::now()
                                .signed_duration_since(last_refresh)
                                .to_std()
                                .unwrap_or(Duration::MAX);
                            elapsed >= *interval
                        }
                        RefreshStrategy::OnChange { .. } => false, // Triggered externally
                    }
                } else {
                    true
                }
            }
        }
    }

    /// Update state and record timestamp
    pub fn set_state(&mut self, state: MaterializedViewState) {
        self.state = state;
        self.updated_at = Utc::now();
    }

    /// Record a successful refresh
    pub fn record_refresh_success(&mut self, result: ExecutionResult, duration: Duration) {
        let now = Utc::now();
        self.last_refresh = Some(now);
        self.updated_at = now;
        self.state = MaterializedViewState::Ready;
        self.cached_result = Some(Arc::new(result));
        self.last_error = None;

        // Update stats
        self.stats.refresh_count.fetch_add(1, Ordering::Relaxed);
        self.stats
            .total_refresh_time_ms
            .fetch_add(duration.as_millis() as u64, Ordering::Relaxed);
        self.stats
            .last_refresh_time_ms
            .store(duration.as_millis() as u64, Ordering::Relaxed);
    }

    /// Record a failed refresh
    pub fn record_refresh_failure(&mut self, error: String) {
        self.updated_at = Utc::now();
        self.state = MaterializedViewState::Failed;
        self.last_error = Some(error);
        self.stats.refresh_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Get the row count of cached data
    pub fn row_count(&self) -> usize {
        self.cached_result
            .as_ref()
            .map_or(0, |r| r.row_count())
    }

    /// Convert to catalog object for persistence
    pub fn to_catalog_object(&self, catalog: &str, namespace: Vec<String>) -> CatalogObject {
        let columns: Vec<CatalogColumn> = self
            .schema
            .iter()
            .enumerate()
            .map(|(i, col)| col.to_catalog_column(i as i32 + 1))
            .collect();

        let object_schema = ObjectSchema {
            columns,
            primary_key: Vec::new(),
            constraints: Vec::new(),
            indexes: Vec::new(),
            model_properties: ModelProperties::None,
        };

        let mut obj =
            CatalogObject::new(catalog, namespace, &self.name, ObjectType::MaterializedView)
                .with_schema(object_schema, SchemaEnforcementMode::Strict);

        // Store MV-specific properties
        obj.properties
            .insert("query".to_string(), self.query.clone());
        obj.properties.insert(
            "refresh_strategy".to_string(),
            self.refresh_strategy.to_string(),
        );
        obj.properties
            .insert("state".to_string(), self.state.to_string());

        if let Some(ref last_refresh) = self.last_refresh {
            obj.properties
                .insert("last_refresh".to_string(), last_refresh.to_rfc3339());
        }

        if let Some(ref comment) = self.comment {
            obj = obj.with_comment(comment);
        }

        obj
    }
}

/// Statistics for a materialized view
#[derive(Debug, Default)]
pub struct MaterializedViewStats {
    /// Number of successful refreshes
    pub refresh_count: AtomicU64,
    /// Number of failed refreshes
    pub refresh_failures: AtomicU64,
    /// Number of queries served from cache
    pub query_count: AtomicU64,
    /// Total refresh time in milliseconds
    pub total_refresh_time_ms: AtomicU64,
    /// Last refresh time in milliseconds
    pub last_refresh_time_ms: AtomicU64,
}

impl MaterializedViewStats {
    /// Get average refresh time in milliseconds
    pub fn avg_refresh_time_ms(&self) -> f64 {
        let count = self.refresh_count.load(Ordering::Relaxed);
        if count == 0 {
            0.0
        } else {
            self.total_refresh_time_ms.load(Ordering::Relaxed) as f64 / count as f64
        }
    }

    /// Get refresh success rate
    pub fn success_rate(&self) -> f64 {
        let successes = self.refresh_count.load(Ordering::Relaxed);
        let failures = self.refresh_failures.load(Ordering::Relaxed);
        let total = successes + failures;
        if total == 0 {
            1.0
        } else {
            successes as f64 / total as f64
        }
    }
}

// ============================================================================
// SQL Parsing for Materialized View Statements
// ============================================================================

/// Parsed materialized view SQL statement
#[derive(Debug, Clone)]
pub enum MaterializedViewStatement {
    /// CREATE MATERIALIZED VIEW ... AS SELECT ... WITH REFRESH ...
    Create(MaterializedViewDefinition),
    /// REFRESH MATERIALIZED VIEW <name>
    Refresh { name: String },
    /// DROP MATERIALIZED VIEW <name>
    Drop { name: String, if_exists: bool },
}

/// Parser for materialized view SQL statements
pub struct MaterializedViewParser;

impl MaterializedViewParser {
    /// Parse a SQL statement that might be a materialized view command
    ///
    /// # Supported Syntax
    ///
    /// ```sql
    /// CREATE MATERIALIZED VIEW <name> AS
    ///   <select_query>
    /// [WITH REFRESH MANUAL]
    /// [WITH REFRESH PERIODIC INTERVAL '<duration>']
    /// [WITH REFRESH ON CHANGE DEBOUNCE '<duration>']
    ///
    /// REFRESH MATERIALIZED VIEW <name>
    ///
    /// DROP MATERIALIZED VIEW [IF EXISTS] <name>
    /// ```
    pub fn parse(sql: &str) -> Option<MaterializedViewStatement> {
        let sql = sql.trim();
        let upper = sql.to_uppercase();

        // Check for CREATE MATERIALIZED VIEW
        if upper.starts_with("CREATE MATERIALIZED VIEW") {
            return Self::parse_create(sql);
        }

        // Check for REFRESH MATERIALIZED VIEW
        if upper.starts_with("REFRESH MATERIALIZED VIEW") {
            return Self::parse_refresh(sql);
        }

        // Check for DROP MATERIALIZED VIEW
        if upper.starts_with("DROP MATERIALIZED VIEW") {
            return Self::parse_drop(sql);
        }

        None
    }

    /// Parse CREATE MATERIALIZED VIEW statement
    fn parse_create(sql: &str) -> Option<MaterializedViewStatement> {
        let upper = sql.to_uppercase();

        // Find the position after "CREATE MATERIALIZED VIEW"
        let prefix = "CREATE MATERIALIZED VIEW";
        if !upper.starts_with(prefix) {
            return None;
        }

        let after_prefix = sql[prefix.len()..].trim();

        // Find the view name (next word)
        let name_end = after_prefix
            .find(|c: char| c.is_whitespace())
            .unwrap_or(after_prefix.len());
        let name = after_prefix[..name_end].trim().to_string();

        if name.is_empty() {
            return None;
        }

        // Find AS keyword
        let after_name = after_prefix[name_end..].trim();
        let upper_after_name = after_name.to_uppercase();
        if !upper_after_name.starts_with("AS") {
            return None;
        }

        let after_as = after_name[2..].trim();

        // Find the query and refresh clause
        let (query, refresh_strategy) = Self::parse_query_and_refresh(after_as);

        let definition =
            MaterializedViewDefinition::new(name, query).with_refresh_strategy(refresh_strategy);

        Some(MaterializedViewStatement::Create(definition))
    }

    /// Parse the query and refresh strategy from CREATE statement
    fn parse_query_and_refresh(sql: &str) -> (String, RefreshStrategy) {
        let upper = sql.to_uppercase();

        // Look for WITH REFRESH clause
        if let Some(with_pos) = upper.find("WITH REFRESH") {
            let query = sql[..with_pos].trim().to_string();
            let refresh_clause = sql[with_pos..].trim();
            let strategy = Self::parse_refresh_strategy(refresh_clause);
            (query, strategy)
        } else {
            // No refresh clause - default to manual
            let query = sql.trim_end_matches(';').trim().to_string();
            (query, RefreshStrategy::Manual)
        }
    }

    /// Parse refresh strategy from WITH REFRESH clause
    fn parse_refresh_strategy(clause: &str) -> RefreshStrategy {
        let upper = clause.to_uppercase();

        // WITH REFRESH MANUAL
        if upper.contains("REFRESH MANUAL") {
            return RefreshStrategy::Manual;
        }

        // WITH REFRESH PERIODIC INTERVAL '<duration>'
        if upper.contains("REFRESH PERIODIC")
            && let Some(interval) = Self::extract_interval(&upper, clause) {
                return RefreshStrategy::Periodic { interval };
            }

        // WITH REFRESH ON CHANGE DEBOUNCE '<duration>'
        if upper.contains("REFRESH ON CHANGE")
            && let Some(debounce) = Self::extract_debounce(&upper, clause) {
                return RefreshStrategy::OnChange { debounce };
            }

        // Default to manual if we can't parse
        RefreshStrategy::Manual
    }

    /// Extract interval duration from PERIODIC clause
    fn extract_interval(upper: &str, original: &str) -> Option<Duration> {
        // Look for INTERVAL 'duration'
        if let Some(interval_pos) = upper.find("INTERVAL") {
            let after_interval = &original[interval_pos + 8..];
            Self::parse_duration_literal(after_interval)
        } else {
            None
        }
    }

    /// Extract debounce duration from ON CHANGE clause
    fn extract_debounce(upper: &str, original: &str) -> Option<Duration> {
        // Look for DEBOUNCE 'duration'
        if let Some(debounce_pos) = upper.find("DEBOUNCE") {
            let after_debounce = &original[debounce_pos + 8..];
            Self::parse_duration_literal(after_debounce)
        } else {
            None
        }
    }

    /// Parse a duration literal like '1 hour', '30 seconds', '5 minutes'
    fn parse_duration_literal(s: &str) -> Option<Duration> {
        let s = s.trim();

        // Find the quoted string
        let quote_start = s.find('\'')?;
        let after_start = &s[quote_start + 1..];
        let quote_end = after_start.find('\'')?;
        let duration_str = &after_start[..quote_end].trim();

        // Parse humantime-style duration
        Self::parse_humantime_duration(duration_str)
    }

    /// Parse a humantime-style duration string
    fn parse_humantime_duration(s: &str) -> Option<Duration> {
        let s = s.trim().to_lowercase();

        // Try to parse with humantime-like patterns
        // Supports: "1 hour", "30 seconds", "5 minutes", "1h", "30s", "5m", etc.

        // First try simple patterns with units
        let patterns = [
            ("seconds", 1u64),
            ("second", 1),
            ("secs", 1),
            ("sec", 1),
            ("s", 1),
            ("minutes", 60),
            ("minute", 60),
            ("mins", 60),
            ("min", 60),
            ("m", 60),
            ("hours", 3600),
            ("hour", 3600),
            ("hrs", 3600),
            ("hr", 3600),
            ("h", 3600),
            ("days", 86400),
            ("day", 86400),
            ("d", 86400),
        ];

        for (unit, multiplier) in patterns {
            if s.ends_with(unit) {
                let num_str = s[..s.len() - unit.len()].trim();
                if let Ok(num) = num_str.parse::<u64>() {
                    return Some(Duration::from_secs(num * multiplier));
                }
            }
        }

        // Try pattern with space: "1 hour"
        let parts: Vec<&str> = s.split_whitespace().collect();
        if parts.len() == 2
            && let Ok(num) = parts[0].parse::<u64>() {
                for (unit, multiplier) in patterns {
                    if parts[1] == unit {
                        return Some(Duration::from_secs(num * multiplier));
                    }
                }
            }

        None
    }

    /// Parse REFRESH MATERIALIZED VIEW statement
    fn parse_refresh(sql: &str) -> Option<MaterializedViewStatement> {
        let upper = sql.to_uppercase();
        let prefix = "REFRESH MATERIALIZED VIEW";

        if !upper.starts_with(prefix) {
            return None;
        }

        let after_prefix = sql[prefix.len()..].trim();
        let name = after_prefix
            .split_whitespace()
            .next()?
            .trim_end_matches(';')
            .to_string();

        if name.is_empty() {
            return None;
        }

        Some(MaterializedViewStatement::Refresh { name })
    }

    /// Parse DROP MATERIALIZED VIEW statement
    fn parse_drop(sql: &str) -> Option<MaterializedViewStatement> {
        let upper = sql.to_uppercase();
        let prefix = "DROP MATERIALIZED VIEW";

        if !upper.starts_with(prefix) {
            return None;
        }

        let after_prefix = sql[prefix.len()..].trim();
        let upper_after = after_prefix.to_uppercase();

        let (if_exists, name_part) = if upper_after.starts_with("IF EXISTS") {
            (true, after_prefix[9..].trim())
        } else {
            (false, after_prefix)
        };

        let name = name_part
            .split_whitespace()
            .next()?
            .trim_end_matches(';')
            .to_string();

        if name.is_empty() {
            return None;
        }

        Some(MaterializedViewStatement::Drop { name, if_exists })
    }

    /// Check if a SQL statement is a materialized view command
    pub fn is_materialized_view_statement(sql: &str) -> bool {
        let upper = sql.trim().to_uppercase();
        upper.starts_with("CREATE MATERIALIZED VIEW")
            || upper.starts_with("REFRESH MATERIALIZED VIEW")
            || upper.starts_with("DROP MATERIALIZED VIEW")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_column_def_creation() {
        let col = ColumnDef::new("id", "int64").nullable(false);
        assert_eq!(col.name, "id");
        assert_eq!(col.data_type, "int64");
        assert!(!col.nullable);
    }

    #[test]
    fn test_column_def_to_catalog() {
        let col = ColumnDef::new("name", "string")
            .nullable(true)
            .with_comment("User name");

        let catalog_col = col.to_catalog_column(1);
        assert_eq!(catalog_col.name, "name");
        assert_eq!(catalog_col.data_type, CatalogDataType::String);
        assert!(catalog_col.nullable);
    }

    #[test]
    fn test_mv_definition_validation() {
        let def = MaterializedViewDefinition::new("test_view", "SELECT * FROM users");
        assert!(def.validate().is_ok());

        let empty_name = MaterializedViewDefinition::new("", "SELECT * FROM users");
        assert!(matches!(
            empty_name.validate(),
            Err(MaterializedViewError::InvalidName(_))
        ));

        let empty_query = MaterializedViewDefinition::new("test", "");
        assert!(matches!(
            empty_query.validate(),
            Err(MaterializedViewError::ParseError(_))
        ));
    }

    #[test]
    fn test_mv_definition_with_refresh_strategy() {
        let def = MaterializedViewDefinition::new("test_view", "SELECT 1").with_refresh_strategy(
            RefreshStrategy::Periodic {
                interval: Duration::from_secs(3600),
            },
        );

        assert!(matches!(
            def.refresh_strategy,
            RefreshStrategy::Periodic { .. }
        ));
    }

    #[test]
    fn test_mv_state_display() {
        assert_eq!(MaterializedViewState::Created.to_string(), "CREATED");
        assert_eq!(MaterializedViewState::Refreshing.to_string(), "REFRESHING");
        assert_eq!(MaterializedViewState::Ready.to_string(), "READY");
        assert_eq!(MaterializedViewState::Failed.to_string(), "FAILED");
    }

    #[test]
    fn test_mv_from_definition() {
        let def = MaterializedViewDefinition::new("test_view", "SELECT * FROM users")
            .with_comment("Test view")
            .with_property("priority", "high");

        let mv = MaterializedView::from_definition(def);

        assert_eq!(mv.name, "test_view");
        assert_eq!(mv.query, "SELECT * FROM users");
        assert_eq!(mv.state, MaterializedViewState::Created);
        assert_eq!(mv.comment, Some("Test view".to_string()));
        assert_eq!(mv.properties.get("priority"), Some(&"high".to_string()));
    }

    #[test]
    fn test_mv_needs_refresh() {
        let def = MaterializedViewDefinition::new("test", "SELECT 1");
        let mut mv = MaterializedView::from_definition(def);

        // Created state always needs refresh
        assert!(mv.needs_refresh());

        // Ready with manual refresh doesn't need refresh
        mv.state = MaterializedViewState::Ready;
        mv.last_refresh = Some(Utc::now());
        assert!(!mv.needs_refresh());

        // Refreshing state doesn't need refresh
        mv.state = MaterializedViewState::Refreshing;
        assert!(!mv.needs_refresh());

        // Failed state needs refresh
        mv.state = MaterializedViewState::Failed;
        assert!(mv.needs_refresh());
    }

    #[test]
    fn test_mv_stats() {
        let stats = MaterializedViewStats::default();

        assert_eq!(stats.avg_refresh_time_ms(), 0.0);
        assert_eq!(stats.success_rate(), 1.0);

        stats.refresh_count.store(10, Ordering::Relaxed);
        stats.total_refresh_time_ms.store(1000, Ordering::Relaxed);

        assert_eq!(stats.avg_refresh_time_ms(), 100.0);
    }

    #[test]
    fn test_mv_to_catalog_object() {
        let def = MaterializedViewDefinition::new("test_mv", "SELECT * FROM t")
            .with_schema(vec![
                ColumnDef::new("id", "int64").nullable(false),
                ColumnDef::new("name", "string"),
            ])
            .with_comment("Test MV");

        let mv = MaterializedView::from_definition(def);
        let catalog_obj = mv.to_catalog_object("default", vec!["public".to_string()]);

        assert_eq!(catalog_obj.name, "test_mv");
        assert_eq!(catalog_obj.object_type, ObjectType::MaterializedView);
        assert_eq!(catalog_obj.schema.columns.len(), 2);
        assert!(catalog_obj.properties.contains_key("query"));
        assert!(catalog_obj.properties.contains_key("refresh_strategy"));
    }

    // ============================================================================
    // SQL Parser Tests
    // ============================================================================

    #[test]
    fn test_parse_create_simple() {
        let sql = "CREATE MATERIALIZED VIEW test_view AS SELECT * FROM users";
        let stmt = MaterializedViewParser::parse(sql);

        assert!(stmt.is_some());
        if let Some(MaterializedViewStatement::Create(def)) = stmt {
            assert_eq!(def.name, "test_view");
            assert_eq!(def.query, "SELECT * FROM users");
            assert!(matches!(def.refresh_strategy, RefreshStrategy::Manual));
        } else {
            panic!("Expected Create statement");
        }
    }

    #[test]
    fn test_parse_create_with_periodic_refresh() {
        let sql = "CREATE MATERIALIZED VIEW hourly_stats AS SELECT * FROM logs WITH REFRESH PERIODIC INTERVAL '1 hour'";
        let stmt = MaterializedViewParser::parse(sql);

        assert!(stmt.is_some());
        if let Some(MaterializedViewStatement::Create(def)) = stmt {
            assert_eq!(def.name, "hourly_stats");
            assert!(def.query.contains("SELECT * FROM logs"));
            if let RefreshStrategy::Periodic { interval } = def.refresh_strategy {
                assert_eq!(interval.as_secs(), 3600);
            } else {
                panic!("Expected Periodic refresh strategy");
            }
        } else {
            panic!("Expected Create statement");
        }
    }

    #[test]
    fn test_parse_create_with_on_change_refresh() {
        let sql = "CREATE MATERIALIZED VIEW live_data AS SELECT * FROM events WITH REFRESH ON CHANGE DEBOUNCE '5 seconds'";
        let stmt = MaterializedViewParser::parse(sql);

        assert!(stmt.is_some());
        if let Some(MaterializedViewStatement::Create(def)) = stmt {
            assert_eq!(def.name, "live_data");
            if let RefreshStrategy::OnChange { debounce } = def.refresh_strategy {
                assert_eq!(debounce.as_secs(), 5);
            } else {
                panic!("Expected OnChange refresh strategy");
            }
        } else {
            panic!("Expected Create statement");
        }
    }

    #[test]
    fn test_parse_create_with_manual_refresh() {
        let sql = "CREATE MATERIALIZED VIEW manual_view AS SELECT * FROM data WITH REFRESH MANUAL";
        let stmt = MaterializedViewParser::parse(sql);

        assert!(stmt.is_some());
        if let Some(MaterializedViewStatement::Create(def)) = stmt {
            assert_eq!(def.name, "manual_view");
            assert!(matches!(def.refresh_strategy, RefreshStrategy::Manual));
        } else {
            panic!("Expected Create statement");
        }
    }

    #[test]
    fn test_parse_create_complex_query() {
        let sql = r#"
            CREATE MATERIALIZED VIEW user_product_matches AS
            SELECT u.id, v.product_id, v.score
            FROM users u
            JOIN LATERAL VECTOR_SEARCH('products', u.preference_vector, 100) v ON true
            WITH REFRESH PERIODIC INTERVAL '30 minutes'
        "#;
        let stmt = MaterializedViewParser::parse(sql);

        assert!(stmt.is_some());
        if let Some(MaterializedViewStatement::Create(def)) = stmt {
            assert_eq!(def.name, "user_product_matches");
            assert!(def.query.contains("VECTOR_SEARCH"));
            if let RefreshStrategy::Periodic { interval } = def.refresh_strategy {
                assert_eq!(interval.as_secs(), 1800);
            } else {
                panic!("Expected Periodic refresh strategy");
            }
        } else {
            panic!("Expected Create statement");
        }
    }

    #[test]
    fn test_parse_refresh() {
        let sql = "REFRESH MATERIALIZED VIEW test_view";
        let stmt = MaterializedViewParser::parse(sql);

        assert!(stmt.is_some());
        if let Some(MaterializedViewStatement::Refresh { name }) = stmt {
            assert_eq!(name, "test_view");
        } else {
            panic!("Expected Refresh statement");
        }
    }

    #[test]
    fn test_parse_refresh_with_semicolon() {
        let sql = "REFRESH MATERIALIZED VIEW my_view;";
        let stmt = MaterializedViewParser::parse(sql);

        assert!(stmt.is_some());
        if let Some(MaterializedViewStatement::Refresh { name }) = stmt {
            assert_eq!(name, "my_view");
        } else {
            panic!("Expected Refresh statement");
        }
    }

    #[test]
    fn test_parse_drop() {
        let sql = "DROP MATERIALIZED VIEW test_view";
        let stmt = MaterializedViewParser::parse(sql);

        assert!(stmt.is_some());
        if let Some(MaterializedViewStatement::Drop { name, if_exists }) = stmt {
            assert_eq!(name, "test_view");
            assert!(!if_exists);
        } else {
            panic!("Expected Drop statement");
        }
    }

    #[test]
    fn test_parse_drop_if_exists() {
        let sql = "DROP MATERIALIZED VIEW IF EXISTS optional_view";
        let stmt = MaterializedViewParser::parse(sql);

        assert!(stmt.is_some());
        if let Some(MaterializedViewStatement::Drop { name, if_exists }) = stmt {
            assert_eq!(name, "optional_view");
            assert!(if_exists);
        } else {
            panic!("Expected Drop statement");
        }
    }

    #[test]
    fn test_parse_non_mv_statement() {
        let sql = "SELECT * FROM users";
        let stmt = MaterializedViewParser::parse(sql);
        assert!(stmt.is_none());
    }

    #[test]
    fn test_is_materialized_view_statement() {
        assert!(MaterializedViewParser::is_materialized_view_statement(
            "CREATE MATERIALIZED VIEW x AS SELECT 1"
        ));
        assert!(MaterializedViewParser::is_materialized_view_statement(
            "REFRESH MATERIALIZED VIEW x"
        ));
        assert!(MaterializedViewParser::is_materialized_view_statement(
            "DROP MATERIALIZED VIEW x"
        ));
        assert!(!MaterializedViewParser::is_materialized_view_statement(
            "SELECT * FROM users"
        ));
        assert!(!MaterializedViewParser::is_materialized_view_statement(
            "CREATE TABLE users"
        ));
    }

    #[test]
    fn test_parse_duration_variants() {
        // Test various duration formats
        let test_cases = [
            ("'1h'", 3600),
            ("'1 hour'", 3600),
            ("'2 hours'", 7200),
            ("'30m'", 1800),
            ("'30 minutes'", 1800),
            ("'60s'", 60),
            ("'60 seconds'", 60),
            ("'1 day'", 86400),
        ];

        for (input, expected_secs) in test_cases {
            let result = MaterializedViewParser::parse_duration_literal(input);
            assert!(result.is_some(), "Failed to parse duration: {}", input);
            assert_eq!(
                result.unwrap().as_secs(),
                expected_secs,
                "Wrong duration for: {}",
                input
            );
        }
    }
}
