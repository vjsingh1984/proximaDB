// JSON path expression parser
//
// Parses and evaluates JSON path expressions like:
// - $.user.name - Object property access
// - $.items[0] - Array index access
// - $.items[*] - All array elements
// - $.items[?(@.price > 10)] - Filter expressions

use anyhow::{Result, anyhow};

use crate::proto::proximadb_v1::{SqlObject, SqlValue, sql_value::Value as SqlValueVariant};

/// Parsed path segment
#[derive(Debug, Clone)]
pub enum PathSegment {
    /// Root ($)
    Root,
    /// Property access (.name or ['name'])
    Property(String),
    /// Array index ([0])
    Index(usize),
    /// All array elements ([*])
    Wildcard,
    /// Recursive descent (..)
    Recursive,
    /// Filter expression ([?(...)])
    Filter(FilterExpr),
}

/// Filter expression for array filtering
#[derive(Debug, Clone)]
pub enum FilterExpr {
    /// Comparison: @.field op value
    Comparison {
        path: Vec<PathSegment>,
        operator: ComparisonOp,
        value: JsonPathValue,
    },
    /// Logical AND
    And(Box<FilterExpr>, Box<FilterExpr>),
    /// Logical OR
    Or(Box<FilterExpr>, Box<FilterExpr>),
    /// Logical NOT
    Not(Box<FilterExpr>),
    /// Existence check
    Exists(Vec<PathSegment>),
}

/// Comparison operators
#[derive(Debug, Clone, Copy)]
pub enum ComparisonOp {
    Eq,  // ==
    Ne,  // !=
    Lt,  // <
    Lte, // <=
    Gt,  // >
    Gte, // >=
}

/// Value in a JSON path expression
#[derive(Debug, Clone)]
pub enum JsonPathValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
}

/// Parsed JSON path
#[derive(Debug, Clone)]
pub struct JsonPath {
    pub segments: Vec<PathSegment>,
}

impl JsonPath {
    /// Parse a JSON path expression
    pub fn parse(path: &str) -> Result<Self> {
        let path = path.trim();

        // Handle empty path
        if path.is_empty() {
            return Ok(Self { segments: vec![] });
        }

        // Must start with $ (root)
        if !path.starts_with('$') {
            return Err(anyhow!("JSON path must start with '$'"));
        }

        let mut segments = vec![PathSegment::Root];
        let chars: Vec<char> = path.chars().collect();
        let mut i = 1; // Skip $

        while i < chars.len() {
            match chars[i] {
                '.' => {
                    i += 1;
                    if i < chars.len() && chars[i] == '.' {
                        // Recursive descent
                        segments.push(PathSegment::Recursive);
                        i += 1;
                    } else {
                        // Property access
                        let (prop, new_i) = Self::parse_property(&chars, i)?;
                        segments.push(PathSegment::Property(prop));
                        i = new_i;
                    }
                }
                '[' => {
                    i += 1;
                    if i < chars.len() {
                        if chars[i] == '*' {
                            // Wildcard
                            segments.push(PathSegment::Wildcard);
                            i += 1;
                            if i < chars.len() && chars[i] == ']' {
                                i += 1;
                            }
                        } else if chars[i] == '?' {
                            // Filter expression
                            let (filter, new_i) = Self::parse_filter(&chars, i + 1)?;
                            segments.push(PathSegment::Filter(filter));
                            i = new_i;
                        } else if chars[i] == '\'' || chars[i] == '"' {
                            // Quoted property
                            let quote = chars[i];
                            i += 1;
                            let start = i;
                            while i < chars.len() && chars[i] != quote {
                                i += 1;
                            }
                            let prop: String = chars[start..i].iter().collect();
                            segments.push(PathSegment::Property(prop));
                            i += 1; // Skip closing quote
                            if i < chars.len() && chars[i] == ']' {
                                i += 1;
                            }
                        } else if chars[i].is_ascii_digit() {
                            // Array index
                            let start = i;
                            while i < chars.len() && chars[i].is_ascii_digit() {
                                i += 1;
                            }
                            let idx_str: String = chars[start..i].iter().collect();
                            let idx: usize = idx_str.parse()?;
                            segments.push(PathSegment::Index(idx));
                            if i < chars.len() && chars[i] == ']' {
                                i += 1;
                            }
                        }
                    }
                }
                _ => {
                    // Unexpected character
                    return Err(anyhow!("Unexpected character in JSON path: {}", chars[i]));
                }
            }
        }

        Ok(Self { segments })
    }

    /// Parse a property name
    fn parse_property(chars: &[char], start: usize) -> Result<(String, usize)> {
        let mut i = start;
        while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
            i += 1;
        }
        let prop: String = chars[start..i].iter().collect();
        if prop.is_empty() {
            return Err(anyhow!("Empty property name"));
        }
        Ok((prop, i))
    }

    /// Parse a filter expression
    fn parse_filter(chars: &[char], start: usize) -> Result<(FilterExpr, usize)> {
        // Simplified filter parsing - just skip to closing bracket for now
        let mut i = start;
        let mut depth = 1;
        while i < chars.len() && depth > 0 {
            match chars[i] {
                '(' => depth += 1,
                ')' => depth -= 1,
                ']' if depth == 1 => break,
                _ => {}
            }
            i += 1;
        }

        // Skip closing ]
        if i < chars.len() && chars[i] == ']' {
            i += 1;
        }

        // For now, return a placeholder filter
        Ok((FilterExpr::Exists(vec![]), i))
    }

    /// Evaluate the path against a SqlObject
    pub fn evaluate(&self, document: &SqlObject) -> Vec<SqlValue> {
        self.evaluate_from_value(&SqlValue {
            value: Some(SqlValueVariant::ObjectValue(document.clone())),
        })
    }

    /// Evaluate from a SqlValue
    fn evaluate_from_value(&self, value: &SqlValue) -> Vec<SqlValue> {
        let mut current = vec![value.clone()];

        for segment in &self.segments {
            if current.is_empty() {
                break;
            }

            current = match segment {
                PathSegment::Root => current, // Already at root
                PathSegment::Property(name) => current
                    .into_iter()
                    .filter_map(|v| self.get_property(&v, name))
                    .collect(),
                PathSegment::Index(idx) => current
                    .into_iter()
                    .filter_map(|v| self.get_index(&v, *idx))
                    .collect(),
                PathSegment::Wildcard => current
                    .into_iter()
                    .flat_map(|v| self.get_all_elements(&v))
                    .collect(),
                PathSegment::Recursive => {
                    // Collect all nested values
                    let mut all = Vec::new();
                    for v in current {
                        self.collect_recursive(&v, &mut all);
                    }
                    all
                }
                PathSegment::Filter(_) => {
                    // TODO: Implement filter evaluation
                    current
                }
            };
        }

        current
    }

    /// Get a property from a SqlValue
    fn get_property(&self, value: &SqlValue, name: &str) -> Option<SqlValue> {
        if let Some(SqlValueVariant::ObjectValue(obj)) = &value.value {
            obj.fields.get(name).cloned()
        } else {
            None
        }
    }

    /// Get an array element by index
    fn get_index(&self, value: &SqlValue, idx: usize) -> Option<SqlValue> {
        if let Some(SqlValueVariant::ArrayValue(arr)) = &value.value {
            arr.values.get(idx).cloned()
        } else {
            None
        }
    }

    /// Get all elements from an array
    fn get_all_elements(&self, value: &SqlValue) -> Vec<SqlValue> {
        if let Some(SqlValueVariant::ArrayValue(arr)) = &value.value {
            arr.values.clone()
        } else {
            vec![]
        }
    }

    /// Recursively collect all nested values
    fn collect_recursive(&self, value: &SqlValue, results: &mut Vec<SqlValue>) {
        results.push(value.clone());

        match &value.value {
            Some(SqlValueVariant::ObjectValue(obj)) => {
                for v in obj.fields.values() {
                    self.collect_recursive(v, results);
                }
            }
            Some(SqlValueVariant::ArrayValue(arr)) => {
                for v in &arr.values {
                    self.collect_recursive(v, results);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_path() {
        let path = JsonPath::parse("$.user.name").unwrap();
        assert_eq!(path.segments.len(), 3);
        assert!(matches!(path.segments[0], PathSegment::Root));
        assert!(matches!(&path.segments[1], PathSegment::Property(s) if s == "user"));
        assert!(matches!(&path.segments[2], PathSegment::Property(s) if s == "name"));
    }

    #[test]
    fn test_parse_array_index() {
        let path = JsonPath::parse("$.items[0]").unwrap();
        assert_eq!(path.segments.len(), 3);
        assert!(matches!(path.segments[2], PathSegment::Index(0)));
    }

    #[test]
    fn test_parse_wildcard() {
        let path = JsonPath::parse("$.items[*]").unwrap();
        assert_eq!(path.segments.len(), 3);
        assert!(matches!(path.segments[2], PathSegment::Wildcard));
    }

    #[test]
    fn test_parse_quoted_property() {
        let path = JsonPath::parse("$['special-name']").unwrap();
        assert_eq!(path.segments.len(), 2);
        assert!(matches!(&path.segments[1], PathSegment::Property(s) if s == "special-name"));
    }
}
