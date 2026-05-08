/// Lowered declarative graph-query descriptor shared by cross-model IR and
/// graph-subset execution adapters.
///
/// This is a contract shape only. Parsing, validation, row shaping, and runtime
/// execution stay in `proximadb-graph-subset`.
#[derive(Debug, Clone)]
pub struct SupportedGraphQueryDescriptor {
    pub graph_name: String,
    pub normalized_query: String,
    pub output_columns: Vec<String>,
    pub uses_legacy_node_rows: bool,
    pub max_depth: u32,
}

impl SupportedGraphQueryDescriptor {
    pub fn new(
        graph_name: String,
        normalized_query: String,
        output_columns: Vec<String>,
        uses_legacy_node_rows: bool,
        max_depth: u32,
    ) -> Self {
        Self {
            graph_name,
            normalized_query,
            output_columns,
            uses_legacy_node_rows,
            max_depth,
        }
    }

    pub fn graph_id(&self) -> &str {
        &self.graph_name
    }

    pub fn normalized_query(&self) -> &str {
        &self.normalized_query
    }

    pub fn output_columns(&self) -> &[String] {
        &self.output_columns
    }

    pub fn uses_legacy_node_rows(&self) -> bool {
        self.uses_legacy_node_rows
    }

    pub fn max_depth(&self) -> u32 {
        self.max_depth
    }
}

pub type LoweredGraphQuery = SupportedGraphQueryDescriptor;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_accessors_preserve_lowered_contract() {
        let descriptor = SupportedGraphQueryDescriptor::new(
            "knowledge".to_string(),
            "MATCH (n) RETURN n".to_string(),
            vec!["node_id".to_string()],
            true,
            2,
        );

        assert_eq!(descriptor.graph_id(), "knowledge");
        assert_eq!(descriptor.normalized_query(), "MATCH (n) RETURN n");
        assert_eq!(descriptor.output_columns(), &["node_id".to_string()]);
        assert!(descriptor.uses_legacy_node_rows());
        assert_eq!(descriptor.max_depth(), 2);
    }
}
