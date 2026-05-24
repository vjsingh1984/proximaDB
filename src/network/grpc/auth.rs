//! Shared gRPC authentication and data-plane capability enforcement.
//!
//! The transport layer authenticates every generic gRPC request when enterprise
//! security is enabled. Narrow operator-issued capability tokens are then
//! constrained to the V2 record data-plane RPCs; body-aware checks for
//! collection, record, and byte limits are applied inside `ProximaRecordService`
//! after protobuf decoding.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tonic::codegen::http::{self, HeaderMap, Request as HttpRequest, Response as HttpResponse};
use tonic::{Code, Request, Status};
use tower::{Layer, Service};

use crate::network::auth::middleware::DataPlaneCapability;
use crate::security::{AuthenticationData, SecurityCoordinator, UnifiedUserContext};

#[derive(Debug, Clone)]
pub struct GrpcAuthContext {
    pub user_context: UnifiedUserContext,
    pub capability: Option<DataPlaneCapability>,
}

#[derive(Clone)]
pub struct GrpcAuthLayer {
    security_coordinator: Arc<SecurityCoordinator>,
}

impl GrpcAuthLayer {
    pub fn new(security_coordinator: Arc<SecurityCoordinator>) -> Self {
        Self {
            security_coordinator,
        }
    }
}

#[derive(Clone)]
pub struct GrpcAuthService<S> {
    inner: S,
    security_coordinator: Arc<SecurityCoordinator>,
}

impl<S> Layer<S> for GrpcAuthLayer {
    type Service = GrpcAuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        GrpcAuthService {
            inner,
            security_coordinator: self.security_coordinator.clone(),
        }
    }
}

impl<S, B> Service<HttpRequest<B>> for GrpcAuthService<S>
where
    S: Service<HttpRequest<B>, Response = HttpResponse<tonic::body::Body>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = HttpResponse<tonic::body::Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: HttpRequest<B>) -> Self::Future {
        let security_coordinator = self.security_coordinator.clone();
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            match authenticate_http_request(&security_coordinator, &mut request).await {
                Ok(()) => inner.call(request).await,
                Err(status) => Ok(status_to_http_response(status)),
            }
        })
    }
}

pub async fn authenticate_http_request<B>(
    security_coordinator: &SecurityCoordinator,
    request: &mut HttpRequest<B>,
) -> Result<(), Status> {
    let path = request.uri().path();
    if is_arrow_flight_path(path) {
        return Ok(());
    }

    let auth_data = auth_data_from_headers(request.headers())?;
    let user_context = security_coordinator
        .authenticate_request(auth_data)
        .await
        .map_err(|error| Status::unauthenticated(format!("authentication failed: {error}")))?;

    let capability = DataPlaneCapability::from_user_context(&user_context);
    if let Some(capability) = capability.as_ref() {
        validate_capability_for_grpc_path(capability, path)?;
        validate_tenant_metadata(&user_context, request.headers())?;
    }

    request.extensions_mut().insert(GrpcAuthContext {
        user_context,
        capability,
    });
    Ok(())
}

pub fn data_plane_capability<T>(request: &Request<T>) -> Option<DataPlaneCapability> {
    request
        .extensions()
        .get::<GrpcAuthContext>()
        .and_then(|context| context.capability.clone())
}

pub fn tenant_id<T>(request: &Request<T>) -> Option<String> {
    request
        .extensions()
        .get::<GrpcAuthContext>()
        .and_then(|context| context.user_context.tenant_id.clone())
        .or_else(|| tenant_id_from_metadata(request.metadata()))
}

pub fn enforce_data_plane_request<T>(
    request: &Request<T>,
    operation: &str,
    collection: &str,
    record_count: usize,
    byte_count: Option<u64>,
) -> Result<(), Status> {
    if let Some(capability) = request
        .extensions()
        .get::<GrpcAuthContext>()
        .and_then(|context| context.capability.as_ref())
    {
        validate_data_plane_capability(
            capability,
            operation,
            collection,
            record_count,
            byte_count,
        )?;
    }
    Ok(())
}

pub fn validate_data_plane_capability(
    capability: &DataPlaneCapability,
    operation: &str,
    collection: &str,
    record_count: usize,
    byte_count: Option<u64>,
) -> Result<(), Status> {
    ensure_protocol(capability, "grpc")?;
    ensure_operation(capability, operation)?;
    ensure_collection(capability, collection)?;
    ensure_scope(capability, operation)?;
    capability
        .ensure_record_count(record_count)
        .map_err(Status::resource_exhausted)?;
    if let Some(max_bytes) = capability.max_bytes
        && let Some(byte_count) = byte_count
        && byte_count > max_bytes
    {
        return Err(Status::resource_exhausted(format!(
            "Request has {byte_count} bytes, exceeding capability limit {max_bytes}"
        )));
    }
    Ok(())
}

fn auth_data_from_headers(headers: &HeaderMap) -> Result<AuthenticationData, Status> {
    let auth_header = headers
        .get(http::header::AUTHORIZATION)
        .ok_or_else(|| Status::unauthenticated("authorization metadata is required"))?
        .to_str()
        .map_err(|_| Status::invalid_argument("authorization metadata is not valid ASCII"))?;

    if let Some(token) = auth_header.strip_prefix("Bearer ") {
        return Ok(AuthenticationData::JWTToken(token.to_string()));
    }
    if let Some(key) = auth_header.strip_prefix("API-Key ") {
        return Ok(AuthenticationData::ApiKey(key.to_string()));
    }
    if let Some(key) = auth_header.strip_prefix("Api-Key ") {
        return Ok(AuthenticationData::ApiKey(key.to_string()));
    }
    Ok(AuthenticationData::ApiKey(auth_header.to_string()))
}

fn validate_capability_for_grpc_path(
    capability: &DataPlaneCapability,
    path: &str,
) -> Result<(), Status> {
    ensure_protocol(capability, "grpc")?;
    let operation = infer_grpc_data_plane_operation(path).ok_or_else(|| {
        Status::permission_denied("capability token is not valid for this gRPC method")
    })?;
    ensure_operation(capability, operation)?;
    ensure_scope(capability, operation)?;
    Ok(())
}

fn validate_tenant_metadata(
    user_context: &UnifiedUserContext,
    headers: &HeaderMap,
) -> Result<(), Status> {
    let token_tenant = user_context
        .tenant_id
        .as_deref()
        .ok_or_else(|| Status::permission_denied("capability token requires tenant_id"))?;
    if let Some(header_tenant) = headers
        .get("x-tenant-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && header_tenant != token_tenant
    {
        return Err(Status::permission_denied(
            "x-tenant-id does not match capability tenant_id",
        ));
    }
    Ok(())
}

fn ensure_protocol(capability: &DataPlaneCapability, expected: &str) -> Result<(), Status> {
    if capability.protocol.as_deref() != Some(expected) {
        return Err(Status::permission_denied(format!(
            "capability token is not valid for {expected}"
        )));
    }
    Ok(())
}

fn ensure_operation(capability: &DataPlaneCapability, expected: &str) -> Result<(), Status> {
    if capability.operation.as_deref() != Some(expected) {
        return Err(Status::permission_denied(
            "capability operation does not match gRPC method",
        ));
    }
    Ok(())
}

fn ensure_collection(capability: &DataPlaneCapability, expected: &str) -> Result<(), Status> {
    if capability.collection.as_deref() != Some(expected) {
        return Err(Status::permission_denied(
            "capability collection does not match gRPC request",
        ));
    }
    Ok(())
}

fn ensure_scope(capability: &DataPlaneCapability, operation: &str) -> Result<(), Status> {
    let allowed = match operation {
        "ingest" => capability
            .scopes
            .iter()
            .any(|scope| scope == "records:write"),
        "search" => capability
            .scopes
            .iter()
            .any(|scope| scope == "search:execute" || scope == "records:read"),
        _ => false,
    };
    if !allowed {
        return Err(Status::permission_denied(
            "capability token lacks the required scope",
        ));
    }
    Ok(())
}

fn infer_grpc_data_plane_operation(path: &str) -> Option<&'static str> {
    match path {
        "/proximadb.v2.ProximaRecordService/InsertRecords"
        | "/proximadb.v2.ProximaRecordService/UpsertRecords"
        | "/proximadb.v2.ProximaRecordService/UpdateRecords"
        | "/proximadb.v2.ProximaRecordService/DeleteRecords"
        | "/proximadb.v2.ProximaRecordService/BatchWriteStream" => Some("ingest"),
        "/proximadb.v2.ProximaRecordService/Search"
        | "/proximadb.v2.ProximaRecordService/SearchStream" => Some("search"),
        _ => None,
    }
}

fn is_arrow_flight_path(path: &str) -> bool {
    path.starts_with("/arrow.flight.protocol.FlightService/")
}

fn tenant_id_from_metadata(metadata: &tonic::metadata::MetadataMap) -> Option<String> {
    metadata
        .get("x-tenant-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn status_to_http_response(status: Status) -> HttpResponse<tonic::body::Body> {
    let code = status.code();
    let message = status.message().replace('\n', " ");
    HttpResponse::builder()
        .status(http::StatusCode::OK)
        .header(http::header::CONTENT_TYPE, "application/grpc")
        .header("grpc-status", grpc_status_code(code))
        .header("grpc-message", message)
        .body(tonic::body::Body::empty())
        .expect("gRPC status response should be valid")
}

fn grpc_status_code(code: Code) -> &'static str {
    match code {
        Code::Ok => "0",
        Code::Cancelled => "1",
        Code::Unknown => "2",
        Code::InvalidArgument => "3",
        Code::DeadlineExceeded => "4",
        Code::NotFound => "5",
        Code::AlreadyExists => "6",
        Code::PermissionDenied => "7",
        Code::ResourceExhausted => "8",
        Code::FailedPrecondition => "9",
        Code::Aborted => "10",
        Code::OutOfRange => "11",
        Code::Unimplemented => "12",
        Code::Internal => "13",
        Code::Unavailable => "14",
        Code::DataLoss => "15",
        Code::Unauthenticated => "16",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::network::auth::JwtService;
    use crate::network::auth::config::JwtAlgorithm;
    use crate::network::auth::jwt::{Claims, TokenType};
    use crate::security::auth_service::{
        AuthenticationConfig, AuthenticationMethod, JwtConfig, MtlsConfig, SSOConfig,
    };
    use crate::security::rbac_service::RBACConfig;
    use crate::security::security_coordinator::{ComplianceConfig, TlsConfig};
    use crate::security::{AuditConfig, SecurityConfig, SecurityMode};

    fn security_config() -> SecurityConfig {
        SecurityConfig {
            enabled: true,
            mode: SecurityMode::Development,
            authentication: AuthenticationConfig {
                enabled: true,
                methods: vec![AuthenticationMethod::JWT],
                require_authentication: true,
                default_session_timeout_minutes: 60,
                api_keys: HashMap::new(),
                jwt: JwtConfig {
                    enabled: true,
                    secret: "dev-jwt-secret".to_string(),
                    access_token_expiration_minutes: 15,
                    refresh_token_expiration_days: 7,
                    issuer: "operator-control-plane".to_string(),
                    audience: "proximadb-data-plane".to_string(),
                    algorithm: "HS256".to_string(),
                },
                sso: SSOConfig {
                    enabled: false,
                    providers: vec![],
                    token_cache_ttl_minutes: 5,
                    aws_iam: None,
                    azure_ad: None,
                },
                mtls: MtlsConfig::default(),
            },
            rbac: RBACConfig::default(),
            audit: AuditConfig::default(),
            tls: TlsConfig {
                enabled: false,
                require_client_certificates: false,
                cert_file: None,
                key_file: None,
                ca_file: None,
            },
            compliance: ComplianceConfig {
                frameworks: vec![],
                data_residency: None,
                encryption_at_rest: false,
                encryption_in_transit: false,
            },
            encryption: crate::security::EncryptionConfig::default(),
            key_store: crate::security::KeyStoreConfig::default(),
        }
    }

    fn capability_token(protocol: &str, operation: &str, collection: &str) -> String {
        let now = chrono::Utc::now().timestamp();
        let claims = Claims {
            sub: "key-a".to_string(),
            iat: now - 10,
            exp: now + 300,
            nbf: now - 10,
            iss: "operator-control-plane".to_string(),
            aud: "proximadb-data-plane".to_string(),
            jti: "capability-token".to_string(),
            tenant_id: Some("tenant-a".to_string()),
            roles: vec!["data_plane".to_string()],
            typ: TokenType::Access,
            capability_type: Some("operator.data-plane.capability.v1".to_string()),
            collection: Some(collection.to_string()),
            operation: Some(operation.to_string()),
            protocol: Some(protocol.to_string()),
            mode: Some("async".to_string()),
            scopes: match operation {
                "search" => vec!["records:read".to_string(), "search:execute".to_string()],
                _ => vec!["records:write".to_string(), "metering:emit".to_string()],
            },
            max_records: Some(10),
            max_bytes: Some(4096),
            tier: Some("business".to_string()),
            route_visibility: Some("public".to_string()),
            metering_required: Some(true),
        };
        jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret("dev-jwt-secret".as_bytes()),
        )
        .expect("capability JWT should encode")
    }

    async fn coordinator() -> SecurityCoordinator {
        SecurityCoordinator::from_config(security_config())
            .await
            .expect("security coordinator should initialize")
    }

    #[tokio::test]
    async fn grpc_auth_accepts_capability_for_v2_search() {
        let token = capability_token("grpc", "search", "example_knowledge");
        let mut request = HttpRequest::builder()
            .uri("/proximadb.v2.ProximaRecordService/Search")
            .header("authorization", format!("Bearer {token}"))
            .header("x-tenant-id", "tenant-a")
            .body(())
            .expect("request should build");

        authenticate_http_request(&coordinator().await, &mut request)
            .await
            .expect("valid gRPC capability should authenticate");

        let context = request
            .extensions()
            .get::<GrpcAuthContext>()
            .expect("auth context should be inserted");
        assert_eq!(context.user_context.tenant_id.as_deref(), Some("tenant-a"));
        assert_eq!(
            context
                .capability
                .as_ref()
                .and_then(|cap| cap.protocol.as_deref()),
            Some("grpc")
        );
    }

    #[tokio::test]
    async fn grpc_auth_rejects_capability_for_wrong_protocol() {
        let token = capability_token("rest", "search", "example_knowledge");
        let mut request = HttpRequest::builder()
            .uri("/proximadb.v2.ProximaRecordService/Search")
            .header("authorization", format!("Bearer {token}"))
            .header("x-tenant-id", "tenant-a")
            .body(())
            .expect("request should build");

        let status = authenticate_http_request(&coordinator().await, &mut request)
            .await
            .expect_err("REST capability must not authorize generic gRPC");
        assert_eq!(status.code(), Code::PermissionDenied);
    }

    #[tokio::test]
    async fn grpc_auth_rejects_capability_for_non_data_plane_method() {
        let token = capability_token("grpc", "search", "example_knowledge");
        let mut request = HttpRequest::builder()
            .uri("/proximadb.v1.CollectionService/ListCollections")
            .header("authorization", format!("Bearer {token}"))
            .header("x-tenant-id", "tenant-a")
            .body(())
            .expect("request should build");

        let status = authenticate_http_request(&coordinator().await, &mut request)
            .await
            .expect_err("capability must not authorize catalog methods");
        assert_eq!(status.code(), Code::PermissionDenied);
    }

    #[test]
    fn data_plane_body_validation_rejects_collection_and_limit_mismatches() {
        let user_context = UnifiedUserContext {
            user_id: "key-a".to_string(),
            tenant_id: Some("tenant-a".to_string()),
            roles: vec!["data_plane".to_string()],
            effective_permissions: Default::default(),
            auth_method: crate::security::UnifiedAuthMethod::JWT,
            session_id: "session-a".to_string(),
            expires_at: None,
            created_at: chrono::Utc::now(),
            metadata: HashMap::from([
                (
                    "capability_type".to_string(),
                    "operator.data-plane.capability.v1".to_string(),
                ),
                ("collection".to_string(), "example_knowledge".to_string()),
                ("operation".to_string(), "ingest".to_string()),
                ("protocol".to_string(), "grpc".to_string()),
                ("scopes".to_string(), "records:write".to_string()),
                ("max_records".to_string(), "2".to_string()),
                ("max_bytes".to_string(), "100".to_string()),
            ]),
        };
        let capability =
            DataPlaneCapability::from_user_context(&user_context).expect("capability should parse");

        validate_data_plane_capability(&capability, "ingest", "example_knowledge", 2, Some(100))
            .expect("matching request should pass");
        assert_eq!(
            validate_data_plane_capability(&capability, "ingest", "other", 1, Some(1))
                .expect_err("collection mismatch should fail")
                .code(),
            Code::PermissionDenied
        );
        assert_eq!(
            validate_data_plane_capability(&capability, "ingest", "example_knowledge", 3, Some(1))
                .expect_err("record limit should fail")
                .code(),
            Code::ResourceExhausted
        );
        assert_eq!(
            validate_data_plane_capability(
                &capability,
                "ingest",
                "example_knowledge",
                1,
                Some(101)
            )
            .expect_err("byte limit should fail")
            .code(),
            Code::ResourceExhausted
        );
    }

    #[tokio::test]
    async fn arrow_flight_paths_are_left_to_flight_auth() {
        let mut request = HttpRequest::builder()
            .uri("/arrow.flight.protocol.FlightService/DoPut")
            .body(())
            .expect("request should build");

        authenticate_http_request(&coordinator().await, &mut request)
            .await
            .expect("Flight service performs its own auth");
        assert!(request.extensions().get::<GrpcAuthContext>().is_none());
    }

    #[tokio::test]
    async fn grpc_auth_rejects_tenant_header_mismatch() {
        let token = capability_token("grpc", "search", "example_knowledge");
        let mut request = HttpRequest::builder()
            .uri("/proximadb.v2.ProximaRecordService/Search")
            .header("authorization", format!("Bearer {token}"))
            .header("x-tenant-id", "tenant-b")
            .body(())
            .expect("request should build");

        let status = authenticate_http_request(&coordinator().await, &mut request)
            .await
            .expect_err("tenant header mismatch must fail");
        assert_eq!(status.code(), Code::PermissionDenied);
    }

    #[tokio::test]
    async fn grpc_auth_accepts_normal_jwt_without_data_plane_capability() {
        let jwt_service = JwtService::new(crate::network::auth::config::JwtConfig {
            secret: Some("dev-jwt-secret".to_string()),
            expiration_secs: 900,
            refresh_expiration_secs: 86400,
            issuer: "operator-control-plane".to_string(),
            audience: "proximadb-data-plane".to_string(),
            algorithm: JwtAlgorithm::HS256,
        })
        .expect("jwt service should initialize");
        let token = jwt_service
            .generate_token_pair(
                "admin-user",
                Some("tenant-a".to_string()),
                vec!["admin".to_string()],
            )
            .await
            .expect("jwt should generate")
            .access_token;
        let mut request = HttpRequest::builder()
            .uri("/proximadb.v1.CollectionService/ListCollections")
            .header("authorization", format!("Bearer {token}"))
            .body(())
            .expect("request should build");

        authenticate_http_request(&coordinator().await, &mut request)
            .await
            .expect("normal JWT should authenticate");
        assert!(
            request
                .extensions()
                .get::<GrpcAuthContext>()
                .and_then(|context| context.capability.as_ref())
                .is_none()
        );
    }
}
