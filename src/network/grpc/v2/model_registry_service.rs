//! gRPC adapter for the tenant-scoped xCatalog embedding-model registry.

use std::collections::BTreeSet;
use std::sync::Arc;

use proximadb_catalog::mlops::{
    CatalogArtifactDescriptor, CatalogDeploymentBinding, CatalogDimensionPolicy,
    CatalogEmbeddingInputContract, CatalogEmbeddingModelRegistry, CatalogEmbeddingModelVersion,
    CatalogEmbeddingOutputContract, CatalogEvaluationEvidence, CatalogLineageInputKind,
    CatalogModelAccess, CatalogModelContractError, CatalogModelDecision, CatalogModelDecisionKind,
    CatalogModelGovernance, CatalogModelLineage, CatalogModelRegistryMutation,
    CatalogModelUsePolicy,
};
use proximadb_catalog::model_registry_service::{
    CatalogModelRegistryRecord, CatalogModelRegistryService, ModelRegistryServiceError,
};
use tonic::{Request, Response, Status};

use crate::network::grpc::auth as grpc_auth;
use crate::proto::proximadb_v2 as pv2;
use pv2::proxima_model_registry_service_server::{
    ProximaModelRegistryService, ProximaModelRegistryServiceServer,
};

#[derive(Clone)]
pub struct ProximaModelRegistryServiceImpl {
    service: Arc<CatalogModelRegistryService>,
}

impl ProximaModelRegistryServiceImpl {
    pub fn new(service: Arc<CatalogModelRegistryService>) -> Self {
        Self { service }
    }

    pub fn into_server(self) -> ProximaModelRegistryServiceServer<Self> {
        ProximaModelRegistryServiceServer::new(self)
    }
}

fn required<T>(value: Option<T>, field: &str) -> Result<T, Status> {
    value.ok_or_else(|| Status::invalid_argument(format!("{field} is required")))
}

fn contract_error_status(error: &CatalogModelContractError) -> tonic::Code {
    match error {
        CatalogModelContractError::RevisionConflict { .. } => tonic::Code::Aborted,
        CatalogModelContractError::Empty { .. }
        | CatalogModelContractError::InvalidDigest { .. }
        | CatalogModelContractError::InvalidTemplate { .. }
        | CatalogModelContractError::InvalidSpecialTokenBudget { .. }
        | CatalogModelContractError::DimensionExceedsNative { .. }
        | CatalogModelContractError::InvalidDimensionPolicy { .. }
        | CatalogModelContractError::InvalidAlias { .. } => tonic::Code::InvalidArgument,
        CatalogModelContractError::Serialization { .. } => tonic::Code::Internal,
        _ => tonic::Code::FailedPrecondition,
    }
}

fn service_status(error: ModelRegistryServiceError) -> Status {
    let message = error.to_string();
    match &error {
        ModelRegistryServiceError::InvalidTenant { .. }
        | ModelRegistryServiceError::InvalidName { .. } => Status::invalid_argument(message),
        ModelRegistryServiceError::AlreadyExists { .. } => Status::already_exists(message),
        ModelRegistryServiceError::NotFound { .. } => Status::not_found(message),
        ModelRegistryServiceError::Contract(contract) => {
            Status::new(contract_error_status(contract), message)
        }
        ModelRegistryServiceError::TenantIdentityUnavailable { .. } => {
            Status::failed_precondition(message)
        }
        ModelRegistryServiceError::MissingAssetId { .. }
        | ModelRegistryServiceError::NotModelRegistry { .. }
        | ModelRegistryServiceError::Catalog(_) => Status::internal(message),
    }
}

fn artifact_from_wire(value: pv2::ModelArtifactDescriptor) -> CatalogArtifactDescriptor {
    CatalogArtifactDescriptor {
        uri: value.uri,
        digest: value.digest,
        size_bytes: value.size_bytes,
        media_type: value.media_type,
    }
}

fn artifact_to_wire(value: CatalogArtifactDescriptor) -> pv2::ModelArtifactDescriptor {
    pv2::ModelArtifactDescriptor {
        uri: value.uri,
        digest: value.digest,
        size_bytes: value.size_bytes,
        media_type: value.media_type,
    }
}

fn input_from_wire(value: pv2::EmbeddingInputContract) -> CatalogEmbeddingInputContract {
    CatalogEmbeddingInputContract {
        model_revision: value.model_revision,
        tokenizer_id: value.tokenizer_id,
        tokenizer_revision: value.tokenizer_revision,
        tokenizer_fingerprint: value.tokenizer_fingerprint,
        declared_context_limit: value.declared_context_limit,
        effective_context_limit: value.effective_context_limit,
        special_token_count: value.special_token_count,
        document_template: value.document_template,
        query_template: value.query_template,
        document_parameters: value.document_parameters.into_iter().collect(),
        query_parameters: value.query_parameters.into_iter().collect(),
    }
}

fn input_to_wire(value: CatalogEmbeddingInputContract) -> pv2::EmbeddingInputContract {
    pv2::EmbeddingInputContract {
        model_revision: value.model_revision,
        tokenizer_id: value.tokenizer_id,
        tokenizer_revision: value.tokenizer_revision,
        tokenizer_fingerprint: value.tokenizer_fingerprint,
        declared_context_limit: value.declared_context_limit,
        effective_context_limit: value.effective_context_limit,
        special_token_count: value.special_token_count,
        document_template: value.document_template,
        query_template: value.query_template,
        document_parameters: value.document_parameters.into_iter().collect(),
        query_parameters: value.query_parameters.into_iter().collect(),
    }
}

fn access_from_wire(value: i32) -> Result<CatalogModelAccess, Status> {
    match pv2::ModelAccess::try_from(value).unwrap_or(pv2::ModelAccess::Unspecified) {
        pv2::ModelAccess::Open => Ok(CatalogModelAccess::Open),
        pv2::ModelAccess::Gated => Ok(CatalogModelAccess::Gated),
        // Unspecified is deliberately fail-closed and matches the catalog's
        // legacy-safe default.
        pv2::ModelAccess::Unreviewed | pv2::ModelAccess::Unspecified => {
            Ok(CatalogModelAccess::Unreviewed)
        }
    }
}

fn access_to_wire(value: CatalogModelAccess) -> i32 {
    match value {
        CatalogModelAccess::Open => pv2::ModelAccess::Open as i32,
        CatalogModelAccess::Gated => pv2::ModelAccess::Gated as i32,
        CatalogModelAccess::Unreviewed => pv2::ModelAccess::Unreviewed as i32,
    }
}

fn governance_from_wire(value: pv2::ModelGovernance) -> Result<CatalogModelGovernance, Status> {
    Ok(CatalogModelGovernance {
        license_id: value.license_id,
        access: access_from_wire(value.access)?,
        requires_remote_code: value.requires_remote_code,
        approved_runtimes: value.approved_runtimes.into_iter().collect::<BTreeSet<_>>(),
    })
}

fn governance_to_wire(value: CatalogModelGovernance) -> pv2::ModelGovernance {
    pv2::ModelGovernance {
        license_id: value.license_id,
        access: access_to_wire(value.access),
        requires_remote_code: value.requires_remote_code,
        approved_runtimes: value.approved_runtimes.into_iter().collect(),
    }
}

fn lineage_kind_from_wire(value: i32) -> Result<CatalogLineageInputKind, Status> {
    match pv2::LineageInputKind::try_from(value).unwrap_or(pv2::LineageInputKind::Unspecified) {
        pv2::LineageInputKind::Dataset => Ok(CatalogLineageInputKind::Dataset),
        pv2::LineageInputKind::FeatureSet => Ok(CatalogLineageInputKind::FeatureSet),
        pv2::LineageInputKind::Model => Ok(CatalogLineageInputKind::Model),
        pv2::LineageInputKind::Artifact => Ok(CatalogLineageInputKind::Artifact),
        pv2::LineageInputKind::Unspecified => {
            Err(Status::invalid_argument("lineage input kind is required"))
        }
    }
}

fn lineage_kind_to_wire(value: CatalogLineageInputKind) -> i32 {
    match value {
        CatalogLineageInputKind::Dataset => pv2::LineageInputKind::Dataset as i32,
        CatalogLineageInputKind::FeatureSet => pv2::LineageInputKind::FeatureSet as i32,
        CatalogLineageInputKind::Model => pv2::LineageInputKind::Model as i32,
        CatalogLineageInputKind::Artifact => pv2::LineageInputKind::Artifact as i32,
    }
}

fn lineage_from_wire(value: pv2::ModelLineage) -> Result<CatalogModelLineage, Status> {
    let inputs = value
        .inputs
        .into_iter()
        .map(|input| {
            Ok(proximadb_catalog::mlops::CatalogLineageInput {
                kind: lineage_kind_from_wire(input.kind)?,
                name: input.name,
                digest: input.digest,
            })
        })
        .collect::<Result<Vec<_>, Status>>()?;
    Ok(CatalogModelLineage {
        producer_execution_id: value.producer_execution_id,
        code_revision: value.code_revision,
        inputs,
    })
}

fn lineage_to_wire(value: CatalogModelLineage) -> pv2::ModelLineage {
    pv2::ModelLineage {
        producer_execution_id: value.producer_execution_id,
        code_revision: value.code_revision,
        inputs: value
            .inputs
            .into_iter()
            .map(|input| pv2::ModelLineageInput {
                kind: lineage_kind_to_wire(input.kind),
                name: input.name,
                digest: input.digest,
            })
            .collect(),
    }
}

fn output_from_wire(
    value: pv2::EmbeddingOutputContract,
) -> Result<CatalogEmbeddingOutputContract, Status> {
    let policy = match pv2::DimensionPolicyKind::try_from(value.dimension_policy)
        .unwrap_or(pv2::DimensionPolicyKind::Unspecified)
    {
        pv2::DimensionPolicyKind::Fixed => CatalogDimensionPolicy::Fixed,
        pv2::DimensionPolicyKind::Discrete => CatalogDimensionPolicy::Discrete,
        pv2::DimensionPolicyKind::Range => {
            CatalogDimensionPolicy::Range(proximadb_catalog::mlops::CatalogDimensionRange {
                minimum: value.range_minimum.ok_or_else(|| {
                    Status::invalid_argument("range_minimum is required for range dimension policy")
                })?,
            })
        }
        pv2::DimensionPolicyKind::Unspecified => {
            return Err(Status::invalid_argument("dimension policy is required"));
        }
    };
    Ok(CatalogEmbeddingOutputContract {
        native_dimension: value.native_dimension,
        dimension_policy: policy,
        supported_dimensions: value.supported_dimensions,
        normalized: value.normalized,
        pooling: value.pooling,
    })
}

fn output_to_wire(value: CatalogEmbeddingOutputContract) -> pv2::EmbeddingOutputContract {
    let (dimension_policy, range_minimum) = match value.dimension_policy {
        CatalogDimensionPolicy::Fixed => (pv2::DimensionPolicyKind::Fixed as i32, None),
        CatalogDimensionPolicy::Discrete => (pv2::DimensionPolicyKind::Discrete as i32, None),
        CatalogDimensionPolicy::Range(range) => {
            (pv2::DimensionPolicyKind::Range as i32, Some(range.minimum))
        }
    };
    pv2::EmbeddingOutputContract {
        native_dimension: value.native_dimension,
        dimension_policy,
        range_minimum,
        supported_dimensions: value.supported_dimensions,
        normalized: value.normalized,
        pooling: value.pooling,
    }
}

fn version_from_wire(
    value: pv2::EmbeddingModelVersion,
) -> Result<CatalogEmbeddingModelVersion, Status> {
    Ok(CatalogEmbeddingModelVersion {
        version: value.version,
        provider_model_id: value.provider_model_id,
        artifact: artifact_from_wire(required(value.artifact, "artifact")?),
        input: input_from_wire(required(value.input, "input")?),
        output: output_from_wire(required(value.output, "output")?)?,
        governance: governance_from_wire(required(value.governance, "governance")?)?,
        lineage: lineage_from_wire(required(value.lineage, "lineage")?)?,
        created_at_ms: value.created_at_ms,
        source_run_id: value.source_run_id,
    })
}

fn version_to_wire(value: CatalogEmbeddingModelVersion) -> pv2::EmbeddingModelVersion {
    pv2::EmbeddingModelVersion {
        version: value.version,
        provider_model_id: value.provider_model_id,
        artifact: Some(artifact_to_wire(value.artifact)),
        input: Some(input_to_wire(value.input)),
        output: Some(output_to_wire(value.output)),
        governance: Some(governance_to_wire(value.governance)),
        lineage: Some(lineage_to_wire(value.lineage)),
        created_at_ms: value.created_at_ms,
        source_run_id: value.source_run_id,
    }
}

fn evidence_from_wire(value: pv2::EvaluationEvidence) -> CatalogEvaluationEvidence {
    CatalogEvaluationEvidence {
        evidence_id: value.evidence_id,
        version: value.version,
        dataset_name: value.dataset_name,
        dataset_digest: value.dataset_digest,
        evaluator: value.evaluator,
        metrics: value.metrics.into_iter().collect(),
        created_at_ms: value.created_at_ms,
    }
}

fn evidence_to_wire(value: CatalogEvaluationEvidence) -> pv2::EvaluationEvidence {
    pv2::EvaluationEvidence {
        evidence_id: value.evidence_id,
        version: value.version,
        dataset_name: value.dataset_name,
        dataset_digest: value.dataset_digest,
        evaluator: value.evaluator,
        metrics: value.metrics.into_iter().collect(),
        created_at_ms: value.created_at_ms,
    }
}

fn decision_from_wire(value: pv2::ModelDecision) -> Result<CatalogModelDecision, Status> {
    let decision = match pv2::ModelDecisionKind::try_from(value.decision)
        .unwrap_or(pv2::ModelDecisionKind::Unspecified)
    {
        pv2::ModelDecisionKind::Approved => CatalogModelDecisionKind::Approved,
        pv2::ModelDecisionKind::Rejected => CatalogModelDecisionKind::Rejected,
        pv2::ModelDecisionKind::Deprecated => CatalogModelDecisionKind::Deprecated,
        pv2::ModelDecisionKind::Unspecified => {
            return Err(Status::invalid_argument("model decision is required"));
        }
    };
    Ok(CatalogModelDecision {
        decision_id: value.decision_id,
        version: value.version,
        decision,
        evidence_ids: value.evidence_ids,
        principal: value.principal,
        created_at_ms: value.created_at_ms,
    })
}

fn decision_to_wire(value: CatalogModelDecision) -> pv2::ModelDecision {
    let decision = match value.decision {
        CatalogModelDecisionKind::Approved => pv2::ModelDecisionKind::Approved,
        CatalogModelDecisionKind::Rejected => pv2::ModelDecisionKind::Rejected,
        CatalogModelDecisionKind::Deprecated => pv2::ModelDecisionKind::Deprecated,
    };
    pv2::ModelDecision {
        decision_id: value.decision_id,
        version: value.version,
        decision: decision as i32,
        evidence_ids: value.evidence_ids,
        principal: value.principal,
        created_at_ms: value.created_at_ms,
    }
}

fn deployment_from_wire(value: pv2::DeploymentBinding) -> CatalogDeploymentBinding {
    CatalogDeploymentBinding {
        name: value.name,
        version: value.version,
        artifact_digest: value.artifact_digest,
        runtime: value.runtime,
        endpoint: value.endpoint,
        updated_at_ms: value.updated_at_ms,
    }
}

fn deployment_to_wire(value: CatalogDeploymentBinding) -> pv2::DeploymentBinding {
    pv2::DeploymentBinding {
        name: value.name,
        version: value.version,
        artifact_digest: value.artifact_digest,
        runtime: value.runtime,
        endpoint: value.endpoint,
        updated_at_ms: value.updated_at_ms,
    }
}

fn registry_to_wire(value: CatalogEmbeddingModelRegistry) -> pv2::ModelRegistry {
    pv2::ModelRegistry {
        schema_version: value.schema_version,
        revision: value.revision,
        name: value.name,
        versions: value.versions.into_values().map(version_to_wire).collect(),
        aliases: value
            .aliases
            .into_iter()
            .map(|(alias, version)| pv2::AliasEntry { alias, version })
            .collect(),
        evidence: value.evidence.into_iter().map(evidence_to_wire).collect(),
        decisions: value.decisions.into_iter().map(decision_to_wire).collect(),
        deployments: value
            .deployments
            .into_values()
            .map(deployment_to_wire)
            .collect(),
        tags: value.tags.into_iter().collect(),
    }
}

fn record_to_wire(value: CatalogModelRegistryRecord) -> pv2::ModelRegistryRecord {
    pv2::ModelRegistryRecord {
        tenant_id: value.tenant_id,
        asset_id: value.asset_id,
        registry: Some(registry_to_wire(value.registry)),
    }
}

fn mutation_from_wire(
    value: Option<pv2::apply_model_registry_mutation_request::Mutation>,
) -> Result<CatalogModelRegistryMutation, Status> {
    use pv2::apply_model_registry_mutation_request::Mutation;
    match required(value, "mutation")? {
        Mutation::RegisterVersion(version) => Ok(CatalogModelRegistryMutation::register_version(
            version_from_wire(version)?,
        )),
        Mutation::SetAlias(command) => Ok(CatalogModelRegistryMutation::set_alias(
            command.alias,
            command.version,
        )),
        Mutation::AppendEvidence(evidence) => Ok(CatalogModelRegistryMutation::AppendEvidence(
            evidence_from_wire(evidence),
        )),
        Mutation::RecordDecision(decision) => Ok(CatalogModelRegistryMutation::RecordDecision(
            decision_from_wire(decision)?,
        )),
        Mutation::UpsertDeployment(deployment) => Ok(
            CatalogModelRegistryMutation::UpsertDeployment(deployment_from_wire(deployment)),
        ),
    }
}

#[tonic::async_trait]
impl ProximaModelRegistryService for ProximaModelRegistryServiceImpl {
    async fn create_model_registry(
        &self,
        request: Request<pv2::CreateModelRegistryRequest>,
    ) -> Result<Response<pv2::CreateModelRegistryResponse>, Status> {
        let tenant = grpc_auth::resolved_tenant_id(&request)?;
        let request = request.into_inner();
        let record = self
            .service
            .create_registry(&tenant, &request.name)
            .await
            .map_err(service_status)?;
        Ok(Response::new(pv2::CreateModelRegistryResponse {
            record: Some(record_to_wire(record)),
        }))
    }

    async fn get_model_registry(
        &self,
        request: Request<pv2::GetModelRegistryRequest>,
    ) -> Result<Response<pv2::GetModelRegistryResponse>, Status> {
        let tenant = grpc_auth::resolved_tenant_id(&request)?;
        let request = request.into_inner();
        let record = self
            .service
            .get_registry(&tenant, &request.name)
            .await
            .map_err(service_status)?;
        Ok(Response::new(pv2::GetModelRegistryResponse {
            record: Some(record_to_wire(record)),
        }))
    }

    async fn list_model_registries(
        &self,
        request: Request<pv2::ListModelRegistriesRequest>,
    ) -> Result<Response<pv2::ListModelRegistriesResponse>, Status> {
        let tenant = grpc_auth::resolved_tenant_id(&request)?;
        let registries = self
            .service
            .list_registries(&tenant)
            .await
            .map_err(service_status)?
            .into_iter()
            .map(record_to_wire)
            .collect();
        Ok(Response::new(pv2::ListModelRegistriesResponse {
            registries,
        }))
    }

    async fn apply_model_registry_mutation(
        &self,
        request: Request<pv2::ApplyModelRegistryMutationRequest>,
    ) -> Result<Response<pv2::ApplyModelRegistryMutationResponse>, Status> {
        let tenant = grpc_auth::resolved_tenant_id(&request)?;
        let request = request.into_inner();
        let mutation = mutation_from_wire(request.mutation)?;
        let record = self
            .service
            .apply_mutation(&tenant, &request.name, request.expected_revision, mutation)
            .await
            .map_err(service_status)?;
        Ok(Response::new(pv2::ApplyModelRegistryMutationResponse {
            record: Some(record_to_wire(record)),
        }))
    }

    async fn resolve_model_alias(
        &self,
        request: Request<pv2::ResolveModelAliasRequest>,
    ) -> Result<Response<pv2::ResolveModelAliasResponse>, Status> {
        let tenant = grpc_auth::resolved_tenant_id(&request)?;
        let request = request.into_inner();
        let policy = CatalogModelUsePolicy {
            runtime: request.runtime,
            require_approved: request.require_approved,
        };
        let resolved = self
            .service
            .resolve_alias_binding(
                &tenant,
                &request.name,
                &request.alias,
                request.dimension,
                &policy,
            )
            .await
            .map_err(service_status)?;
        let binding = resolved.binding;
        let model_asset_id = binding
            .model_asset_id
            .ok_or_else(|| Status::internal("resolved binding has no model asset id"))?;
        let model_version = binding
            .model_version
            .ok_or_else(|| Status::internal("resolved binding has no model version"))?;
        let contract_sha256 = binding
            .contract_sha256
            .ok_or_else(|| Status::internal("resolved binding has no contract digest"))?;
        let snapshot = resolved.snapshot;
        Ok(Response::new(pv2::ResolveModelAliasResponse {
            resolved: Some(pv2::ResolvedModelBinding {
                model: binding.model,
                dimension: binding.dimension,
                model_asset_id,
                model_version,
                contract_sha256,
                snapshot: Some(pv2::ResolvedEmbeddingModel {
                    asset_id: snapshot.asset_id,
                    registry_name: snapshot.registry_name,
                    registry_revision: snapshot.registry_revision,
                    contract_sha256: snapshot.contract_sha256,
                    model: Some(version_to_wire(snapshot.model)),
                }),
            }),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn grpc_service() -> (tempfile::TempDir, ProximaModelRegistryServiceImpl) {
        use proximadb_catalog::CatalogManager;
        use proximadb_catalog::native::{NativeCatalog, NativeCatalogConfig};

        let directory = tempfile::tempdir().unwrap();
        let catalog = NativeCatalog::new(
            "default".to_string(),
            NativeCatalogConfig {
                storage_url: directory.path().to_string_lossy().to_string(),
                metadata_format: "json".to_string(),
                versioned: false,
                max_versions: 100,
            },
            Arc::new(proximadb_catalog::cache::CatalogCache::new(64, 60)),
        )
        .await
        .unwrap();
        let manager = Arc::new(CatalogManager::new());
        manager.register(Arc::new(catalog)).await.unwrap();
        let service = Arc::new(CatalogModelRegistryService::new(manager));
        (directory, ProximaModelRegistryServiceImpl::new(service))
    }

    fn tenant_request<T>(tenant: &str, body: T) -> Request<T> {
        let mut request = Request::new(body);
        request
            .metadata_mut()
            .insert("x-tenant-id", tenant.parse().unwrap());
        request
    }

    fn fixture_version() -> CatalogEmbeddingModelVersion {
        let asset: proximadb_catalog::mlops::CatalogMlopsAsset =
            serde_json::from_str(include_str!(
                "../../../../clients/python/tests/fixtures/embedding_model_xcatalog_asset.json"
            ))
            .unwrap();
        let proximadb_catalog::mlops::CatalogMlopsAsset::EmbeddingModel(registry) = asset;
        registry.version(1).unwrap().clone()
    }

    #[test]
    fn mutation_requires_exactly_one_command() {
        let status = mutation_from_wire(None).unwrap_err();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn registry_maps_are_emitted_in_stable_order() {
        let mut registry = CatalogEmbeddingModelRegistry::new("embed").unwrap();
        registry.aliases =
            std::collections::BTreeMap::from([("zeta".to_string(), 2), ("alpha".to_string(), 1)]);
        let wire = registry_to_wire(registry);
        assert_eq!(wire.aliases[0].alias, "alpha");
        assert_eq!(wire.aliases[1].alias, "zeta");
    }

    #[test]
    fn revision_conflicts_map_to_aborted() {
        let status = service_status(ModelRegistryServiceError::Contract(
            CatalogModelContractError::RevisionConflict {
                expected: 4,
                current: 5,
            },
        ));
        assert_eq!(status.code(), tonic::Code::Aborted);
    }

    #[tokio::test]
    async fn grpc_lifecycle_is_tenant_scoped_and_resolves_immutable_binding() {
        let (_directory, grpc) = grpc_service().await;
        grpc.create_model_registry(tenant_request(
            "tenant-a",
            pv2::CreateModelRegistryRequest {
                name: "search-embedding".to_string(),
            },
        ))
        .await
        .unwrap();

        grpc.apply_model_registry_mutation(tenant_request(
            "tenant-a",
            pv2::ApplyModelRegistryMutationRequest {
                name: "search-embedding".to_string(),
                expected_revision: 0,
                mutation: Some(
                    pv2::apply_model_registry_mutation_request::Mutation::RegisterVersion(
                        version_to_wire(fixture_version()),
                    ),
                ),
            },
        ))
        .await
        .unwrap();
        grpc.apply_model_registry_mutation(tenant_request(
            "tenant-a",
            pv2::ApplyModelRegistryMutationRequest {
                name: "search-embedding".to_string(),
                expected_revision: 1,
                mutation: Some(
                    pv2::apply_model_registry_mutation_request::Mutation::SetAlias(
                        pv2::SetModelAliasCommand {
                            alias: "champion".to_string(),
                            version: 1,
                        },
                    ),
                ),
            },
        ))
        .await
        .unwrap();

        let resolved = grpc
            .resolve_model_alias(tenant_request(
                "tenant-a",
                pv2::ResolveModelAliasRequest {
                    name: "search-embedding".to_string(),
                    alias: "champion".to_string(),
                    dimension: 768,
                    runtime: None,
                    require_approved: false,
                },
            ))
            .await
            .unwrap()
            .into_inner()
            .resolved
            .unwrap();
        assert_eq!(resolved.model_version, 1);
        assert!(!resolved.contract_sha256.is_empty());
        assert_eq!(
            resolved.snapshot.unwrap().contract_sha256,
            resolved.contract_sha256
        );

        let status = grpc
            .get_model_registry(tenant_request(
                "tenant-b",
                pv2::GetModelRegistryRequest {
                    name: "search-embedding".to_string(),
                },
            ))
            .await
            .unwrap_err();
        assert_eq!(status.code(), tonic::Code::NotFound);
    }
}
