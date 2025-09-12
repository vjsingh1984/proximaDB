**Detailed Plan for Multi-Tenant Metering and Routing with Explicit Sharing in `proximaDB`**

This plan addresses the requirement for multi-tenancy with explicit collection sharing within organizations, alongside the in-house router and metering service. It introduces a new dedicated service for tenant and access management, which is crucial for handling sharing policies.

**I. Tenant and Access Management Service (New Core Component)**

This service will be responsible for managing tenant identities, organization affiliations, collection ownership, and explicit sharing permissions. It will be a critical dependency for `CollectionService` and other components requiring access control.

*   **Purpose:** Centralized, authoritative source for tenant, organization, and collection access control.
*   **Deployment:** Can be implemented as a separate microservice or a dedicated module within `proximaDB` that interacts with a persistent store (e.g., a relational database, a distributed key-value store). For this phase, we can assume a module within `proximaDB` that uses an existing persistent store (e.g., the metadata backend if it supports complex queries, or a new dedicated store).
*   **Data Model:**
    *   `Tenant` entity: `tenant_id` (unique identifier), `organization_id` (links to an organization), `name`, `status`, etc.
    *   `Organization` entity: `organization_id`, `name`, etc.
    *   `CollectionOwnership` entity: `collection_id`, `owner_tenant_id`.
    *   `CollectionSharing` entity: `collection_id`, `shared_with_tenant_id`, `permissions` (e.g., `READ`, `WRITE`, `ADMIN`).
*   **Key APIs (exposed internally to `proximaDB` services):**
    *   `get_tenant_info(tenant_id)`: Retrieves tenant details.
    *   `check_collection_access(tenant_id, collection_id, required_permission)`: Verifies if a tenant has the specified permission on a collection.
    *   `get_owned_collections(tenant_id)`: Returns a list of `collection_id`s owned by the tenant.
    *   `get_shared_collections(tenant_id)`: Returns a list of `collection_id`s explicitly shared with the tenant.
    *   `grant_collection_access(collection_id, shared_with_tenant_id, permissions)`: Records an explicit sharing grant.
    *   `revoke_collection_access(collection_id, shared_with_tenant_id)`: Removes a sharing grant.

**II. In-House Router (`CustomerRouter`) Design & Integration (Revised for Tenant Context)**

The `CustomerRouter` will focus on tenant identification and injecting the `tenant_id` into the request context for downstream services. It will *not* forward requests to other `proximaDB` instances, as all instances are stateless and can serve any tenant's data.

1.  **`CustomerRouter` Structure:**
    *   Location: `src/network/router.rs` (new file).
    *   Holds: `Arc<UnifiedHandlers>` (for local processing), `Arc<dyn InternalMetricsUpdater>` (for metering).
    *   (No need for clients to other `proximaDB` instances).

2.  **Integration into `MultiServer` (`src/network/multi_server.rs`):**
    *   `MultiServer::new()` will accept an `Arc<CustomerRouter>` and pass it to gRPC and REST service constructors.

3.  **Integration into gRPC/REST Services:**
    *   `new()` methods of `VectorServiceImpl`, `SqlServiceImpl`, `CollectionServiceImpl`, `GraphServiceImpl`, and `RestServer` will accept `Arc<CustomerRouter>`.
    *   Their `async fn` methods will call the corresponding methods on the `CustomerRouter`.

4.  **`CustomerRouter` Request Processing Logic:**
    *   **Tenant Identification:** Extract `tenant_id` from request (e.g., `X-Tenant-ID` header for REST, gRPC metadata).
    *   **Inject Tenant Context:** Use `tonic::Request`'s extensions (gRPC) or `axum::Extension` (REST) to attach the `tenant_id` to the request context. This `tenant_id` will then be accessible by `UnifiedHandlers` and other services.
    *   **Call Local `UnifiedHandlers`:** The `CustomerRouter` will call the appropriate method on its wrapped `UnifiedHandlers`, passing the original request (now with `tenant_id` in context).
    *   **Metering:** Collect metering data (API type, sizes, latency, success) and report it using the `InternalMetricsUpdater`, tagged with the extracted `tenant_id`.

**III. `UnifiedHandlers` (Accepts `tenant_id` and Delegates Access Checks)**

*   All methods in `UnifiedHandlers` that operate on collections or data will be modified to extract the `tenant_id` from the request context (e.g., `tonic::Request.extensions()` or `axum::Extension`).
*   They will then pass this `tenant_id` to the relevant `CollectionService` or `VectorOperationsService` methods.

**IV. `CollectionService` (Major Changes for Cached Access, Access Control, Sharing, Ownership, and Auditing)**

*   **Dependency:** Will depend on the new Tenant and Access Management Service, and will integrate a **Cached Access Service**.
*   **Cached Access Service:**
    *   The `CollectionService` will utilize an internal cache for access decisions.
    *   When `check_collection_access` is called, it will first check the cache. If not found, it will call the Tenant and Access Management Service and cache the result (with a configurable TTL).
    *   **Access Auditing:** This cache will log *all* API calls to the Tenant Access Management Service for auditing purposes, regardless of success or failure. These audit logs will be sent to a dedicated audit log system.
*   **`create_collection`:**
    *   Accepts `tenant_id` as an argument.
    *   Before creating the collection metadata in object storage, it will call the Tenant Access Management Service to record `collection_id` and `owner_tenant_id`.
    *   The collection metadata in object storage will explicitly include `owner_tenant_id`.
    *   **Ownership Check:** Implicitly, the creating `tenant_id` becomes the owner.
    *   **Metering Trigger:** Upon successful creation and access audit, triggers metering for this operation.
*   **`get_collection`:**
    *   Accepts `tenant_id` and `collection_id`.
    *   Calls `CachedAccessService.check_collection_access(tenant_id, collection_id, READ_PERMISSION)`.
    *   Returns authorization error if access is denied.
    *   **Metering Trigger:** Upon successful operation and access audit, triggers metering for this operation. Unsuccessful operations (including authorization failures) will *not* be metered for billing.
*   **`update_collection` / `delete_collection`:**
    *   Accepts `tenant_id` and `collection_id`.
    *   Calls `CachedAccessService.check_collection_access(tenant_id, collection_id, WRITE_PERMISSION)`.
    *   **Ownership Check:** Additionally, verifies that the `tenant_id` making the request is the `owner_tenant_id` of the collection. Returns an error if not.
    *   Returns authorization error if access is denied.
    *   **Metering Trigger:** Upon successful operation and access audit, triggers metering for this operation. Unsuccessful operations (including authorization failures) will *not* be metered for billing.
*   **`list_collections`:**
    *   Accepts `tenant_id`.
    *   Calls `TenantAccessService.get_owned_collections(tenant_id)` to get owned collections.
    *   Calls `TenantAccessService.get_shared_collections(tenant_id)` to get explicitly shared collections.
    *   Combines these lists, retrieves metadata for each (which includes `owner_tenant_id`), and returns the result.
    *   **Metering Trigger:** Upon successful listing and access audit, triggers metering for this operation (e.g., for the number of collections listed).
*   **`share_collection` / `revoke_share` (New Methods):**
    *   These methods will be added to `CollectionService` to allow tenants to explicitly manage sharing. They will interact with the Tenant and Access Management Service.
    *   **Ownership Check:** Verifies that the `tenant_id` making the request is the `owner_tenant_id` of the collection being shared/unshared. Returns an error if not.
    *   **Metering Trigger:** Upon successful sharing/revocation and access audit, triggers metering for this operation.

**V. `VectorOperationsService` and SQL Engine (Tenant-Aware Operations and Metering Trigger)**

*   **`VectorOperationsService`:** Methods will accept `tenant_id` and `collection_id`. They will rely on `CollectionService` to ensure the `tenant_id` has access to the `collection_id` before performing vector operations. **Upon successful completion of vector operations and access audit, `CollectionService` will trigger the metering.**
*   **SQL Engine:** Will be modified to enforce tenant isolation. SQL queries will implicitly operate within the context of the `tenant_id` provided in the request. This means the SQL engine will need to ensure that any collection referenced in a query is accessible by the `tenant_id` (via `CollectionService` access checks). **Upon successful SQL query execution and access audit, `CollectionService` will trigger the metering.**

**VI. Lightweight Metering Service Integration (Tenant-Based with Audited Trigger)**

*   **Metering Trigger Point:** Metering for billing purposes will be triggered by the `CollectionService` (or the `CachedAccessService` it uses) *after* a successful and authorized operation. Unsuccessful operations (including authorization failures) will *not* be metered for billing.
*   **Define New Metric Types (in `src/metrics/schema.rs`):**
    *   `CustomerMetrics` struct: `tenant_id: String`, `total_api_calls: i64`, `total_request_bytes: i64`, `total_response_bytes: i64`, `total_inserts: i64`, `total_insert_bytes: i64`, `total_searches: i64`, `total_search_bytes: i64`, `avg_api_latency_us: f64`, `last_updated: i64`.
    *   `CustomerApiCallUpdate` struct: `api_type: String`, `request_size_bytes: u64`, `response_size_bytes: u64`, `processing_time_us: u64`, `success: bool` (will always be `true` for metered calls), `data_inserted_bytes: u64`, `data_scanned_bytes: u64`.
*   **Modify `MetricsUpdate` Enum (in `src/metrics/updater.rs`):** Add `MetricsUpdate::CustomerApiCall { tenant_id: String, update: CustomerApiCallUpdate }`.
*   **Modify `InternalMetricsUpdater` Trait and Implementations:** Add `async fn record_customer_api_call(&self, tenant_id: &str, update: CustomerApiCallUpdate) -> Result<()>`.
*   **`CustomerRouter`'s Role in Metering:** The `CustomerRouter` will primarily focus on tenant identification and context injection. It may collect raw request counts for *internal diagnostics/debugging*, but it will *not* trigger billing-relevant metering.
*   **Data Flow and Aggregation:** `AsyncMetricsUpdater` processes updates, aggregates into `CustomerMetrics`, and `MetricsPersistenceLayer` persists to cloud storage.

**VII. Billing Integration (External Service)**

*   External service reads aggregated `CustomerMetrics` from cloud storage, calculates usage per tenant, and reports to cloud marketplace billing APIs.

---