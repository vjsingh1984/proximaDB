use proximadb_filter_expression::FilterExpression;

/// Pipeline operator types for query execution.
#[derive(Debug, Clone, PartialEq)]
pub enum PipelineOperator {
    /// Scan operator - reads data from source.
    Scan {
        /// Source identifier (file path, collection ID, etc.).
        source: String,
    },
    /// Filter operator - applies filter predicate.
    Filter {
        /// Filter expression.
        expression: FilterExpression,
    },
    /// Project operator - projects specific columns.
    Project {
        /// Column names to project.
        columns: Vec<String>,
    },
    /// Sort operator - sorts by specified column.
    Sort {
        /// Sort column name.
        column: String,
        /// Ascending or descending.
        ascending: bool,
        /// Limit on number of results.
        limit: Option<usize>,
    },
    /// TopK operator - selects top K results.
    TopK {
        /// K value.
        k: usize,
        /// Sort column for ranking.
        sort_column: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_filter_expression::ComparisonOperator;

    #[test]
    fn filter_operator_carries_expression() {
        let operator = PipelineOperator::Filter {
            expression: FilterExpression::Comparison {
                field: "score".to_string(),
                operator: ComparisonOperator::GreaterThan,
                value: serde_json::json!(0.5),
            },
        };

        assert!(matches!(operator, PipelineOperator::Filter { .. }));
    }

    #[test]
    fn topk_operator_preserves_ranking_inputs() {
        let operator = PipelineOperator::TopK {
            k: 10,
            sort_column: "score".to_string(),
        };

        match operator {
            PipelineOperator::TopK { k, sort_column } => {
                assert_eq!(k, 10);
                assert_eq!(sort_column, "score");
            }
            _ => panic!("expected topk operator"),
        }
    }
}
