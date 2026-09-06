//! TD-MLOPS-1 slice 1: the substrate-backed RunStore passes the same port
//! conformance battery as the in-memory reference (experiments, runs, params,
//! metrics history, tags, datasets, soft-delete/restore, finish-freeze) and
//! enforces structural tenant isolation.

use std::sync::Arc;

use proximadb::services::mlflow_run_store::SubstrateRunStore;
use proximadb::storage::document::DocumentService;
use proximadb::storage::engines::sst::SstEngine;
use proximadb_catalog::run_store::RunStore;

#[tokio::test]
async fn substrate_run_store_passes_port_conformance() {
    let engine = Arc::new(SstEngine::new().await.unwrap());
    let document = Arc::new(DocumentService::new(engine));
    let store = SubstrateRunStore::for_tenant(document, "default").unwrap();

    proximadb_catalog::run_store::conformance_tests::port_conformance(&store).await;
}

#[tokio::test]
async fn substrate_run_store_isolates_tenants_structurally() {
    let engine = Arc::new(SstEngine::new().await.unwrap());
    let document = Arc::new(DocumentService::new(engine));
    let alpha = SubstrateRunStore::for_tenant(document.clone(), "alpha").unwrap();
    let beta = SubstrateRunStore::for_tenant(document, "beta").unwrap();

    let exp_a = alpha
        .create_experiment("shared-name", None, Default::default())
        .await
        .unwrap();

    // Same name is legal in the other tenant; ids are per-tenant sequences.
    let exp_b = beta
        .create_experiment("shared-name", None, Default::default())
        .await
        .unwrap();
    assert_eq!(exp_a.experiment_id, exp_b.experiment_id);

    // Cross-tenant run visibility: none.
    alpha
        .create_run(
            exp_a.experiment_id,
            "run-a",
            None,
            None,
            Default::default(),
            1,
        )
        .await
        .unwrap();
    beta.create_run(
        exp_b.experiment_id,
        "run-b",
        None,
        None,
        Default::default(),
        1,
    )
    .await
    .unwrap();
    let beta_runs = beta.list_runs(exp_b.experiment_id, true).await.unwrap();
    assert_eq!(beta_runs.len(), 1);
    assert_eq!(beta_runs[0].run_id, "run-b");
    assert!(beta.get_run("run-a").await.is_err());
    assert!(alpha.get_run("run-b").await.is_err());
}
