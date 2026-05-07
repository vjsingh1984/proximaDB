#[cfg(test)]
mod tests {
    use crate::query::aql::executor::AqlExecutor;
    use crate::query::aql::{
        AqlFind, AqlFrom, AqlPredicate, AqlProjection, AqlQuery, AqlValue, AqlWhere, DataModel,
        JoinType,
    };
    use std::collections::HashMap;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_aql_executor_join_audit() {
        // 1. Setup mock/dummy services (simplified)
        // Since we are testing the executor's coordination and audit trail,
        // we can use the real services with empty/dummy state if they don't panic.
        // Actually, it's better to create a mock AqlSource for pure executor testing.

        struct MockSource {
            model: DataModel,
        }

        #[async_trait::async_trait]
        impl crate::query::aql::AqlSource for MockSource {
            fn model(&self) -> DataModel {
                self.model
            }
            async fn execute(
                &self,
                _q: &AqlQuery,
                ctx: &mut crate::query::aql::AuditContext,
            ) -> crate::query::aql::Result<crate::query::aql::AqlResult> {
                let mut row = HashMap::new();
                row.insert("id".to_string(), AqlValue::String("test_id".to_string()));

                let frame_id = ctx.push_frame(crate::query::aql::AuditFrame {
                    frame_id: 0,
                    source: self.model,
                    op: crate::query::aql::AuditOp::Scan {
                        source: "mock".to_string(),
                    },
                    filters_pushed: vec![],
                    filters_post: vec![],
                    records_scanned: 1,
                    records_returned: 1,
                    wall_time_us: 100,
                    error: None,
                    redaction_count: 0,
                });

                Ok(crate::query::aql::AqlResult {
                    rows: vec![row],
                    frame_id,
                })
            }
        }

        let mut executor = AqlExecutor::new();
        executor.register_source(
            "vec".to_string(),
            Arc::new(MockSource {
                model: DataModel::Vector,
            }),
        );
        executor.register_source(
            "grp".to_string(),
            Arc::new(MockSource {
                model: DataModel::Graph,
            }),
        );

        // 2. Construct Join Query
        let query = AqlQuery {
            find: AqlFind {
                projections: vec![AqlProjection {
                    field: "*".to_string(),
                    alias: None,
                }],
            },
            from: AqlFrom::Join {
                left: Box::new(AqlFrom::Source {
                    name: "vec".to_string(),
                    alias: None,
                }),
                right: Box::new(AqlFrom::Source {
                    name: "grp".to_string(),
                    alias: None,
                }),
                on: AqlPredicate::Equals {
                    field: "id".to_string(),
                    value: AqlValue::String("test".to_string()),
                },
                join_type: JoinType::Inner,
            },
            where_clause: AqlWhere { predicate: None },
        };

        // 3. Execute
        let (result, trail) = executor.execute(query).await.unwrap();

        // 4. Verify
        assert_eq!(result.rows.len(), 1);
        assert_eq!(trail.frames.len(), 3); // 2 scans + 1 join

        assert_eq!(trail.frames[0].source, DataModel::Vector);
        assert_eq!(trail.frames[1].source, DataModel::Graph);
        assert_eq!(trail.frames[2].source, DataModel::Relational);

        if let crate::query::aql::AuditOp::Join {
            left_frame,
            right_frame,
            ..
        } = &trail.frames[2].op
        {
            assert_eq!(*left_frame, trail.frames[0].frame_id);
            assert_eq!(*right_frame, trail.frames[1].frame_id);
        } else {
            panic!("Last frame should be a join");
        }

        assert!(matches!(
            trail.outcome,
            crate::query::aql::AuditOutcome::Success
        ));
    }
}
