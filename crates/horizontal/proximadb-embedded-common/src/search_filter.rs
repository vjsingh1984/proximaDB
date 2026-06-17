//! Lightweight embedded-mode search filter helpers.
//!
//! This module intentionally supports only simple equality predicates that can
//! be represented by embedded vector search surfaces without depending on the
//! root query parser.

/// Parse a simple filter string into `(field, value)` predicate pairs.
///
/// Supported forms:
/// - `field = 'value'`
/// - `field == "value"`
/// - `field = value AND other == value2`
pub fn parse_vector_filter(filter: &str) -> Vec<(String, String)> {
    filter
        .split(" AND ")
        .filter_map(|part| {
            let part = part.trim();
            let (key, rest) = part.split_once(" == ").or_else(|| part.split_once(" = "))?;
            let val = rest.trim().trim_matches('\'').trim_matches('"');
            Some((key.trim().to_string(), val.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse_vector_filter;

    #[test]
    fn parses_single_quoted_equality() {
        assert_eq!(
            parse_vector_filter("tenant = 'acme'"),
            vec![("tenant".to_string(), "acme".to_string())]
        );
    }

    #[test]
    fn parses_double_equals_and_double_quotes() {
        assert_eq!(
            parse_vector_filter("kind == \"invoice\""),
            vec![("kind".to_string(), "invoice".to_string())]
        );
    }

    #[test]
    fn parses_and_joined_predicates() {
        assert_eq!(
            parse_vector_filter("tenant = 'acme' AND kind == \"invoice\""),
            vec![
                ("tenant".to_string(), "acme".to_string()),
                ("kind".to_string(), "invoice".to_string()),
            ]
        );
    }

    #[test]
    fn ignores_unsupported_predicates() {
        assert!(parse_vector_filter("score > 10").is_empty());
    }
}
