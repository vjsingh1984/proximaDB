// PromQL-compatible query language parser and executor
//
// Provides:
// - PromQL expression parsing (subset)
// - Metric selectors with label matchers
// - Range vectors with duration
// - Aggregation functions (sum, avg, rate, etc.)
// - Time duration parsing

use std::collections::HashMap;

use anyhow::{Result, anyhow};

use super::metrics::MetricResult;
use crate::proto::proximadb_v1::MetricSample;

/// PromQL query parser
pub struct PromQLParser;

impl PromQLParser {
    /// Parse a PromQL expression string
    pub fn parse(query: &str) -> Result<PromQLExpr> {
        let query = query.trim();

        // Check for aggregation functions first
        if let Some(agg_expr) = Self::parse_aggregation(query)? {
            return Ok(agg_expr);
        }

        // Check for binary operations
        if let Some(binary_expr) = Self::parse_binary_operation(query)? {
            return Ok(binary_expr);
        }

        // Parse as vector selector (instant or range)
        Self::parse_vector_selector(query)
    }

    /// Parse aggregation function: sum(metric{label="value"}) by (label)
    fn parse_aggregation(query: &str) -> Result<Option<PromQLExpr>> {
        // Check for known aggregation functions
        let agg_funcs = [
            ("sum", AggregationOp::Sum),
            ("avg", AggregationOp::Avg),
            ("min", AggregationOp::Min),
            ("max", AggregationOp::Max),
            ("count", AggregationOp::Count),
            ("stddev", AggregationOp::Stddev),
            ("rate", AggregationOp::Rate),
            ("irate", AggregationOp::Irate),
            ("increase", AggregationOp::Increase),
            ("histogram_quantile", AggregationOp::HistogramQuantile),
            ("topk", AggregationOp::TopK),
            ("bottomk", AggregationOp::BottomK),
            ("count_values", AggregationOp::CountValues),
            ("quantile", AggregationOp::Quantile),
        ];

        for (name, op) in &agg_funcs {
            if query.starts_with(name) && query[name.len()..].trim_start().starts_with('(') {
                // Find matching closing parenthesis
                let rest = &query[name.len()..].trim_start();
                let inner = Self::extract_parentheses(rest)?;

                // Parse the inner expression
                let inner_expr = Self::parse(inner)?;

                // Check for "by" or "without" clause
                let after_paren = rest[inner.len() + 2..].trim();
                let (group_by, without) = Self::parse_grouping(after_paren)?;

                // Handle special functions with parameters (like histogram_quantile, topk)
                let param = Self::extract_function_param(op, inner);

                return Ok(Some(PromQLExpr::Aggregation {
                    op: op.clone(),
                    expr: Box::new(inner_expr),
                    by: group_by,
                    without,
                    param,
                }));
            }
        }

        Ok(None)
    }

    /// Extract function parameter for functions like histogram_quantile(0.95, ...) or topk(5, ...)
    fn extract_function_param(op: &AggregationOp, inner: &str) -> Option<f64> {
        match op {
            AggregationOp::HistogramQuantile
            | AggregationOp::TopK
            | AggregationOp::BottomK
            | AggregationOp::Quantile => {
                // Look for first comma and extract number before it
                if let Some(comma_pos) = inner.find(',') {
                    let param_str = inner[..comma_pos].trim();
                    param_str.parse::<f64>().ok()
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Parse "by (label1, label2)" or "without (label1, label2)"
    fn parse_grouping(s: &str) -> Result<(Vec<String>, bool)> {
        let s = s.trim();

        if s.starts_with("by") {
            let rest = s[2..].trim();
            if rest.starts_with('(') {
                let labels = Self::extract_parentheses(rest)?;
                let label_list = Self::parse_label_list(labels)?;
                return Ok((label_list, false));
            }
        } else if s.starts_with("without") {
            let rest = s[7..].trim();
            if rest.starts_with('(') {
                let labels = Self::extract_parentheses(rest)?;
                let label_list = Self::parse_label_list(labels)?;
                return Ok((label_list, true));
            }
        }

        Ok((Vec::new(), false))
    }

    /// Parse comma-separated label list
    fn parse_label_list(s: &str) -> Result<Vec<String>> {
        Ok(s.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect())
    }

    /// Parse binary operations: expr + expr, expr - expr, etc.
    fn parse_binary_operation(query: &str) -> Result<Option<PromQLExpr>> {
        // Simple binary operation detection (would need proper precedence for production)
        let ops = [
            (" + ", BinaryOp::Add),
            (" - ", BinaryOp::Sub),
            (" * ", BinaryOp::Mul),
            (" / ", BinaryOp::Div),
            (" % ", BinaryOp::Mod),
            (" ^ ", BinaryOp::Pow),
            (" == ", BinaryOp::Eq),
            (" != ", BinaryOp::Ne),
            (" > ", BinaryOp::Gt),
            (" < ", BinaryOp::Lt),
            (" >= ", BinaryOp::Ge),
            (" <= ", BinaryOp::Le),
            (" and ", BinaryOp::And),
            (" or ", BinaryOp::Or),
            (" unless ", BinaryOp::Unless),
        ];

        // Find binary operator (simple approach - doesn't handle nested expressions well)
        for (op_str, op) in &ops {
            if let Some(pos) = query.find(op_str) {
                let left = &query[..pos];
                let right = &query[pos + op_str.len()..];

                // Avoid splitting inside parentheses
                if Self::paren_depth_at(query, pos) == 0 {
                    let left_expr = Self::parse(left)?;
                    let right_expr = Self::parse(right)?;

                    return Ok(Some(PromQLExpr::Binary {
                        op: op.clone(),
                        lhs: Box::new(left_expr),
                        rhs: Box::new(right_expr),
                        matching: None,
                    }));
                }
            }
        }

        Ok(None)
    }

    /// Calculate parenthesis depth at a position
    fn paren_depth_at(s: &str, pos: usize) -> i32 {
        let mut depth = 0;
        for c in s[..pos].chars() {
            match c {
                '(' | '{' | '[' => depth += 1,
                ')' | '}' | ']' => depth -= 1,
                _ => {}
            }
        }
        depth
    }

    /// Parse vector selector: metric_name{label="value"}[5m]
    fn parse_vector_selector(query: &str) -> Result<PromQLExpr> {
        let query = query.trim();

        // Check for range vector: metric{...}[5m]
        let (selector_part, range) = if query.ends_with(']') {
            if let Some(bracket_start) = query.rfind('[') {
                let range_str = &query[bracket_start + 1..query.len() - 1];
                let duration = Self::parse_duration(range_str)?;
                (&query[..bracket_start], Some(duration))
            } else {
                return Err(anyhow!("Invalid range vector syntax"));
            }
        } else {
            (query, None)
        };

        // Parse metric name and label matchers
        let (metric_name, matchers) = if let Some(brace_start) = selector_part.find('{') {
            let name = &selector_part[..brace_start];
            let matchers_str = if selector_part.ends_with('}') {
                &selector_part[brace_start + 1..selector_part.len() - 1]
            } else {
                return Err(anyhow!("Unclosed label matcher braces"));
            };
            let matchers = Self::parse_label_matchers(matchers_str)?;
            (name.to_string(), matchers)
        } else {
            (selector_part.to_string(), Vec::new())
        };

        Ok(PromQLExpr::VectorSelector {
            name: metric_name,
            matchers,
            range,
            offset: None,
        })
    }

    /// Parse label matchers: label="value", label=~"regex", label!="value", label!~"regex"
    fn parse_label_matchers(s: &str) -> Result<Vec<LabelMatcher>> {
        let mut matchers = Vec::new();

        // Simple comma split (doesn't handle escaped commas in values)
        for part in s.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }

            // Determine match type
            let (label, op, value) = if part.contains("!~") {
                let parts: Vec<&str> = part.splitn(2, "!~").collect();
                (
                    parts[0].trim(),
                    MatchOp::NotRegex,
                    parts.get(1).map_or("", |s| s.trim()),
                )
            } else if part.contains("=~") {
                let parts: Vec<&str> = part.splitn(2, "=~").collect();
                (
                    parts[0].trim(),
                    MatchOp::Regex,
                    parts.get(1).map_or("", |s| s.trim()),
                )
            } else if part.contains("!=") {
                let parts: Vec<&str> = part.splitn(2, "!=").collect();
                (
                    parts[0].trim(),
                    MatchOp::NotEqual,
                    parts.get(1).map_or("", |s| s.trim()),
                )
            } else if part.contains('=') {
                let parts: Vec<&str> = part.splitn(2, '=').collect();
                (
                    parts[0].trim(),
                    MatchOp::Equal,
                    parts.get(1).map_or("", |s| s.trim()),
                )
            } else {
                return Err(anyhow!("Invalid label matcher: {}", part));
            };

            // Remove quotes from value
            let value = value.trim_matches('"').trim_matches('\'');

            matchers.push(LabelMatcher {
                name: label.to_string(),
                op,
                value: value.to_string(),
            });
        }

        Ok(matchers)
    }

    /// Parse duration string: 5m, 1h, 30s, 1d, etc.
    pub fn parse_duration(s: &str) -> Result<Duration> {
        let s = s.trim();
        if s.is_empty() {
            return Err(anyhow!("Empty duration string"));
        }

        let mut total_ns: i64 = 0;
        let mut current_num = String::new();

        for c in s.chars() {
            if c.is_ascii_digit() || c == '.' {
                current_num.push(c);
            } else {
                if current_num.is_empty() {
                    return Err(anyhow!("Invalid duration format: {}", s));
                }

                let num: f64 = current_num
                    .parse()
                    .map_err(|_| anyhow!("Invalid number in duration: {}", current_num))?;

                let multiplier = match c {
                    's' => 1_000_000_000i64,          // seconds
                    'm' => 60_000_000_000i64,         // minutes
                    'h' => 3_600_000_000_000i64,      // hours
                    'd' => 86_400_000_000_000i64,     // days
                    'w' => 604_800_000_000_000i64,    // weeks
                    'y' => 31_536_000_000_000_000i64, // years (365 days)
                    _ => return Err(anyhow!("Unknown duration unit: {}", c)),
                };

                total_ns += (num * multiplier as f64) as i64;
                current_num.clear();
            }
        }

        if !current_num.is_empty() {
            return Err(anyhow!("Duration must end with a unit: {}", s));
        }

        Ok(Duration {
            nanoseconds: total_ns,
        })
    }

    /// Extract content inside parentheses
    fn extract_parentheses(s: &str) -> Result<&str> {
        let s = s.trim();
        if !s.starts_with('(') {
            return Err(anyhow!("Expected opening parenthesis"));
        }

        let mut depth = 0;
        for (i, c) in s.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(&s[1..i]);
                    }
                }
                _ => {}
            }
        }

        Err(anyhow!("Unmatched parentheses"))
    }
}

/// PromQL expression AST
#[derive(Debug, Clone)]
pub enum PromQLExpr {
    /// Vector selector (instant or range)
    VectorSelector {
        name: String,
        matchers: Vec<LabelMatcher>,
        range: Option<Duration>,
        offset: Option<Duration>,
    },
    /// Aggregation operation
    Aggregation {
        op: AggregationOp,
        expr: Box<PromQLExpr>,
        by: Vec<String>,
        without: bool,
        param: Option<f64>,
    },
    /// Binary operation
    Binary {
        op: BinaryOp,
        lhs: Box<PromQLExpr>,
        rhs: Box<PromQLExpr>,
        matching: Option<VectorMatching>,
    },
    /// Scalar value
    Scalar(f64),
}

/// Label matcher
#[derive(Debug, Clone)]
pub struct LabelMatcher {
    pub name: String,
    pub op: MatchOp,
    pub value: String,
}

/// Label match operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchOp {
    Equal,
    NotEqual,
    Regex,
    NotRegex,
}

/// Aggregation operator
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggregationOp {
    Sum,
    Avg,
    Min,
    Max,
    Count,
    Stddev,
    Rate,
    Irate,
    Increase,
    HistogramQuantile,
    TopK,
    BottomK,
    CountValues,
    Quantile,
}

/// Binary operator
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
    And,
    Or,
    Unless,
}

/// Vector matching configuration
#[derive(Debug, Clone)]
pub struct VectorMatching {
    pub on: Vec<String>,
    pub ignoring: Vec<String>,
    pub group_left: bool,
    pub group_right: bool,
}

/// Duration in nanoseconds
#[derive(Debug, Clone, Copy)]
pub struct Duration {
    pub nanoseconds: i64,
}

impl Duration {
    pub fn from_minutes(m: i64) -> Self {
        Self {
            nanoseconds: m * 60_000_000_000,
        }
    }

    pub fn from_hours(h: i64) -> Self {
        Self {
            nanoseconds: h * 3_600_000_000_000,
        }
    }

    pub fn as_seconds(&self) -> f64 {
        self.nanoseconds as f64 / 1_000_000_000.0
    }
}

/// PromQL query executor
pub struct PromQLExecutor;

impl PromQLExecutor {
    /// Execute a PromQL expression against metric samples
    pub fn execute(
        expr: &PromQLExpr,
        samples: Vec<MetricSample>,
        eval_time_ns: i64,
        lookback_ns: i64,
    ) -> Result<Vec<MetricResult>> {
        match expr {
            PromQLExpr::VectorSelector {
                name,
                matchers,
                range,
                offset,
            } => Self::execute_vector_selector(
                name,
                matchers,
                range,
                offset,
                samples,
                eval_time_ns,
                lookback_ns,
            ),
            PromQLExpr::Aggregation {
                op,
                expr,
                by,
                without,
                param,
            } => {
                let inner_results = Self::execute(expr, samples, eval_time_ns, lookback_ns)?;
                Self::execute_aggregation(op, inner_results, by, *without, *param)
            }
            PromQLExpr::Binary {
                op,
                lhs,
                rhs,
                matching,
            } => {
                // For binary operations, we'd need to execute both sides
                // This is simplified - full implementation would handle vector matching
                let lhs_results = Self::execute(lhs, samples.clone(), eval_time_ns, lookback_ns)?;
                let rhs_results = Self::execute(rhs, samples, eval_time_ns, lookback_ns)?;
                Self::execute_binary(op, lhs_results, rhs_results, matching)
            }
            PromQLExpr::Scalar(v) => Ok(vec![MetricResult {
                timestamp_ns: eval_time_ns,
                value: *v,
                labels: HashMap::new(),
            }]),
        }
    }

    /// Execute vector selector
    fn execute_vector_selector(
        name: &str,
        matchers: &[LabelMatcher],
        range: &Option<Duration>,
        offset: &Option<Duration>,
        samples: Vec<MetricSample>,
        eval_time_ns: i64,
        lookback_ns: i64,
    ) -> Result<Vec<MetricResult>> {
        let offset_ns = offset.map(|d| d.nanoseconds).unwrap_or(0);
        let effective_time = eval_time_ns - offset_ns;

        // Filter samples by name and labels
        let filtered: Vec<_> = samples
            .into_iter()
            .filter(|s| {
                // Name match
                if s.name != name {
                    return false;
                }

                // Label matchers
                for matcher in matchers {
                    let label_value = s
                        .labels
                        .get(&matcher.name)
                        .map_or("", |s| s.as_str());
                    let matches = match matcher.op {
                        MatchOp::Equal => label_value == matcher.value,
                        MatchOp::NotEqual => label_value != matcher.value,
                        MatchOp::Regex => regex::Regex::new(&matcher.value)
                            .map(|re| re.is_match(label_value))
                            .unwrap_or(false),
                        MatchOp::NotRegex => regex::Regex::new(&matcher.value)
                            .map(|re| !re.is_match(label_value))
                            .unwrap_or(true),
                    };
                    if !matches {
                        return false;
                    }
                }

                // Time range
                if let Some(range) = range {
                    let start = effective_time - range.nanoseconds;
                    s.timestamp_ns >= start && s.timestamp_ns <= effective_time
                } else {
                    // Instant vector - use lookback
                    let start = effective_time - lookback_ns;
                    s.timestamp_ns >= start && s.timestamp_ns <= effective_time
                }
            })
            .collect();

        // For range vectors, we return all samples; for instant vectors, we return the latest per series
        if range.is_some() {
            // Range vector - return all samples
            Ok(filtered
                .into_iter()
                .map(|s| MetricResult {
                    timestamp_ns: s.timestamp_ns,
                    value: s.value,
                    labels: s.labels,
                })
                .collect())
        } else {
            // Instant vector - return latest sample per label set
            let mut latest: HashMap<String, MetricResult> = HashMap::new();
            for s in filtered {
                let key = Self::labels_to_key(&s.labels);
                let result = MetricResult {
                    timestamp_ns: s.timestamp_ns,
                    value: s.value,
                    labels: s.labels,
                };
                latest
                    .entry(key)
                    .and_modify(|existing| {
                        if result.timestamp_ns > existing.timestamp_ns {
                            *existing = result.clone();
                        }
                    })
                    .or_insert(result);
            }
            Ok(latest.into_values().collect())
        }
    }

    /// Execute aggregation operation
    fn execute_aggregation(
        op: &AggregationOp,
        results: Vec<MetricResult>,
        by: &[String],
        without: bool,
        param: Option<f64>,
    ) -> Result<Vec<MetricResult>> {
        // Group results by label set
        let mut groups: HashMap<String, Vec<MetricResult>> = HashMap::new();

        for result in results {
            let key = if !by.is_empty() {
                if without {
                    // Keep all labels except those in 'by'
                    let kept: HashMap<_, _> = result
                        .labels
                        .iter()
                        .filter(|(k, _)| !by.contains(k))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    Self::labels_to_key(&kept)
                } else {
                    // Keep only labels in 'by'
                    let kept: HashMap<_, _> = result
                        .labels
                        .iter()
                        .filter(|(k, _)| by.contains(k))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    Self::labels_to_key(&kept)
                }
            } else {
                // Aggregate all into one group
                String::new()
            };

            groups.entry(key).or_default().push(result);
        }

        // Apply aggregation to each group
        let mut output = Vec::new();
        for (_, group) in groups {
            if group.is_empty() {
                continue;
            }

            let labels = if !by.is_empty() && !without {
                // Keep only 'by' labels
                group[0]
                    .labels
                    .iter()
                    .filter(|(k, _)| by.contains(k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            } else if !by.is_empty() && without {
                // Remove 'by' labels
                group[0]
                    .labels
                    .iter()
                    .filter(|(k, _)| !by.contains(k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            } else {
                HashMap::new()
            };

            let values: Vec<f64> = group.iter().map(|r| r.value).collect();
            let timestamp_ns = group.iter().map(|r| r.timestamp_ns).max().unwrap_or(0);

            let value = match op {
                AggregationOp::Sum => values.iter().sum(),
                AggregationOp::Avg => values.iter().sum::<f64>() / values.len() as f64,
                AggregationOp::Min => values.iter().cloned().fold(f64::INFINITY, f64::min),
                AggregationOp::Max => values.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                AggregationOp::Count => values.len() as f64,
                AggregationOp::Stddev => {
                    let mean = values.iter().sum::<f64>() / values.len() as f64;
                    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>()
                        / values.len() as f64;
                    variance.sqrt()
                }
                AggregationOp::Rate | AggregationOp::Irate | AggregationOp::Increase => {
                    // Rate needs ordered samples by time
                    let mut ordered: Vec<_> = group.iter().collect();
                    ordered.sort_by_key(|r| r.timestamp_ns);
                    if ordered.len() < 2 {
                        0.0
                    } else {
                        let first = ordered.first().ok_or_else(|| {
                            anyhow!("ordered vec should not be empty (checked by len() >= 2 above")
                        })?;
                        let last = ordered.last().ok_or_else(|| {
                            anyhow!("ordered vec should not be empty (checked by len() >= 2 above")
                        })?;
                        let time_diff_s = (last.timestamp_ns - first.timestamp_ns) as f64 / 1e9;
                        if time_diff_s <= 0.0 {
                            0.0
                        } else {
                            match op {
                                AggregationOp::Rate => (last.value - first.value) / time_diff_s,
                                AggregationOp::Irate => {
                                    // Use last two samples
                                    if ordered.len() >= 2 {
                                        let prev = ordered[ordered.len() - 2];
                                        let time_diff =
                                            (last.timestamp_ns - prev.timestamp_ns) as f64 / 1e9;
                                        if time_diff > 0.0 {
                                            (last.value - prev.value) / time_diff
                                        } else {
                                            0.0
                                        }
                                    } else {
                                        0.0
                                    }
                                }
                                AggregationOp::Increase => last.value - first.value,
                                _ => 0.0,
                            }
                        }
                    }
                }
                AggregationOp::Quantile | AggregationOp::HistogramQuantile => {
                    let quantile = param.unwrap_or(0.5);
                    let mut sorted = values.clone();
                    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    let index = (quantile * (sorted.len() - 1) as f64) as usize;
                    sorted.get(index).copied().unwrap_or(0.0)
                }
                AggregationOp::TopK | AggregationOp::BottomK => {
                    // TopK/BottomK return multiple values - for now return first
                    values.first().copied().unwrap_or(0.0)
                }
                AggregationOp::CountValues => {
                    // Count unique values
                    let mut unique: std::collections::HashSet<u64> =
                        std::collections::HashSet::new();
                    for v in &values {
                        unique.insert(v.to_bits());
                    }
                    unique.len() as f64
                }
            };

            output.push(MetricResult {
                timestamp_ns,
                value,
                labels,
            });
        }

        Ok(output)
    }

    /// Execute binary operation
    fn execute_binary(
        op: &BinaryOp,
        lhs: Vec<MetricResult>,
        rhs: Vec<MetricResult>,
        _matching: &Option<VectorMatching>,
    ) -> Result<Vec<MetricResult>> {
        // Simplified: match on label sets and apply operation
        // Full implementation would handle on/ignoring/group_left/group_right

        let mut output = Vec::new();

        for l in &lhs {
            for r in &rhs {
                // Check if labels match (simplified - should use matching config)
                if l.labels == r.labels {
                    let value = match op {
                        BinaryOp::Add => l.value + r.value,
                        BinaryOp::Sub => l.value - r.value,
                        BinaryOp::Mul => l.value * r.value,
                        BinaryOp::Div => {
                            if r.value != 0.0 {
                                l.value / r.value
                            } else {
                                f64::NAN
                            }
                        }
                        BinaryOp::Mod => {
                            if r.value != 0.0 {
                                l.value % r.value
                            } else {
                                f64::NAN
                            }
                        }
                        BinaryOp::Pow => l.value.powf(r.value),
                        BinaryOp::Eq => {
                            if (l.value - r.value).abs() < f64::EPSILON {
                                1.0
                            } else {
                                0.0
                            }
                        }
                        BinaryOp::Ne => {
                            if (l.value - r.value).abs() >= f64::EPSILON {
                                1.0
                            } else {
                                0.0
                            }
                        }
                        BinaryOp::Gt => {
                            if l.value > r.value {
                                1.0
                            } else {
                                0.0
                            }
                        }
                        BinaryOp::Lt => {
                            if l.value < r.value {
                                1.0
                            } else {
                                0.0
                            }
                        }
                        BinaryOp::Ge => {
                            if l.value >= r.value {
                                1.0
                            } else {
                                0.0
                            }
                        }
                        BinaryOp::Le => {
                            if l.value <= r.value {
                                1.0
                            } else {
                                0.0
                            }
                        }
                        BinaryOp::And => l.value, // Return lhs if both exist
                        BinaryOp::Or => l.value,
                        BinaryOp::Unless => continue, // Skip matching pairs
                    };

                    output.push(MetricResult {
                        timestamp_ns: l.timestamp_ns.max(r.timestamp_ns),
                        value,
                        labels: l.labels.clone(),
                    });
                }
            }
        }

        // Handle OR and UNLESS
        if *op == BinaryOp::Or {
            // Add rhs elements that don't have matching lhs
            for r in &rhs {
                if !lhs.iter().any(|l| l.labels == r.labels) {
                    output.push(r.clone());
                }
            }
        }

        Ok(output)
    }

    /// Convert labels to a consistent string key
    fn labels_to_key(labels: &HashMap<String, String>) -> String {
        let mut pairs: Vec<_> = labels.iter().collect();
        pairs.sort_by_key(|(k, _)| *k);
        pairs
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        let d = PromQLParser::parse_duration("5m").expect("duration should parse");
        assert_eq!(d.nanoseconds, 5 * 60_000_000_000);

        let d = PromQLParser::parse_duration("1h").expect("duration should parse");
        assert_eq!(d.nanoseconds, 3_600_000_000_000);

        let d = PromQLParser::parse_duration("30s").expect("duration should parse");
        assert_eq!(d.nanoseconds, 30_000_000_000);

        let d = PromQLParser::parse_duration("1d").expect("duration should parse");
        assert_eq!(d.nanoseconds, 86_400_000_000_000);
    }

    #[test]
    fn test_parse_simple_selector() {
        let expr = PromQLParser::parse("http_requests_total").expect("should parse selector");
        match expr {
            PromQLExpr::VectorSelector {
                name,
                matchers,
                range,
                ..
            } => {
                assert_eq!(name, "http_requests_total");
                assert!(matchers.is_empty());
                assert!(range.is_none());
            }
            _ => panic!("Expected VectorSelector"),
        }
    }

    #[test]
    fn test_parse_selector_with_labels() {
        let expr = PromQLParser::parse(r#"http_requests_total{method="GET", status="200"}"#)
            .expect("should parse selector with labels");
        match expr {
            PromQLExpr::VectorSelector { name, matchers, .. } => {
                assert_eq!(name, "http_requests_total");
                assert_eq!(matchers.len(), 2);
                assert_eq!(matchers[0].name, "method");
                assert_eq!(matchers[0].value, "GET");
                assert_eq!(matchers[1].name, "status");
                assert_eq!(matchers[1].value, "200");
            }
            _ => panic!("Expected VectorSelector"),
        }
    }

    #[test]
    fn test_parse_range_vector() {
        let expr =
            PromQLParser::parse("http_requests_total[5m]").expect("should parse range vector");
        match expr {
            PromQLExpr::VectorSelector { name, range, .. } => {
                assert_eq!(name, "http_requests_total");
                assert!(range.is_some());
                let range = range.expect("range should be Some");
                assert_eq!(range.nanoseconds, 5 * 60_000_000_000);
            }
            _ => panic!("Expected VectorSelector"),
        }
    }

    #[test]
    fn test_parse_aggregation_sum() {
        let expr =
            PromQLParser::parse("sum(http_requests_total)").expect("should parse sum aggregation");
        match expr {
            PromQLExpr::Aggregation { op, .. } => {
                assert_eq!(op, AggregationOp::Sum);
            }
            _ => panic!("Expected Aggregation"),
        }
    }

    #[test]
    fn test_parse_aggregation_with_by() {
        let expr = PromQLParser::parse("sum(http_requests_total) by (method)")
            .expect("should parse aggregation with by clause");
        match expr {
            PromQLExpr::Aggregation {
                op, by, without, ..
            } => {
                assert_eq!(op, AggregationOp::Sum);
                assert_eq!(by, vec!["method"]);
                assert!(!without);
            }
            _ => panic!("Expected Aggregation"),
        }
    }

    #[test]
    fn test_parse_rate() {
        let expr = PromQLParser::parse("rate(http_requests_total[5m])")
            .expect("should parse rate function");
        match expr {
            PromQLExpr::Aggregation { op, expr, .. } => {
                assert_eq!(op, AggregationOp::Rate);
                match *expr {
                    PromQLExpr::VectorSelector { range, .. } => {
                        assert!(range.is_some());
                    }
                    _ => panic!("Expected range vector inside rate"),
                }
            }
            _ => panic!("Expected Aggregation"),
        }
    }

    #[test]
    fn test_execute_sum() {
        let samples = vec![
            MetricSample {
                name: "http_requests".to_string(),
                timestamp_ns: 1000,
                value: 10.0,
                labels: HashMap::from([("method".to_string(), "GET".to_string())]),
            },
            MetricSample {
                name: "http_requests".to_string(),
                timestamp_ns: 1000,
                value: 20.0,
                labels: HashMap::from([("method".to_string(), "POST".to_string())]),
            },
        ];

        let expr = PromQLParser::parse("sum(http_requests)").expect("should parse sum");
        let results =
            PromQLExecutor::execute(&expr, samples, 2000, 5000).expect("should execute sum");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, 30.0);
    }

    #[test]
    fn test_execute_sum_by() {
        // Each sample has different instance label (different series) but same method
        // PromQL sum() by (method) should aggregate across different instances
        let samples = vec![
            MetricSample {
                name: "http_requests".to_string(),
                timestamp_ns: 3000,
                value: 10.0,
                labels: HashMap::from([
                    ("method".to_string(), "GET".to_string()),
                    ("instance".to_string(), "server1".to_string()),
                ]),
            },
            MetricSample {
                name: "http_requests".to_string(),
                timestamp_ns: 3000,
                value: 5.0,
                labels: HashMap::from([
                    ("method".to_string(), "GET".to_string()),
                    ("instance".to_string(), "server2".to_string()),
                ]),
            },
            MetricSample {
                name: "http_requests".to_string(),
                timestamp_ns: 3000,
                value: 20.0,
                labels: HashMap::from([
                    ("method".to_string(), "POST".to_string()),
                    ("instance".to_string(), "server1".to_string()),
                ]),
            },
        ];

        let expr = PromQLParser::parse("sum(http_requests) by (method)")
            .expect("should parse sum by method");
        // eval_time_ns must be >= sample timestamps, lookback_ns is how far back to look
        let results = PromQLExecutor::execute(&expr, samples, 4000, 5000)
            .expect("should execute sum by method");

        assert_eq!(results.len(), 2);
        // Results should be grouped by method
        let get_result = results
            .iter()
            .find(|r| r.labels.get("method") == Some(&"GET".to_string()));
        let post_result = results
            .iter()
            .find(|r| r.labels.get("method") == Some(&"POST".to_string()));

        assert!(get_result.is_some());
        assert!(post_result.is_some());
        let get_result = get_result.expect("GET result should exist");
        let post_result = post_result.expect("POST result should exist");
        assert_eq!(get_result.value, 15.0); // 10 + 5
        assert_eq!(post_result.value, 20.0);
    }

    #[test]
    fn test_execute_rate() {
        let samples = vec![
            MetricSample {
                name: "counter".to_string(),
                timestamp_ns: 0,
                value: 100.0,
                labels: HashMap::new(),
            },
            MetricSample {
                name: "counter".to_string(),
                timestamp_ns: 1_000_000_000, // 1 second later
                value: 110.0,
                labels: HashMap::new(),
            },
        ];

        let expr = PromQLParser::parse("rate(counter[1m])").expect("should parse rate");
        let results = PromQLExecutor::execute(&expr, samples, 2_000_000_000, 60_000_000_000)
            .expect("should execute rate");

        assert_eq!(results.len(), 1);
        // Rate should be (110 - 100) / 1 second = 10 per second
        assert!((results[0].value - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_parse_label_not_equal() {
        let expr = PromQLParser::parse(r#"http_requests{status!="500"}"#)
            .expect("should parse label not equal");
        match expr {
            PromQLExpr::VectorSelector { matchers, .. } => {
                assert_eq!(matchers.len(), 1);
                assert_eq!(matchers[0].op, MatchOp::NotEqual);
                assert_eq!(matchers[0].value, "500");
            }
            _ => panic!("Expected VectorSelector"),
        }
    }

    #[test]
    fn test_parse_label_regex() {
        let expr = PromQLParser::parse(r#"http_requests{method=~"GET|POST"}"#)
            .expect("should parse label regex");
        match expr {
            PromQLExpr::VectorSelector { matchers, .. } => {
                assert_eq!(matchers.len(), 1);
                assert_eq!(matchers[0].op, MatchOp::Regex);
                assert_eq!(matchers[0].value, "GET|POST");
            }
            _ => panic!("Expected VectorSelector"),
        }
    }
}
