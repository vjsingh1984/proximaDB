//! Schema Validation (extracted from service.rs)
//!
//! Provides node/edge schema enforcement and property constraint evaluation.
//! This keeps GraphOperationsService free from deeply nested validation logic.
//! Responsibilities:
//! - Validate label presence, required/optional properties
//! - Type checking against declared PropertyType
//! - Evaluate String/Numeric/Array/Regex constraints
//! - Enforce edge source/target label compatibility

use super::Result;
use crate::core::error::ProximaDBError;
use crate::graph::{Edge, Node};

impl super::GraphOperationsService {
    /// Enforce schema constraints for a node if schema is defined
    pub(super) async fn enforce_schema_on_node(&self, graph_id: &str, node: &Node) -> Result<()> {
        let maybe_collection = self.collection_service.get_graph(graph_id).await?;
        if let Some(coll) = maybe_collection
            && let Some(schema) = &coll.schema
        {
            let strict = schema.strict_mode;
            // Build quick lookup for node label schemas
            for label in &node.labels {
                let label_schema = schema.node_labels.iter().find(|ls| &ls.label == label);
                if label_schema.is_none() && strict {
                    return Err(ProximaDBError::InvalidInput(format!(
                        "Label '{}' is not allowed by schema",
                        label
                    )));
                }
                if label_schema.is_none() {
                    continue;
                }
                let ls = label_schema.as_ref().ok_or_else(|| {
                    ProximaDBError::Internal(format!(
                        "Label schema not found for label '{}'",
                        label
                    ))
                })?;
                // Required properties present
                for req in &ls.required_properties {
                    if !node.properties.contains_key(req) {
                        return Err(ProximaDBError::InvalidInput(format!(
                            "Missing required property '{}' for label '{}'",
                            req, label
                        )));
                    }
                }
                // Validate property types and constraints (schema-level + label-level)
                for (k, v) in &node.properties {
                    if let Some(ps) = schema.properties.get(k) {
                        Self::validate_property_value_type(k, v, ps)?;
                        Self::validate_property_constraints(k, v, &ps.constraints)?;
                    }
                    if let Some(pc) = ls.property_constraints.get(k) {
                        Self::validate_property_constraint_one(k, v, pc)?;
                    }
                }
                // Disallow additional properties if configured
                if !ls.allow_additional_properties {
                    let mut allowed: std::collections::HashSet<&str> =
                        std::collections::HashSet::new();
                    for s in &ls.required_properties {
                        allowed.insert(s.as_str());
                    }
                    for s in &ls.optional_properties {
                        allowed.insert(s.as_str());
                    }
                    for p in ls.property_constraints.keys() {
                        allowed.insert(p.as_str());
                    }
                    for key in node.properties.keys() {
                        if !allowed.contains(key.as_str()) {
                            return Err(ProximaDBError::InvalidInput(format!(
                                "Property '{}' not allowed by schema for label '{}'",
                                key, label
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Enforce schema constraints for an edge if schema is defined
    pub(super) async fn enforce_schema_on_edge(
        &self,
        graph_id: &str,
        edge: &Edge,
        from_labels: &[String],
        to_labels: &[String],
    ) -> Result<()> {
        let maybe_collection = self.collection_service.get_graph(graph_id).await?;
        if let Some(coll) = maybe_collection
            && let Some(schema) = &coll.schema
        {
            let strict = schema.strict_mode;
            let ets = schema
                .edge_types
                .iter()
                .find(|et| et.edge_type == edge.edge_type);
            if ets.is_none() && strict {
                return Err(ProximaDBError::InvalidInput(format!(
                    "Edge type '{}' is not allowed by schema",
                    edge.edge_type
                )));
            }
            if ets.is_none() {
                return Ok(());
            }
            let ets = ets.as_ref().ok_or_else(|| {
                ProximaDBError::Internal(format!(
                    "Edge type schema not found for edge type '{}'",
                    edge.edge_type
                ))
            })?;
            // Required properties present
            for req in &ets.required_properties {
                if !edge.properties.contains_key(req) {
                    return Err(ProximaDBError::InvalidInput(format!(
                        "Missing required property '{}' for edge type '{}'",
                        req, edge.edge_type
                    )));
                }
            }
            // Validate edge property types and constraints (schema-level + edge-type level)
            for (k, v) in &edge.properties {
                if let Some(ps) = schema.properties.get(k) {
                    Self::validate_property_value_type(k, v, ps)?;
                    Self::validate_property_constraints(k, v, &ps.constraints)?;
                }
                if let Some(pc) = ets.property_constraints.get(k) {
                    Self::validate_property_constraint_one(k, v, pc)?;
                }
            }
            // Source/target label constraints
            if !ets.source_labels.is_empty()
                && !from_labels.iter().any(|l| ets.source_labels.contains(l))
            {
                return Err(ProximaDBError::InvalidInput(format!(
                    "Source node labels {:?} do not satisfy schema for edge type '{}'",
                    from_labels, edge.edge_type
                )));
            }
            if !ets.target_labels.is_empty()
                && !to_labels.iter().any(|l| ets.target_labels.contains(l))
            {
                return Err(ProximaDBError::InvalidInput(format!(
                    "Target node labels {:?} do not satisfy schema for edge type '{}'",
                    to_labels, edge.edge_type
                )));
            }
            // Disallow additional properties if configured
            if !ets.allow_additional_properties {
                let mut allowed: std::collections::HashSet<&str> = std::collections::HashSet::new();
                for s in &ets.required_properties {
                    allowed.insert(s.as_str());
                }
                for s in &ets.optional_properties {
                    allowed.insert(s.as_str());
                }
                for p in ets.property_constraints.keys() {
                    allowed.insert(p.as_str());
                }
                for key in edge.properties.keys() {
                    if !allowed.contains(key.as_str()) {
                        return Err(ProximaDBError::InvalidInput(format!(
                            "Property '{}' not allowed by schema for edge type '{}'",
                            key, edge.edge_type
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    // ===== Validation helpers =====
    pub(super) fn validate_property_value_type(
        key: &str,
        value: &crate::proto::proximadb_v1::PropertyValue,
        schema: &crate::proto::proximadb_v1::PropertySchema,
    ) -> Result<()> {
        use crate::proto::proximadb_v1::PropertyType as PT;
        use crate::proto::proximadb_v1::property_value::Value as PV;
        match (schema.r#type, &value.value) {
            (x, Some(PV::StringValue(_))) if x == PT::String as i32 => Ok(()),
            (x, Some(PV::IntValue(_))) if x == PT::Integer as i32 => Ok(()),
            (x, Some(PV::DoubleValue(_))) if x == PT::Float as i32 => Ok(()),
            (x, Some(PV::BoolValue(_))) if x == PT::Boolean as i32 => Ok(()),
            (x, Some(PV::ArrayValue(_))) if x == PT::Array as i32 => Ok(()),
            (x, Some(PV::VectorValue(_))) if x == PT::Embedding as i32 => Ok(()),
            (x, Some(PV::ObjectValue(_))) if x == PT::Json as i32 => Ok(()),
            _ => Err(ProximaDBError::InvalidInput(format!(
                "Property '{}' has type mismatch against schema",
                key
            ))),
        }
    }

    pub(super) fn validate_property_constraints(
        key: &str,
        value: &crate::proto::proximadb_v1::PropertyValue,
        constraints: &Vec<crate::proto::proximadb_v1::PropertyConstraint>,
    ) -> Result<()> {
        for c in constraints {
            Self::validate_property_constraint_one(key, value, c)?;
        }
        Ok(())
    }

    pub(super) fn validate_property_constraint_one(
        key: &str,
        value: &crate::proto::proximadb_v1::PropertyValue,
        constraint: &crate::proto::proximadb_v1::PropertyConstraint,
    ) -> Result<()> {
        use crate::proto::proximadb_v1::property_constraint::Constraint as C;
        use crate::proto::proximadb_v1::property_value::Value as PV;
        let constraint_type = constraint.constraint.as_ref().ok_or_else(|| {
            ProximaDBError::InvalidInput("Property constraint has no type specified".to_string())
        })?;
        match constraint_type {
            C::StringConstraint(sc) => {
                if let Some(PV::StringValue(s)) = &value.value {
                    if let Some(min) = sc.min_length
                        && s.len() < min as usize {
                            return Err(ProximaDBError::InvalidInput(format!(
                                "'{}' shorter than min_length",
                                key
                            )));
                        }
                    if let Some(max) = sc.max_length
                        && s.len() > max as usize {
                            return Err(ProximaDBError::InvalidInput(format!(
                                "'{}' longer than max_length",
                                key
                            )));
                        }
                }
            }
            C::NumericConstraint(nc) => {
                let num = match &value.value {
                    Some(PV::IntValue(i)) => *i as f64,
                    Some(PV::DoubleValue(d)) => *d,
                    Some(PV::StringValue(s)) => s.parse::<f64>().unwrap_or(f64::NAN),
                    _ => f64::NAN,
                };
                if num.is_nan() {
                    return Err(ProximaDBError::InvalidInput(format!(
                        "'{}' not numeric for numeric constraint",
                        key
                    )));
                }
                if let Some(min) = nc.min_value
                    && num < min {
                        return Err(ProximaDBError::InvalidInput(format!(
                            "'{}' less than min_value",
                            key
                        )));
                    }
                if let Some(max) = nc.max_value
                    && num > max {
                        return Err(ProximaDBError::InvalidInput(format!(
                            "'{}' greater than max_value",
                            key
                        )));
                    }
                if let Some(m) = nc.multiple_of
                    && m != 0.0 && (num / m).fract() != 0.0 {
                        return Err(ProximaDBError::InvalidInput(format!(
                            "'{}' not a multiple_of {}",
                            key, m
                        )));
                    }
            }
            C::ArrayConstraint(ac) => {
                if let Some(PV::ArrayValue(arr)) = &value.value {
                    let len = arr.values.len() as i32;
                    if let Some(min) = ac.min_items
                        && len < min {
                            return Err(ProximaDBError::InvalidInput(format!(
                                "'{}' array smaller than min_items",
                                key
                            )));
                        }
                    if let Some(max) = ac.max_items
                        && len > max {
                            return Err(ProximaDBError::InvalidInput(format!(
                                "'{}' array larger than max_items",
                                key
                            )));
                        }
                }
            }
            C::RegexConstraint(rc) => {
                if let Some(PV::StringValue(s)) = &value.value {
                    let re = regex::RegexBuilder::new(&rc.pattern)
                        .case_insensitive(rc.flags.contains('i'))
                        .multi_line(rc.flags.contains('m'))
                        .dot_matches_new_line(rc.flags.contains('s'))
                        .build()
                        .map_err(|e| {
                            ProximaDBError::InvalidInput(format!("Invalid regex in schema: {}", e))
                        })?;
                    if !re.is_match(s) {
                        return Err(ProximaDBError::InvalidInput(format!(
                            "'{}' does not match regex",
                            key
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}
