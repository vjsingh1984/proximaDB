use proximadb_query_filter::{FilterOperator, FilterValue};

/// Document query expression used by cross-model query orchestration.
#[derive(Debug, Clone)]
pub struct DocumentQueryExpr {
    /// Collection to query.
    pub collection: String,
    /// JSON path filters.
    pub path_filters: Vec<PathFilter>,
    /// Full-text search query.
    pub text_search: Option<String>,
    /// Projection (fields to return).
    pub projection: Vec<String>,
    /// Sort order.
    pub sort: Option<DocumentSort>,
    /// Limit.
    pub limit: Option<u32>,
}

/// JSON path filter used by document queries.
#[derive(Debug, Clone)]
pub struct PathFilter {
    /// JSON path (e.g. `"$.user.name"`).
    pub path: String,
    /// Comparison operator.
    pub operator: FilterOperator,
    /// Value to compare against.
    pub value: FilterValue,
}

/// Document sort specification.
#[derive(Debug, Clone)]
pub struct DocumentSort {
    /// Path to sort by.
    pub path: String,
    /// Ascending or descending.
    pub ascending: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_query_expr_carries_filter_projection_and_limit() {
        let expr = DocumentQueryExpr {
            collection: "products".to_string(),
            path_filters: vec![PathFilter {
                path: "$.category".to_string(),
                operator: FilterOperator::Eq,
                value: FilterValue::String("electronics".to_string()),
            }],
            text_search: Some("laptop".to_string()),
            projection: vec!["id".to_string(), "name".to_string()],
            sort: Some(DocumentSort {
                path: "$.price".to_string(),
                ascending: true,
            }),
            limit: Some(25),
        };

        assert_eq!(expr.collection, "products");
        assert_eq!(expr.path_filters.len(), 1);
        assert_eq!(expr.projection, vec!["id", "name"]);
        assert_eq!(expr.limit, Some(25));
        assert!(expr.text_search.is_some());
    }

    #[test]
    fn document_sort_direction_is_explicit() {
        let ascending = DocumentSort {
            path: "$.created_at".to_string(),
            ascending: true,
        };
        let descending = DocumentSort {
            path: "$.created_at".to_string(),
            ascending: false,
        };

        assert!(ascending.ascending);
        assert!(!descending.ascending);
    }
}
