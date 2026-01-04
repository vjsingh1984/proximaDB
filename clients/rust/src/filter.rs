//! Filter builder for complex query filtering
//!
//! Provides a fluent API for building metadata filters on vector searches.
//!
//! # Examples
//!
//! ```rust,ignore
//! use proximadb_sdk::{FilterBuilder, FilterOp};
//!
//! // Simple equality filter
//! let filter = FilterBuilder::new()
//!     .eq("category", "tech")
//!     .build();
//!
//! // Range filter
//! let filter = FilterBuilder::new()
//!     .gte("price", 100)
//!     .lte("price", 500)
//!     .build();
//!
//! // Complex filter with AND/OR
//! let filter = FilterBuilder::new()
//!     .eq("status", "active")
//!     .and()
//!     .group(|f| f.eq("category", "tech").or().eq("category", "science"))
//!     .build();
//!
//! // Use with search
//! let results = client.collection("items")
//!     .search()
//!     .vector(&query)
//!     .with_filter(filter)
//!     .execute()
//!     .await?;
//! ```

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

/// Filter operations
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    /// Equal to
    Eq,
    /// Not equal to
    Ne,
    /// Greater than
    Gt,
    /// Greater than or equal
    Gte,
    /// Less than
    Lt,
    /// Less than or equal
    Lte,
    /// In list of values
    In,
    /// Not in list of values
    NotIn,
    /// Contains (for strings/arrays)
    Contains,
    /// Starts with (for strings)
    StartsWith,
    /// Ends with (for strings)
    EndsWith,
    /// Field exists
    Exists,
    /// Field is null
    IsNull,
}

impl FilterOp {
    /// Convert to API string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            FilterOp::Eq => "equals",
            FilterOp::Ne => "not_equals",
            FilterOp::Gt => "gt",
            FilterOp::Gte => "gte",
            FilterOp::Lt => "lt",
            FilterOp::Lte => "lte",
            FilterOp::In => "in",
            FilterOp::NotIn => "not_in",
            FilterOp::Contains => "contains",
            FilterOp::StartsWith => "starts_with",
            FilterOp::EndsWith => "ends_with",
            FilterOp::Exists => "exists",
            FilterOp::IsNull => "is_null",
        }
    }
}

/// Logical operators for combining conditions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogicalOp {
    /// All conditions must match
    And,
    /// Any condition must match
    Or,
}

/// A single filter condition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterCondition {
    /// Field name
    pub field: String,
    /// Operation
    pub operation: String,
    /// Value to compare (optional for exists/is_null)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

impl FilterCondition {
    /// Create a new filter condition
    pub fn new(field: impl Into<String>, op: FilterOp, value: impl Into<Value>) -> Self {
        Self {
            field: field.into(),
            operation: op.as_str().to_string(),
            value: Some(value.into()),
        }
    }

    /// Create a condition without a value (for exists, is_null)
    pub fn new_unary(field: impl Into<String>, op: FilterOp) -> Self {
        Self {
            field: field.into(),
            operation: op.as_str().to_string(),
            value: None,
        }
    }
}

/// A filter group containing conditions and nested groups
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterGroup {
    /// Logical operator for combining conditions
    pub operator: String,
    /// List of conditions
    pub conditions: Vec<FilterNode>,
}

/// A node in the filter tree (either a condition or a group)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FilterNode {
    /// A single condition
    Condition(FilterCondition),
    /// A nested group of conditions
    Group(FilterGroup),
}

/// The compiled filter expression
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Filter {
    /// Root filter group
    #[serde(flatten)]
    pub root: FilterGroup,
}

impl Filter {
    /// Convert to JSON string for API
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Convert to filter expression string (simple cases)
    pub fn to_expression(&self) -> String {
        self.root.to_expression()
    }
}

impl FilterGroup {
    /// Convert group to expression string
    fn to_expression(&self) -> String {
        let exprs: Vec<String> = self
            .conditions
            .iter()
            .filter_map(|node| match node {
                FilterNode::Condition(c) => Some(c.to_expression()),
                FilterNode::Group(g) => Some(format!("({})", g.to_expression())),
            })
            .collect();

        let sep = if self.operator == "and" {
            " AND "
        } else {
            " OR "
        };
        exprs.join(sep)
    }
}

impl FilterCondition {
    /// Convert condition to expression string
    fn to_expression(&self) -> String {
        match self.value.as_ref() {
            Some(v) => {
                let val_str = match v {
                    Value::String(s) => format!("'{}'", s),
                    Value::Array(arr) => {
                        let items: Vec<String> = arr
                            .iter()
                            .map(|x| match x {
                                Value::String(s) => format!("'{}'", s),
                                _ => x.to_string(),
                            })
                            .collect();
                        format!("[{}]", items.join(", "))
                    }
                    _ => v.to_string(),
                };

                let op_str = match self.operation.as_str() {
                    "equals" => "=",
                    "not_equals" => "!=",
                    "gt" => ">",
                    "gte" => ">=",
                    "lt" => "<",
                    "lte" => "<=",
                    "in" => "IN",
                    "not_in" => "NOT IN",
                    "contains" => "CONTAINS",
                    "starts_with" => "STARTS WITH",
                    "ends_with" => "ENDS WITH",
                    _ => &self.operation,
                };

                format!("{} {} {}", self.field, op_str, val_str)
            }
            None => {
                let op_str = match self.operation.as_str() {
                    "exists" => "EXISTS",
                    "is_null" => "IS NULL",
                    _ => &self.operation,
                };
                format!("{} {}", self.field, op_str)
            }
        }
    }
}

impl fmt::Display for Filter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_expression())
    }
}

/// Builder for constructing complex filter expressions
///
/// # Examples
///
/// ```rust,ignore
/// use proximadb_sdk::FilterBuilder;
///
/// // Simple filter
/// let filter = FilterBuilder::new()
///     .eq("status", "active")
///     .build();
///
/// // Complex filter
/// let filter = FilterBuilder::new()
///     .eq("category", "tech")
///     .and()
///     .gte("price", 100)
///     .lte("price", 500)
///     .or()
///     .in_list("tags", vec!["featured", "popular"])
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct FilterBuilder {
    conditions: Vec<FilterNode>,
    current_operator: LogicalOp,
    pending_operator: Option<LogicalOp>,
}

impl FilterBuilder {
    /// Create a new filter builder
    pub fn new() -> Self {
        Self {
            conditions: Vec::new(),
            current_operator: LogicalOp::And,
            pending_operator: None,
        }
    }

    /// Add an equality condition
    pub fn eq(mut self, field: impl Into<String>, value: impl Into<Value>) -> Self {
        self.add_condition(FilterCondition::new(field, FilterOp::Eq, value));
        self
    }

    /// Add a not-equal condition
    pub fn ne(mut self, field: impl Into<String>, value: impl Into<Value>) -> Self {
        self.add_condition(FilterCondition::new(field, FilterOp::Ne, value));
        self
    }

    /// Add a greater-than condition
    pub fn gt(mut self, field: impl Into<String>, value: impl Into<Value>) -> Self {
        self.add_condition(FilterCondition::new(field, FilterOp::Gt, value));
        self
    }

    /// Add a greater-than-or-equal condition
    pub fn gte(mut self, field: impl Into<String>, value: impl Into<Value>) -> Self {
        self.add_condition(FilterCondition::new(field, FilterOp::Gte, value));
        self
    }

    /// Add a less-than condition
    pub fn lt(mut self, field: impl Into<String>, value: impl Into<Value>) -> Self {
        self.add_condition(FilterCondition::new(field, FilterOp::Lt, value));
        self
    }

    /// Add a less-than-or-equal condition
    pub fn lte(mut self, field: impl Into<String>, value: impl Into<Value>) -> Self {
        self.add_condition(FilterCondition::new(field, FilterOp::Lte, value));
        self
    }

    /// Add an IN condition (value in list)
    pub fn in_list<V: Into<Value>>(mut self, field: impl Into<String>, values: Vec<V>) -> Self {
        let arr: Vec<Value> = values.into_iter().map(|v| v.into()).collect();
        self.add_condition(FilterCondition::new(field, FilterOp::In, Value::Array(arr)));
        self
    }

    /// Add a NOT IN condition
    pub fn not_in<V: Into<Value>>(mut self, field: impl Into<String>, values: Vec<V>) -> Self {
        let arr: Vec<Value> = values.into_iter().map(|v| v.into()).collect();
        self.add_condition(FilterCondition::new(
            field,
            FilterOp::NotIn,
            Value::Array(arr),
        ));
        self
    }

    /// Add a contains condition (for strings/arrays)
    pub fn contains(mut self, field: impl Into<String>, value: impl Into<Value>) -> Self {
        self.add_condition(FilterCondition::new(field, FilterOp::Contains, value));
        self
    }

    /// Add a starts-with condition
    pub fn starts_with(mut self, field: impl Into<String>, prefix: impl Into<String>) -> Self {
        self.add_condition(FilterCondition::new(field, FilterOp::StartsWith, prefix.into()));
        self
    }

    /// Add an ends-with condition
    pub fn ends_with(mut self, field: impl Into<String>, suffix: impl Into<String>) -> Self {
        self.add_condition(FilterCondition::new(field, FilterOp::EndsWith, suffix.into()));
        self
    }

    /// Add an exists condition (field is present)
    pub fn exists(mut self, field: impl Into<String>) -> Self {
        self.add_condition(FilterCondition::new_unary(field, FilterOp::Exists));
        self
    }

    /// Add an is-null condition
    pub fn is_null(mut self, field: impl Into<String>) -> Self {
        self.add_condition(FilterCondition::new_unary(field, FilterOp::IsNull));
        self
    }

    /// Add a range condition (inclusive)
    pub fn range<V: Into<Value> + Clone>(
        mut self,
        field: impl Into<String>,
        min: V,
        max: V,
    ) -> Self {
        let field = field.into();
        self.add_condition(FilterCondition::new(&field, FilterOp::Gte, min.into()));
        self.add_condition(FilterCondition::new(&field, FilterOp::Lte, max.into()));
        self
    }

    /// Set the next operator to AND
    pub fn and(mut self) -> Self {
        self.pending_operator = Some(LogicalOp::And);
        self
    }

    /// Set the next operator to OR
    pub fn or(mut self) -> Self {
        self.pending_operator = Some(LogicalOp::Or);
        self
    }

    /// Add a nested group of conditions
    pub fn group<F>(mut self, builder_fn: F) -> Self
    where
        F: FnOnce(FilterBuilder) -> FilterBuilder,
    {
        let inner = builder_fn(FilterBuilder::new());
        let filter = inner.build();
        self.conditions.push(FilterNode::Group(filter.root));
        self
    }

    /// Add a condition to the list
    fn add_condition(&mut self, condition: FilterCondition) {
        // Handle pending operator (for OR conditions)
        if let Some(op) = self.pending_operator.take() {
            if op == LogicalOp::Or && self.current_operator == LogicalOp::And {
                // Need to restructure for OR
                self.current_operator = LogicalOp::Or;
            }
        }
        self.conditions.push(FilterNode::Condition(condition));
    }

    /// Build the filter expression
    pub fn build(self) -> Filter {
        let operator = match self.current_operator {
            LogicalOp::And => "and",
            LogicalOp::Or => "or",
        };

        Filter {
            root: FilterGroup {
                operator: operator.to_string(),
                conditions: self.conditions,
            },
        }
    }

    /// Build and convert to expression string
    pub fn to_expression(self) -> String {
        self.build().to_expression()
    }
}

impl Default for FilterBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// Convenience functions for common filter patterns

/// Create an equality filter
pub fn eq(field: impl Into<String>, value: impl Into<Value>) -> Filter {
    FilterBuilder::new().eq(field, value).build()
}

/// Create a not-equal filter
pub fn ne(field: impl Into<String>, value: impl Into<Value>) -> Filter {
    FilterBuilder::new().ne(field, value).build()
}

/// Create an IN filter
pub fn in_list<V: Into<Value>>(field: impl Into<String>, values: Vec<V>) -> Filter {
    FilterBuilder::new().in_list(field, values).build()
}

/// Create a range filter
pub fn range<V: Into<Value> + Clone>(field: impl Into<String>, min: V, max: V) -> Filter {
    FilterBuilder::new().range(field, min, max).build()
}

/// Create an AND filter combining multiple filters
pub fn and_filters(filters: Vec<Filter>) -> Filter {
    let conditions: Vec<FilterNode> = filters
        .into_iter()
        .flat_map(|f| f.root.conditions)
        .collect();

    Filter {
        root: FilterGroup {
            operator: "and".to_string(),
            conditions,
        },
    }
}

/// Create an OR filter combining multiple filters
pub fn or_filters(filters: Vec<Filter>) -> Filter {
    let conditions: Vec<FilterNode> = filters.into_iter().map(|f| FilterNode::Group(f.root)).collect();

    Filter {
        root: FilterGroup {
            operator: "or".to_string(),
            conditions,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_eq_filter() {
        let filter = FilterBuilder::new().eq("category", "tech").build();
        assert_eq!(filter.to_expression(), "category = 'tech'");
    }

    #[test]
    fn test_numeric_filter() {
        let filter = FilterBuilder::new().gt("price", 100).build();
        assert_eq!(filter.to_expression(), "price > 100");
    }

    #[test]
    fn test_range_filter() {
        let filter = FilterBuilder::new().range("price", 100, 500).build();
        assert_eq!(filter.to_expression(), "price >= 100 AND price <= 500");
    }

    #[test]
    fn test_in_list_filter() {
        let filter = FilterBuilder::new()
            .in_list("status", vec!["active", "pending"])
            .build();
        assert_eq!(filter.to_expression(), "status IN ['active', 'pending']");
    }

    #[test]
    fn test_and_conditions() {
        let filter = FilterBuilder::new()
            .eq("category", "tech")
            .gte("rating", 4)
            .build();
        assert_eq!(
            filter.to_expression(),
            "category = 'tech' AND rating >= 4"
        );
    }

    #[test]
    fn test_exists_filter() {
        let filter = FilterBuilder::new().exists("thumbnail").build();
        assert_eq!(filter.to_expression(), "thumbnail EXISTS");
    }

    #[test]
    fn test_convenience_eq() {
        let filter = eq("status", "active");
        assert_eq!(filter.to_expression(), "status = 'active'");
    }

    #[test]
    fn test_convenience_range() {
        let filter = range("price", 0, 100);
        assert_eq!(filter.to_expression(), "price >= 0 AND price <= 100");
    }
}
