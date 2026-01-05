// Query engine for observability data
//
// Provides:
// - Log queries with filtering
// - Full-text log search with Tantivy
// - Metric aggregation with PromQL-like syntax
// - Trace queries and analysis

pub mod logs;
pub mod metrics;
pub mod promql;
pub mod tantivy_log_index;
pub mod traces;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;

use super::storage::ObservabilityStorage;
use super::{
    DataPoint, LogQueryParams, LogQueryResult, MetricAggParams, MetricAggResult, MetricAggregation,
    TimeSeriesResult,
};
use crate::proto::proximadb_v1::{LogEntry, Severity};

use self::logs::{LogAggregation, LogAggregationResult, LogQueryBuilder};
use self::metrics::{MetricAggregationFn, MetricQueryBuilder, MetricResult};
use self::promql::{PromQLExecutor, PromQLParser};
pub use self::tantivy_log_index::{LogSearchOptions, LogSearchResult, TantivyLogIndex};

/// Observability query engine
pub struct ObservabilityQueryEngine {
    /// Storage layer
    storage: Arc<ObservabilityStorage>,
    /// Tantivy log indexes by namespace
    log_indexes: tokio::sync::RwLock<HashMap<String, Arc<TantivyLogIndex>>>,
}

impl ObservabilityQueryEngine {
    /// Create a new query engine
    pub fn new(storage: Arc<ObservabilityStorage>) -> Self {
        Self {
            storage,
            log_indexes: tokio::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Get access to the storage layer
    pub fn storage(&self) -> &Arc<ObservabilityStorage> {
        &self.storage
    }

    /// Create or get a Tantivy log index for a namespace
    pub async fn get_or_create_log_index(&self, namespace: &str) -> Result<Arc<TantivyLogIndex>> {
        // Check if index exists
        {
            let indexes = self.log_indexes.read().await;
            if let Some(index) = indexes.get(namespace) {
                return Ok(index.clone());
            }
        }

        // Create new index
        let index = Arc::new(TantivyLogIndex::new(namespace)?);

        // Store it
        {
            let mut indexes = self.log_indexes.write().await;
            indexes.insert(namespace.to_string(), index.clone());
        }

        Ok(index)
    }

    /// Index logs for full-text search
    ///
    /// Indexes the provided logs into the Tantivy full-text index for the namespace.
    /// This enables fast BM25-ranked text search across log messages and fields.
    ///
    /// # Arguments
    /// * `namespace` - The observability namespace
    /// * `logs` - Vector of (log_id, LogEntry) pairs to index
    ///
    /// # Returns
    /// Number of logs indexed
    pub async fn index_logs_for_search(
        &self,
        namespace: &str,
        logs: Vec<(String, LogEntry)>,
    ) -> Result<usize> {
        let index = self.get_or_create_log_index(namespace).await?;
        let count = index.index_logs(&logs)?;
        index.commit()?;
        Ok(count)
    }

    /// Full-text search in logs using Tantivy
    ///
    /// Performs BM25-ranked full-text search across log messages and fields.
    /// Supports phrase queries, boolean operators, and field-specific search.
    ///
    /// # Arguments
    /// * `namespace` - The observability namespace
    /// * `query` - Search query string (supports Tantivy query syntax)
    /// * `options` - Search options (limit, filters, time range)
    ///
    /// # Returns
    /// Ranked search results with log IDs and relevance scores
    pub async fn search_logs_fulltext(
        &self,
        namespace: &str,
        query: &str,
        options: &LogSearchOptions,
    ) -> Result<Vec<LogSearchResult>> {
        let index = self.get_or_create_log_index(namespace).await?;
        index.search(query, options)
    }

    /// Full-text phrase search in logs
    ///
    /// Searches for exact phrase matches in log messages.
    ///
    /// # Arguments
    /// * `namespace` - The observability namespace
    /// * `phrase` - Exact phrase to search for
    /// * `options` - Search options
    ///
    /// # Returns
    /// Ranked search results containing the exact phrase
    pub async fn search_logs_phrase(
        &self,
        namespace: &str,
        phrase: &str,
        options: &LogSearchOptions,
    ) -> Result<Vec<LogSearchResult>> {
        let index = self.get_or_create_log_index(namespace).await?;
        index.search_phrase(phrase, options)
    }

    /// Get log index statistics
    pub async fn log_index_stats(&self, namespace: &str) -> Result<LogIndexStats> {
        let index = self.get_or_create_log_index(namespace).await?;
        Ok(LogIndexStats {
            namespace: namespace.to_string(),
            doc_count: index.doc_count(),
        })
    }

    /// Sync logs from storage to Tantivy index
    ///
    /// Indexes all logs from storage into the Tantivy full-text index.
    /// This enables full-text search for logs that were written before
    /// the index was created.
    ///
    /// # Arguments
    /// * `namespace` - The observability namespace
    /// * `start_ns` - Start time for logs to index
    /// * `end_ns` - End time for logs to index
    /// * `batch_size` - Number of logs to index per batch
    ///
    /// # Returns
    /// Total number of logs indexed
    pub async fn sync_logs_to_index(
        &self,
        namespace: &str,
        start_ns: i64,
        end_ns: i64,
        batch_size: usize,
    ) -> Result<usize> {
        let index = self.get_or_create_log_index(namespace).await?;
        let mut total_indexed = 0;
        let mut offset = 0;

        loop {
            // Fetch a batch of logs from storage
            let logs = self
                .storage
                .query_logs(namespace, start_ns, end_ns, batch_size)
                .await?;

            if logs.is_empty() {
                break;
            }

            // Create (id, log) pairs for indexing
            let logs_with_ids: Vec<(String, LogEntry)> = logs
                .into_iter()
                .enumerate()
                .map(|(i, log)| {
                    // Generate unique ID based on timestamp and offset
                    let id = format!("log_{}_{}", log.timestamp_ns, offset + i);
                    (id, log)
                })
                .collect();

            let batch_count = logs_with_ids.len();
            index.index_logs(&logs_with_ids)?;
            total_indexed += batch_count;
            offset += batch_count;

            // If we got fewer than batch_size, we're done
            if batch_count < batch_size {
                break;
            }
        }

        // Commit all indexed logs
        index.commit()?;

        Ok(total_indexed)
    }

    /// Query logs with optional Tantivy full-text search
    ///
    /// When a text query is provided and Tantivy indexing is enabled,
    /// uses BM25-ranked full-text search for better relevance.
    pub async fn query_logs_with_fulltext(
        &self,
        namespace: &str,
        params: LogQueryParams,
        use_fulltext: bool,
    ) -> Result<LogQueryResult> {
        let start = std::time::Instant::now();

        // If text query and fulltext enabled, try Tantivy first
        if use_fulltext && params.query.is_some() {
            let query = params.query.as_ref().unwrap();

            // Check if we have indexed documents
            let index = self.get_or_create_log_index(namespace).await?;
            if index.doc_count() > 0 {
                // Build search options from params
                let mut options = LogSearchOptions::with_limit(params.limit as usize);

                if params.start_time_ns > 0 {
                    options.start_time_ns = Some(params.start_time_ns);
                }
                if params.end_time_ns < i64::MAX {
                    options.end_time_ns = Some(params.end_time_ns);
                }

                // Add service filter if only one service specified
                if params.services.len() == 1 {
                    options.service_filter = Some(params.services[0].clone());
                }

                // Add severity filter if only one severity specified
                if params.severities.len() == 1 {
                    options.severity_filter = Some(params.severities[0]);
                }

                // Perform Tantivy search
                let search_results = index.search(query, &options)?;

                // Fetch full log entries for search results
                let logs = self.fetch_logs_by_ids(namespace, &search_results).await?;

                let query_time_ms = start.elapsed().as_millis() as u64;
                return Ok(LogQueryResult {
                    logs,
                    next_cursor: None,
                    total_matched: Some(search_results.len() as u64),
                    query_time_ms,
                });
            }
        }

        // Fall back to standard query
        self.query_logs(namespace, params).await
    }

    /// Fetch full log entries by their IDs from search results
    async fn fetch_logs_by_ids(
        &self,
        namespace: &str,
        search_results: &[LogSearchResult],
    ) -> Result<Vec<LogEntry>> {
        if search_results.is_empty() {
            return Ok(Vec::new());
        }

        // Extract timestamps from results for range query
        let min_ts = search_results
            .iter()
            .map(|r| r.timestamp_ns)
            .min()
            .unwrap_or(0);
        let max_ts = search_results
            .iter()
            .map(|r| r.timestamp_ns)
            .max()
            .unwrap_or(i64::MAX);

        // Fetch logs in the time range
        let all_logs = self
            .storage
            .query_logs(namespace, min_ts, max_ts + 1, search_results.len() * 2)
            .await?;

        // Build a set of result timestamps for matching
        let result_timestamps: std::collections::HashSet<i64> =
            search_results.iter().map(|r| r.timestamp_ns).collect();

        // Filter to only matching logs
        let logs: Vec<LogEntry> = all_logs
            .into_iter()
            .filter(|log| result_timestamps.contains(&log.timestamp_ns))
            .collect();

        Ok(logs)
    }

    /// Query logs
    pub async fn query_logs(
        &self,
        namespace: &str,
        params: LogQueryParams,
    ) -> Result<LogQueryResult> {
        let start = std::time::Instant::now();

        // Get raw logs from storage
        let mut logs = self
            .storage
            .query_logs(
                namespace,
                params.start_time_ns,
                params.end_time_ns,
                params.limit as usize * 2,
            )
            .await?;

        // Apply filters
        if !params.severities.is_empty() {
            logs.retain(|log| params.severities.iter().any(|s| log.severity == *s as i32));
        }

        if !params.services.is_empty() {
            logs.retain(|log| {
                log.service
                    .as_ref()
                    .map_or(false, |s| params.services.contains(s))
            });
        }

        if !params.sources.is_empty() {
            logs.retain(|log| {
                log.source
                    .as_ref()
                    .map_or(false, |s| params.sources.contains(s))
            });
        }

        // Apply query filter if present
        if let Some(query) = &params.query {
            logs = self.apply_query_filter(logs, query);
        }

        // Limit results
        let total_matched = Some(logs.len() as u64);
        if logs.len() > params.limit as usize {
            logs.truncate(params.limit as usize);
        }

        let query_time_ms = start.elapsed().as_millis() as u64;

        Ok(LogQueryResult {
            logs,
            next_cursor: None,
            total_matched,
            query_time_ms,
        })
    }

    /// Apply query filter to logs
    fn apply_query_filter(&self, logs: Vec<LogEntry>, query: &str) -> Vec<LogEntry> {
        // Parse query (Datadog-style or simple text search)
        let parsed = self.parse_query(query);

        logs.into_iter()
            .filter(|log| self.matches_query(log, &parsed))
            .collect()
    }

    /// Parse a query string
    fn parse_query(&self, query: &str) -> ParsedQuery {
        let mut parsed = ParsedQuery::default();

        // Split into terms
        for term in query.split_whitespace() {
            if term.contains(':') {
                let parts: Vec<&str> = term.splitn(2, ':').collect();
                if parts.len() == 2 {
                    let key = parts[0].to_lowercase();
                    let value = parts[1].to_string();

                    match key.as_str() {
                        "service" => parsed.service_filter = Some(value),
                        "source" | "host" => parsed.source_filter = Some(value),
                        "severity" | "level" => parsed.severity_filter = Some(value),
                        "trace_id" => parsed.trace_id_filter = Some(value),
                        _ => {
                            parsed.attribute_filters.insert(key, value);
                        }
                    }
                }
            } else {
                // Plain text search
                parsed.text_search.push(term.to_lowercase());
            }
        }

        parsed
    }

    /// Check if a log matches a parsed query
    fn matches_query(&self, log: &LogEntry, query: &ParsedQuery) -> bool {
        // Service filter
        if let Some(service) = &query.service_filter {
            match &log.service {
                Some(log_service) => {
                    if !log_service.to_lowercase().contains(&service.to_lowercase()) {
                        return false;
                    }
                }
                None => return false,
            }
        }

        // Source filter
        if let Some(source) = &query.source_filter {
            match &log.source {
                Some(log_source) => {
                    if !log_source.to_lowercase().contains(&source.to_lowercase()) {
                        return false;
                    }
                }
                None => return false,
            }
        }

        // Severity filter
        if let Some(severity) = &query.severity_filter {
            let expected = self.parse_severity(severity);
            if log.severity != expected as i32 {
                return false;
            }
        }

        // Trace ID filter - check in fields map
        if let Some(trace_id) = &query.trace_id_filter {
            if let Some(field_value) = log.fields.get("trace_id") {
                if let Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) =
                    &field_value.value
                {
                    if s != trace_id {
                        return false;
                    }
                } else {
                    return false;
                }
            } else {
                return false;
            }
        }

        // Attribute filters (from fields map)
        for (key, value) in &query.attribute_filters {
            if let Some(field_value) = log.fields.get(key) {
                let field_str = self.sql_value_to_string(field_value);
                if !field_str.to_lowercase().contains(&value.to_lowercase()) {
                    return false;
                }
            } else {
                return false;
            }
        }

        // Text search in message
        let message_lower = log.message.to_lowercase();
        for term in &query.text_search {
            if !message_lower.contains(term) {
                return false;
            }
        }

        true
    }

    /// Parse severity string
    fn parse_severity(&self, s: &str) -> Severity {
        match s.to_lowercase().as_str() {
            "debug" | "trace" => Severity::Debug,
            "info" | "information" => Severity::Info,
            "warn" | "warning" => Severity::Warn,
            "error" | "err" => Severity::Error,
            "fatal" | "critical" => Severity::Fatal,
            _ => Severity::Info,
        }
    }

    /// Convert SqlValue to string for comparison
    fn sql_value_to_string(&self, value: &crate::proto::proximadb_v1::SqlValue) -> String {
        use crate::proto::proximadb_v1::sql_value::Value;
        match &value.value {
            Some(Value::StringValue(s)) => s.clone(),
            Some(Value::Int64Value(i)) => i.to_string(),
            Some(Value::NumberValue(f)) => f.to_string(),
            Some(Value::BoolValue(b)) => b.to_string(),
            Some(Value::BytesValue(b)) => String::from_utf8_lossy(b).to_string(),
            Some(Value::NullValue(_)) => "null".to_string(),
            Some(Value::ArrayValue(_)) => "[array]".to_string(),
            Some(Value::ObjectValue(_)) => "{object}".to_string(),
            None => String::new(),
        }
    }

    /// Aggregate metrics
    pub async fn aggregate_metrics(
        &self,
        namespace: &str,
        params: MetricAggParams,
    ) -> Result<MetricAggResult> {
        let start = std::time::Instant::now();

        // Get raw metrics from storage
        let metrics = self
            .storage
            .query_metrics(
                namespace,
                &params.metric_name,
                params.start_time_ns,
                params.end_time_ns,
            )
            .await?;

        // Convert MetricAggregation to MetricAggregationFn
        let agg_fn = match params.aggregation {
            MetricAggregation::Avg => MetricAggregationFn::Avg,
            MetricAggregation::Sum => MetricAggregationFn::Sum,
            MetricAggregation::Min => MetricAggregationFn::Min,
            MetricAggregation::Max => MetricAggregationFn::Max,
            MetricAggregation::Count => MetricAggregationFn::Count,
            MetricAggregation::Rate => MetricAggregationFn::Rate,
            MetricAggregation::P50 => MetricAggregationFn::P50,
            MetricAggregation::P90 => MetricAggregationFn::P90,
            MetricAggregation::P95 => MetricAggregationFn::P95,
            MetricAggregation::P99 => MetricAggregationFn::P99,
        };

        // Build the metric query using the full aggregation engine
        let bucket_size_ns = params.step_seconds as i64 * 1_000_000_000; // Convert seconds to nanoseconds
        let mut builder = MetricQueryBuilder::new()
            .metric(&params.metric_name)
            .time_range(params.start_time_ns, params.end_time_ns)
            .aggregate(agg_fn)
            .bucket(bucket_size_ns);

        // Add label filters
        for (key, value) in &params.label_filters {
            builder = builder.label(key, value);
        }

        // Add group by labels
        for label in &params.group_by {
            builder = builder.group_by(label);
        }

        let query = builder.build();

        // Execute the query on the raw metrics
        let results = query.execute(metrics);

        // Group results by label set into time series
        let mut series_map: HashMap<String, TimeSeriesResult> = HashMap::new();

        for result in results {
            // Create a key from labels for grouping
            let label_key: String = result
                .labels
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join(",");

            let series = series_map
                .entry(label_key.clone())
                .or_insert_with(|| TimeSeriesResult {
                    labels: result.labels.clone(),
                    points: Vec::new(),
                });

            series.points.push(DataPoint {
                timestamp_ns: result.timestamp_ns,
                value: result.value,
            });
        }

        // Sort points in each series by timestamp
        let mut series: Vec<TimeSeriesResult> = series_map.into_values().collect();
        for ts in &mut series {
            ts.points.sort_by_key(|p| p.timestamp_ns);
        }

        let query_time_ms = start.elapsed().as_millis() as u64;

        Ok(MetricAggResult {
            series,
            query_time_ms,
        })
    }

    /// Aggregate logs with GROUP BY support
    pub async fn aggregate_logs(
        &self,
        namespace: &str,
        params: LogAggregationParams,
    ) -> Result<LogAggregationResult> {
        // Get raw logs from storage
        let logs = self
            .storage
            .query_logs(
                namespace,
                params.start_time_ns,
                params.end_time_ns,
                params.max_logs_to_scan,
            )
            .await?;

        // Build a query with filters
        let mut builder =
            LogQueryBuilder::new().time_range(params.start_time_ns, params.end_time_ns);

        // Apply service filter
        for service in &params.services {
            builder = builder.service(service);
        }

        // Apply severity filter
        for severity in &params.severities {
            builder = builder.severity(*severity);
        }

        // Apply text query
        if let Some(ref query) = params.query {
            builder = builder.text(query);
        }

        let log_query = builder.build();

        // Execute aggregation
        let result = log_query.aggregate(&logs, &params.aggregation);

        Ok(result)
    }

    /// Execute a PromQL query
    ///
    /// Provides Prometheus-compatible query language support for metrics.
    /// Supports vector selectors, aggregations (sum, avg, rate, etc.),
    /// and label matchers.
    ///
    /// # Arguments
    /// * `namespace` - The observability namespace
    /// * `params` - PromQL query parameters
    ///
    /// # Returns
    /// Query results with metric values and labels
    pub async fn query_promql(
        &self,
        namespace: &str,
        params: PromQLQueryParams,
    ) -> Result<PromQLQueryResult> {
        let start = std::time::Instant::now();

        // Parse the PromQL expression
        let expr = PromQLParser::parse(&params.query)?;

        // Extract metric names from the expression to fetch relevant data
        let metric_names = Self::extract_metric_names(&expr);

        // Fetch samples for all referenced metrics
        let mut all_samples = Vec::new();
        for metric_name in &metric_names {
            let samples = self
                .storage
                .query_metrics(
                    namespace,
                    metric_name,
                    params.start_time_ns,
                    params.end_time_ns,
                )
                .await?;
            all_samples.extend(samples);
        }

        // Execute the PromQL expression
        let eval_time = params.eval_time_ns.unwrap_or(params.end_time_ns);
        let lookback_ns = params.lookback_ns.unwrap_or(5 * 60 * 1_000_000_000); // Default 5 minutes
        let results = PromQLExecutor::execute(&expr, all_samples, eval_time, lookback_ns)?;

        let query_time_ms = start.elapsed().as_millis() as u64;

        Ok(PromQLQueryResult {
            results,
            query_time_ms,
        })
    }

    /// Extract metric names from a PromQL expression
    fn extract_metric_names(expr: &promql::PromQLExpr) -> Vec<String> {
        let mut names = Vec::new();
        Self::collect_metric_names(expr, &mut names);
        names.sort();
        names.dedup();
        names
    }

    /// Recursively collect metric names from expression tree
    fn collect_metric_names(expr: &promql::PromQLExpr, names: &mut Vec<String>) {
        match expr {
            promql::PromQLExpr::VectorSelector { name, .. } => {
                names.push(name.clone());
            }
            promql::PromQLExpr::Aggregation { expr, .. } => {
                Self::collect_metric_names(expr, names);
            }
            promql::PromQLExpr::Binary { lhs, rhs, .. } => {
                Self::collect_metric_names(lhs, names);
                Self::collect_metric_names(rhs, names);
            }
            promql::PromQLExpr::Scalar(_) => {}
        }
    }
}

/// Parameters for PromQL query
#[derive(Debug, Clone)]
pub struct PromQLQueryParams {
    /// PromQL query string
    pub query: String,
    /// Start time (nanoseconds since epoch)
    pub start_time_ns: i64,
    /// End time (nanoseconds since epoch)
    pub end_time_ns: i64,
    /// Evaluation time (defaults to end_time_ns)
    pub eval_time_ns: Option<i64>,
    /// Lookback window in nanoseconds (defaults to 5 minutes)
    pub lookback_ns: Option<i64>,
}

impl PromQLQueryParams {
    /// Create new PromQL query params
    pub fn new(query: &str, start_time_ns: i64, end_time_ns: i64) -> Self {
        Self {
            query: query.to_string(),
            start_time_ns,
            end_time_ns,
            eval_time_ns: None,
            lookback_ns: None,
        }
    }

    /// Set evaluation time
    pub fn with_eval_time(mut self, eval_time_ns: i64) -> Self {
        self.eval_time_ns = Some(eval_time_ns);
        self
    }

    /// Set lookback window
    pub fn with_lookback(mut self, lookback_ns: i64) -> Self {
        self.lookback_ns = Some(lookback_ns);
        self
    }
}

/// Result of a PromQL query
#[derive(Debug, Clone)]
pub struct PromQLQueryResult {
    /// Query results
    pub results: Vec<MetricResult>,
    /// Query execution time in milliseconds
    pub query_time_ms: u64,
}

/// Parameters for log aggregation
#[derive(Debug, Clone)]
pub struct LogAggregationParams {
    /// Start time (nanoseconds since epoch)
    pub start_time_ns: i64,
    /// End time (nanoseconds since epoch)
    pub end_time_ns: i64,
    /// Text query filter
    pub query: Option<String>,
    /// Service filters
    pub services: Vec<String>,
    /// Severity filters
    pub severities: Vec<Severity>,
    /// Maximum logs to scan (for performance)
    pub max_logs_to_scan: usize,
    /// Aggregation type
    pub aggregation: LogAggregation,
}

impl Default for LogAggregationParams {
    fn default() -> Self {
        Self {
            start_time_ns: 0,
            end_time_ns: i64::MAX,
            query: None,
            services: Vec::new(),
            severities: Vec::new(),
            max_logs_to_scan: 100_000,
            aggregation: LogAggregation::Count,
        }
    }
}

impl LogAggregationParams {
    /// Create new aggregation params
    pub fn new(start_time_ns: i64, end_time_ns: i64, aggregation: LogAggregation) -> Self {
        Self {
            start_time_ns,
            end_time_ns,
            aggregation,
            ..Default::default()
        }
    }

    /// Add service filter
    pub fn with_service(mut self, service: &str) -> Self {
        self.services.push(service.to_string());
        self
    }

    /// Add severity filter
    pub fn with_severity(mut self, severity: Severity) -> Self {
        self.severities.push(severity);
        self
    }

    /// Set text query
    pub fn with_query(mut self, query: &str) -> Self {
        self.query = Some(query.to_string());
        self
    }

    /// Set max logs to scan
    pub fn with_max_logs(mut self, max: usize) -> Self {
        self.max_logs_to_scan = max;
        self
    }
}

/// Parsed query structure
#[derive(Debug, Default)]
struct ParsedQuery {
    /// Service filter
    service_filter: Option<String>,
    /// Source filter
    source_filter: Option<String>,
    /// Severity filter
    severity_filter: Option<String>,
    /// Trace ID filter
    trace_id_filter: Option<String>,
    /// Attribute filters
    attribute_filters: HashMap<String, String>,
    /// Text search terms
    text_search: Vec<String>,
}

/// Log index statistics
#[derive(Debug, Clone)]
pub struct LogIndexStats {
    /// Namespace name
    pub namespace: String,
    /// Number of indexed documents
    pub doc_count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_query() {
        let storage = Arc::new(ObservabilityStorage::new("/tmp/test"));
        let engine = ObservabilityQueryEngine::new(storage);

        let query = "service:api error connection";
        let parsed = engine.parse_query(query);

        assert_eq!(parsed.service_filter, Some("api".to_string()));
        assert!(parsed.text_search.contains(&"error".to_string()));
        assert!(parsed.text_search.contains(&"connection".to_string()));
    }

    #[test]
    fn test_parse_severity() {
        let storage = Arc::new(ObservabilityStorage::new("/tmp/test"));
        let engine = ObservabilityQueryEngine::new(storage);

        assert_eq!(engine.parse_severity("debug"), Severity::Debug);
        assert_eq!(engine.parse_severity("ERROR"), Severity::Error);
        assert_eq!(engine.parse_severity("WARN"), Severity::Warn);
    }

    #[tokio::test]
    async fn test_index_logs_for_search() {
        let storage = Arc::new(ObservabilityStorage::new("/tmp/test_tantivy_index"));
        let engine = ObservabilityQueryEngine::new(storage);

        // Create logs to index
        let logs = vec![
            (
                "log_1".to_string(),
                LogEntry {
                    timestamp_ns: 1000,
                    severity: Severity::Error as i32,
                    message: "Database connection timeout occurred".to_string(),
                    service: Some("api-gateway".to_string()),
                    source: Some("host1".to_string()),
                    fields: HashMap::new(),
                },
            ),
            (
                "log_2".to_string(),
                LogEntry {
                    timestamp_ns: 2000,
                    severity: Severity::Info as i32,
                    message: "Request completed successfully".to_string(),
                    service: Some("web-server".to_string()),
                    source: Some("host2".to_string()),
                    fields: HashMap::new(),
                },
            ),
            (
                "log_3".to_string(),
                LogEntry {
                    timestamp_ns: 3000,
                    severity: Severity::Error as i32,
                    message: "Connection refused by database".to_string(),
                    service: Some("data-service".to_string()),
                    source: Some("host3".to_string()),
                    fields: HashMap::new(),
                },
            ),
        ];

        // Index the logs
        let indexed = engine.index_logs_for_search("test_ns", logs).await.unwrap();
        assert_eq!(indexed, 3);

        // Verify index stats
        let stats = engine.log_index_stats("test_ns").await.unwrap();
        assert_eq!(stats.doc_count, 3);
    }

    #[tokio::test]
    async fn test_search_logs_fulltext() {
        let storage = Arc::new(ObservabilityStorage::new("/tmp/test_tantivy_search"));
        let engine = ObservabilityQueryEngine::new(storage);

        // Create and index logs
        let logs = vec![
            (
                "log_1".to_string(),
                LogEntry {
                    timestamp_ns: 1000,
                    severity: Severity::Error as i32,
                    message: "Database connection timeout error".to_string(),
                    service: Some("api".to_string()),
                    source: Some("host1".to_string()),
                    fields: HashMap::new(),
                },
            ),
            (
                "log_2".to_string(),
                LogEntry {
                    timestamp_ns: 2000,
                    severity: Severity::Info as i32,
                    message: "Request completed without issues".to_string(),
                    service: Some("web".to_string()),
                    source: Some("host2".to_string()),
                    fields: HashMap::new(),
                },
            ),
            (
                "log_3".to_string(),
                LogEntry {
                    timestamp_ns: 3000,
                    severity: Severity::Error as i32,
                    message: "Connection failed to database".to_string(),
                    service: Some("data".to_string()),
                    source: Some("host3".to_string()),
                    fields: HashMap::new(),
                },
            ),
        ];

        engine
            .index_logs_for_search("search_ns", logs)
            .await
            .unwrap();

        // Search for "connection"
        let options = LogSearchOptions::with_limit(10);
        let results = engine
            .search_logs_fulltext("search_ns", "connection", &options)
            .await
            .unwrap();

        // Should find 2 logs containing "connection"
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_search_logs_phrase() {
        let storage = Arc::new(ObservabilityStorage::new("/tmp/test_tantivy_phrase"));
        let engine = ObservabilityQueryEngine::new(storage);

        let logs = vec![
            (
                "log_1".to_string(),
                LogEntry {
                    timestamp_ns: 1000,
                    severity: Severity::Error as i32,
                    message: "Connection timeout error in processing".to_string(),
                    service: Some("api".to_string()),
                    source: Some("host1".to_string()),
                    fields: HashMap::new(),
                },
            ),
            (
                "log_2".to_string(),
                LogEntry {
                    timestamp_ns: 2000,
                    severity: Severity::Error as i32,
                    message: "Error timeout connection".to_string(),
                    service: Some("api".to_string()),
                    source: Some("host2".to_string()),
                    fields: HashMap::new(),
                },
            ),
        ];

        engine
            .index_logs_for_search("phrase_ns", logs)
            .await
            .unwrap();

        // Phrase search - exact phrase
        let options = LogSearchOptions::with_limit(10);
        let results = engine
            .search_logs_phrase("phrase_ns", "timeout error", &options)
            .await
            .unwrap();

        // Only log_1 has exact phrase "timeout error"
        assert!(results.len() >= 1);
    }

    #[tokio::test]
    async fn test_search_with_filters() {
        let storage = Arc::new(ObservabilityStorage::new("/tmp/test_tantivy_filters"));
        let engine = ObservabilityQueryEngine::new(storage);

        let logs = vec![
            (
                "log_1".to_string(),
                LogEntry {
                    timestamp_ns: 1000,
                    severity: Severity::Error as i32,
                    message: "Error in api processing".to_string(),
                    service: Some("api".to_string()),
                    source: Some("host1".to_string()),
                    fields: HashMap::new(),
                },
            ),
            (
                "log_2".to_string(),
                LogEntry {
                    timestamp_ns: 2000,
                    severity: Severity::Warn as i32,
                    message: "Warning in api layer".to_string(),
                    service: Some("api".to_string()),
                    source: Some("host2".to_string()),
                    fields: HashMap::new(),
                },
            ),
            (
                "log_3".to_string(),
                LogEntry {
                    timestamp_ns: 3000,
                    severity: Severity::Error as i32,
                    message: "Error in web service".to_string(),
                    service: Some("web".to_string()),
                    source: Some("host3".to_string()),
                    fields: HashMap::new(),
                },
            ),
        ];

        engine
            .index_logs_for_search("filter_ns", logs)
            .await
            .unwrap();

        // Search with severity filter
        let options = LogSearchOptions::with_limit(10).severity(Severity::Error);
        let results = engine
            .search_logs_fulltext("filter_ns", "error", &options)
            .await
            .unwrap();

        // Should find only Error severity logs
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_log_index_stats() {
        let storage = Arc::new(ObservabilityStorage::new("/tmp/test_tantivy_stats"));
        let engine = ObservabilityQueryEngine::new(storage);

        // Initially empty
        let stats = engine.log_index_stats("stats_ns").await.unwrap();
        assert_eq!(stats.namespace, "stats_ns");
        assert_eq!(stats.doc_count, 0);

        // Add some logs
        let logs = vec![(
            "log_1".to_string(),
            LogEntry {
                timestamp_ns: 1000,
                severity: Severity::Info as i32,
                message: "Test message".to_string(),
                service: Some("api".to_string()),
                source: None,
                fields: HashMap::new(),
            },
        )];

        engine
            .index_logs_for_search("stats_ns", logs)
            .await
            .unwrap();

        let stats = engine.log_index_stats("stats_ns").await.unwrap();
        assert_eq!(stats.doc_count, 1);
    }
}
