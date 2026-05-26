use std::sync::Arc;

use anyhow::Result;
use serde::Serialize;

use crate::catalog::{
    CatalogDataType, CatalogIndexType, CatalogManager, CatalogTableSchema, TableIdentifier,
};

/// Tabular catalog metadata for SQL-compatible introspection surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogIntrospectionResult {
    pub columns: Vec<String>,
    pub column_types: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl CatalogIntrospectionResult {
    pub fn empty() -> Self {
        Self {
            columns: Vec::new(),
            column_types: Vec::new(),
            rows: Vec::new(),
        }
    }
}

/// Projects the native xCatalog into pgwire/embedded SQL metadata tables.
///
/// This is intentionally read-only and table-shaped so JDBC/SQLAlchemy/dbt-style clients can
/// introspect schemas without depending on Rust structs or SDK-only APIs.
#[derive(Clone)]
pub struct CatalogIntrospectionService {
    catalog_manager: Arc<CatalogManager>,
}

impl CatalogIntrospectionService {
    pub fn new(catalog_manager: Arc<CatalogManager>) -> Self {
        Self { catalog_manager }
    }

    pub fn is_catalog_query(sql: &str) -> bool {
        let normalized = normalize_sql(sql);
        normalized.contains(" from xcatalog.tables")
            || normalized.contains(" from xcatalog.columns")
            || normalized.contains(" from xcatalog.indexes")
            || normalized.contains(" from xcatalog.table_routing")
            || normalized.contains(" from xcatalog.namespaces")
            || normalized.contains(" from information_schema.tables")
            || normalized.contains(" from information_schema.columns")
            || normalized.contains(" from information_schema.table_routing")
            || normalized.contains(" from information_schema.schemata")
            || normalized.contains("proximadb_catalog.props_promotion_candidates")
            || (normalized.contains("pg_catalog.pg_class")
                && normalized.contains("pg_catalog.pg_namespace"))
            || (normalized.contains("pg_catalog.pg_attribute")
                && normalized.contains("pg_catalog.pg_class"))
            || normalized.contains("pg_catalog.pg_index")
            // Bare `FROM pg_class` (no qualifier) — common shape from
            // psql `\dt` and lightweight ORM connection probes.
            || normalized.contains(" from pg_class")
    }

    pub async fn execute_select(&self, sql: &str) -> Result<Option<CatalogIntrospectionResult>> {
        let normalized = normalize_sql(sql);
        let table_filter = extract_string_filter(sql, "table_name");

        // `information_schema.tables` returns the SQL-standard
        // 4-column shape `(table_catalog, table_schema, table_name,
        // table_type)`. Admin tools and ORMs probe for this exact
        // shape; returning ProximaDB-extended columns (as
        // `xcatalog.tables` does) breaks them. Keep the two views
        // distinct so each carries its own contract.
        if normalized.contains(" from information_schema.tables") {
            return Ok(Some(
                self.information_schema_tables(table_filter.as_deref())
                    .await?,
            ));
        }
        if normalized.contains(" from xcatalog.tables") {
            return Ok(Some(self.tables(table_filter.as_deref()).await?));
        }
        if normalized.contains(" from xcatalog.namespaces")
            || normalized.contains(" from information_schema.schemata")
        {
            return Ok(Some(self.namespaces().await?));
        }
        // Bare `FROM pg_class` (without joins) — return the
        // SQLAlchemy/psql-compatible shape used elsewhere for
        // pg_class + pg_namespace queries. The shape covers `relname`,
        // `relkind`, and the columns most lightweight probes ask
        // for; extending it is Phase 2's `pg_catalog` work.
        if normalized.contains(" from pg_class")
            && !normalized.contains("pg_attribute")
            && !normalized.contains("pg_namespace")
        {
            return Ok(Some(
                self.sqlalchemy_table_names(None).await?,
            ));
        }
        if normalized.contains(" from xcatalog.columns")
            || normalized.contains(" from information_schema.columns")
        {
            return Ok(Some(self.columns(table_filter.as_deref()).await?));
        }
        if normalized.contains(" from xcatalog.indexes") {
            return Ok(Some(self.indexes(table_filter.as_deref()).await?));
        }
        if normalized.contains(" from xcatalog.table_routing")
            || normalized.contains(" from information_schema.table_routing")
        {
            return Ok(Some(self.table_routing(table_filter.as_deref()).await?));
        }
        if normalized.contains("proximadb_catalog.props_promotion_candidates") {
            let table_filter = extract_string_filter(sql, "table_name")
                .or_else(|| extract_function_arg(sql, "props_promotion_candidates"));
            return Ok(Some(
                self.props_promotion_candidates(table_filter.as_deref())
                    .await?,
            ));
        }
        if normalized.contains("pg_catalog.pg_index") {
            return Ok(Some(self.jdbc_indexes(None).await?));
        }
        if normalized.contains("pg_catalog.pg_attribute")
            && normalized.contains("pg_catalog.pg_class")
            && normalized.contains("attname")
        {
            let relname_filter = extract_string_filter(sql, "c.relname")
                .or_else(|| extract_string_filter(sql, "ct.relname"))
                .or_else(|| extract_string_filter(sql, "pg_class.relname"))
                .or(table_filter);
            return Ok(Some(
                self.sqlalchemy_columns(relname_filter.as_deref()).await?,
            ));
        }
        if normalized.contains("pg_catalog.pg_attribute") && normalized.contains("column_name") {
            let relname_filter = extract_string_filter(sql, "c.relname")
                .or_else(|| extract_string_filter(sql, "ct.relname"))
                .or(table_filter);
            return Ok(Some(self.jdbc_columns(relname_filter.as_deref()).await?));
        }
        if normalized.contains("pg_catalog.pg_class")
            && normalized.contains("pg_catalog.pg_namespace")
            && normalized.contains("relname")
        {
            let relname_filter = extract_string_filter(sql, "c.relname")
                .or_else(|| extract_string_filter(sql, "pg_class.relname"))
                .or(table_filter);
            return Ok(Some(
                self.sqlalchemy_table_names(relname_filter.as_deref())
                    .await?,
            ));
        }
        if normalized.contains("pg_catalog.pg_class")
            && normalized.contains("pg_catalog.pg_namespace")
            && normalized.contains("table_name")
        {
            let relname_filter = extract_string_filter(sql, "c.relname").or(table_filter);
            return Ok(Some(self.jdbc_tables(relname_filter.as_deref()).await?));
        }

        Ok(None)
    }

    /// `information_schema.tables` with the SQL-standard 4-column
    /// shape `(table_catalog, table_schema, table_name, table_type)`.
    /// Distinct from `xcatalog.tables` which carries ProximaDB-
    /// specific extras (schema_kind, storage_engine, layout, etc.).
    async fn information_schema_tables(
        &self,
        table_filter: Option<&str>,
    ) -> Result<CatalogIntrospectionResult> {
        let mut rows = Vec::new();
        for (catalog_name, table_id, _schema) in self.cataloged_tables().await? {
            if !matches_filter(&table_id.name, table_filter) {
                continue;
            }
            rows.push(vec![
                catalog_name,
                namespace_name(&table_id),
                table_id.name,
                // Per ANSI: BASE TABLE | VIEW | LOCAL TEMPORARY |
                // GLOBAL TEMPORARY. We always return BASE TABLE
                // until views land.
                "BASE TABLE".to_string(),
            ]);
        }
        rows.sort();
        Ok(CatalogIntrospectionResult {
            columns: vec![
                "table_catalog".to_string(),
                "table_schema".to_string(),
                "table_name".to_string(),
                "table_type".to_string(),
            ],
            column_types: vec![
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
            ],
            rows,
        })
    }

    /// `xcatalog.namespaces` (and `information_schema.schemata`) —
    /// the set of registered namespaces. Carries the engine
    /// authority fields added in P0.5 (namespace_id, tenant_id,
    /// region_home, storage_pool_class) so operator tools can
    /// inspect them via SQL.
    ///
    /// Iteration pattern mirrors `cataloged_tables`: walk every
    /// catalog registered on `CatalogManager`, then every namespace
    /// each catalog returns from `list_namespaces`.
    async fn namespaces(&self) -> Result<CatalogIntrospectionResult> {
        let mut rows = Vec::new();
        for catalog_name in self.catalog_manager.list_catalogs().await {
            let catalog = match self
                .catalog_manager
                .get_catalog(&catalog_name)
                .await
            {
                Ok(c) => c,
                Err(_) => continue,
            };
            let listed = match catalog.list_namespaces(None).await {
                Ok(ns) => ns,
                Err(_) => continue,
            };
            for ns in listed {
                rows.push(vec![
                    catalog_name.clone(),
                    ns.levels.join("."),
                    ns.namespace_id.clone().unwrap_or_default(),
                    ns.tenant_id.clone().unwrap_or_default(),
                    ns.region_home.clone().unwrap_or_default(),
                    format!("{:?}", ns.storage_pool_class).to_lowercase(),
                    ns.owner.clone().unwrap_or_default(),
                    ns.location.clone().unwrap_or_default(),
                ]);
            }
        }
        rows.sort();
        Ok(CatalogIntrospectionResult {
            columns: vec![
                "catalog_name".to_string(),
                "namespace_name".to_string(),
                "namespace_id".to_string(),
                "tenant_id".to_string(),
                "region_home".to_string(),
                "storage_pool_class".to_string(),
                "owner".to_string(),
                "location".to_string(),
            ],
            column_types: vec![
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
            ],
            rows,
        })
    }

    async fn tables(&self, table_filter: Option<&str>) -> Result<CatalogIntrospectionResult> {
        let mut rows = Vec::new();

        for (catalog_name, table_id, schema) in self.cataloged_tables().await? {
            if !matches_filter(&table_id.name, table_filter) {
                continue;
            }
            rows.push(vec![
                catalog_name,
                namespace_name(&table_id),
                table_id.name,
                schema
                    .properties
                    .get("schema_kind")
                    .cloned()
                    .unwrap_or_else(|| infer_schema_kind(&schema).to_string()),
                schema
                    .properties
                    .get("storage_engine")
                    .cloned()
                    .unwrap_or_default(),
                schema.properties.get("layout").cloned().unwrap_or_default(),
                schema
                    .properties
                    .get("table_format")
                    .cloned()
                    .unwrap_or_default(),
                schema
                    .properties
                    .get("xcatalog_namespace")
                    .cloned()
                    .unwrap_or_default(),
                schema.schema_version.to_string(),
            ]);
        }

        rows.sort();
        Ok(CatalogIntrospectionResult {
            columns: vec![
                "table_catalog".to_string(),
                "table_schema".to_string(),
                "table_name".to_string(),
                "schema_kind".to_string(),
                "storage_engine".to_string(),
                "layout".to_string(),
                "table_format".to_string(),
                "xcatalog_namespace".to_string(),
                "schema_version".to_string(),
            ],
            column_types: vec![
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "int4".to_string(),
            ],
            rows,
        })
    }

    async fn columns(&self, table_filter: Option<&str>) -> Result<CatalogIntrospectionResult> {
        let mut rows = Vec::new();

        for (catalog_name, table_id, schema) in self.cataloged_tables().await? {
            if !matches_filter(&table_id.name, table_filter) {
                continue;
            }
            for column in &schema.columns {
                rows.push(vec![
                    catalog_name.clone(),
                    namespace_name(&table_id),
                    table_id.name.clone(),
                    column.name.clone(),
                    column.id.to_string(),
                    catalog_type_name(column.data_type, &column.properties).to_string(),
                    if column.nullable { "YES" } else { "NO" }.to_string(),
                    column.default_value.clone().unwrap_or_default(),
                    column
                        .properties
                        .get("json_encoding")
                        .cloned()
                        .unwrap_or_default(),
                    column
                        .properties
                        .get("dimension")
                        .cloned()
                        .unwrap_or_else(|| "0".to_string()),
                    if schema.primary_key.contains(&column.name) {
                        "YES"
                    } else {
                        "NO"
                    }
                    .to_string(),
                ]);
            }
        }

        rows.sort();
        Ok(CatalogIntrospectionResult {
            columns: vec![
                "table_catalog".to_string(),
                "table_schema".to_string(),
                "table_name".to_string(),
                "column_name".to_string(),
                "ordinal_position".to_string(),
                "data_type".to_string(),
                "is_nullable".to_string(),
                "column_default".to_string(),
                "json_encoding".to_string(),
                "vector_dimension".to_string(),
                "is_primary_key".to_string(),
            ],
            column_types: vec![
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "int4".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "int4".to_string(),
                "text".to_string(),
            ],
            rows,
        })
    }

    async fn indexes(&self, table_filter: Option<&str>) -> Result<CatalogIntrospectionResult> {
        let mut rows = Vec::new();

        for (catalog_name, table_id, schema) in self.cataloged_tables().await? {
            if !matches_filter(&table_id.name, table_filter) {
                continue;
            }
            let catalog = self.catalog_manager.get_catalog(&catalog_name).await?;
            let mut indexes = schema.indexes.clone();
            indexes.extend(catalog.list_indexes(&table_id).await?);
            indexes.sort_by(|left, right| left.name.cmp(&right.name));
            indexes.dedup_by(|left, right| left.name == right.name);

            for index in indexes {
                rows.push(vec![
                    catalog_name.clone(),
                    namespace_name(&table_id),
                    table_id.name.clone(),
                    index.name,
                    catalog_index_type_name(index.index_type).to_string(),
                    index.columns.join(","),
                    index.is_unique.to_string(),
                ]);
            }
        }

        rows.sort();
        Ok(CatalogIntrospectionResult {
            columns: vec![
                "table_catalog".to_string(),
                "table_schema".to_string(),
                "table_name".to_string(),
                "index_name".to_string(),
                "index_type".to_string(),
                "columns".to_string(),
                "is_unique".to_string(),
            ],
            column_types: vec![
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "bool".to_string(),
            ],
            rows,
        })
    }

    async fn table_routing(
        &self,
        table_filter: Option<&str>,
    ) -> Result<CatalogIntrospectionResult> {
        let mut rows = Vec::new();

        for (catalog_name, table_id, schema) in self.cataloged_tables().await? {
            if !matches_filter(&table_id.name, table_filter) {
                continue;
            }
            let primary_layout = primary_layout(&schema);
            rows.push(vec![
                catalog_name,
                namespace_name(&table_id),
                table_id.name,
                primary_layout
                    .map(|layout| layout.authority.ownership_mode_name().to_string())
                    .unwrap_or_else(|| "ProximaAuthoritative".to_string()),
                schema.workload_profile.as_str().to_string(),
                schema.storage_specialization.as_str().to_string(),
                primary_layout
                    .map(|layout| format!("{:?}", layout.physical_format))
                    .unwrap_or_default(),
                property_value(&schema, &["compute_route", "preferred_compute_route"])
                    .unwrap_or_default(),
                property_value(
                    &schema,
                    &["partitioning", "partition_key", "distribution_key"],
                )
                .unwrap_or_default(),
                schema
                    .relational_capabilities
                    .transaction_profile
                    .clone()
                    .or_else(|| property_value(&schema, &["isolation_profile", "isolation"]))
                    .unwrap_or_default(),
                property_value(&schema, &["freshness_sla", "projection_freshness"])
                    .unwrap_or_default(),
                policy_boundary(&schema),
            ]);
        }

        rows.sort();
        Ok(CatalogIntrospectionResult {
            columns: vec![
                "table_catalog".to_string(),
                "table_schema".to_string(),
                "table_name".to_string(),
                "authority_mode".to_string(),
                "workload_profile".to_string(),
                "storage_specialization".to_string(),
                "primary_format".to_string(),
                "compute_route".to_string(),
                "partitioning".to_string(),
                "isolation_profile".to_string(),
                "freshness_sla".to_string(),
                "policy_boundary".to_string(),
            ],
            column_types: vec![
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
            ],
            rows,
        })
    }

    async fn jdbc_tables(&self, table_filter: Option<&str>) -> Result<CatalogIntrospectionResult> {
        let mut rows = Vec::new();

        for (_catalog_name, table_id, _schema) in self.cataloged_tables().await? {
            if !matches_filter(&table_id.name, table_filter) {
                continue;
            }
            rows.push(vec![
                String::new(),
                namespace_name(&table_id),
                table_id.name,
                "TABLE".to_string(),
            ]);
        }

        rows.sort();
        Ok(CatalogIntrospectionResult {
            columns: vec![
                "TABLE_CAT".to_string(),
                "TABLE_SCHEM".to_string(),
                "TABLE_NAME".to_string(),
                "TABLE_TYPE".to_string(),
            ],
            column_types: vec![
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
            ],
            rows,
        })
    }

    async fn jdbc_columns(&self, table_filter: Option<&str>) -> Result<CatalogIntrospectionResult> {
        let mut rows = Vec::new();

        for (_catalog_name, table_id, schema) in self.cataloged_tables().await? {
            if !matches_filter(&table_id.name, table_filter) {
                continue;
            }
            for column in &schema.columns {
                let (data_type, column_size) = jdbc_type_and_size(column.data_type);
                rows.push(vec![
                    String::new(),
                    namespace_name(&table_id),
                    table_id.name.clone(),
                    column.name.clone(),
                    data_type.to_string(),
                    column_size.to_string(),
                    if column.nullable { "YES" } else { "NO" }.to_string(),
                ]);
            }
        }

        rows.sort();
        Ok(CatalogIntrospectionResult {
            columns: vec![
                "TABLE_CAT".to_string(),
                "TABLE_SCHEM".to_string(),
                "TABLE_NAME".to_string(),
                "COLUMN_NAME".to_string(),
                "DATA_TYPE".to_string(),
                "COLUMN_SIZE".to_string(),
                "IS_NULLABLE".to_string(),
            ],
            column_types: vec![
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "int4".to_string(),
                "int4".to_string(),
                "text".to_string(),
            ],
            rows,
        })
    }

    async fn sqlalchemy_table_names(
        &self,
        table_filter: Option<&str>,
    ) -> Result<CatalogIntrospectionResult> {
        let mut rows = Vec::new();

        for (_catalog_name, table_id, _schema) in self.cataloged_tables().await? {
            if !matches_filter(&table_id.name, table_filter) {
                continue;
            }
            rows.push(vec![table_id.name]);
        }

        rows.sort();
        Ok(CatalogIntrospectionResult {
            columns: vec!["relname".to_string()],
            column_types: vec!["text".to_string()],
            rows,
        })
    }

    async fn sqlalchemy_columns(
        &self,
        table_filter: Option<&str>,
    ) -> Result<CatalogIntrospectionResult> {
        let mut rows = Vec::new();

        for (_catalog_name, table_id, schema) in self.cataloged_tables().await? {
            if !matches_filter(&table_id.name, table_filter) {
                continue;
            }
            for column in &schema.columns {
                rows.push(vec![
                    column.name.clone(),
                    postgres_format_type(column.data_type, &column.properties).to_string(),
                    column.default_value.clone().unwrap_or_default(),
                    (!column.nullable).to_string(),
                    table_id.name.clone(),
                    column.comment.clone().unwrap_or_default(),
                    String::new(),
                    String::new(),
                ]);
            }
        }

        rows.sort();
        Ok(CatalogIntrospectionResult {
            columns: vec![
                "name".to_string(),
                "format_type".to_string(),
                "default".to_string(),
                "not_null".to_string(),
                "table_name".to_string(),
                "comment".to_string(),
                "generated".to_string(),
                "identity_options".to_string(),
            ],
            column_types: vec![
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "bool".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
            ],
            rows,
        })
    }

    async fn jdbc_indexes(&self, table_filter: Option<&str>) -> Result<CatalogIntrospectionResult> {
        let mut rows = Vec::new();

        for (_catalog_name, table_id, schema) in self.cataloged_tables().await? {
            if !matches_filter(&table_id.name, table_filter) {
                continue;
            }
            for (idx, column_name) in schema.primary_key.iter().enumerate() {
                rows.push(vec![
                    String::new(),
                    namespace_name(&table_id),
                    table_id.name.clone(),
                    "false".to_string(),
                    String::new(),
                    format!("{}_pkey", table_id.name),
                    "3".to_string(),
                    (idx + 1).to_string(),
                    column_name.clone(),
                    "A".to_string(),
                ]);
            }
        }

        rows.sort();
        Ok(CatalogIntrospectionResult {
            columns: vec![
                "TABLE_CAT".to_string(),
                "TABLE_SCHEM".to_string(),
                "TABLE_NAME".to_string(),
                "NON_UNIQUE".to_string(),
                "INDEX_QUALIFIER".to_string(),
                "INDEX_NAME".to_string(),
                "TYPE".to_string(),
                "ORDINAL_POSITION".to_string(),
                "COLUMN_NAME".to_string(),
                "ASC_OR_DESC".to_string(),
            ],
            column_types: vec![
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "bool".to_string(),
                "text".to_string(),
                "text".to_string(),
                "int4".to_string(),
                "int4".to_string(),
                "text".to_string(),
                "text".to_string(),
            ],
            rows,
        })
    }

    /// Return props promotion state for all tables (or a single table when `table_filter` is set).
    ///
    /// Columns: table_name, props_key, promoted_column, promotion_enabled,
    ///          frequency_threshold, min_record_count, status
    async fn props_promotion_candidates(
        &self,
        table_filter: Option<&str>,
    ) -> Result<CatalogIntrospectionResult> {
        let mut rows: Vec<Vec<String>> = Vec::new();

        for (_catalog_name, table_id, schema) in self.cataloged_tables().await? {
            if !matches_filter(&table_id.name, table_filter) {
                continue;
            }

            let policy = &schema.props_auto_promotion;
            let enabled_str = if policy.enabled { "true" } else { "false" };
            let threshold_str = format!("{:.2}", policy.frequency_threshold);
            let min_count_str = policy.min_record_count.to_string();

            // Rows for already-promoted keys.
            for (key, col_name) in &policy.promoted_keys {
                rows.push(vec![
                    table_id.name.clone(),
                    key.clone(),
                    col_name.clone(),
                    enabled_str.to_string(),
                    threshold_str.clone(),
                    min_count_str.clone(),
                    "promoted".to_string(),
                ]);
            }

            // If auto-promotion is enabled and no keys are promoted yet, emit one
            // summary row so the table is visible even before any promotions.
            if policy.enabled && policy.promoted_keys.is_empty() {
                rows.push(vec![
                    table_id.name.clone(),
                    String::new(),
                    String::new(),
                    enabled_str.to_string(),
                    threshold_str.clone(),
                    min_count_str.clone(),
                    "no_candidates_yet".to_string(),
                ]);
            }
        }

        rows.sort();
        Ok(CatalogIntrospectionResult {
            columns: vec![
                "table_name".to_string(),
                "props_key".to_string(),
                "promoted_column".to_string(),
                "promotion_enabled".to_string(),
                "frequency_threshold".to_string(),
                "min_record_count".to_string(),
                "status".to_string(),
            ],
            column_types: vec![
                "text".to_string(),
                "text".to_string(),
                "text".to_string(),
                "boolean".to_string(),
                "float8".to_string(),
                "int8".to_string(),
                "text".to_string(),
            ],
            rows,
        })
    }

    async fn cataloged_tables(&self) -> Result<Vec<(String, TableIdentifier, CatalogTableSchema)>> {
        let mut tables = Vec::new();

        for catalog_name in self.catalog_manager.list_catalogs().await {
            let catalog = self.catalog_manager.get_catalog(&catalog_name).await?;
            for namespace in catalog.list_namespaces(None).await? {
                for table_id in catalog.list_tables(&namespace.levels).await? {
                    let schema = catalog.get_table(&table_id).await?;
                    tables.push((catalog_name.clone(), table_id, schema));
                }
            }
        }

        Ok(tables)
    }
}

fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn extract_string_filter(sql: &str, column: &str) -> Option<String> {
    let normalized = normalize_sql(sql);
    let needle = format!("{column} = '");
    let start = normalized.find(&needle)? + needle.len();
    let rest = &normalized[start..];
    let end = rest.find('\'')?;
    Some(rest[..end].to_string())
}

/// Extract the first single-quoted string argument from a function call.
/// e.g. `SELECT * FROM proximadb_catalog.props_promotion_candidates('events')` → `Some("events")`
fn extract_function_arg(sql: &str, fn_name: &str) -> Option<String> {
    let lower = sql.to_lowercase();
    let needle = format!("{}('", fn_name.to_lowercase());
    let start = lower.find(&needle)? + needle.len();
    let rest = &sql[start..];
    let end = rest.find('\'')?;
    Some(rest[..end].to_string())
}

fn matches_filter(value: &str, filter: Option<&str>) -> bool {
    filter.is_none_or(|filter| value.eq_ignore_ascii_case(filter))
}

fn primary_layout(schema: &CatalogTableSchema) -> Option<&crate::catalog::CatalogStorageLayout> {
    schema
        .storage_layouts
        .iter()
        .rev()
        .find(|layout| layout.name == "primary")
        .or_else(|| schema.storage_layouts.first())
}

fn property_value(schema: &CatalogTableSchema, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| schema.properties.get(*key).cloned())
}

fn policy_boundary(schema: &CatalogTableSchema) -> String {
    if let Some(boundary) = property_value(schema, &["policy_boundary", "rls_boundary"]) {
        return boundary;
    }

    match primary_layout(schema) {
        Some(layout) if layout.policy_enforced_in_proxima => "engine-enforced".to_string(),
        Some(layout) if layout.authority.is_external_authoritative() => {
            "external-policy".to_string()
        }
        Some(layout)
            if matches!(
                layout.authority,
                crate::catalog::CatalogAuthorityMode::FederatedRead
            ) =>
        {
            "connector-enforced".to_string()
        }
        Some(_) => "unsupported".to_string(),
        None => "engine-enforced".to_string(),
    }
}

fn namespace_name(table_id: &TableIdentifier) -> String {
    let namespace = table_id.namespace.join(".");
    if namespace.is_empty() || namespace == "default" {
        "public".to_string()
    } else {
        namespace
    }
}

fn infer_schema_kind(schema: &CatalogTableSchema) -> &'static str {
    let has_json = schema
        .columns
        .iter()
        .any(|column| matches!(column.data_type, CatalogDataType::Json));
    let has_vector = schema.columns.iter().any(|column| {
        matches!(
            column.data_type,
            CatalogDataType::Vector | CatalogDataType::SparseVector | CatalogDataType::BinaryVector
        )
    });

    match (has_json, has_vector) {
        (true, true) => "mixed_relational_document_vector",
        (true, false) => "relational_document",
        (false, true) => "relational_vector",
        (false, false) => "relational",
    }
}

fn catalog_type_name(
    data_type: CatalogDataType,
    properties: &std::collections::HashMap<String, String>,
) -> &'static str {
    match data_type {
        CatalogDataType::Boolean => "boolean",
        CatalogDataType::Int8 => "int8",
        CatalogDataType::Int16 => "int16",
        CatalogDataType::Int32 => "int32",
        CatalogDataType::Int64 => "int64",
        CatalogDataType::Float32 => "float32",
        CatalogDataType::Float64 => "float64",
        CatalogDataType::String => "text",
        CatalogDataType::Binary => "bytea",
        CatalogDataType::Date => "date",
        CatalogDataType::Time => "time",
        CatalogDataType::Timestamp => "timestamp",
        CatalogDataType::TimestampTz => "timestamptz",
        CatalogDataType::Decimal => "decimal",
        CatalogDataType::Uuid => "uuid",
        CatalogDataType::Json => match properties.get("json_encoding").map(String::as_str) {
            Some("jsonb") => "jsonb",
            _ => "json",
        },
        CatalogDataType::Vector => "vector",
        CatalogDataType::SparseVector => "sparse_vector",
        CatalogDataType::BinaryVector => "binary_vector",
    }
}

fn postgres_format_type(
    data_type: CatalogDataType,
    properties: &std::collections::HashMap<String, String>,
) -> &'static str {
    match data_type {
        CatalogDataType::Boolean => "boolean",
        CatalogDataType::Int8 => "smallint",
        CatalogDataType::Int16 => "smallint",
        CatalogDataType::Int32 => "integer",
        CatalogDataType::Int64 => "bigint",
        CatalogDataType::Float32 => "real",
        CatalogDataType::Float64 => "double precision",
        CatalogDataType::String => "character varying",
        CatalogDataType::Binary => "bytea",
        CatalogDataType::Date => "date",
        CatalogDataType::Time => "time without time zone",
        CatalogDataType::Timestamp => "timestamp without time zone",
        CatalogDataType::TimestampTz => "timestamp with time zone",
        CatalogDataType::Decimal => "numeric",
        CatalogDataType::Uuid => "uuid",
        CatalogDataType::Json => match properties.get("json_encoding").map(String::as_str) {
            Some("jsonb") => "jsonb",
            _ => "json",
        },
        CatalogDataType::Vector => "vector",
        CatalogDataType::SparseVector => "sparse_vector",
        CatalogDataType::BinaryVector => "binary_vector",
    }
}

fn jdbc_type_and_size(data_type: CatalogDataType) -> (i32, i32) {
    match data_type {
        CatalogDataType::Boolean => (16, 1),
        CatalogDataType::Int8 => (-6, 3),
        CatalogDataType::Int16 => (5, 5),
        CatalogDataType::Int32 => (4, 10),
        CatalogDataType::Int64 => (-5, 19),
        CatalogDataType::Float32 => (6, 24),
        CatalogDataType::Float64 => (8, 53),
        CatalogDataType::String => (12, 255),
        CatalogDataType::Binary => (-2, 255),
        CatalogDataType::Date => (91, 13),
        CatalogDataType::Time => (92, 15),
        CatalogDataType::Timestamp | CatalogDataType::TimestampTz => (93, 29),
        CatalogDataType::Decimal => (3, 38),
        CatalogDataType::Uuid => (1111, 36),
        CatalogDataType::Json => (1111, 0),
        CatalogDataType::Vector | CatalogDataType::SparseVector | CatalogDataType::BinaryVector => {
            (1111, 0)
        }
    }
}

fn catalog_index_type_name(index_type: CatalogIndexType) -> &'static str {
    match index_type {
        CatalogIndexType::BTree => "btree",
        CatalogIndexType::Hash => "hash",
        CatalogIndexType::FullText => "fulltext",
        CatalogIndexType::Gin => "gin",
        CatalogIndexType::Hnsw => "hnsw",
        CatalogIndexType::Ivf => "ivf",
        CatalogIndexType::Pq => "pq",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::query::sql_frontend::SqlFrontendParser;
    use crate::services::{DdlService, DdlStatement};

    #[tokio::test]
    async fn test_catalog_introspection_projects_agentic_mixed_schema() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");

        let ddl = DdlService::new(manager.clone());
        ddl.execute(DdlStatement::CreateNamespace {
            namespace: vec!["default".to_string()],
            if_not_exists: true,
            properties: HashMap::new(),
        })
        .await
        .expect("namespace");

        let parser = SqlFrontendParser::new();
        let table = parser
            .parse_ddl(
                "CREATE TABLE IF NOT EXISTS agent_store (
                    record_id TEXT NOT NULL,
                    payload JSONB NOT NULL,
                    embedding VECTOR(16),
                    PRIMARY KEY (record_id)
                ) WITH (
                    storage_engine = 'SST',
                    workload = 'htap',
                    layout = 'hybrid',
                    xcatalog_namespace = 'agentic.langgraph',
                    schema_kind = 'agentic_mixed',
                    compute_route = 'datafusion-local',
                    partitioning = 'tenant_id,bucket',
                    isolation = 'snapshot-isolation',
                    freshness_sla = '5s',
                    policy_boundary = 'engine-enforced'
                );",
            )
            .expect("parse table")
            .expect("table ddl");
        ddl.execute(table).await.expect("create table");

        for sql in [
            "CREATE INDEX idx_payload ON agent_store USING GIN (payload);",
            "CREATE INDEX idx_embedding ON agent_store USING HNSW (embedding);",
        ] {
            let index = parser
                .parse_ddl(sql)
                .expect("parse index")
                .expect("index ddl");
            ddl.execute(index).await.expect("create index");
        }

        let introspection = CatalogIntrospectionService::new(manager);
        let tables = introspection
            .execute_select("SELECT * FROM xcatalog.tables WHERE table_name = 'agent_store'")
            .await
            .expect("tables query")
            .expect("tables result");
        assert_eq!(tables.rows.len(), 1);
        assert_eq!(tables.rows[0][2], "agent_store");
        assert_eq!(tables.rows[0][3], "agentic_mixed");
        assert_eq!(tables.rows[0][4], "SST");
        assert_eq!(tables.rows[0][5], "hybrid");
        assert_eq!(tables.rows[0][7], "agentic.langgraph");

        let columns = introspection
            .execute_select("SELECT * FROM xcatalog.columns WHERE table_name = 'agent_store'")
            .await
            .expect("columns query")
            .expect("columns result");
        assert!(
            columns
                .rows
                .iter()
                .any(|row| row[3] == "payload" && row[5] == "jsonb")
        );
        assert!(
            columns
                .rows
                .iter()
                .any(|row| row[3] == "embedding" && row[5] == "vector" && row[9] == "16")
        );

        let indexes = introspection
            .execute_select("SELECT * FROM xcatalog.indexes WHERE table_name = 'agent_store'")
            .await
            .expect("indexes query")
            .expect("indexes result");
        assert!(
            indexes
                .rows
                .iter()
                .any(|row| row[3] == "idx_payload" && row[4] == "gin")
        );
        assert!(
            indexes
                .rows
                .iter()
                .any(|row| row[3] == "idx_embedding" && row[4] == "hnsw")
        );

        let routing = introspection
            .execute_select(
                "SELECT * FROM information_schema.table_routing WHERE table_name = 'agent_store'",
            )
            .await
            .expect("routing query")
            .expect("routing result");
        assert_eq!(routing.rows.len(), 1);
        assert_eq!(routing.rows[0][2], "agent_store");
        assert_eq!(routing.rows[0][3], "ProximaAuthoritative");
        assert_eq!(routing.rows[0][4], "htap");
        assert_eq!(routing.rows[0][5], "pax_row_family");
        assert_eq!(routing.rows[0][7], "datafusion-local");
        assert_eq!(routing.rows[0][8], "tenant_id,bucket");
        assert_eq!(routing.rows[0][9], "snapshot-isolation");
        assert_eq!(routing.rows[0][10], "5s");
        assert_eq!(routing.rows[0][11], "engine-enforced");
    }

    #[tokio::test]
    async fn test_props_promotion_candidates_reflects_promoted_keys() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");

        let ddl = DdlService::new(manager.clone());
        ddl.execute(DdlStatement::CreateNamespace {
            namespace: vec!["default".to_string()],
            if_not_exists: true,
            properties: HashMap::new(),
        })
        .await
        .expect("namespace");

        let parser = SqlFrontendParser::new();
        let create = parser
            .parse_ddl("CREATE TABLE events (id TEXT NOT NULL, payload JSONB, PRIMARY KEY (id))")
            .expect("parse")
            .expect("ddl");
        ddl.execute(create).await.expect("create table");

        // Enable auto-promotion
        let set_opt = parser
            .parse_ddl("ALTER TABLE events SET (props_auto_promotion = 'enabled')")
            .expect("parse set option")
            .expect("ddl");
        ddl.execute(set_opt).await.expect("set option");

        // Promote a key
        let promote = parser
            .parse_ddl("ALTER TABLE events PROMOTE PROPS KEY user_id TYPE BIGINT")
            .expect("parse promote")
            .expect("ddl");
        ddl.execute(promote).await.expect("promote key");

        let introspection = CatalogIntrospectionService::new(manager.clone());

        // Query via function-call form
        let result = introspection
            .execute_select("SELECT * FROM proximadb_catalog.props_promotion_candidates('events')")
            .await
            .expect("query")
            .expect("result");

        assert_eq!(
            result.columns,
            [
                "table_name",
                "props_key",
                "promoted_column",
                "promotion_enabled",
                "frequency_threshold",
                "min_record_count",
                "status"
            ]
        );

        let promoted = result
            .rows
            .iter()
            .find(|r| r[1] == "user_id")
            .expect("expected user_id row");
        assert_eq!(promoted[0], "events");
        assert_eq!(promoted[2], "props__user_id");
        assert_eq!(promoted[3], "true");
        assert_eq!(promoted[6], "promoted");
    }
}
