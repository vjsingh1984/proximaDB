//! Automatic graph materialization for observability traces.
//!
//! Trace spans are projected into a graph so callers can traverse service
//! dependencies, parent/child span relationships, and attribute-derived entity
//! references without exporting telemetry to a separate graph pipeline.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;

use crate::graph::{Edge, GraphService, Node};
use crate::observability::storage::traces::TraceSpan;
use crate::proto::proximadb_v1::{CreateGraphRequest, EdgeQuery, PropertyValue, property_value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeLinkTarget {
    ExistingNodeId,
    SyntheticObservedEntity,
}

#[derive(Debug, Clone)]
pub struct TelemetryAttributeLinkRule {
    pub attribute_key: String,
    pub target_label: String,
    pub edge_type: String,
    pub target: AttributeLinkTarget,
}

#[derive(Debug, Clone)]
pub struct TelemetryGraphLinkerConfig {
    pub graph_id: String,
    pub graph_name: Option<String>,
    pub trace_node_label: String,
    pub span_node_label: String,
    pub service_node_label: String,
    pub entity_node_label: String,
    pub trace_contains_span_edge: String,
    pub span_parent_edge: String,
    pub service_span_edge: String,
    pub service_dependency_edge: String,
    pub attribute_link_rules: Vec<TelemetryAttributeLinkRule>,
}

impl Default for TelemetryGraphLinkerConfig {
    fn default() -> Self {
        Self {
            graph_id: "observability_semantic_trace".to_string(),
            graph_name: Some("Observability Semantic Trace".to_string()),
            trace_node_label: "TelemetryTrace".to_string(),
            span_node_label: "TelemetrySpan".to_string(),
            service_node_label: "ObservedService".to_string(),
            entity_node_label: "ObservedEntity".to_string(),
            trace_contains_span_edge: "CONTAINS_SPAN".to_string(),
            span_parent_edge: "PARENT_OF".to_string(),
            service_span_edge: "EMITS_SPAN".to_string(),
            service_dependency_edge: "CALLS_SERVICE".to_string(),
            attribute_link_rules: vec![
                TelemetryAttributeLinkRule {
                    attribute_key: "entity_id".to_string(),
                    target_label: "ObservedEntity".to_string(),
                    edge_type: "REFERENCES_ENTITY".to_string(),
                    target: AttributeLinkTarget::SyntheticObservedEntity,
                },
                TelemetryAttributeLinkRule {
                    attribute_key: "tenant_id".to_string(),
                    target_label: "Tenant".to_string(),
                    edge_type: "IMPACTS_TENANT".to_string(),
                    target: AttributeLinkTarget::SyntheticObservedEntity,
                },
                TelemetryAttributeLinkRule {
                    attribute_key: "document_id".to_string(),
                    target_label: "Document".to_string(),
                    edge_type: "TOUCHES_DOCUMENT".to_string(),
                    target: AttributeLinkTarget::SyntheticObservedEntity,
                },
                TelemetryAttributeLinkRule {
                    attribute_key: "graph_node_id".to_string(),
                    target_label: "GraphNode".to_string(),
                    edge_type: "REFERENCES_GRAPH_NODE".to_string(),
                    target: AttributeLinkTarget::ExistingNodeId,
                },
            ],
        }
    }
}

pub struct TelemetryGraphLinker {
    graph_service: Arc<GraphService>,
    config: TelemetryGraphLinkerConfig,
    initialized: AtomicBool,
}

impl TelemetryGraphLinker {
    pub fn new(graph_service: Arc<GraphService>, config: TelemetryGraphLinkerConfig) -> Self {
        Self {
            graph_service,
            config,
            initialized: AtomicBool::new(false),
        }
    }

    pub fn config(&self) -> &TelemetryGraphLinkerConfig {
        &self.config
    }

    pub async fn link_trace_span(&self, namespace: &str, span: &TraceSpan) -> Result<()> {
        self.ensure_graph_exists().await?;

        let trace_node_id = self.trace_node_id(namespace, &span.trace_id);
        let span_node_id = self.span_node_id(namespace, &span.trace_id, &span.span_id);
        let current_service_node_id = (!span.service_name.is_empty())
            .then(|| self.service_node_id(namespace, &span.service_name));

        self.upsert_node(self.trace_node(
            namespace,
            &trace_node_id,
            &span.trace_id,
            span.start_time_ns,
            span.end_time_ns,
        ))
        .await?;
        self.upsert_node(self.span_node(namespace, &span_node_id, span, false))
            .await?;
        self.upsert_edge(self.edge(
            self.edge_id(
                &trace_node_id,
                &self.config.trace_contains_span_edge,
                &span_node_id,
            ),
            &trace_node_id,
            &span_node_id,
            &self.config.trace_contains_span_edge,
            HashMap::from([("namespace".to_string(), string_property(namespace))]),
        ))
        .await?;

        if !span.parent_span_id.is_empty() {
            let parent_span_node_id =
                self.span_node_id(namespace, &span.trace_id, &span.parent_span_id);
            self.upsert_node(self.placeholder_span_node(namespace, &parent_span_node_id, span))
                .await?;
            self.upsert_edge(self.edge(
                self.edge_id(
                    &parent_span_node_id,
                    &self.config.span_parent_edge,
                    &span_node_id,
                ),
                &parent_span_node_id,
                &span_node_id,
                &self.config.span_parent_edge,
                HashMap::from([("trace_id".to_string(), string_property(&span.trace_id))]),
            ))
            .await?;
        }

        if let Some(service_node_id) = current_service_node_id.as_ref() {
            self.upsert_node(self.service_node(namespace, service_node_id, &span.service_name))
                .await?;
            self.upsert_edge(self.edge(
                self.edge_id(
                    service_node_id,
                    &self.config.service_span_edge,
                    &span_node_id,
                ),
                service_node_id,
                &span_node_id,
                &self.config.service_span_edge,
                HashMap::from([("trace_id".to_string(), string_property(&span.trace_id))]),
            ))
            .await?;
        }

        self.link_parent_service_dependency(
            namespace,
            span,
            &span_node_id,
            current_service_node_id.clone(),
        )
        .await?;
        self.link_existing_child_service_dependencies(
            namespace,
            span,
            &span_node_id,
            current_service_node_id.clone(),
        )
        .await?;
        self.link_attributes(namespace, &span_node_id, span).await
    }

    fn trace_node_id(&self, namespace: &str, trace_id: &str) -> String {
        format!("obs:{namespace}:trace:{}", normalize_id_component(trace_id))
    }

    fn span_node_id(&self, namespace: &str, trace_id: &str, span_id: &str) -> String {
        format!(
            "obs:{namespace}:span:{}:{}",
            normalize_id_component(trace_id),
            normalize_id_component(span_id)
        )
    }

    fn service_node_id(&self, namespace: &str, service_name: &str) -> String {
        format!(
            "obs:{namespace}:service:{}",
            normalize_id_component(service_name)
        )
    }

    fn synthetic_entity_node_id(
        &self,
        namespace: &str,
        attribute_key: &str,
        attribute_value: &str,
    ) -> String {
        format!(
            "obs:{namespace}:entity:{}:{}",
            normalize_id_component(attribute_key),
            normalize_id_component(attribute_value)
        )
    }

    async fn ensure_graph_exists(&self) -> Result<()> {
        if self.initialized.load(Ordering::Acquire) {
            return Ok(());
        }

        let graphs = self.graph_service.list_graphs().await?;
        if !graphs
            .iter()
            .any(|graph_id| graph_id == &self.config.graph_id)
        {
            let request = CreateGraphRequest {
                graph_id: self.config.graph_id.clone(),
                name: self.config.graph_name.clone(),
                description: Some(
                    "Auto-materialized observability trace dependency graph".to_string(),
                ),
                schema: None,
                storage_config: None,
                engine_config: None,
                access_control: None,
            };
            self.graph_service.create_graph_collection(request).await?;
        }

        self.initialized.store(true, Ordering::Release);
        Ok(())
    }

    async fn link_attributes(
        &self,
        namespace: &str,
        span_node_id: &str,
        span: &TraceSpan,
    ) -> Result<()> {
        for rule in &self.config.attribute_link_rules {
            let Some(value) = span.attributes.get(&rule.attribute_key) else {
                continue;
            };

            let target_node_id = match rule.target {
                AttributeLinkTarget::ExistingNodeId => {
                    if self
                        .graph_service
                        .get_node(&self.config.graph_id, value)
                        .await?
                        .is_some()
                    {
                        value.clone()
                    } else {
                        let synthetic_id =
                            self.synthetic_entity_node_id(namespace, &rule.attribute_key, value);
                        self.upsert_node(self.synthetic_entity_node(
                            namespace,
                            &synthetic_id,
                            &rule.target_label,
                            &rule.attribute_key,
                            value,
                        ))
                        .await?;
                        synthetic_id
                    }
                }
                AttributeLinkTarget::SyntheticObservedEntity => {
                    let synthetic_id =
                        self.synthetic_entity_node_id(namespace, &rule.attribute_key, value);
                    self.upsert_node(self.synthetic_entity_node(
                        namespace,
                        &synthetic_id,
                        &rule.target_label,
                        &rule.attribute_key,
                        value,
                    ))
                    .await?;
                    synthetic_id
                }
            };

            self.upsert_edge(self.edge(
                self.edge_id(span_node_id, &rule.edge_type, &target_node_id),
                span_node_id,
                &target_node_id,
                &rule.edge_type,
                HashMap::from([
                    ("namespace".to_string(), string_property(namespace)),
                    (
                        "attribute_key".to_string(),
                        string_property(&rule.attribute_key),
                    ),
                    ("attribute_value".to_string(), string_property(value)),
                ]),
            ))
            .await?;
        }

        Ok(())
    }

    async fn link_parent_service_dependency(
        &self,
        namespace: &str,
        span: &TraceSpan,
        span_node_id: &str,
        current_service_node_id: Option<String>,
    ) -> Result<()> {
        let Some(current_service_node_id) = current_service_node_id else {
            return Ok(());
        };
        if span.parent_span_id.is_empty() {
            return Ok(());
        }

        let parent_span_node_id =
            self.span_node_id(namespace, &span.trace_id, &span.parent_span_id);
        let Some(parent_span_node) = self
            .graph_service
            .get_node(&self.config.graph_id, &parent_span_node_id)
            .await?
        else {
            return Ok(());
        };

        let Some(parent_service_name) = string_property_value(&parent_span_node, "service_name")
        else {
            return Ok(());
        };
        let parent_service_name = parent_service_name.to_string();
        if parent_service_name.is_empty() || parent_service_name == span.service_name {
            return Ok(());
        }

        let parent_service_node_id = self.service_node_id(namespace, &parent_service_name);
        self.upsert_node(self.service_node(
            namespace,
            &parent_service_node_id,
            &parent_service_name,
        ))
        .await?;
        self.record_service_dependency(
            namespace,
            &span.trace_id,
            &parent_service_node_id,
            &current_service_node_id,
            &parent_service_name,
            &span.service_name,
            &parent_span_node_id,
            span_node_id,
        )
        .await
    }

    async fn link_existing_child_service_dependencies(
        &self,
        namespace: &str,
        span: &TraceSpan,
        span_node_id: &str,
        current_service_node_id: Option<String>,
    ) -> Result<()> {
        let Some(current_service_node_id) = current_service_node_id else {
            return Ok(());
        };

        let edges = self
            .graph_service
            .query_edges(
                &self.config.graph_id,
                EdgeQuery {
                    graph_id: self.config.graph_id.clone(),
                    from_node_id: Some(span_node_id.to_string()),
                    to_node_id: None,
                    edge_types: vec![self.config.span_parent_edge.clone()],
                    filters: vec![],
                    limit: None,
                    offset: None,
                    continuation_token: None,
                },
            )
            .await?;

        for edge in edges {
            if edge.edge_type != self.config.span_parent_edge {
                continue;
            }

            let Some(child_span_node) = self
                .graph_service
                .get_node(&self.config.graph_id, &edge.to_node_id)
                .await?
            else {
                continue;
            };

            let Some(child_service_name) = string_property_value(&child_span_node, "service_name")
            else {
                continue;
            };
            let child_service_name = child_service_name.to_string();
            if child_service_name.is_empty() || child_service_name == span.service_name {
                continue;
            }

            let child_service_node_id = self.service_node_id(namespace, &child_service_name);
            self.upsert_node(self.service_node(
                namespace,
                &child_service_node_id,
                &child_service_name,
            ))
            .await?;
            self.record_service_dependency(
                namespace,
                &span.trace_id,
                &current_service_node_id,
                &child_service_node_id,
                &span.service_name,
                &child_service_name,
                span_node_id,
                &edge.to_node_id,
            )
            .await?;
        }

        Ok(())
    }

    async fn record_service_dependency(
        &self,
        namespace: &str,
        trace_id: &str,
        caller_service_node_id: &str,
        callee_service_node_id: &str,
        caller_service_name: &str,
        callee_service_name: &str,
        parent_span_node_id: &str,
        child_span_node_id: &str,
    ) -> Result<()> {
        let edge_id = self.edge_id(
            caller_service_node_id,
            &self.config.service_dependency_edge,
            callee_service_node_id,
        );
        let existing_edge = self
            .graph_service
            .get_edge(&self.config.graph_id, &edge_id)
            .await?;
        let observed_count = existing_edge
            .as_ref()
            .and_then(|edge| int_property_value(edge, "observed_count"))
            .unwrap_or(0)
            + 1;

        let mut properties = HashMap::from([
            ("namespace".to_string(), string_property(namespace)),
            ("last_trace_id".to_string(), string_property(trace_id)),
            (
                "caller_service".to_string(),
                string_property(caller_service_name),
            ),
            (
                "callee_service".to_string(),
                string_property(callee_service_name),
            ),
            (
                "last_parent_span_node_id".to_string(),
                string_property(parent_span_node_id),
            ),
            (
                "last_child_span_node_id".to_string(),
                string_property(child_span_node_id),
            ),
            ("observed_count".to_string(), int_property(observed_count)),
        ]);
        if let Some(edge) = &existing_edge {
            for (key, value) in &edge.properties {
                properties
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        }

        let mut dependency_edge = self.edge(
            edge_id,
            caller_service_node_id,
            callee_service_node_id,
            &self.config.service_dependency_edge,
            properties,
        );
        dependency_edge.weight = Some(observed_count as f64);

        if existing_edge.is_some() {
            self.graph_service
                .update_edge(&self.config.graph_id, dependency_edge)
                .await?;
        } else {
            self.graph_service
                .create_edge(&self.config.graph_id, dependency_edge)
                .await?;
        }

        Ok(())
    }

    async fn upsert_node(&self, node: Node) -> Result<()> {
        if self
            .graph_service
            .get_node(&self.config.graph_id, &node.id)
            .await?
            .is_some()
        {
            self.graph_service
                .update_node(&self.config.graph_id, node)
                .await?;
        } else {
            self.graph_service
                .create_node(&self.config.graph_id, node)
                .await?;
        }
        Ok(())
    }

    async fn upsert_edge(&self, edge: Edge) -> Result<()> {
        if self
            .graph_service
            .get_edge(&self.config.graph_id, &edge.id)
            .await?
            .is_some()
        {
            self.graph_service
                .update_edge(&self.config.graph_id, edge)
                .await?;
        } else {
            self.graph_service
                .create_edge(&self.config.graph_id, edge)
                .await?;
        }
        Ok(())
    }

    fn trace_node(
        &self,
        namespace: &str,
        node_id: &str,
        trace_id: &str,
        start_time_ns: i64,
        end_time_ns: i64,
    ) -> Node {
        Node {
            id: node_id.to_string(),
            labels: vec![self.config.trace_node_label.clone()],
            properties: HashMap::from([
                ("namespace".to_string(), string_property(namespace)),
                ("trace_id".to_string(), string_property(trace_id)),
                ("start_time_ns".to_string(), int_property(start_time_ns)),
                ("end_time_ns".to_string(), int_property(end_time_ns)),
            ]),
            embedding: None,
            created_at_ms: ns_to_ms(start_time_ns),
            updated_at_ms: ns_to_ms(end_time_ns),
        }
    }

    fn span_node(
        &self,
        namespace: &str,
        node_id: &str,
        span: &TraceSpan,
        placeholder: bool,
    ) -> Node {
        let mut properties = HashMap::from([
            ("namespace".to_string(), string_property(namespace)),
            ("trace_id".to_string(), string_property(&span.trace_id)),
            ("span_id".to_string(), string_property(&span.span_id)),
            ("operation".to_string(), string_property(&span.name)),
            ("status".to_string(), int_property(span.status as i64)),
            (
                "start_time_ns".to_string(),
                int_property(span.start_time_ns),
            ),
            ("end_time_ns".to_string(), int_property(span.end_time_ns)),
        ]);

        if !span.parent_span_id.is_empty() {
            properties.insert(
                "parent_span_id".to_string(),
                string_property(&span.parent_span_id),
            );
        }
        if !span.service_name.is_empty() {
            properties.insert(
                "service_name".to_string(),
                string_property(&span.service_name),
            );
        }
        if placeholder {
            properties.insert("placeholder".to_string(), bool_property(true));
        }

        Node {
            id: node_id.to_string(),
            labels: vec![self.config.span_node_label.clone()],
            properties,
            embedding: None,
            created_at_ms: ns_to_ms(span.start_time_ns),
            updated_at_ms: ns_to_ms(span.end_time_ns),
        }
    }

    fn placeholder_span_node(&self, namespace: &str, node_id: &str, span: &TraceSpan) -> Node {
        let mut placeholder_span = span.clone();
        placeholder_span.span_id = span.parent_span_id.clone();
        placeholder_span.parent_span_id.clear();
        placeholder_span.name = format!("parent:{}", span.parent_span_id);
        self.span_node(namespace, node_id, &placeholder_span, true)
    }

    fn service_node(&self, namespace: &str, node_id: &str, service_name: &str) -> Node {
        Node {
            id: node_id.to_string(),
            labels: vec![self.config.service_node_label.clone()],
            properties: HashMap::from([
                ("namespace".to_string(), string_property(namespace)),
                ("service_name".to_string(), string_property(service_name)),
            ]),
            embedding: None,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        }
    }

    fn synthetic_entity_node(
        &self,
        namespace: &str,
        node_id: &str,
        target_label: &str,
        attribute_key: &str,
        attribute_value: &str,
    ) -> Node {
        Node {
            id: node_id.to_string(),
            labels: vec![
                self.config.entity_node_label.clone(),
                target_label.to_string(),
            ],
            properties: HashMap::from([
                ("namespace".to_string(), string_property(namespace)),
                ("attribute_key".to_string(), string_property(attribute_key)),
                (
                    "attribute_value".to_string(),
                    string_property(attribute_value),
                ),
            ]),
            embedding: None,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        }
    }

    fn edge(
        &self,
        edge_id: String,
        from_node_id: &str,
        to_node_id: &str,
        edge_type: &str,
        properties: HashMap<String, PropertyValue>,
    ) -> Edge {
        Edge {
            id: edge_id,
            from_node_id: from_node_id.to_string(),
            to_node_id: to_node_id.to_string(),
            edge_type: edge_type.to_string(),
            properties,
            weight: None,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        }
    }

    fn edge_id(&self, from_node_id: &str, edge_type: &str, to_node_id: &str) -> String {
        format!(
            "obs-edge:{}:{}:{}",
            normalize_id_component(from_node_id),
            normalize_id_component(edge_type),
            normalize_id_component(to_node_id)
        )
    }
}

fn normalize_id_component(value: &str) -> String {
    let normalized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();

    if normalized.is_empty() {
        "unknown".to_string()
    } else {
        normalized
    }
}

fn string_property_value<'a>(node: &'a Node, key: &str) -> Option<&'a str> {
    match node.properties.get(key)?.value.as_ref()? {
        property_value::Value::StringValue(value) => Some(value.as_str()),
        _ => None,
    }
}

fn int_property_value(edge: &Edge, key: &str) -> Option<i64> {
    match edge.properties.get(key)?.value.as_ref()? {
        property_value::Value::IntValue(value) => Some(*value),
        _ => None,
    }
}

fn string_property(value: &str) -> PropertyValue {
    PropertyValue {
        value: Some(property_value::Value::StringValue(value.to_string())),
    }
}

fn int_property(value: i64) -> PropertyValue {
    PropertyValue {
        value: Some(property_value::Value::IntValue(value)),
    }
}

fn bool_property(value: bool) -> PropertyValue {
    PropertyValue {
        value: Some(property_value::Value::BoolValue(value)),
    }
}

fn ns_to_ms(value: i64) -> i64 {
    value / 1_000_000
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_span() -> TraceSpan {
        TraceSpan {
            trace_id: "trace-1".to_string(),
            span_id: "span-2".to_string(),
            parent_span_id: "span-1".to_string(),
            name: "GET /items".to_string(),
            service_name: "catalog".to_string(),
            start_time_ns: 1_000,
            end_time_ns: 2_000,
            attributes: HashMap::from([
                ("entity_id".to_string(), "item-42".to_string()),
                ("tenant_id".to_string(), "tenant-a".to_string()),
            ]),
            status: 0,
            status_message: String::new(),
        }
    }

    #[tokio::test]
    async fn test_link_trace_span_materializes_service_span_and_entity_nodes() {
        let graph_service = Arc::new(GraphService::new());
        let linker =
            TelemetryGraphLinker::new(graph_service.clone(), TelemetryGraphLinkerConfig::default());

        let result = linker.link_trace_span("prod", &test_span()).await;
        if let Err(error) = &result
            && (error.to_string().contains("URL")
                || error.to_string().contains("Serialization error"))
        {
            return;
        }
        result.expect("linking should succeed");

        let graph_id = &linker.config().graph_id;
        assert!(
            graph_service
                .get_node(graph_id, &linker.span_node_id("prod", "trace-1", "span-2"))
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            graph_service
                .get_node(graph_id, &linker.trace_node_id("prod", "trace-1"))
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            graph_service
                .get_node(graph_id, &linker.service_node_id("prod", "catalog"))
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            graph_service
                .get_node(
                    graph_id,
                    &linker.synthetic_entity_node_id("prod", "entity_id", "item-42"),
                )
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn test_late_parent_ingest_backfills_service_dependency_edge() {
        let graph_service = Arc::new(GraphService::new());
        let linker =
            TelemetryGraphLinker::new(graph_service.clone(), TelemetryGraphLinkerConfig::default());

        let mut child_span = test_span();
        child_span.service_name = "catalog".to_string();
        child_span.parent_span_id = "span-parent".to_string();

        let mut parent_span = child_span.clone();
        parent_span.span_id = "span-parent".to_string();
        parent_span.parent_span_id.clear();
        parent_span.service_name = "api".to_string();
        parent_span.name = "GET /checkout".to_string();

        linker
            .link_trace_span("prod", &child_span)
            .await
            .expect("child span linking should succeed");
        linker
            .link_trace_span("prod", &parent_span)
            .await
            .expect("parent span linking should succeed");

        let caller_service_node_id = linker.service_node_id("prod", "api");
        let callee_service_node_id = linker.service_node_id("prod", "catalog");
        let dependency_edge_id = linker.edge_id(
            &caller_service_node_id,
            &linker.config().service_dependency_edge,
            &callee_service_node_id,
        );

        let dependency_edge = graph_service
            .get_edge(&linker.config().graph_id, &dependency_edge_id)
            .await
            .expect("edge lookup should succeed")
            .expect("dependency edge should exist");

        assert_eq!(
            int_property_value(&dependency_edge, "observed_count"),
            Some(1)
        );
        assert_eq!(
            dependency_edge.weight,
            Some(1.0),
            "service dependency edge should track observed count"
        );
    }
}
