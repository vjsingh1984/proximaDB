pub mod index_algorithm;
/// Advanced metadata query engine with runtime evaluation and regex caching.
pub mod metadata_query;
pub mod query_params;
pub mod search_query;

pub use index_algorithm::IndexAlgorithmConfig;
pub use query_params::Params;
pub use search_query::SearchQuery;

/// Complex filter expression for advanced metadata filtering.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FilterExpression {
    /// Single comparison operation.
    Comparison {
        /// Metadata field name to compare.
        field: String,
        /// Comparison operator to apply.
        operator: ComparisonOperator,
        /// Value to compare against.
        value: serde_json::Value,
    },
    /// Logical AND of multiple expressions.
    And(Vec<FilterExpression>),
    /// Logical OR of multiple expressions.
    Or(Vec<FilterExpression>),
    /// Logical NOT of an expression.
    Not(Box<FilterExpression>),
}

/// Comparison operators for metadata filtering.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ComparisonOperator {
    /// Equal to.
    Equals,
    /// Not equal to.
    NotEquals,
    /// Greater than.
    GreaterThan,
    /// Greater than or equal to.
    GreaterThanOrEqual,
    /// Less than.
    LessThan,
    /// Less than or equal to.
    LessThanOrEqual,
    /// Value is in the provided list.
    In,
    /// Value is not in the provided list.
    NotIn,
    /// String contains substring.
    Contains,
    /// String starts with prefix.
    StartsWith,
    /// String ends with suffix.
    EndsWith,
    /// Value is between two bounds (inclusive).
    Between,
    /// Value is null.
    IsNull,
    /// Value is not null.
    IsNotNull,
    /// SQL-style LIKE pattern matching (supports `%` and `_` wildcards).
    Like,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_expression_is_structural() {
        let expr = FilterExpression::Comparison {
            field: "score".to_string(),
            operator: ComparisonOperator::GreaterThan,
            value: serde_json::json!(0.8),
        };

        match expr {
            FilterExpression::Comparison {
                field,
                operator,
                value,
            } => {
                assert_eq!(field, "score");
                assert_eq!(operator, ComparisonOperator::GreaterThan);
                assert_eq!(value, serde_json::json!(0.8));
            }
            _ => panic!("expected comparison expression"),
        }
    }

    #[test]
    fn nested_boolean_filter_expressions_round_trip_shape() {
        let expr = FilterExpression::And(vec![
            FilterExpression::Comparison {
                field: "tenant".to_string(),
                operator: ComparisonOperator::Equals,
                value: serde_json::json!("acme"),
            },
            FilterExpression::Not(Box::new(FilterExpression::Comparison {
                field: "deleted".to_string(),
                operator: ComparisonOperator::Equals,
                value: serde_json::json!(true),
            })),
        ]);

        match expr {
            FilterExpression::And(parts) => {
                assert_eq!(parts.len(), 2);
                assert!(matches!(parts[1], FilterExpression::Not(_)));
            }
            _ => panic!("expected and expression"),
        }
    }
}
