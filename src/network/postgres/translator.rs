// SQL query translator for PostgreSQL to ProximaDB
//
// Provides:
// - PostgreSQL SQL to ProximaDB SQL translation
// - pgvector syntax support
// - Type mapping

use anyhow::Result;
use tracing::debug;

/// Query translator from PostgreSQL to ProximaDB
pub struct QueryTranslator {
    /// Whether to enable pgvector compatibility
    pgvector_compat: bool,
}

impl QueryTranslator {
    /// Create a new translator
    pub fn new() -> Self {
        Self {
            pgvector_compat: true,
        }
    }

    /// Translate a PostgreSQL query to ProximaDB format
    pub fn translate(&self, query: &str) -> Result<String> {
        let trimmed = query.trim();

        // Handle empty queries
        if trimmed.is_empty() {
            return Ok(String::new());
        }

        // Handle special PostgreSQL commands
        if let Some(result) = self.handle_special_command(trimmed) {
            return Ok(result);
        }

        // Parse and translate SQL
        let translated = self.translate_sql(trimmed)?;

        debug!("Translated query: {} -> {}", query, translated);

        Ok(translated)
    }

    /// Handle special PostgreSQL commands
    fn handle_special_command(&self, query: &str) -> Option<String> {
        let upper = query.to_uppercase();

        // Handle SET commands
        if upper.starts_with("SET ") {
            return Some(String::new()); // Ignore SET commands
        }

        // Handle SHOW commands
        if upper.starts_with("SHOW ") {
            return Some(self.handle_show(&query[5..]));
        }

        // Handle transaction commands
        if upper == "BEGIN" || upper == "START TRANSACTION" {
            return Some("BEGIN".to_string());
        }
        if upper == "COMMIT" {
            return Some("COMMIT".to_string());
        }
        if upper == "ROLLBACK" {
            return Some("ROLLBACK".to_string());
        }

        // Handle pg_catalog queries
        if upper.contains("PG_CATALOG") || upper.contains("INFORMATION_SCHEMA") {
            return Some(self.handle_catalog_query(query));
        }

        None
    }

    /// Handle SHOW commands
    fn handle_show(&self, param: &str) -> String {
        let param = param.trim().trim_end_matches(';').to_uppercase();

        match param.as_str() {
            "SERVER_VERSION" => "SELECT '16.0' AS server_version".to_string(),
            "SERVER_ENCODING" => "SELECT 'UTF8' AS server_encoding".to_string(),
            "CLIENT_ENCODING" => "SELECT 'UTF8' AS client_encoding".to_string(),
            "TIMEZONE" => "SELECT 'UTC' AS timezone".to_string(),
            "SEARCH_PATH" => "SELECT 'public' AS search_path".to_string(),
            "TRANSACTION ISOLATION LEVEL" => {
                "SELECT 'read committed' AS transaction_isolation".to_string()
            }
            _ => format!("SELECT '' AS {}", param.to_lowercase()),
        }
    }

    /// Handle catalog queries
    fn handle_catalog_query(&self, _query: &str) -> String {
        // Return empty result for catalog queries
        "SELECT 1 WHERE false".to_string()
    }

    /// Translate SQL query
    fn translate_sql(&self, query: &str) -> Result<String> {
        let mut translated = query.to_string();

        // Translate pgvector syntax
        if self.pgvector_compat {
            translated = self.translate_pgvector(&translated)?;
        }

        // Translate PostgreSQL JSON/JSONB syntax into ProximaDB JSON helpers.
        translated = self.translate_jsonb(&translated)?;

        // Translate PostgreSQL-specific functions
        translated = self.translate_functions(&translated);

        // Translate data types
        translated = self.translate_types(&translated);

        Ok(translated)
    }

    /// Translate pgvector syntax
    fn translate_pgvector(&self, query: &str) -> Result<String> {
        let mut result = query.to_string();

        // Translate vector type notation: '[1,2,3]'::vector -> [1,2,3]
        let re_vector = regex::Regex::new(r"'(\[[\d\.,\s-]+\])'::vector(\(\d+\))?")?;
        result = re_vector.replace_all(&result, "$1").to_string();

        // Translate distance operators
        // <-> (L2 distance) -> VECTOR_DISTANCE(..., 'l2')
        // <=> (cosine distance) -> VECTOR_DISTANCE(..., 'cosine')
        // <#> (inner product) -> VECTOR_DISTANCE(..., 'ip')

        // This is a simplified translation; full support would need proper SQL parsing
        if result.contains("<->") {
            result = self.translate_distance_operator(&result, "<->", "l2");
        }
        if result.contains("<=>") {
            result = self.translate_distance_operator(&result, "<=>", "cosine");
        }
        if result.contains("<#>") {
            result = self.translate_distance_operator(&result, "<#>", "ip");
        }

        Ok(result)
    }

    /// Translate PostgreSQL JSONB operators to portable JSON helper functions.
    fn translate_jsonb(&self, query: &str) -> Result<String> {
        let mut result = query.to_string();

        let re_text_extract = regex::Regex::new(r#"([A-Za-z_][A-Za-z0-9_\.]*)\s*->>\s*'([^']+)'"#)?;
        result = re_text_extract
            .replace_all(&result, "JSON_EXTRACT_TEXT($1, '$2')")
            .to_string();

        let re_json_extract = regex::Regex::new(r#"([A-Za-z_][A-Za-z0-9_\.]*)\s*->\s*'([^']+)'"#)?;
        result = re_json_extract
            .replace_all(&result, "JSON_EXTRACT($1, '$2')")
            .to_string();

        let re_contains =
            regex::Regex::new(r#"([A-Za-z_][A-Za-z0-9_\.]*)\s*@>\s*('[^']+'(?:::jsonb)?)"#)?;
        result = re_contains
            .replace_all(&result, "JSON_CONTAINS($1, $2)")
            .to_string();

        let re_exists = regex::Regex::new(r#"([A-Za-z_][A-Za-z0-9_\.]*)\s*\?\s*'([^']+)'"#)?;
        result = re_exists
            .replace_all(&result, "JSON_EXISTS($1, '$2')")
            .to_string();

        let re_path_exists =
            regex::Regex::new(r#"jsonb_path_exists\s*\(\s*([^,]+)\s*,\s*('[^']+')\s*\)"#)?;
        result = re_path_exists
            .replace_all(&result, "JSON_PATH_EXISTS($1, $2)")
            .to_string();

        Ok(result)
    }

    /// Translate distance operator to function call
    fn translate_distance_operator(&self, query: &str, op: &str, metric: &str) -> String {
        // Keep this pgwire compatibility layer intentionally conservative:
        // full SQL expression normalization belongs in the SQL frontend.
        let escaped_op = regex::escape(op);
        let Ok(re) = regex::Regex::new(&format!(
            r"([A-Za-z_][A-Za-z0-9_\.]*)\s*{}\s*(\[[^\]]+\]|\$\d+|[A-Za-z_][A-Za-z0-9_\.]*)",
            escaped_op
        )) else {
            return query.to_string();
        };

        re.replace_all(query, format!("VECTOR_DISTANCE($1, $2, '{}')", metric))
            .to_string()
    }

    /// Translate PostgreSQL-specific functions
    fn translate_functions(&self, query: &str) -> String {
        let mut result = query.to_string();

        // NOW() -> CURRENT_TIMESTAMP
        result = result.replace("NOW()", "CURRENT_TIMESTAMP");

        // COALESCE stays the same
        // NULLIF stays the same

        // array_agg -> not supported, return empty
        // string_agg -> not supported, return empty

        result
    }

    /// Translate PostgreSQL data types
    fn translate_types(&self, query: &str) -> String {
        let mut result = query.to_string();

        // Type casts
        result = result.replace("::text", "");
        result = result.replace("::varchar", "");
        result = result.replace("::integer", "");
        result = result.replace("::bigint", "");
        result = result.replace("::float", "");
        result = result.replace("::double precision", "");
        result = result.replace("::boolean", "");
        result = result.replace("::timestamp", "");
        result = result.replace("::date", "");
        result = result.replace("::jsonb", "");
        result = result.replace("::json", "");

        result
    }

    /// Check if a query is a DDL statement
    pub fn is_ddl(&self, query: &str) -> bool {
        let upper = query.trim().to_uppercase();
        upper.starts_with("CREATE ")
            || upper.starts_with("DROP ")
            || upper.starts_with("ALTER ")
            || upper.starts_with("TRUNCATE ")
    }

    /// Check if a query is a DML statement
    pub fn is_dml(&self, query: &str) -> bool {
        let upper = query.trim().to_uppercase();
        upper.starts_with("INSERT ") || upper.starts_with("UPDATE ") || upper.starts_with("DELETE ")
    }

    /// Check if a query is a SELECT statement
    pub fn is_select(&self, query: &str) -> bool {
        query.trim().to_uppercase().starts_with("SELECT ")
    }
}

impl Default for QueryTranslator {
    fn default() -> Self {
        Self::new()
    }
}

/// Translated query result
#[derive(Debug, Clone)]
pub struct TranslatedQuery {
    /// Original query
    pub original: String,
    /// Translated query
    pub translated: String,
    /// Query type
    pub query_type: QueryType,
    /// Vector search parameters (if applicable)
    pub vector_search: Option<VectorSearchParams>,
}

/// Query type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QueryType {
    /// SELECT query
    Select,
    /// INSERT query
    Insert,
    /// UPDATE query
    Update,
    /// DELETE query
    Delete,
    /// DDL (CREATE, ALTER, DROP)
    Ddl,
    /// Transaction control
    Transaction,
    /// Other
    Other,
}

/// Vector search parameters extracted from query
#[derive(Debug, Clone)]
pub struct VectorSearchParams {
    /// Column name
    pub column: String,
    /// Query vector
    pub vector: Vec<f32>,
    /// Distance metric
    pub metric: String,
    /// Limit (k)
    pub limit: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_translate_empty() {
        let translator = QueryTranslator::new();
        let result = translator.translate("").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_translate_set() {
        let translator = QueryTranslator::new();
        let result = translator
            .translate("SET client_encoding TO 'UTF8'")
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_translate_show() {
        let translator = QueryTranslator::new();
        let result = translator.translate("SHOW server_version").unwrap();
        assert!(result.contains("16.0"));
    }

    #[test]
    fn test_translate_begin() {
        let translator = QueryTranslator::new();
        let result = translator.translate("BEGIN").unwrap();
        assert_eq!(result, "BEGIN");
    }

    #[test]
    fn test_is_ddl() {
        let translator = QueryTranslator::new();
        assert!(translator.is_ddl("CREATE TABLE foo (id INT)"));
        assert!(translator.is_ddl("DROP TABLE foo"));
        assert!(!translator.is_ddl("SELECT * FROM foo"));
    }

    #[test]
    fn test_is_select() {
        let translator = QueryTranslator::new();
        assert!(translator.is_select("SELECT * FROM foo"));
        assert!(!translator.is_select("INSERT INTO foo VALUES (1)"));
    }

    #[test]
    fn test_translate_functions() {
        let translator = QueryTranslator::new();
        let result = translator.translate_functions("SELECT NOW(), id FROM foo");
        assert!(result.contains("CURRENT_TIMESTAMP"));
    }

    #[test]
    fn test_translate_pgvector_distance_operators_to_functions() {
        let translator = QueryTranslator::new();

        let l2 = translator
            .translate("SELECT id FROM docs ORDER BY embedding <-> '[0.1, 0.2]'::vector LIMIT 5")
            .unwrap();
        assert!(l2.contains("ORDER BY VECTOR_DISTANCE(embedding, [0.1, 0.2], 'l2') LIMIT 5"));
        assert!(!l2.contains("<->"));

        let cosine = translator
            .translate("SELECT id, vec <=> $1 AS distance FROM docs")
            .unwrap();
        assert!(cosine.contains("VECTOR_DISTANCE(vec, $1, 'cosine') AS distance"));
        assert!(!cosine.contains("<=>"));

        let inner_product = translator
            .translate("SELECT id FROM docs ORDER BY doc.embedding <#> query.embedding")
            .unwrap();
        assert!(
            inner_product
                .contains("ORDER BY VECTOR_DISTANCE(doc.embedding, query.embedding, 'ip')")
        );
        assert!(!inner_product.contains("<#>"));
    }

    #[test]
    fn test_translate_pgwire_jsonb_vector_and_cypher_extensions() {
        let translator = QueryTranslator::new();
        let result = translator
            .translate(
                "SELECT id FROM docs \
                 WHERE metadata->>'tenant' = 'acme' \
                   AND metadata @> '{\"role\":\"planner\"}'::jsonb \
                   AND metadata ? 'skills' \
                   AND jsonb_path_exists(metadata, '$.skills[*]') \
                 ORDER BY embedding <-> '[0.1, 0.2, 0.3]'::vector \
                 LIMIT 10",
            )
            .unwrap();

        assert!(result.contains("JSON_EXTRACT_TEXT(metadata, 'tenant')"));
        assert!(result.contains("JSON_CONTAINS(metadata, '{\"role\":\"planner\"}')"));
        assert!(result.contains("JSON_EXISTS(metadata, 'skills')"));
        assert!(result.contains("JSON_PATH_EXISTS(metadata, '$.skills[*]')"));
        assert!(result.contains("[0.1, 0.2, 0.3]"));
        assert!(result.contains("VECTOR_DISTANCE(embedding, [0.1, 0.2, 0.3], 'l2')"));
        assert!(!result.contains("::jsonb"));
        assert!(!result.contains("::vector"));
        assert!(!result.contains("<->"));

        let cypher = translator
            .translate("SELECT * FROM GRAPH_QUERY('MATCH (n)-[:CALLS]->(m) RETURN m')")
            .unwrap();
        assert!(cypher.contains("GRAPH_QUERY"));
        assert!(cypher.contains("MATCH (n)-[:CALLS]->(m) RETURN m"));
    }
}
