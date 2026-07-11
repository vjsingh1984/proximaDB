//! Assertion helpers for approximate equality
//!
//! These helpers are essential for testing:
//! - Floating-point comparisons (accounting for precision errors)
//! - Vector similarity (with configurable tolerance)
//! - Recall metrics (with minimum thresholds)

use std::fmt;

/// Assertion helpers for approximate equality
pub struct AssertApprox;

impl AssertApprox {
    /// Assert two floats are approximately equal
    ///
    /// # Arguments
    /// * `a` - First value
    /// * `b` - Second value
    /// * `epsilon` - Maximum allowed difference
    ///
    /// # Example
    /// ```no_run
    /// use proxima::tdd::test_utils::AssertApprox;
    ///
    /// AssertApprox::assert_close(0.1 + 0.2, 0.3, 0.0001);
    /// ```
    pub fn assert_close(a: f64, b: f64, epsilon: f64) {
        let diff = (a - b).abs();
        assert!(
            diff < epsilon,
            "Values not close: {} vs {} (diff: {}, epsilon: {})",
            a,
            b,
            diff,
            epsilon
        );
    }

    /// Assert two floats are approximately equal (f32 version)
    pub fn assert_close_f32(a: f32, b: f32, epsilon: f32) {
        let diff = (a - b).abs();
        assert!(
            diff < epsilon,
            "Values not close: {} vs {} (diff: {}, epsilon: {})",
            a,
            b,
            diff,
            epsilon
        );
    }

    /// Assert two vectors are approximately equal
    ///
    /// # Arguments
    /// * `a` - First vector
    /// * `b` - Second vector
    /// * `epsilon` - Maximum allowed difference per element
    ///
    /// # Panics
    /// - If vector lengths differ
    /// - If any element difference exceeds epsilon
    pub fn assert_vec_close(a: &[f32], b: &[f32], epsilon: f32) {
        assert_eq!(
            a.len(),
            b.len(),
            "Vector lengths differ: {} vs {}",
            a.len(),
            b.len()
        );

        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            let diff = (x - y).abs();
            assert!(
                diff < epsilon,
                "Vector elements differ at index {}: {} vs {} (diff: {}, epsilon: {})",
                i,
                x,
                y,
                diff,
                epsilon
            );
        }
    }

    /// Assert two vectors are approximately equal (f64 version)
    pub fn assert_vec_close_f64(a: &[f64], b: &[f64], epsilon: f64) {
        assert_eq!(
            a.len(),
            b.len(),
            "Vector lengths differ: {} vs {}",
            a.len(),
            b.len()
        );

        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            let diff = (x - y).abs();
            assert!(
                diff < epsilon,
                "Vector elements differ at index {}: {} vs {} (diff: {}, epsilon: {})",
                i,
                x,
                y,
                diff,
                epsilon
            );
        }
    }

    /// Assert recall percentage is above threshold
    ///
    /// # Arguments
    /// * `actual_recall` - Actual recall value (0.0 to 1.0)
    /// * `min_recall` - Minimum acceptable recall (0.0 to 1.0)
    ///
    /// # Example
    /// ```no_run
    /// use proxima::tdd::test_utils::AssertApprox;
    ///
    /// // Recall of 95% exceeds 90% minimum
    /// AssertApprox::assert_recall_above(0.95, 0.90);
    /// ```
    pub fn assert_recall_above(actual_recall: f64, min_recall: f64) {
        assert!(
            actual_recall >= min_recall,
            "Recall {}% is below minimum {}%",
            actual_recall * 100.0,
            min_recall * 100.0
        );
    }

    /// Assert precision is above threshold
    pub fn assert_precision_above(actual_precision: f64, min_precision: f64) {
        assert!(
            actual_precision >= min_precision,
            "Precision {}% is below minimum {}%",
            actual_precision * 100.0,
            min_precision * 100.0
        );
    }

    /// Assert F1 score is above threshold
    pub fn assert_f1_above(actual_f1: f64, min_f1: f64) {
        assert!(
            actual_f1 >= min_f1,
            "F1 score {} is below minimum {}",
            actual_f1,
            min_f1
        );
    }

    /// Assert value is within range [min, max]
    pub fn assert_in_range(value: f64, min: f64, max: f64) {
        assert!(
            value >= min && value <= max,
            "Value {} is outside range [{}, {}]",
            value,
            min,
            max
        );
    }

    /// Assert percentage difference is within tolerance
    pub fn assert_percent_diff_within(a: f64, b: f64, max_percent_diff: f64) {
        let avg = (a + b) / 2.0;
        let diff = ((a - b).abs() / avg) * 100.0;

        assert!(
            diff <= max_percent_diff,
            "Percentage difference {}% exceeds tolerance {}% (values: {}, {})",
            diff,
            max_percent_diff,
            a,
            b
        );
    }
}

/// Struct for building comparison reports
pub struct ComparisonReport {
    mismatches: Vec<ComparisonMismatch>,
    tolerance: f32,
}

#[derive(Debug, Clone)]
pub struct ComparisonMismatch {
    pub index: usize,
    pub expected: f32,
    pub actual: f32,
    pub diff: f32,
}

impl ComparisonReport {
    pub fn new(tolerance: f32) -> Self {
        Self {
            mismatches: Vec::new(),
            tolerance,
        }
    }

    pub fn compare(&mut self, expected: &[f32], actual: &[f32]) {
        for (i, (exp, act)) in expected.iter().zip(actual.iter()).enumerate() {
            let diff = (exp - act).abs();
            if diff > self.tolerance {
                self.mismatches.push(ComparisonMismatch {
                    index: i,
                    expected: *exp,
                    actual: *act,
                    diff,
                });
            }
        }
    }

    pub fn has_mismatches(&self) -> bool {
        !self.mismatches.is_empty()
    }

    pub fn mismatch_count(&self) -> usize {
        self.mismatches.len()
    }

    pub fn max_diff(&self) -> f32 {
        self.mismatches
            .iter()
            .map(|m| m.diff)
            .fold(0.0_f32, f32::max)
    }
}

impl fmt::Display for ComparisonReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "Comparison Report (tolerance: {}):",
            self.tolerance
        )?;
        writeln!(f, "Mismatches: {}", self.mismatches.len())?;

        if self.mismatches.len() <= 10 {
            for mismatch in &self.mismatches {
                writeln!(
                    f,
                    "  [{}] expected: {:.6}, actual: {:.6}, diff: {:.6}",
                    mismatch.index, mismatch.expected, mismatch.actual, mismatch.diff
                )?;
            }
        } else {
            writeln!(f, "  (showing first 10 of {})", self.mismatches.len())?;
            for mismatch in self.mismatches.iter().take(10) {
                writeln!(
                    f,
                    "  [{}] expected: {:.6}, actual: {:.6}, diff: {:.6}",
                    mismatch.index, mismatch.expected, mismatch.actual, mismatch.diff
                )?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assert_close_passes() {
        AssertApprox::assert_close(1.0, 1.000001, 0.00001);
    }

    #[test]
    #[should_panic]
    fn test_assert_close_fails() {
        AssertApprox::assert_close(1.0, 2.0, 0.00001);
    }

    #[test]
    fn test_assert_vec_close_passes() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0001, 2.0001, 3.0001];
        AssertApprox::assert_vec_close_f64(&a, &b, 0.001);
    }

    #[test]
    #[should_panic]
    fn test_assert_vec_close_fails_on_length() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0];
        AssertApprox::assert_vec_close_f64(&a, &b, 0.001);
    }

    #[test]
    #[should_panic]
    fn test_assert_vec_close_fails_on_value() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 4.0]; // 3.0 vs 4.0 differs by 1.0
        AssertApprox::assert_vec_close_f64(&a, &b, 0.001);
    }

    #[test]
    fn test_assert_recall_above_passes() {
        AssertApprox::assert_recall_above(0.95, 0.90);
    }

    #[test]
    #[should_panic]
    fn test_assert_recall_above_fails() {
        AssertApprox::assert_recall_above(0.85, 0.90);
    }

    #[test]
    fn test_comparison_report() {
        let mut report = ComparisonReport::new(0.01);
        let expected = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let actual = vec![1.0, 2.02, 3.0, 4.0, 5.03];

        report.compare(&expected, &actual);

        assert!(report.has_mismatches());
        assert_eq!(report.mismatch_count(), 2);
        assert_eq!(report.max_diff(), 0.03);
    }
}
