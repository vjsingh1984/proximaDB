// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Columnar metadata-filter types (ColumnarMetadataFilter, FilterLogic,
//! FilterCondition + FilterExpression conversion) — hoisted from root
//! `formats/columnar/mod.rs` (TD-DECOMP-73) so metadata_filter_strategy can
//! move into storage-common's text_search/metadata cluster.

/// Backwards-compat alias for [`ColumnarMetadataFilter`].
pub type MetadataFilter = ColumnarMetadataFilter;

/// Metadata filter for queries
#[derive(Debug, Clone)]
pub struct ColumnarMetadataFilter {
    pub conditions: Vec<FilterCondition>,
    pub logic: FilterLogic,
}

#[derive(Debug, Clone)]
pub enum FilterLogic {
    And,
    Or,
}

#[derive(Debug, Clone)]
pub enum FilterCondition {
    Equals(String, serde_json::Value),
    Range(String, serde_json::Value, serde_json::Value),
    In(String, Vec<serde_json::Value>),
    IsNull(String),
    IsNotNull(String),
}

impl FilterCondition {
    /// Get the column name from the filter condition
    pub fn column(&self) -> &str {
        match self {
            FilterCondition::Equals(col, _) => col,
            FilterCondition::Range(col, _, _) => col,
            FilterCondition::In(col, _) => col,
            FilterCondition::IsNull(col) => col,
            FilterCondition::IsNotNull(col) => col,
        }
    }
}

impl ColumnarMetadataFilter {
    /// Convert from core::search::FilterExpression to columnar::ColumnarMetadataFilter
    /// This enables row group pruning using FilterExpression
    pub fn from_filter_expression(
        expr: &proximadb_filter_expression::FilterExpression,
    ) -> Option<Self> {
        use proximadb_filter_expression::{ComparisonOperator, FilterExpression};

        fn convert_condition(expr: &FilterExpression) -> Option<FilterCondition> {
            match expr {
                FilterExpression::Comparison {
                    field,
                    operator,
                    value,
                } => {
                    match operator {
                        ComparisonOperator::Equals => {
                            Some(FilterCondition::Equals(field.clone(), value.clone()))
                        }
                        ComparisonOperator::In => {
                            if let Some(arr) = value.as_array() {
                                Some(FilterCondition::In(field.clone(), arr.clone()))
                            } else {
                                Some(FilterCondition::In(field.clone(), vec![value.clone()]))
                            }
                        }
                        ComparisonOperator::Between => {
                            // Between expects an array of [min, max]
                            if let Some(arr) = value.as_array() {
                                if arr.len() >= 2 {
                                    Some(FilterCondition::Range(
                                        field.clone(),
                                        arr[0].clone(),
                                        arr[1].clone(),
                                    ))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }
                        ComparisonOperator::GreaterThan
                        | ComparisonOperator::GreaterThanOrEqual => {
                            // Range with open upper bound (use MAX values)
                            let max_val = serde_json::json!(f64::MAX);
                            Some(FilterCondition::Range(
                                field.clone(),
                                value.clone(),
                                max_val,
                            ))
                        }
                        ComparisonOperator::LessThan | ComparisonOperator::LessThanOrEqual => {
                            // Range with open lower bound (use MIN values)
                            let min_val = serde_json::json!(f64::MIN);
                            Some(FilterCondition::Range(
                                field.clone(),
                                min_val,
                                value.clone(),
                            ))
                        }
                        ComparisonOperator::IsNull => Some(FilterCondition::IsNull(field.clone())),
                        ComparisonOperator::IsNotNull => {
                            Some(FilterCondition::IsNotNull(field.clone()))
                        }
                        _ => None, // NotEquals, NotIn, Contains, StartsWith, EndsWith, Like not directly supported
                    }
                }
                _ => None, // And, Or, Not handled at top level
            }
        }

        fn collect_conditions(
            expr: &FilterExpression,
            conditions: &mut Vec<FilterCondition>,
            logic: &mut FilterLogic,
        ) {
            match expr {
                FilterExpression::And(exprs) => {
                    *logic = FilterLogic::And;
                    for e in exprs {
                        if let Some(cond) = convert_condition(e) {
                            conditions.push(cond);
                        } else {
                            // Recursively handle nested And/Or
                            collect_conditions(e, conditions, logic);
                        }
                    }
                }
                FilterExpression::Or(exprs) => {
                    *logic = FilterLogic::Or;
                    for e in exprs {
                        if let Some(cond) = convert_condition(e) {
                            conditions.push(cond);
                        } else {
                            collect_conditions(e, conditions, logic);
                        }
                    }
                }
                FilterExpression::Comparison { .. } => {
                    if let Some(cond) = convert_condition(expr) {
                        conditions.push(cond);
                    }
                }
                FilterExpression::Not(_) => {
                    // NOT expressions can't be easily converted to ColumnarMetadataFilter
                    // Skip them for now
                }
            }
        }

        let mut conditions = Vec::new();
        let mut logic = FilterLogic::And;

        collect_conditions(expr, &mut conditions, &mut logic);

        if conditions.is_empty() {
            None
        } else {
            Some(ColumnarMetadataFilter { conditions, logic })
        }
    }
}
