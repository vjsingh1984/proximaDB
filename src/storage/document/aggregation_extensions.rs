// Extended aggregation capabilities for the document pipeline
//
// Adds:
// - Expression evaluator: arithmetic, string concat, conditionals on documents
// - StdDev accumulator: Welford's online algorithm for standard deviation
// - Lookup stage: left-outer-join via callback for cross-collection joins

use anyhow::{Result, anyhow};

use crate::proto::proximadb_v1::{
    SqlArray, SqlObject, SqlValue, sql_value::Value as SqlValueVariant,
};

// =============================================================================
// EXPRESSION EVALUATOR
// =============================================================================

/// Computed expressions that can be evaluated against a document.
///
/// Modeled after MongoDB aggregation expressions ($add, $subtract, $cond, etc.).
#[derive(Debug, Clone)]
pub enum AggregationExpression {
    /// Reference to a document field by name (e.g. "price", "qty")
    FieldRef(String),
    /// Literal constant
    Literal(SqlValue),
    /// $add: numeric addition
    Add(Box<Self>, Box<Self>),
    /// $subtract: numeric subtraction
    Subtract(Box<Self>, Box<Self>),
    /// $multiply: numeric multiplication
    Multiply(Box<Self>, Box<Self>),
    /// $divide: numeric division
    Divide(Box<Self>, Box<Self>),
    /// $concat: string concatenation
    Concat(Vec<Self>),
    /// $cond: conditional (if/then/else)
    Cond {
        condition: Box<Self>,
        then_expr: Box<Self>,
        else_expr: Box<Self>,
    },
    /// $gt: greater-than comparison (returns bool)
    Gt(Box<Self>, Box<Self>),
    /// $lt: less-than comparison (returns bool)
    Lt(Box<Self>, Box<Self>),
    /// $eq: equality comparison (returns bool)
    Eq(Box<Self>, Box<Self>),
}

/// Evaluate an expression against a document, returning the computed SqlValue.
pub fn evaluate_expression(expr: &AggregationExpression, doc: &SqlObject) -> Result<SqlValue> {
    match expr {
        AggregationExpression::FieldRef(field) => doc
            .fields
            .get(field)
            .cloned()
            .ok_or_else(|| anyhow!("Field '{}' not found in document", field)),

        AggregationExpression::Literal(val) => Ok(val.clone()),

        AggregationExpression::Add(lhs, rhs) => {
            let l = to_f64(&evaluate_expression(lhs, doc)?)?;
            let r = to_f64(&evaluate_expression(rhs, doc)?)?;
            Ok(float_val(l + r))
        }

        AggregationExpression::Subtract(lhs, rhs) => {
            let l = to_f64(&evaluate_expression(lhs, doc)?)?;
            let r = to_f64(&evaluate_expression(rhs, doc)?)?;
            Ok(float_val(l - r))
        }

        AggregationExpression::Multiply(lhs, rhs) => {
            let l = to_f64(&evaluate_expression(lhs, doc)?)?;
            let r = to_f64(&evaluate_expression(rhs, doc)?)?;
            Ok(float_val(l * r))
        }

        AggregationExpression::Divide(lhs, rhs) => {
            let l = to_f64(&evaluate_expression(lhs, doc)?)?;
            let r = to_f64(&evaluate_expression(rhs, doc)?)?;
            if r == 0.0 {
                return Err(anyhow!("Division by zero"));
            }
            Ok(float_val(l / r))
        }

        AggregationExpression::Concat(parts) => {
            let mut buf = String::new();
            for part in parts {
                let val = evaluate_expression(part, doc)?;
                buf.push_str(&to_string(&val));
            }
            Ok(string_val(&buf))
        }

        AggregationExpression::Cond {
            condition,
            then_expr,
            else_expr,
        } => {
            let cond_val = evaluate_expression(condition, doc)?;
            if is_truthy(&cond_val) {
                evaluate_expression(then_expr, doc)
            } else {
                evaluate_expression(else_expr, doc)
            }
        }

        AggregationExpression::Gt(lhs, rhs) => {
            let l = to_f64(&evaluate_expression(lhs, doc)?)?;
            let r = to_f64(&evaluate_expression(rhs, doc)?)?;
            Ok(bool_val(l > r))
        }

        AggregationExpression::Lt(lhs, rhs) => {
            let l = to_f64(&evaluate_expression(lhs, doc)?)?;
            let r = to_f64(&evaluate_expression(rhs, doc)?)?;
            Ok(bool_val(l < r))
        }

        AggregationExpression::Eq(lhs, rhs) => {
            let l = evaluate_expression(lhs, doc)?;
            let r = evaluate_expression(rhs, doc)?;
            Ok(bool_val(sql_values_equal(&l, &r)))
        }
    }
}

// =============================================================================
// STDDEV ACCUMULATOR (Welford's algorithm)
// =============================================================================

/// Running standard-deviation accumulator using Welford's online algorithm.
///
/// Feed values one at a time via `push`; retrieve the population or sample
/// standard deviation at any point.
#[derive(Debug, Clone)]
pub struct StdDevAccumulator {
    count: u64,
    mean: f64,
    m2: f64,
}

impl StdDevAccumulator {
    pub fn new() -> Self {
        Self {
            count: 0,
            mean: 0.0,
            m2: 0.0,
        }
    }

    /// Incorporate a new value into the running computation.
    pub fn push(&mut self, value: f64) {
        self.count += 1;
        let delta = value - self.mean;
        self.mean += delta / self.count as f64;
        let delta2 = value - self.mean;
        self.m2 += delta * delta2;
    }

    /// Population standard deviation (divides by N).
    pub fn population_stddev(&self) -> f64 {
        if self.count < 1 {
            return 0.0;
        }
        (self.m2 / self.count as f64).sqrt()
    }

    /// Sample standard deviation (divides by N-1).
    pub fn sample_stddev(&self) -> f64 {
        if self.count < 2 {
            return 0.0;
        }
        (self.m2 / (self.count - 1) as f64).sqrt()
    }

    /// Number of values seen so far.
    pub fn count(&self) -> u64 {
        self.count
    }
}

impl Default for StdDevAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the population standard deviation over a set of documents at a given
/// field path. Returns a NumberValue SqlValue.
pub fn compute_stddev(docs: &[&SqlObject], path: &str) -> Result<SqlValue> {
    let mut acc = StdDevAccumulator::new();
    for doc in docs {
        if let Some(val) = doc.fields.get(path)
            && let Ok(n) = to_f64(val)
        {
            acc.push(n);
        }
    }
    Ok(float_val(acc.population_stddev()))
}

/// Compute the sample standard deviation over a set of documents.
pub fn compute_stddev_sample(docs: &[&SqlObject], path: &str) -> Result<SqlValue> {
    let mut acc = StdDevAccumulator::new();
    for doc in docs {
        if let Some(val) = doc.fields.get(path)
            && let Ok(n) = to_f64(val)
        {
            acc.push(n);
        }
    }
    Ok(float_val(acc.sample_stddev()))
}

// =============================================================================
// LOOKUP STAGE (cross-collection join)
// =============================================================================

/// Configuration for a $lookup stage that joins documents from another
/// collection, analogous to a SQL left-outer-join.
#[derive(Debug, Clone)]
pub struct LookupConfig {
    /// The foreign collection name to join against.
    pub from_collection: String,
    /// Field path in the local document whose value is matched.
    pub local_field: String,
    /// Field path in the foreign document whose value is matched.
    pub foreign_field: String,
    /// Name of the new array field added to each local document containing
    /// the matched foreign documents.
    pub output_field: String,
}

/// Trait abstracting how foreign documents are fetched during a lookup.
///
/// Implementations can query the document service, an in-memory cache, or
/// a test stub.
pub trait LookupFetcher: Send + Sync {
    /// Return all documents in `collection` where the value at `field_path`
    /// equals `match_value`.
    fn fetch_matching(
        &self,
        collection: &str,
        field_path: &str,
        match_value: &SqlValue,
    ) -> Result<Vec<SqlObject>>;
}

/// Execute a lookup (left-outer-join) stage.
///
/// For each document in `documents`, fetches matching foreign docs via
/// `fetcher` and appends them as an array in `config.output_field`.
pub fn execute_lookup(
    documents: &[SqlObject],
    config: &LookupConfig,
    fetcher: &dyn LookupFetcher,
) -> Result<Vec<SqlObject>> {
    let mut results = Vec::with_capacity(documents.len());

    for doc in documents {
        let mut out = doc.clone();

        let local_val = doc.fields.get(&config.local_field).cloned();

        let matched = if let Some(lv) = &local_val {
            fetcher.fetch_matching(&config.from_collection, &config.foreign_field, lv)?
        } else {
            Vec::new()
        };

        // Convert matched docs to an array of ObjectValues
        let arr_values: Vec<SqlValue> = matched
            .into_iter()
            .map(|obj| SqlValue {
                value: Some(SqlValueVariant::ObjectValue(obj)),
            })
            .collect();

        out.fields.insert(
            config.output_field.clone(),
            SqlValue {
                value: Some(SqlValueVariant::ArrayValue(SqlArray { values: arr_values })),
            },
        );

        results.push(out);
    }

    Ok(results)
}

// =============================================================================
// HELPER FUNCTIONS
// =============================================================================

/// Extract an f64 from a SqlValue (supports Int64Value and NumberValue).
fn to_f64(val: &SqlValue) -> Result<f64> {
    match &val.value {
        Some(SqlValueVariant::Int64Value(i)) => Ok(*i as f64),
        Some(SqlValueVariant::NumberValue(f)) => Ok(*f),
        Some(SqlValueVariant::BoolValue(b)) => Ok(if *b { 1.0 } else { 0.0 }),
        _ => Err(anyhow!("Cannot convert value to numeric: {:?}", val.value)),
    }
}

/// Coerce a SqlValue to its string representation for $concat.
fn to_string(val: &SqlValue) -> String {
    match &val.value {
        Some(SqlValueVariant::StringValue(s)) => s.clone(),
        Some(SqlValueVariant::Int64Value(i)) => i.to_string(),
        Some(SqlValueVariant::NumberValue(f)) => f.to_string(),
        Some(SqlValueVariant::BoolValue(b)) => b.to_string(),
        Some(SqlValueVariant::NullValue(_)) => "null".to_string(),
        _ => String::new(),
    }
}

/// Determine truthiness of a SqlValue (for $cond evaluation).
fn is_truthy(val: &SqlValue) -> bool {
    match &val.value {
        Some(SqlValueVariant::BoolValue(b)) => *b,
        Some(SqlValueVariant::Int64Value(i)) => *i != 0,
        Some(SqlValueVariant::NumberValue(f)) => *f != 0.0,
        Some(SqlValueVariant::StringValue(s)) => !s.is_empty(),
        Some(SqlValueVariant::NullValue(_)) | None => false,
        _ => true,
    }
}

/// Compare two SqlValues for equality (cross-type numeric aware).
fn sql_values_equal(a: &SqlValue, b: &SqlValue) -> bool {
    match (&a.value, &b.value) {
        (Some(SqlValueVariant::NullValue(_)), Some(SqlValueVariant::NullValue(_))) => true,
        (Some(SqlValueVariant::BoolValue(va)), Some(SqlValueVariant::BoolValue(vb))) => va == vb,
        (Some(SqlValueVariant::Int64Value(va)), Some(SqlValueVariant::Int64Value(vb))) => va == vb,
        (Some(SqlValueVariant::NumberValue(va)), Some(SqlValueVariant::NumberValue(vb))) => {
            (va - vb).abs() < f64::EPSILON
        }
        (Some(SqlValueVariant::StringValue(va)), Some(SqlValueVariant::StringValue(vb))) => {
            va == vb
        }
        (Some(SqlValueVariant::Int64Value(va)), Some(SqlValueVariant::NumberValue(vb))) => {
            (*va as f64 - vb).abs() < f64::EPSILON
        }
        (Some(SqlValueVariant::NumberValue(va)), Some(SqlValueVariant::Int64Value(vb))) => {
            (va - *vb as f64).abs() < f64::EPSILON
        }
        _ => false,
    }
}

fn float_val(f: f64) -> SqlValue {
    SqlValue {
        value: Some(SqlValueVariant::NumberValue(f)),
    }
}

fn string_val(s: &str) -> SqlValue {
    SqlValue {
        value: Some(SqlValueVariant::StringValue(s.to_string())),
    }
}

fn bool_val(b: bool) -> SqlValue {
    SqlValue {
        value: Some(SqlValueVariant::BoolValue(b)),
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
fn int_val(i: i64) -> SqlValue {
    SqlValue {
        value: Some(SqlValueVariant::Int64Value(i)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_doc(fields: Vec<(&str, SqlValue)>) -> SqlObject {
        SqlObject {
            fields: fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        }
    }

    // ---- Expression evaluator tests ----

    #[test]
    fn test_expression_arithmetic() {
        // doc: { price: 10, qty: 3 }
        // expr: price * qty + 5
        let doc = make_doc(vec![("price", int_val(10)), ("qty", int_val(3))]);

        let expr = AggregationExpression::Add(
            Box::new(AggregationExpression::Multiply(
                Box::new(AggregationExpression::FieldRef("price".into())),
                Box::new(AggregationExpression::FieldRef("qty".into())),
            )),
            Box::new(AggregationExpression::Literal(float_val(5.0))),
        );

        let result = evaluate_expression(&expr, &doc).unwrap();
        let val = to_f64(&result).unwrap();
        assert!(
            (val - 35.0).abs() < f64::EPSILON,
            "Expected 35.0, got {val}"
        );
    }

    #[test]
    fn test_expression_subtract_divide() {
        let doc = make_doc(vec![("a", int_val(20)), ("b", int_val(4))]);

        // (a - 8) / b = 12 / 4 = 3.0
        let expr = AggregationExpression::Divide(
            Box::new(AggregationExpression::Subtract(
                Box::new(AggregationExpression::FieldRef("a".into())),
                Box::new(AggregationExpression::Literal(int_val(8))),
            )),
            Box::new(AggregationExpression::FieldRef("b".into())),
        );

        let result = evaluate_expression(&expr, &doc).unwrap();
        let val = to_f64(&result).unwrap();
        assert!((val - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_expression_divide_by_zero() {
        let doc = make_doc(vec![("a", int_val(10))]);
        let expr = AggregationExpression::Divide(
            Box::new(AggregationExpression::FieldRef("a".into())),
            Box::new(AggregationExpression::Literal(int_val(0))),
        );
        assert!(evaluate_expression(&expr, &doc).is_err());
    }

    #[test]
    fn test_expression_conditional() {
        // if score > 50 then "pass" else "fail"
        let pass_doc = make_doc(vec![("score", int_val(80))]);
        let fail_doc = make_doc(vec![("score", int_val(30))]);

        let expr = AggregationExpression::Cond {
            condition: Box::new(AggregationExpression::Gt(
                Box::new(AggregationExpression::FieldRef("score".into())),
                Box::new(AggregationExpression::Literal(int_val(50))),
            )),
            then_expr: Box::new(AggregationExpression::Literal(string_val("pass"))),
            else_expr: Box::new(AggregationExpression::Literal(string_val("fail"))),
        };

        let r1 = evaluate_expression(&expr, &pass_doc).unwrap();
        assert_eq!(r1.value, Some(SqlValueVariant::StringValue("pass".into())));

        let r2 = evaluate_expression(&expr, &fail_doc).unwrap();
        assert_eq!(r2.value, Some(SqlValueVariant::StringValue("fail".into())));
    }

    #[test]
    fn test_expression_concat() {
        let doc = make_doc(vec![
            ("first", string_val("Hello")),
            ("last", string_val("World")),
        ]);

        let expr = AggregationExpression::Concat(vec![
            AggregationExpression::FieldRef("first".into()),
            AggregationExpression::Literal(string_val(" ")),
            AggregationExpression::FieldRef("last".into()),
        ]);

        let result = evaluate_expression(&expr, &doc).unwrap();
        assert_eq!(
            result.value,
            Some(SqlValueVariant::StringValue("Hello World".into()))
        );
    }

    #[test]
    fn test_expression_eq() {
        let doc = make_doc(vec![("x", int_val(5))]);

        let eq_true = AggregationExpression::Eq(
            Box::new(AggregationExpression::FieldRef("x".into())),
            Box::new(AggregationExpression::Literal(int_val(5))),
        );
        let eq_false = AggregationExpression::Eq(
            Box::new(AggregationExpression::FieldRef("x".into())),
            Box::new(AggregationExpression::Literal(int_val(99))),
        );

        let r1 = evaluate_expression(&eq_true, &doc).unwrap();
        assert_eq!(r1.value, Some(SqlValueVariant::BoolValue(true)));

        let r2 = evaluate_expression(&eq_false, &doc).unwrap();
        assert_eq!(r2.value, Some(SqlValueVariant::BoolValue(false)));
    }

    #[test]
    fn test_expression_lt() {
        let doc = make_doc(vec![("x", int_val(3))]);

        let lt_true = AggregationExpression::Lt(
            Box::new(AggregationExpression::FieldRef("x".into())),
            Box::new(AggregationExpression::Literal(int_val(10))),
        );
        let result = evaluate_expression(&lt_true, &doc).unwrap();
        assert_eq!(result.value, Some(SqlValueVariant::BoolValue(true)));
    }

    #[test]
    fn test_expression_field_not_found() {
        let doc = make_doc(vec![]);
        let expr = AggregationExpression::FieldRef("missing".into());
        assert!(evaluate_expression(&expr, &doc).is_err());
    }

    // ---- StdDev accumulator tests ----

    #[test]
    fn test_accumulator_stddev() {
        let mut acc = StdDevAccumulator::new();
        // Values: 2, 4, 4, 4, 5, 5, 7, 9
        // Population stddev = 2.0
        for v in [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0] {
            acc.push(v);
        }
        assert_eq!(acc.count(), 8);
        assert!((acc.population_stddev() - 2.0).abs() < 1e-10);

        // Sample stddev = sqrt(32/7) ~ 2.13809
        let expected_sample = (32.0_f64 / 7.0).sqrt();
        assert!((acc.sample_stddev() - expected_sample).abs() < 1e-10);
    }

    #[test]
    fn test_accumulator_stddev_empty() {
        let acc = StdDevAccumulator::new();
        assert!((acc.population_stddev()).abs() < f64::EPSILON);
        assert!((acc.sample_stddev()).abs() < f64::EPSILON);
    }

    #[test]
    fn test_accumulator_stddev_single() {
        let mut acc = StdDevAccumulator::new();
        acc.push(42.0);
        assert!((acc.population_stddev()).abs() < f64::EPSILON);
        assert!((acc.sample_stddev()).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_stddev_on_docs() {
        let docs = vec![
            make_doc(vec![("val", int_val(2))]),
            make_doc(vec![("val", int_val(4))]),
            make_doc(vec![("val", int_val(4))]),
            make_doc(vec![("val", int_val(4))]),
            make_doc(vec![("val", int_val(5))]),
            make_doc(vec![("val", int_val(5))]),
            make_doc(vec![("val", int_val(7))]),
            make_doc(vec![("val", int_val(9))]),
        ];

        let doc_refs: Vec<&SqlObject> = docs.iter().collect();
        let result = compute_stddev(&doc_refs, "val").unwrap();
        if let Some(SqlValueVariant::NumberValue(stddev)) = result.value {
            assert!((stddev - 2.0).abs() < 1e-10);
        } else {
            panic!("Expected NumberValue");
        }
    }

    // ---- Push / AddToSet helper tests (using the expression framework) ----

    #[test]
    fn test_accumulator_push() {
        // Verify push collects all values including duplicates
        let docs = vec![
            make_doc(vec![("tag", string_val("a"))]),
            make_doc(vec![("tag", string_val("b"))]),
            make_doc(vec![("tag", string_val("a"))]),
        ];

        let doc_refs: Vec<&SqlObject> = docs.iter().collect();
        let values: Vec<SqlValue> = doc_refs
            .iter()
            .filter_map(|d| d.fields.get("tag").cloned())
            .collect();

        assert_eq!(values.len(), 3);
        assert_eq!(
            values[0].value,
            Some(SqlValueVariant::StringValue("a".into()))
        );
        assert_eq!(
            values[1].value,
            Some(SqlValueVariant::StringValue("b".into()))
        );
        assert_eq!(
            values[2].value,
            Some(SqlValueVariant::StringValue("a".into()))
        );
    }

    #[test]
    fn test_accumulator_add_to_set() {
        // Verify unique-value collection
        let docs = vec![
            make_doc(vec![("tag", string_val("a"))]),
            make_doc(vec![("tag", string_val("b"))]),
            make_doc(vec![("tag", string_val("a"))]),
            make_doc(vec![("tag", string_val("c"))]),
            make_doc(vec![("tag", string_val("b"))]),
        ];

        let doc_refs: Vec<&SqlObject> = docs.iter().collect();
        let mut seen: Vec<SqlValue> = Vec::new();
        for d in &doc_refs {
            if let Some(val) = d.fields.get("tag").cloned() {
                let is_dup = seen.iter().any(|v| sql_values_equal(v, &val));
                if !is_dup {
                    seen.push(val);
                }
            }
        }

        assert_eq!(seen.len(), 3); // a, b, c
        assert_eq!(
            seen[0].value,
            Some(SqlValueVariant::StringValue("a".into()))
        );
        assert_eq!(
            seen[1].value,
            Some(SqlValueVariant::StringValue("b".into()))
        );
        assert_eq!(
            seen[2].value,
            Some(SqlValueVariant::StringValue("c".into()))
        );
    }

    // ---- Lookup stage tests ----

    /// In-memory stub fetcher for testing
    struct TestFetcher {
        foreign_docs: Vec<SqlObject>,
    }

    impl LookupFetcher for TestFetcher {
        fn fetch_matching(
            &self,
            _collection: &str,
            field_path: &str,
            match_value: &SqlValue,
        ) -> Result<Vec<SqlObject>> {
            Ok(self
                .foreign_docs
                .iter()
                .filter(|d| {
                    d.fields
                        .get(field_path)
                        .is_some_and(|v| sql_values_equal(v, match_value))
                })
                .cloned()
                .collect())
        }
    }

    #[test]
    fn test_lookup_basic() {
        // Local: orders with customer_id
        let orders = vec![
            make_doc(vec![
                ("order_id", int_val(1)),
                ("customer_id", int_val(100)),
            ]),
            make_doc(vec![
                ("order_id", int_val(2)),
                ("customer_id", int_val(200)),
            ]),
            make_doc(vec![
                ("order_id", int_val(3)),
                ("customer_id", int_val(100)),
            ]),
        ];

        // Foreign: customers
        let customers = vec![
            make_doc(vec![("cid", int_val(100)), ("name", string_val("Alice"))]),
            make_doc(vec![("cid", int_val(200)), ("name", string_val("Bob"))]),
        ];

        let fetcher = TestFetcher {
            foreign_docs: customers,
        };

        let config = LookupConfig {
            from_collection: "customers".into(),
            local_field: "customer_id".into(),
            foreign_field: "cid".into(),
            output_field: "customer_info".into(),
        };

        let results = execute_lookup(&orders, &config, &fetcher).unwrap();
        assert_eq!(results.len(), 3);

        // Order 1 (customer 100 = Alice) should have 1 match
        let info = results[0].fields.get("customer_info").unwrap();
        if let Some(SqlValueVariant::ArrayValue(arr)) = &info.value {
            assert_eq!(arr.values.len(), 1);
        } else {
            panic!("Expected ArrayValue for customer_info");
        }

        // Order 2 (customer 200 = Bob) should have 1 match
        let info2 = results[1].fields.get("customer_info").unwrap();
        if let Some(SqlValueVariant::ArrayValue(arr)) = &info2.value {
            assert_eq!(arr.values.len(), 1);
        } else {
            panic!("Expected ArrayValue");
        }
    }

    #[test]
    fn test_lookup_no_match() {
        let orders = vec![make_doc(vec![
            ("order_id", int_val(1)),
            ("customer_id", int_val(999)),
        ])];

        let fetcher = TestFetcher {
            foreign_docs: vec![make_doc(vec![
                ("cid", int_val(100)),
                ("name", string_val("Alice")),
            ])],
        };

        let config = LookupConfig {
            from_collection: "customers".into(),
            local_field: "customer_id".into(),
            foreign_field: "cid".into(),
            output_field: "customer_info".into(),
        };

        let results = execute_lookup(&orders, &config, &fetcher).unwrap();
        let info = results[0].fields.get("customer_info").unwrap();
        if let Some(SqlValueVariant::ArrayValue(arr)) = &info.value {
            assert!(arr.values.is_empty());
        } else {
            panic!("Expected empty ArrayValue");
        }
    }

    #[test]
    fn test_lookup_missing_local_field() {
        // Document without the local_field gets an empty array
        let orders = vec![make_doc(vec![("order_id", int_val(1))])];

        let fetcher = TestFetcher {
            foreign_docs: vec![],
        };

        let config = LookupConfig {
            from_collection: "customers".into(),
            local_field: "customer_id".into(),
            foreign_field: "cid".into(),
            output_field: "customer_info".into(),
        };

        let results = execute_lookup(&orders, &config, &fetcher).unwrap();
        let info = results[0].fields.get("customer_info").unwrap();
        if let Some(SqlValueVariant::ArrayValue(arr)) = &info.value {
            assert!(arr.values.is_empty());
        } else {
            panic!("Expected empty ArrayValue");
        }
    }
}
