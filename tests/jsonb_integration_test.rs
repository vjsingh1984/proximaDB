/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use proximadb::proto::proximadb_v1::{
    DocFilterCondition, DocFilterOperator, DocumentFilter, SqlObject, SqlValue,
    sql_value::Value as SqlVal,
};
use proximadb::storage::document::DocumentRecord;
use proximadb::storage::document::query::filter::FilterEvaluator;
use proximadb::storage::document::storage::DocumentBlock;
use proximadb_data_model::ProximaValue;
use std::collections::HashMap;

#[tokio::test]
async fn test_jsonb_storage_and_filtering_e2e() {
    let collection_id = "agent_state";

    let doc_obj = SqlObject {
        fields: HashMap::from([(
            "state".to_string(),
            SqlValue {
                value: Some(SqlVal::ObjectValue(SqlObject {
                    fields: HashMap::from([
                        (
                            "agent_id".to_string(),
                            SqlValue {
                                value: Some(SqlVal::StringValue("nexus-6".to_string())),
                            },
                        ),
                        (
                            "objective".to_string(),
                            SqlValue {
                                value: Some(SqlVal::StringValue(
                                    "maintain system stability".to_string(),
                                )),
                            },
                        ),
                        (
                            "metrics".to_string(),
                            SqlValue {
                                value: Some(SqlVal::ObjectValue(SqlObject {
                                    fields: HashMap::from([
                                        (
                                            "uptime".to_string(),
                                            SqlValue {
                                                value: Some(SqlVal::NumberValue(99.99)),
                                            },
                                        ),
                                        (
                                            "load".to_string(),
                                            SqlValue {
                                                value: Some(SqlVal::NumberValue(0.42)),
                                            },
                                        ),
                                    ]),
                                })),
                            },
                        ),
                    ]),
                })),
            },
        )]),
    };

    let documents = vec![("doc1".to_string(), doc_obj)];

    let block =
        DocumentBlock::from_documents(documents, &["state.agent_id".to_string()], true).unwrap();
    assert!(block.header.use_jsonb);
    assert!(block.might_contain_path("state.agent_id"));

    let evaluator = FilterEvaluator::new();
    let record = DocumentRecord::new(
        "doc1".to_string(),
        document_from_block(&block, 0),
        collection_id.to_string(),
    );

    let filter = DocumentFilter {
        conditions: vec![DocFilterCondition {
            path: "state.metrics.uptime".to_string(),
            operator: DocFilterOperator::Gt as i32,
            value: Some(SqlValue {
                value: Some(SqlVal::NumberValue(99.0)),
            }),
            ..Default::default()
        }],
        ..Default::default()
    };

    assert!(evaluator.evaluate(&filter, &record));
}

fn document_from_block(block: &DocumentBlock, index: usize) -> SqlObject {
    let mut pos = 0;
    for _ in 0..index {
        let len = u32::from_le_bytes([
            block.data[pos],
            block.data[pos + 1],
            block.data[pos + 2],
            block.data[pos + 3],
        ]) as usize;
        pos += 4 + len;
    }

    let len = u32::from_le_bytes([
        block.data[pos],
        block.data[pos + 1],
        block.data[pos + 2],
        block.data[pos + 3],
    ]) as usize;
    let bytes = &block.data[pos + 4..pos + 4 + len];

    if block.header.use_jsonb {
        let json = ProximaValue::from_jsonb_slice(bytes).unwrap();
        serde_json::from_value(json).unwrap()
    } else {
        serde_json::from_slice(bytes).unwrap()
    }
}
