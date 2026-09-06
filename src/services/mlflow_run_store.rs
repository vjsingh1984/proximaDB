//! Substrate-backed RunStore (TD-MLOPS-1 slice 1).
//!
//! Experiments, runs, params, metrics and tags persist as documents in ONE
//! tenant-scoped collection in the document substrate — the single storage
//! spine (mandate 18a), never a private metadata database. Structural tenant
//! isolation comes from the scoped collection key (constructed once per
//! store); record payloads never carry tenant identity.
//!
//! Document layout (one collection per tenant):
//! * `meta`              — `{next_experiment_id}`
//! * `exp-{id}`          — experiment record (serde JSON in `payload` +
//!                          indexed `name` / `stage` fields)
//! * `run-{run_id}`      — run record (payload + indexed `experiment_id` /
//!                          `stage` / per-run append counters)
//! * `mtr-{run}-{seq}`   — one append-only metric sample (indexed `run_id`,
//!                          `key`, `seq`) — history is a filtered query
//! * `ds-{run}-{n}`      — dataset lineage input (indexed `run_id`)
//!
//! Mutations serialize under one process mutex: tracking is low-frequency
//! control-plane traffic, and the lock substitutes for per-document
//! optimistic concurrency in this slice. The seq counters live on the run
//! document so ordering survives process restarts.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use anyhow::{Context, Result};
use proximadb_catalog::run_store::{
    ExperimentRecord, ExperimentStage, MetricAppend, MetricPoint, RunDatasetInput, RunRecord,
    RunStage, RunStore, RunStoreError,
};
use proximadb_data_model::ProximaValue;
use proximadb_records::{ProximaTree, ProximaTreeNode};
use tokio::sync::Mutex;

use crate::storage::document::service::scoped_document_collection;
use crate::storage::document::{DocumentRecord, DocumentService};

const COLLECTION: &str = "mlflow_tracking";

pub struct SubstrateRunStore {
    document: Arc<DocumentService>,
    collection: String,
    mutation_lock: Mutex<()>,
}

impl SubstrateRunStore {
    /// Tenant-scoped construction: the collection key embeds the tenant once,
    /// structurally — mirrors `GraphOperationsService::for_tenant`.
    pub fn for_tenant(document: Arc<DocumentService>, tenant: &str) -> Result<Self> {
        Ok(Self {
            collection: scoped_document_collection(tenant, COLLECTION)
                .context("scope mlflow tracking collection to tenant")?,
            document,
            mutation_lock: Mutex::new(()),
        })
    }

    async fn ensure(&self) -> Result<()> {
        self.document
            .ensure_or_create_collection(&self.collection)
            .await?;
        Ok(())
    }

    fn err(e: anyhow::Error) -> RunStoreError {
        RunStoreError::Internal {
            message: e.to_string(),
        }
    }

    async fn put_payload(
        &self,
        id: &str,
        index_fields: &[(&str, ProximaValue)],
        payload: &impl serde::Serialize,
    ) -> Result<()> {
        let mut tree: ProximaTree = HashMap::new();
        for (key, value) in index_fields {
            tree.insert((*key).to_string(), ProximaTreeNode::Value(value.clone()));
        }
        tree.insert(
            "payload".to_string(),
            ProximaTreeNode::Value(ProximaValue::String(serde_json::to_string(payload)?)),
        );
        let record = DocumentRecord::from_tree(
            id.to_string(),
            tree,
            self.collection.clone(),
            None,
            Some("mlflow_tracking".to_string()),
        );
        self.document
            .insert_document_record(&self.collection, record)
            .await?;
        Ok(())
    }

    async fn get_payload<T: serde::de::DeserializeOwned>(&self, id: &str) -> Result<Option<T>> {
        let Some(record) = self
            .document
            .get_document(&self.collection, id, None)
            .await?
        else {
            return Ok(None);
        };
        let Some(ProximaTreeNode::Value(ProximaValue::String(json))) = record.props.get("payload")
        else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(json)?))
    }

    async fn next_experiment_id(&self) -> Result<u64> {
        let current: Option<u64> = self.get_payload("meta").await?;
        let next = current.map(|n| n + 1).unwrap_or(0);
        self.put_payload("meta", &[], &next).await?;
        Ok(next)
    }

    async fn experiments(&self) -> Result<Vec<ExperimentRecord>> {
        // Slice-1 scale (per-tenant experiment counts are small): full scan
        // of the kind via list-by-prefix is not exposed by the seam, so
        // enumerate by walking the indexed query below.
        let params = crate::storage::document::DocumentQueryParams {
            filter: Some(filter_of(&[eq_cond("kind", "experiment")])),
            ..Default::default()
        };
        let result = self
            .document
            .query_documents(&self.collection, params)
            .await?;
        let mut records = Vec::new();
        for doc in result.documents {
            if let Some(ProximaTreeNode::Value(ProximaValue::String(json))) =
                doc.props.get("payload")
            {
                if let Ok(record) = serde_json::from_str::<ExperimentRecord>(json) {
                    records.push(record);
                }
            }
        }
        records.sort_by_key(|r| r.experiment_id);
        Ok(records)
    }

    async fn runs_of(&self, experiment_id: u64) -> Result<Vec<RunRecord>> {
        let params = crate::storage::document::DocumentQueryParams {
            filter: Some(filter_of(&[
                eq_cond("kind", "run"),
                crate::proto::proximadb_v1::DocFilterCondition {
                    path: "experiment_id".to_string(),
                    operator: crate::proto::proximadb_v1::DocFilterOperator::Eq as i32,
                    value: Some(crate::proto::proximadb_v1::SqlValue {
                        value: Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(
                            experiment_id as i64,
                        )),
                    }),
                    values: vec![],
                },
            ])),
            ..Default::default()
        };
        let result = self
            .document
            .query_documents(&self.collection, params)
            .await?;
        let mut records = Vec::new();
        for doc in result.documents {
            if let Some(ProximaTreeNode::Value(ProximaValue::String(json))) =
                doc.props.get("payload")
            {
                if let Ok(record) = serde_json::from_str::<RunRecord>(json) {
                    records.push(record);
                }
            }
        }
        records.sort_by_key(|r| r.start_time_ms);
        Ok(records)
    }
}

// --- indexed-field filters (JSONPath filter DSL over the indexed fields) ---

fn eq_cond(path: &str, value: &str) -> crate::proto::proximadb_v1::DocFilterCondition {
    crate::proto::proximadb_v1::DocFilterCondition {
        path: path.to_string(),
        operator: crate::proto::proximadb_v1::DocFilterOperator::Eq as i32,
        value: Some(crate::proto::proximadb_v1::SqlValue {
            value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                value.to_string(),
            )),
        }),
        values: vec![],
    }
}

fn filter_of(
    conditions: &[crate::proto::proximadb_v1::DocFilterCondition],
) -> crate::proto::proximadb_v1::DocumentFilter {
    crate::proto::proximadb_v1::DocumentFilter {
        conditions: conditions.to_vec(),
        ..Default::default()
    }
}

impl RunStore for SubstrateRunStore {
    async fn create_experiment(
        &self,
        name: &str,
        artifact_location: Option<&str>,
        tags: BTreeMap<String, String>,
    ) -> Result<ExperimentRecord, RunStoreError> {
        if name.is_empty() {
            return Err(RunStoreError::Empty { field: "name" });
        }
        let _guard = self.mutation_lock.lock().await;
        self.ensure()
            .await
            .map_err(|e| Self::err(e.context("ensure collection")))?;
        if self
            .experiments()
            .await
            .map_err(Self::err)?
            .iter()
            .any(|e| e.name == name)
        {
            return Err(RunStoreError::ExperimentNameConflict {
                name: name.to_string(),
            });
        }
        let id = self.next_experiment_id().await.map_err(Self::err)?;
        let now = chrono::Utc::now().timestamp_millis();
        let record = ExperimentRecord {
            experiment_id: id,
            name: name.to_string(),
            artifact_location: artifact_location.map(str::to_string),
            tags,
            stage: ExperimentStage::Active,
            creation_time_ms: now,
            last_update_time_ms: now,
        };
        self.put_payload(
            &format!("exp-{id}"),
            &[
                ("kind", ProximaValue::String("experiment".to_string())),
                ("name", ProximaValue::String(record.name.clone())),
                (
                    "stage",
                    ProximaValue::String(serde_json::to_string(&record.stage).unwrap_or_default()),
                ),
            ],
            &record,
        )
        .await
        .map_err(Self::err)?;
        Ok(record)
    }

    async fn get_experiment(&self, experiment_id: u64) -> Result<ExperimentRecord, RunStoreError> {
        self.get_payload(&format!("exp-{experiment_id}"))
            .await
            .map_err(Self::err)?
            .ok_or(RunStoreError::UnknownExperiment { experiment_id })
    }

    async fn list_experiments(
        &self,
        include_deleted: bool,
    ) -> Result<Vec<ExperimentRecord>, RunStoreError> {
        self.ensure()
            .await
            .map_err(|e| Self::err(e.context("ensure collection")))?;
        Ok(self
            .experiments()
            .await
            .map_err(Self::err)?
            .into_iter()
            .filter(|e| include_deleted || e.stage == ExperimentStage::Active)
            .collect())
    }

    async fn delete_experiment(&self, experiment_id: u64) -> Result<(), RunStoreError> {
        let _guard = self.mutation_lock.lock().await;
        let mut record = self.get_experiment(experiment_id).await?;
        record.stage = ExperimentStage::Deleted;
        record.last_update_time_ms = chrono::Utc::now().timestamp_millis();
        self.put_payload(
            &format!("exp-{experiment_id}"),
            &[
                ("kind", ProximaValue::String("experiment".to_string())),
                ("name", ProximaValue::String(record.name.clone())),
                (
                    "stage",
                    ProximaValue::String(serde_json::to_string(&record.stage).unwrap_or_default()),
                ),
            ],
            &record,
        )
        .await
        .map_err(Self::err)
    }

    async fn restore_experiment(&self, experiment_id: u64) -> Result<(), RunStoreError> {
        let _guard = self.mutation_lock.lock().await;
        let mut record = self.get_experiment(experiment_id).await?;
        record.stage = ExperimentStage::Active;
        record.last_update_time_ms = chrono::Utc::now().timestamp_millis();
        self.put_payload(
            &format!("exp-{experiment_id}"),
            &[
                ("kind", ProximaValue::String("experiment".to_string())),
                ("name", ProximaValue::String(record.name.clone())),
                (
                    "stage",
                    ProximaValue::String(serde_json::to_string(&record.stage).unwrap_or_default()),
                ),
            ],
            &record,
        )
        .await
        .map_err(Self::err)
    }

    async fn create_run(
        &self,
        experiment_id: u64,
        run_id: &str,
        run_name: Option<&str>,
        user_id: Option<&str>,
        tags: BTreeMap<String, String>,
        start_time_ms: i64,
    ) -> Result<RunRecord, RunStoreError> {
        if run_id.is_empty() {
            return Err(RunStoreError::Empty { field: "run_id" });
        }
        let _guard = self.mutation_lock.lock().await;
        self.ensure()
            .await
            .map_err(|e| Self::err(e.context("ensure collection")))?;
        self.get_experiment(experiment_id).await?;
        if self
            .get_payload::<RunRecord>(&format!("run-{run_id}"))
            .await
            .map_err(Self::err)?
            .is_some()
        {
            return Err(RunStoreError::RunIdConflict {
                run_id: run_id.to_string(),
            });
        }
        let record = RunRecord {
            run_id: run_id.to_string(),
            experiment_id,
            run_name: run_name.map(str::to_string),
            user_id: user_id.map(str::to_string),
            status: RunStage::Running,
            start_time_ms,
            end_time_ms: None,
            params: BTreeMap::new(),
            latest_metrics: BTreeMap::new(),
            tags,
        };
        self.put_run(&record).await.map_err(Self::err)?;
        Ok(record)
    }

    async fn get_run(&self, run_id: &str) -> Result<RunRecord, RunStoreError> {
        self.get_payload(&format!("run-{run_id}"))
            .await
            .map_err(Self::err)?
            .ok_or_else(|| RunStoreError::UnknownRun {
                run_id: run_id.to_string(),
            })
    }

    async fn list_runs(
        &self,
        experiment_id: u64,
        include_deleted: bool,
    ) -> Result<Vec<RunRecord>, RunStoreError> {
        self.ensure()
            .await
            .map_err(|e| Self::err(e.context("ensure collection")))?;
        self.get_experiment(experiment_id).await?;
        Ok(self
            .runs_of(experiment_id)
            .await
            .map_err(Self::err)?
            .into_iter()
            .filter(|r| include_deleted || r.status != RunStage::Deleted)
            .collect())
    }

    async fn finish_run(&self, run_id: &str, end_time_ms: i64) -> Result<(), RunStoreError> {
        let _guard = self.mutation_lock.lock().await;
        let mut run = self.get_run(run_id).await?;
        run.status = RunStage::Finished;
        run.end_time_ms = Some(end_time_ms);
        self.put_run(&run).await.map_err(Self::err)
    }

    async fn delete_run(&self, run_id: &str) -> Result<(), RunStoreError> {
        let _guard = self.mutation_lock.lock().await;
        let mut run = self.get_run(run_id).await?;
        run.status = RunStage::Deleted;
        self.put_run(&run).await.map_err(Self::err)
    }

    async fn restore_run(&self, run_id: &str) -> Result<(), RunStoreError> {
        let _guard = self.mutation_lock.lock().await;
        let mut run = self.get_run(run_id).await?;
        run.status = RunStage::Running;
        self.put_run(&run).await.map_err(Self::err)
    }

    async fn log_param(&self, run_id: &str, key: &str, value: &str) -> Result<(), RunStoreError> {
        let _guard = self.mutation_lock.lock().await;
        let mut run = self.get_run(run_id).await?;
        match run.params.get(key) {
            Some(existing) if existing == value => Ok(()),
            Some(_) => Err(RunStoreError::ParamImmutable {
                key: key.to_string(),
                run_id: run_id.to_string(),
            }),
            None => {
                run.params.insert(key.to_string(), value.to_string());
                self.put_run(&run).await.map_err(Self::err)
            }
        }
    }

    async fn log_metric(
        &self,
        run_id: &str,
        point: MetricPoint,
    ) -> Result<MetricAppend, RunStoreError> {
        let _guard = self.mutation_lock.lock().await;
        let mut run = self.get_run(run_id).await?;
        if run.status == RunStage::Finished {
            return Err(RunStoreError::RunFinished {
                run_id: run_id.to_string(),
            });
        }
        // seq + latest live on the run document; history in its own doc.
        let seq_field = format!("seq_{}", sanitize(&point.key));
        let seq = run
            .tags
            .get(&seq_field)
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let next_seq = seq + 1;
        // Counters ride the tags map (stringly) so the record schema stays
        // wire-stable for the MLflow adapter; keys are namespaced `seq_`.
        run.tags.insert(seq_field, next_seq.to_string());
        run.latest_metrics.insert(point.key.clone(), point.clone());
        self.put_run(&run).await.map_err(Self::err)?;
        self.put_payload(
            &format!("mtr-{run_id}-{next_seq}"),
            &[
                ("kind", ProximaValue::String("metric".to_string())),
                ("run_id", ProximaValue::String(run_id.to_string())),
                ("key", ProximaValue::String(point.key.clone())),
                ("seq", ProximaValue::Int64(next_seq as i64)),
            ],
            &point,
        )
        .await
        .map_err(Self::err)?;
        Ok(MetricAppend {
            history_len: next_seq,
        })
    }

    async fn metric_history(
        &self,
        run_id: &str,
        key: &str,
    ) -> Result<Vec<MetricPoint>, RunStoreError> {
        self.get_run(run_id).await?;
        self.ensure()
            .await
            .map_err(|e| Self::err(e.context("ensure collection")))?;
        let filter = filter_of(&[
            eq_cond("kind", "metric"),
            eq_cond("run_id", run_id),
            eq_cond("key", key),
        ]);
        let params = crate::storage::document::DocumentQueryParams {
            filter: Some(filter),
            ..Default::default()
        };
        let result = self
            .document
            .query_documents(&self.collection, params)
            .await
            .map_err(Self::err)?;
        let mut points: Vec<(u64, MetricPoint)> = Vec::new();
        for doc in result.documents {
            let seq = match doc.props.get("seq") {
                Some(ProximaTreeNode::Value(ProximaValue::Int64(s))) => *s as u64,
                _ => 0,
            };
            if let Some(ProximaTreeNode::Value(ProximaValue::String(json))) =
                doc.props.get("payload")
            {
                if let Ok(point) = serde_json::from_str::<MetricPoint>(json) {
                    points.push((seq, point));
                }
            }
        }
        points.sort_by_key(|(seq, _)| *seq);
        Ok(points.into_iter().map(|(_, p)| p).collect())
    }

    async fn set_tag(&self, run_id: &str, key: &str, value: &str) -> Result<(), RunStoreError> {
        let _guard = self.mutation_lock.lock().await;
        let mut run = self.get_run(run_id).await?;
        run.tags.insert(key.to_string(), value.to_string());
        self.put_run(&run).await.map_err(Self::err)
    }

    async fn delete_tag(&self, run_id: &str, key: &str) -> Result<(), RunStoreError> {
        let _guard = self.mutation_lock.lock().await;
        let mut run = self.get_run(run_id).await?;
        run.tags.remove(key);
        self.put_run(&run).await.map_err(Self::err)
    }

    async fn log_dataset_input(
        &self,
        run_id: &str,
        input: RunDatasetInput,
    ) -> Result<(), RunStoreError> {
        let _guard = self.mutation_lock.lock().await;
        let mut run = self.get_run(run_id).await?;
        let n = run
            .tags
            .get("ds_seq")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            + 1;
        run.tags.insert("ds_seq".to_string(), n.to_string());
        self.put_run(&run).await.map_err(Self::err)?;
        self.put_payload(
            &format!("ds-{run_id}-{n}"),
            &[
                ("kind", ProximaValue::String("dataset".to_string())),
                ("run_id", ProximaValue::String(run_id.to_string())),
            ],
            &input,
        )
        .await
        .map_err(Self::err)
    }

    async fn dataset_inputs(&self, run_id: &str) -> Result<Vec<RunDatasetInput>, RunStoreError> {
        self.get_run(run_id).await?;
        self.ensure()
            .await
            .map_err(|e| Self::err(e.context("ensure collection")))?;
        let filter = filter_of(&[eq_cond("kind", "dataset"), eq_cond("run_id", run_id)]);
        let params = crate::storage::document::DocumentQueryParams {
            filter: Some(filter),
            ..Default::default()
        };
        let result = self
            .document
            .query_documents(&self.collection, params)
            .await
            .map_err(Self::err)?;
        let mut inputs = Vec::new();
        for doc in result.documents {
            if let Some(ProximaTreeNode::Value(ProximaValue::String(json))) =
                doc.props.get("payload")
            {
                if let Ok(input) = serde_json::from_str::<RunDatasetInput>(json) {
                    inputs.push(input);
                }
            }
        }
        Ok(inputs)
    }
}

impl SubstrateRunStore {
    async fn put_run(&self, run: &RunRecord) -> Result<()> {
        self.put_payload(
            &format!("run-{}", run.run_id),
            &[
                ("kind", ProximaValue::String("run".to_string())),
                (
                    "experiment_id",
                    ProximaValue::Int64(run.experiment_id as i64),
                ),
                (
                    "stage",
                    ProximaValue::String(serde_json::to_string(&run.status).unwrap_or_default()),
                ),
            ],
            run,
        )
        .await
    }
}

/// Metric keys become tag-map keys (`seq_<key>`); keep them in the tag
/// charset to avoid colliding with user tags visibly.
fn sanitize(key: &str) -> String {
    key.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
