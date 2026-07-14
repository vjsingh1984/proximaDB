//! JSON format metrics exporter

use super::{MetricsExportSnapshot, MetricsExporter};
use anyhow::Result;
use serde_json;

pub struct JsonExporter {
    pretty: bool,
}

impl JsonExporter {
    pub fn new() -> Self {
        Self { pretty: false }
    }

    pub fn pretty() -> Self {
        Self { pretty: true }
    }
}

impl MetricsExporter for JsonExporter {
    fn export(&self, metrics: &MetricsExportSnapshot) -> Result<String> {
        if self.pretty {
            Ok(serde_json::to_string_pretty(metrics)?)
        } else {
            Ok(serde_json::to_string(metrics)?)
        }
    }

    fn content_type(&self) -> &'static str {
        "application/json"
    }

    fn format_name(&self) -> &'static str {
        "json"
    }
}

impl Default for JsonExporter {
    fn default() -> Self {
        Self::new()
    }
}
