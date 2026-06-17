use super::compare_sql_values;
use super::*;
use crate::proto::proximadb_v1::SqlValue;
use crate::proto::proximadb_v1::sql_value::Value as SqlVal;

#[test]
fn test_compare_sql_values_numbers() {
    let a = SqlValue {
        value: Some(SqlVal::NumberValue(1.0)),
    };
    let b = SqlValue {
        value: Some(SqlVal::NumberValue(2.0)),
    };
    assert_eq!(compare_sql_values(&a, &b), std::cmp::Ordering::Less);
    assert_eq!(compare_sql_values(&b, &a), std::cmp::Ordering::Greater);
    assert_eq!(compare_sql_values(&a, &a), std::cmp::Ordering::Equal);
}

#[test]
fn test_compare_sql_values_int64() {
    let a = SqlValue {
        value: Some(SqlVal::Int64Value(10)),
    };
    let b = SqlValue {
        value: Some(SqlVal::Int64Value(20)),
    };
    assert_eq!(compare_sql_values(&a, &b), std::cmp::Ordering::Less);
    assert_eq!(compare_sql_values(&b, &a), std::cmp::Ordering::Greater);
}

#[test]
fn test_compare_sql_values_strings() {
    let a = SqlValue {
        value: Some(SqlVal::StringValue("apple".to_string())),
    };
    let b = SqlValue {
        value: Some(SqlVal::StringValue("banana".to_string())),
    };
    assert_eq!(compare_sql_values(&a, &b), std::cmp::Ordering::Less);
}

#[test]
fn test_compare_sql_values_cross_numeric_types() {
    let int_val = SqlValue {
        value: Some(SqlVal::Int64Value(5)),
    };
    let float_val = SqlValue {
        value: Some(SqlVal::NumberValue(10.0)),
    };
    assert_eq!(
        compare_sql_values(&int_val, &float_val),
        std::cmp::Ordering::Less
    );
}

#[test]
fn test_predicate_eq_within_range() {
    use super::super::common::{ColumnEncoding, ColumnStats};

    let min = SqlValue {
        value: Some(SqlVal::NumberValue(1.0)),
    };
    let max = SqlValue {
        value: Some(SqlVal::NumberValue(10.0)),
    };
    let value = SqlValue {
        value: Some(SqlVal::NumberValue(5.0)),
    };

    // Value 5.0 is within [1.0, 10.0] — Eq should pass
    assert!(
        compare_sql_values(&value, &min) != std::cmp::Ordering::Less
            && compare_sql_values(&value, &max) != std::cmp::Ordering::Greater
    );
}

#[test]
fn test_predicate_eq_outside_range() {
    let min = SqlValue {
        value: Some(SqlVal::NumberValue(1.0)),
    };
    let max = SqlValue {
        value: Some(SqlVal::NumberValue(10.0)),
    };
    let value = SqlValue {
        value: Some(SqlVal::NumberValue(15.0)),
    };

    // Value 15.0 is outside [1.0, 10.0] — Eq should fail
    assert!(
        !(compare_sql_values(&value, &min) != std::cmp::Ordering::Less
            && compare_sql_values(&value, &max) != std::cmp::Ordering::Greater)
    );
}

#[test]
fn test_predicate_lt_pruning() {
    let min = SqlValue {
        value: Some(SqlVal::NumberValue(10.0)),
    };
    let value = SqlValue {
        value: Some(SqlVal::NumberValue(5.0)),
    };

    // Lt: min < value should be false (min=10 is NOT < value=5)
    assert_ne!(compare_sql_values(&min, &value), std::cmp::Ordering::Less);

    // Lt: min < value should be true (min=1 IS < value=5)
    let low_min = SqlValue {
        value: Some(SqlVal::NumberValue(1.0)),
    };
    assert_eq!(
        compare_sql_values(&low_min, &value),
        std::cmp::Ordering::Less
    );
}

#[test]
fn test_predicate_gt_pruning() {
    let max = SqlValue {
        value: Some(SqlVal::NumberValue(3.0)),
    };
    let value = SqlValue {
        value: Some(SqlVal::NumberValue(5.0)),
    };

    // Gt: max > value should be false (max=3 is NOT > value=5)
    assert_ne!(
        compare_sql_values(&max, &value),
        std::cmp::Ordering::Greater
    );

    // Gt: max > value should be true (max=10 IS > value=5)
    let high_max = SqlValue {
        value: Some(SqlVal::NumberValue(10.0)),
    };
    assert_eq!(
        compare_sql_values(&high_max, &value),
        std::cmp::Ordering::Greater
    );
}

// ========== NEW TESTS ==========

#[test]
fn test_compare_sql_values_null_vs_null() {
    let a = SqlValue { value: None };
    let b = SqlValue { value: None };
    // Two Nulls are incomparable, treated as Equal (conservative)
    assert_eq!(compare_sql_values(&a, &b), std::cmp::Ordering::Equal);
}

#[test]
fn test_compare_sql_values_null_vs_number() {
    let null_val = SqlValue { value: None };
    let num_val = SqlValue {
        value: Some(SqlVal::NumberValue(42.0)),
    };
    // Null vs typed value is incomparable, treated as Equal (conservative)
    assert_eq!(
        compare_sql_values(&null_val, &num_val),
        std::cmp::Ordering::Equal
    );
    assert_eq!(
        compare_sql_values(&num_val, &null_val),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn test_compare_sql_values_bool_ordering() {
    let f = SqlValue {
        value: Some(SqlVal::BoolValue(false)),
    };
    let t = SqlValue {
        value: Some(SqlVal::BoolValue(true)),
    };
    assert_eq!(compare_sql_values(&f, &t), std::cmp::Ordering::Less);
    assert_eq!(compare_sql_values(&t, &f), std::cmp::Ordering::Greater);
    assert_eq!(compare_sql_values(&t, &t), std::cmp::Ordering::Equal);
}

#[test]
fn test_compare_sql_values_int64_to_number_coercion() {
    // Int64(100) vs Number(99.5)
    let int_val = SqlValue {
        value: Some(SqlVal::Int64Value(100)),
    };
    let float_val = SqlValue {
        value: Some(SqlVal::NumberValue(99.5)),
    };
    assert_eq!(
        compare_sql_values(&int_val, &float_val),
        std::cmp::Ordering::Greater
    );

    // Reverse direction: Number vs Int64
    assert_eq!(
        compare_sql_values(&float_val, &int_val),
        std::cmp::Ordering::Less
    );
}

#[test]
fn test_compare_sql_values_int64_equal_as_float() {
    // Int64(5) vs Number(5.0) should be Equal
    let int_val = SqlValue {
        value: Some(SqlVal::Int64Value(5)),
    };
    let float_val = SqlValue {
        value: Some(SqlVal::NumberValue(5.0)),
    };
    assert_eq!(
        compare_sql_values(&int_val, &float_val),
        std::cmp::Ordering::Equal
    );
    assert_eq!(
        compare_sql_values(&float_val, &int_val),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn test_compare_sql_values_negative_numbers() {
    let neg = SqlValue {
        value: Some(SqlVal::NumberValue(-10.0)),
    };
    let pos = SqlValue {
        value: Some(SqlVal::NumberValue(10.0)),
    };
    assert_eq!(compare_sql_values(&neg, &pos), std::cmp::Ordering::Less);
    assert_eq!(compare_sql_values(&pos, &neg), std::cmp::Ordering::Greater);
}

#[test]
fn test_compare_sql_values_incompatible_types() {
    // String vs Number: incomparable, treated as Equal
    let str_val = SqlValue {
        value: Some(SqlVal::StringValue("abc".to_string())),
    };
    let num_val = SqlValue {
        value: Some(SqlVal::NumberValue(1.0)),
    };
    assert_eq!(
        compare_sql_values(&str_val, &num_val),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn test_compare_sql_values_nan_handling() {
    // NaN comparisons should not panic; partial_cmp returns None -> Equal
    let nan_val = SqlValue {
        value: Some(SqlVal::NumberValue(f64::NAN)),
    };
    let normal = SqlValue {
        value: Some(SqlVal::NumberValue(1.0)),
    };
    // NaN compared to anything should be Equal (fallback behavior)
    assert_eq!(
        compare_sql_values(&nan_val, &normal),
        std::cmp::Ordering::Equal
    );
    assert_eq!(
        compare_sql_values(&nan_val, &nan_val),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn test_compare_sql_values_infinity() {
    let pos_inf = SqlValue {
        value: Some(SqlVal::NumberValue(f64::INFINITY)),
    };
    let neg_inf = SqlValue {
        value: Some(SqlVal::NumberValue(f64::NEG_INFINITY)),
    };
    let normal = SqlValue {
        value: Some(SqlVal::NumberValue(100.0)),
    };
    assert_eq!(
        compare_sql_values(&pos_inf, &normal),
        std::cmp::Ordering::Greater
    );
    assert_eq!(
        compare_sql_values(&neg_inf, &normal),
        std::cmp::Ordering::Less
    );
    assert_eq!(
        compare_sql_values(&neg_inf, &pos_inf),
        std::cmp::Ordering::Less
    );
}

#[test]
fn test_predicate_lte_pruning() {
    let min = SqlValue {
        value: Some(SqlVal::NumberValue(5.0)),
    };
    let value = SqlValue {
        value: Some(SqlVal::NumberValue(5.0)),
    };
    // Lte: value <= max is true when value equals the bound
    assert!(compare_sql_values(&value, &min) != std::cmp::Ordering::Greater);

    let small_val = SqlValue {
        value: Some(SqlVal::NumberValue(3.0)),
    };
    assert!(compare_sql_values(&small_val, &min) != std::cmp::Ordering::Greater);

    let large_val = SqlValue {
        value: Some(SqlVal::NumberValue(7.0)),
    };
    // 7.0 > 5.0, so Lte should fail
    assert!(compare_sql_values(&large_val, &min) == std::cmp::Ordering::Greater);
}

#[test]
fn test_predicate_gte_pruning() {
    let max = SqlValue {
        value: Some(SqlVal::NumberValue(10.0)),
    };
    let value = SqlValue {
        value: Some(SqlVal::NumberValue(10.0)),
    };
    // Gte: value >= min is true when value equals the bound
    assert!(compare_sql_values(&value, &max) != std::cmp::Ordering::Less);

    let large_val = SqlValue {
        value: Some(SqlVal::NumberValue(15.0)),
    };
    assert!(compare_sql_values(&large_val, &max) != std::cmp::Ordering::Less);

    let small_val = SqlValue {
        value: Some(SqlVal::NumberValue(3.0)),
    };
    // 3.0 < 10.0, so Gte should fail
    assert!(compare_sql_values(&small_val, &max) == std::cmp::Ordering::Less);
}

#[test]
fn test_predicate_between_range_check() {
    // Between: min <= value <= max
    let min = SqlValue {
        value: Some(SqlVal::NumberValue(1.0)),
    };
    let max = SqlValue {
        value: Some(SqlVal::NumberValue(10.0)),
    };

    // Value inside range
    let inside = SqlValue {
        value: Some(SqlVal::NumberValue(5.0)),
    };
    assert!(
        compare_sql_values(&inside, &min) != std::cmp::Ordering::Less
            && compare_sql_values(&inside, &max) != std::cmp::Ordering::Greater
    );

    // Value at min boundary
    let at_min = SqlValue {
        value: Some(SqlVal::NumberValue(1.0)),
    };
    assert!(
        compare_sql_values(&at_min, &min) != std::cmp::Ordering::Less
            && compare_sql_values(&at_min, &max) != std::cmp::Ordering::Greater
    );

    // Value at max boundary
    let at_max = SqlValue {
        value: Some(SqlVal::NumberValue(10.0)),
    };
    assert!(
        compare_sql_values(&at_max, &min) != std::cmp::Ordering::Less
            && compare_sql_values(&at_max, &max) != std::cmp::Ordering::Greater
    );

    // Value below range
    let below = SqlValue {
        value: Some(SqlVal::NumberValue(0.0)),
    };
    assert!(compare_sql_values(&below, &min) == std::cmp::Ordering::Less);

    // Value above range
    let above = SqlValue {
        value: Some(SqlVal::NumberValue(11.0)),
    };
    assert!(compare_sql_values(&above, &max) == std::cmp::Ordering::Greater);
}

#[test]
fn test_predicate_not_equal_detection() {
    // NotEqual: value != target means we keep rowgroup if range includes other values
    let val_a = SqlValue {
        value: Some(SqlVal::NumberValue(5.0)),
    };
    let val_b = SqlValue {
        value: Some(SqlVal::NumberValue(5.0)),
    };
    // Equal values
    assert_eq!(
        compare_sql_values(&val_a, &val_b),
        std::cmp::Ordering::Equal
    );

    // If min == max == value, NotEqual should prune the rowgroup
    // If min != max, there might be other values, so keep it
    let min = SqlValue {
        value: Some(SqlVal::NumberValue(5.0)),
    };
    let max = SqlValue {
        value: Some(SqlVal::NumberValue(5.0)),
    };
    let target = SqlValue {
        value: Some(SqlVal::NumberValue(5.0)),
    };
    let can_prune = compare_sql_values(&min, &target) == std::cmp::Ordering::Equal
        && compare_sql_values(&max, &target) == std::cmp::Ordering::Equal;
    assert!(can_prune, "Should prune when min=max=target for NotEqual");

    // When range is wider, cannot prune
    let wide_max = SqlValue {
        value: Some(SqlVal::NumberValue(10.0)),
    };
    let cannot_prune = compare_sql_values(&min, &target) == std::cmp::Ordering::Equal
        && compare_sql_values(&wide_max, &target) == std::cmp::Ordering::Equal;
    assert!(
        !cannot_prune,
        "Should NOT prune when max != target for NotEqual"
    );
}

#[test]
fn test_compare_sql_values_empty_strings() {
    let empty = SqlValue {
        value: Some(SqlVal::StringValue(String::new())),
    };
    let non_empty = SqlValue {
        value: Some(SqlVal::StringValue("a".to_string())),
    };
    assert_eq!(
        compare_sql_values(&empty, &non_empty),
        std::cmp::Ordering::Less
    );
    assert_eq!(
        compare_sql_values(&empty, &empty),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn test_scan_strategy_default_is_filtering() {
    let strategy = ScanStrategy::default();
    match strategy {
        ScanStrategy::Filtering {
            target_ids,
            predicates,
            max_rowgroups,
        } => {
            assert!(target_ids.is_none());
            assert!(predicates.is_none());
            assert!(max_rowgroups.is_none());
        }
        _ => panic!("Default should be Filtering"),
    }
}

#[test]
fn test_scan_strategy_equality() {
    assert_eq!(ScanStrategy::FullScan, ScanStrategy::FullScan);
    assert_ne!(ScanStrategy::FullScan, ScanStrategy::default());
}

#[test]
fn test_boost_config_defaults() {
    let config = BoostConfig::default();
    assert!(config.alpha_own > 0.0);
    assert!(config.alpha_other > 0.0);
    assert!(config.alpha_variance > 0.0);
    assert!(config.beta_min > 0.0);
    assert!(config.beta_max > 0.0);
    assert!(config.boundary_threshold > 0.0);
    assert!(config.alpha_inter > 0.0);
    assert!(config.beta_cross > 0.0);
}

#[test]
fn test_search_stats_cluster_tracking() {
    let mut stats = SearchStats::new();
    assert_eq!(stats.clusters_visited.len(), 0);

    stats.record_cluster_visit(0);
    stats.record_cluster_visit(1);
    stats.record_cluster_visit(0); // duplicate
    assert_eq!(stats.clusters_visited.len(), 2);
    assert!(stats.clusters_visited.contains(&0));
    assert!(stats.clusters_visited.contains(&1));
}
