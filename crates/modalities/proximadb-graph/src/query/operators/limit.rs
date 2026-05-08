use super::{ColumnSpec, PhysicalOperator, ResultTuple};
use anyhow::Result;

/// Limit operator implementing LIMIT and SKIP.
pub struct LimitOperator {
    input: Box<dyn PhysicalOperator>,
    skip: Option<usize>,
    limit: Option<usize>,
    current_row: usize,
    returned_count: usize,
}

impl LimitOperator {
    pub fn new(
        input: Box<dyn PhysicalOperator>,
        skip: Option<usize>,
        limit: Option<usize>,
    ) -> Self {
        Self {
            input,
            skip,
            limit,
            current_row: 0,
            returned_count: 0,
        }
    }
}

impl PhysicalOperator for LimitOperator {
    fn open(&mut self) -> Result<()> {
        self.input.open()?;
        self.current_row = 0;
        self.returned_count = 0;
        Ok(())
    }

    fn next(&mut self) -> Result<Option<ResultTuple>> {
        if let Some(limit) = self.limit
            && self.returned_count >= limit
        {
            return Ok(None);
        }

        let skip_count = self.skip.unwrap_or(0);
        while self.current_row < skip_count {
            if self.input.next()?.is_none() {
                return Ok(None);
            }
            self.current_row += 1;
        }

        if let Some(tuple) = self.input.next()? {
            self.current_row += 1;
            self.returned_count += 1;
            Ok(Some(tuple))
        } else {
            Ok(None)
        }
    }

    fn close(&mut self) -> Result<()> {
        self.input.close()
    }

    fn estimated_cardinality(&self) -> usize {
        let input_card = self.input.estimated_cardinality();
        let skip_count = self.skip.unwrap_or(0);

        if let Some(limit) = self.limit {
            limit.min(input_card.saturating_sub(skip_count))
        } else {
            input_card.saturating_sub(skip_count)
        }
    }

    fn schema(&self) -> &[ColumnSpec] {
        self.input.schema()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::execution::{QueryValue, ValueType};
    use proximadb_proto::proximadb_v1::Node;
    use std::sync::Arc;

    struct MockInputOperator {
        tuples: Vec<ResultTuple>,
        index: usize,
        schema: Vec<ColumnSpec>,
    }

    impl MockInputOperator {
        fn new(count: usize) -> Self {
            let tuples = (0..count)
                .map(|i| {
                    let mut tuple = ResultTuple::new();
                    tuple.set(
                        "n".to_string(),
                        QueryValue::Node(Arc::new(Node {
                            id: format!("n{i}"),
                            ..Default::default()
                        })),
                    );
                    tuple
                })
                .collect();
            Self {
                tuples,
                index: 0,
                schema: vec![ColumnSpec {
                    name: "n".to_string(),
                    value_type: ValueType::Node,
                }],
            }
        }
    }

    impl PhysicalOperator for MockInputOperator {
        fn open(&mut self) -> Result<()> {
            self.index = 0;
            Ok(())
        }

        fn next(&mut self) -> Result<Option<ResultTuple>> {
            if let Some(tuple) = self.tuples.get(self.index).cloned() {
                self.index += 1;
                Ok(Some(tuple))
            } else {
                Ok(None)
            }
        }

        fn close(&mut self) -> Result<()> {
            Ok(())
        }

        fn estimated_cardinality(&self) -> usize {
            self.tuples.len()
        }

        fn schema(&self) -> &[ColumnSpec] {
            &self.schema
        }
    }

    #[test]
    fn limit_operator_honors_skip_and_limit() {
        let mut limit =
            LimitOperator::new(Box::new(MockInputOperator::new(100)), Some(20), Some(10));
        limit.open().unwrap();

        let mut count = 0;
        while limit.next().unwrap().is_some() {
            count += 1;
        }

        assert_eq!(count, 10);
        assert_eq!(limit.estimated_cardinality(), 10);
    }
}
