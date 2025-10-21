//! Node Operations API (extracted from service.rs)
//!
//! Provides node CRUD, neighbor retrieval, node querying, and maintenance of
//! single-field and multi-field unique constraints for per-graph scopes.

use super::Result;
use crate::graph::engines::GraphEngine;
use crate::graph::{Node, NodeId};
use crate::proto::proximadb_v1::NodeQuery;
use std::collections::HashSet;
use std::sync::Arc;

impl super::GraphOperationsService {
    /// Get neighbors of a node
    pub async fn get_neighbors(&self, graph_id: &str, node_id: &NodeId) -> Result<Vec<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(crate::core::error::ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        let engine = self.get_or_create_graph_engine(graph_id).await?;
        engine.get_neighbors(node_id, None)
    }

    /// Query nodes by labels and properties
    pub async fn query_nodes(&self, graph_id: &str, query: NodeQuery) -> Result<Vec<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(crate::core::error::ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        let engine = self.get_or_create_graph_engine(graph_id).await?;

        // Initial candidate set from labels or all nodes
        let mut candidates: HashSet<NodeId> = if !query.labels.is_empty() {
            let mut set = HashSet::new();
            for label in &query.labels {
                if let Ok(nodes) = engine.get_nodes_by_label(label) {
                    for n in nodes {
                        set.insert(n.id.clone());
                    }
                }
            }
            set
        } else {
            engine
                .get_all_nodes()?
                .into_iter()
                .map(|n| n.id.clone())
                .collect()
        };

        // Use property indexes / ordered indexes for prefiltering
        for filter in &query.filters {
            use crate::proto::proximadb_v1::PropertyFilterOperator as Op;
            match Op::try_from(filter.operator).unwrap_or(Op::Unspecified) {
                Op::Equals => {
                    // Look up index for this property
                    if let Some(index_map) = self.memory_pool.node_property_indexes.get(&filter.key)
                    {
                        let key = super::index_key_for_value(filter.value.as_ref().unwrap());
                        if let Some(ids_vec) = index_map.get(&key) {
                            let id_set: HashSet<NodeId> = ids_vec.iter().cloned().collect();
                            candidates = candidates
                                .into_iter()
                                .filter(|id| id_set.contains(id))
                                .collect();
                        } else {
                            // No matches for this property value; result is empty
                            candidates.clear();
                            break;
                        }
                    } else {
                        // No index for this property; will verify via scan later
                        continue;
                    }
                }
                Op::StartsWith => {
                    if let Some(prefix) =
                        super::extract_string_from_value(filter.value.as_ref().unwrap())
                    {
                        if let Some(map_lock) =
                            self.memory_pool.node_property_str_ordered.get(&filter.key)
                        {
                            let map = map_lock.read().unwrap();
                            let mut matched = HashSet::new();
                            for (k, ids) in map
                                .range(prefix.to_string()..)
                                .take_while(|(k, _)| k.starts_with(prefix))
                            {
                                matched.extend(ids.iter().cloned());
                            }
                            candidates = candidates
                                .into_iter()
                                .filter(|id| matched.contains(id))
                                .collect();
                        }
                    }
                }
                Op::GreaterThan | Op::GreaterEqual | Op::LessThan | Op::LessEqual => {
                    // Prefer numeric range if value numeric, else fallback to string ordered
                    if let Some(num) =
                        super::extract_number_from_value(filter.value.as_ref().unwrap())
                    {
                        if let Some(map_lock) =
                            self.memory_pool.node_property_num_indexes.get(&filter.key)
                        {
                            let map = map_lock.read().unwrap();
                            let mut matched = HashSet::new();
                            match Op::try_from(filter.operator).unwrap_or(Op::Unspecified) {
                                Op::GreaterThan => {
                                    for (k, ids) in map.iter() {
                                        if (*k as f64) > num {
                                            matched.extend(ids.iter().cloned());
                                        }
                                    }
                                }
                                Op::GreaterEqual => {
                                    for (k, ids) in map.iter() {
                                        if (*k as f64) >= num {
                                            matched.extend(ids.iter().cloned());
                                        }
                                    }
                                }
                                Op::LessThan => {
                                    for (k, ids) in map.iter() {
                                        if (*k as f64) < num {
                                            matched.extend(ids.iter().cloned());
                                        }
                                    }
                                }
                                Op::LessEqual => {
                                    for (k, ids) in map.iter() {
                                        if (*k as f64) <= num {
                                            matched.extend(ids.iter().cloned());
                                        }
                                    }
                                }
                                _ => {}
                            }
                            candidates = candidates
                                .into_iter()
                                .filter(|id| matched.contains(id))
                                .collect();
                        }
                    } else if let Some(map_lock) =
                        self.memory_pool.node_property_str_ordered.get(&filter.key)
                    {
                        let map = map_lock.read().unwrap();
                        let mut matched = HashSet::new();
                        let s = super::extract_string_from_value(filter.value.as_ref().unwrap())
                            .unwrap_or("");
                        match Op::try_from(filter.operator).unwrap_or(Op::Unspecified) {
                            Op::GreaterThan => {
                                for (_k, ids) in map.range((
                                    std::ops::Bound::Excluded(s.to_string()),
                                    std::ops::Bound::Unbounded,
                                )) {
                                    matched.extend(ids.iter().cloned());
                                }
                            }
                            Op::GreaterEqual => {
                                for (_k, ids) in map.range((
                                    std::ops::Bound::Included(s.to_string()),
                                    std::ops::Bound::Unbounded,
                                )) {
                                    matched.extend(ids.iter().cloned());
                                }
                            }
                            Op::LessThan => {
                                for (_k, ids) in map.range((
                                    std::ops::Bound::Unbounded,
                                    std::ops::Bound::Excluded(s.to_string()),
                                )) {
                                    matched.extend(ids.iter().cloned());
                                }
                            }
                            Op::LessEqual => {
                                for (_k, ids) in map.range((
                                    std::ops::Bound::Unbounded,
                                    std::ops::Bound::Included(s.to_string()),
                                )) {
                                    matched.extend(ids.iter().cloned());
                                }
                            }
                            _ => {}
                        }
                        candidates = candidates
                            .into_iter()
                            .filter(|id| matched.contains(id))
                            .collect();
                    }
                }
                _ => {}
            }
        }

        // Final verification scan over candidate set
        let mut results: Vec<Arc<Node>> = Vec::new();
        for id in candidates.into_iter() {
            if let Ok(Some(node)) = engine.get_node(&id) {
                // Additional property filters
                if !query.filters.is_empty() {
                    let mut pass_all = true;
                    for filter in &query.filters {
                        use crate::proto::proximadb_v1::PropertyFilterOperator as Op;
                        let prop_val_opt = node.properties.get(&filter.key);
                        let pass = match Op::try_from(filter.operator).unwrap_or(Op::Unspecified) {
                            Op::Equals => match prop_val_opt {
                                Some(v) => v.value == filter.value.as_ref().unwrap().value,
                                None => false,
                            },
                            Op::NotEquals => match prop_val_opt {
                                Some(v) => v.value != filter.value.as_ref().unwrap().value,
                                None => true,
                            },
                            Op::GreaterThan => {
                                super::cmp_prop_gt(prop_val_opt, filter.value.as_ref().unwrap())
                            }
                            Op::GreaterEqual => {
                                super::cmp_prop_ge(prop_val_opt, filter.value.as_ref().unwrap())
                            }
                            Op::LessThan => {
                                super::cmp_prop_lt(prop_val_opt, filter.value.as_ref().unwrap())
                            }
                            Op::LessEqual => {
                                super::cmp_prop_le(prop_val_opt, filter.value.as_ref().unwrap())
                            }
                            Op::StartsWith => super::prop_starts_with(
                                prop_val_opt,
                                filter.value.as_ref().unwrap(),
                            ),
                            Op::Contains => {
                                super::prop_contains(prop_val_opt, filter.value.as_ref().unwrap())
                            }
                            _ => false,
                        };
                        if !pass {
                            pass_all = false;
                            break;
                        }
                    }
                    if !pass_all {
                        continue;
                    }
                }
                results.push(node);
            }
        }

        // Apply offset/limit for pagination
        let offset = query.offset.unwrap_or(0) as usize;
        let limit = query.limit.unwrap_or(results.len() as u32) as usize;
        if offset >= results.len() {
            return Ok(Vec::new());
        }
        let end = (offset + limit).min(results.len());
        Ok(results.drain(offset..end).collect())
    }
    /// Create a new node
    pub async fn create_node(&self, graph_id: &str, node: Node) -> Result<Arc<Node>> {
        if !self.graph_enabled() {
            return Err(crate::core::error::ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }

        let engine = self.get_or_create_graph_engine(graph_id).await?;
        self.enforce_schema_on_node(graph_id, &node).await?;
        self.enforce_unique_constraints_on_node(graph_id, &node)?;
        self.enforce_multi_unique_constraints_on_node(graph_id, &node)?;

        let node_arc = engine.insert_node(node)?;
        self.register_node_in_unique_constraints(graph_id, &node_arc);
        self.register_node_in_multi_unique_constraints(graph_id, &node_arc);
        Ok(node_arc)
    }

    /// Get a node by ID
    pub async fn get_node(&self, graph_id: &str, id: &NodeId) -> Result<Option<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(crate::core::error::ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        let engine = self.get_or_create_graph_engine(graph_id).await?;
        engine.get_node(id)
    }

    /// Update a node
    pub async fn update_node(&self, graph_id: &str, node: Node) -> Result<Arc<Node>> {
        if !self.graph_enabled() {
            return Err(crate::core::error::ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        let engine = self.get_or_create_graph_engine(graph_id).await?;
        self.enforce_schema_on_node(graph_id, &node).await?;
        self.enforce_unique_constraints_on_node(graph_id, &node)?;
        self.enforce_multi_unique_constraints_on_node(graph_id, &node)?;
        let node_arc = engine.update_node(node)?;
        self.register_node_in_unique_constraints(graph_id, &node_arc);
        self.register_node_in_multi_unique_constraints(graph_id, &node_arc);
        Ok(node_arc)
    }

    /// Delete a node
    pub async fn delete_node(&self, graph_id: &str, id: &NodeId) -> Result<Option<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(crate::core::error::ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        let engine = self.get_or_create_graph_engine(graph_id).await?;
        if let Some(node) = engine.get_node(id)? {
            self.unregister_node_from_unique_constraints(graph_id, &node);
            self.unregister_node_from_multi_unique_constraints(graph_id, &node);
        }
        crate::graph::engines::GraphEngine::delete_node(&*engine, id)
    }

    /// Delete a node and detach all incident edges (DETACH mode)
    pub async fn delete_node_detach(
        &self,
        graph_id: &str,
        id: &NodeId,
    ) -> Result<Option<Arc<Node>>> {
        if !self.graph_enabled() {
            return Err(crate::core::error::ProximaDBError::InvalidInput(
                "Graph operations disabled in current mode".to_string(),
            ));
        }
        let engine = self.get_or_create_graph_engine(graph_id).await?;
        let mut edge_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for e in engine.get_outgoing_edges(id, None)? {
            edge_ids.insert(e.id.clone());
        }
        for e in engine.get_incoming_edges(id, None)? {
            edge_ids.insert(e.id.clone());
        }
        for eid in edge_ids.into_iter() {
            if let Some(edge) = crate::graph::engines::GraphEngine::delete_edge(&*engine, &eid)? {
                self.stats_edges
                    .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                if let Some(v) = self.edge_type_counts.get(&edge.edge_type) {
                    v.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
        if let Some(node) = engine.get_node(id)? {
            self.unregister_node_from_unique_constraints(graph_id, &node);
            self.unregister_node_from_multi_unique_constraints(graph_id, &node);
        }
        crate::graph::engines::GraphEngine::delete_node(&*engine, id)
    }

    // ===== Unique constraints (single-field) =====
    pub(super) fn enforce_unique_constraints_on_node(
        &self,
        graph_id: &str,
        node: &Node,
    ) -> Result<()> {
        for label in &node.labels {
            for entry in self.memory_pool.unique_constraints.iter() {
                let (cgraph, clabel, cprop) = entry.key();
                let map = entry.value();
                if cgraph == graph_id && clabel == label {
                    if let Some(val) = node.properties.get(cprop) {
                        let k = super::index_key_for_value(val);
                        if let Some(existing) = map.get(&k) {
                            if existing.value() != &node.id {
                                return Err(crate::core::error::ProximaDBError::InvalidInput(
                                    format!(
                                        "Unique constraint violation on (label='{}', property='{}') for value '{}'",
                                        clabel, cprop, k
                                    ),
                                ));
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn register_node_in_unique_constraints(&self, graph_id: &str, node: &Arc<Node>) {
        for label in &node.labels {
            let label = label.clone();
            for entry in self.memory_pool.unique_constraints.iter() {
                let (cgraph, clabel, cprop) = entry.key();
                let map = entry.value();
                if cgraph == graph_id && *clabel == label {
                    if let Some(val) = node.properties.get(cprop) {
                        let k = super::index_key_for_value(val);
                        map.insert(k, node.id.clone());
                    }
                }
            }
        }
    }

    pub(super) fn unregister_node_from_unique_constraints(&self, graph_id: &str, node: &Arc<Node>) {
        for label in &node.labels {
            let label = label.clone();
            for entry in self.memory_pool.unique_constraints.iter() {
                let (cgraph, clabel, cprop) = entry.key();
                let map = entry.value();
                if cgraph == graph_id && *clabel == label {
                    if let Some(val) = node.properties.get(cprop) {
                        let k = super::index_key_for_value(val);
                        if let Some(existing) = map.get(&k) {
                            if existing.value() == &node.id {
                                map.remove(&k);
                            }
                        }
                    }
                }
            }
        }
    }

    // ===== Unique constraints (multi-field) =====
    pub(super) fn enforce_multi_unique_constraints_on_node(
        &self,
        graph_id: &str,
        node: &Node,
    ) -> Result<()> {
        for entry in self.memory_pool.unique_constraints_multi.iter() {
            let (cgraph, labels_key, props_key) = entry.key();
            if cgraph != graph_id {
                continue;
            }
            let props: Vec<String> = props_key.split('|').map(|s| s.to_string()).collect();
            let labels: Vec<String> = labels_key.split('|').map(|s| s.to_string()).collect();
            if !super::GraphOperationsService::node_has_all_labels(node, &labels) {
                continue;
            }
            if let Some(comp) = super::GraphOperationsService::composite_key_for_node(node, &props)
            {
                let map = entry.value();
                if let Some(existing) = map.get(&comp) {
                    if existing.value() != &node.id {
                        return Err(crate::core::error::ProximaDBError::InvalidInput(format!(
                            "Duplicate composite key for unique ({:?})",
                            props
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn register_node_in_multi_unique_constraints(
        &self,
        graph_id: &str,
        node: &Arc<Node>,
    ) {
        for mut entry in self.memory_pool.unique_constraints_multi.iter_mut() {
            let (cgraph, labels_key, props_key) = entry.key().clone();
            if cgraph != graph_id {
                continue;
            }
            let props: Vec<String> = props_key.split('|').map(|s| s.to_string()).collect();
            let labels: Vec<String> = labels_key.split('|').map(|s| s.to_string()).collect();
            if !super::GraphOperationsService::node_has_all_labels(node, &labels) {
                continue;
            }
            if let Some(comp) = super::GraphOperationsService::composite_key_for_node(node, &props)
            {
                entry.value().insert(comp, node.id.clone());
            }
        }
    }

    pub(super) fn unregister_node_from_multi_unique_constraints(
        &self,
        graph_id: &str,
        node: &Arc<Node>,
    ) {
        for mut entry in self.memory_pool.unique_constraints_multi.iter_mut() {
            let (cgraph, labels_key, props_key) = entry.key().clone();
            if cgraph != graph_id {
                continue;
            }
            let props: Vec<String> = props_key.split('|').map(|s| s.to_string()).collect();
            let labels: Vec<String> = labels_key.split('|').map(|s| s.to_string()).collect();
            if !super::GraphOperationsService::node_has_all_labels(node, &labels) {
                continue;
            }
            if let Some(comp) = super::GraphOperationsService::composite_key_for_node(node, &props)
            {
                entry.value().remove(&comp);
            }
        }
    }
}
