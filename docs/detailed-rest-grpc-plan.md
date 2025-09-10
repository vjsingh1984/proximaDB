= API Handler Alignment Roadmap

  1. Executive Summary

  This roadmap outlines a strategy to align ProximaDB's REST and gRPC API
  handlers, addressing current inconsistencies and establishing a
  "protobuf-first" design principle. The primary goal is to achieve a unified,
  consistent, and maintainable API surface across both protocols by centralizing
  the API contract around Protocol Buffers and fully leveraging the
  UnifiedHandlers business logic layer. This will reduce code duplication,
  improve developer experience, and enhance the overall robustness and
  performance of the API.

  2. Current State Analysis: Inconsistencies Identified

  A detailed review of src/network/grpc/v1/service.rs,
  src/network/rest/progressive_search_handler.rs, and
  src/api_handlers/unified_handlers.rs reveals several key areas of
  inconsistency:

  2.1. Request/Response DTOs

   * gRPC (`ProximaDbGrpcService`): Directly utilizes protobuf-generated types
     (e.g., VectorSearchRequest, VectorOperationResponse). This approach is
     inherently type-safe and optimized for wire efficiency.
   * REST (`progressive_search_handler.rs`): Employs custom serde-derived structs
     (ProgressiveSearchRequest, ProgressiveSearchResponse, SearchResult,
     SearchResultDto).
   * Inconsistency: There is a significant duplication of data transfer object (DTO)
      definitions. The REST layer defines its own DTOs that largely mirror the
     protobuf structures but require manual mapping and conversion, introducing
     unnecessary complexity and potential for divergence.

  2.2. Unified Handler Usage

   * `UnifiedHandlers` Module: This module is explicitly designed to encapsulate
     the core business logic, acting as a protocol-agnostic intermediary between
     network handlers and backend services.
   * gRPC: Effectively utilizes UnifiedHandlers methods. It translates incoming
     protobuf requests into the format expected by UnifiedHandlers and converts the
     UnifiedHandlers' responses back into protobuf for gRPC clients.
   * REST: While progressive_search_handler.rs does call UnifiedHandlers'
     unified_search method (via vector_operations_service), it then performs manual
     flattening and conversion of the proto_results into its own custom REST DTOs.
   * Inconsistency: The REST layer undermines the "single source of truth"
     principle of UnifiedHandlers by re-processing and re-structuring the output,
     leading to redundant logic and potential for inconsistent API responses.

  2.3. Error Handling

   * gRPC: Adheres to the gRPC standard for error reporting, utilizing
     tonic::Status (which includes a status code, message, and optional details).
   * REST: Employs axum::http::StatusCode for HTTP status and custom error messages
     embedded within the Result type.
   * Inconsistency: The disparate error handling mechanisms necessitate separate,
     often duplicated, logic for error propagation and client-facing error messages
     across the two protocols.

  2.4. Feature Parity and Request Parameters

   * gRPC `VectorSearchRequest`: A comprehensive protobuf message that supports a
     wide range of search parameters, including multiple queries, top_k,
     include_fields (for selective data retrieval), distance_metric_override, and a
     flexible search_params field for advanced optimization hints (e.g.,
     quantization hints, progressive search scenarios).
   * REST `ProgressiveSearchRequest`: Introduces specialized fields (scenario,
     adaptive_recall, custom_recalls) that are specific to the "progressive search"
     concept. These fields are then mapped to more generic SearchParams fields
     before being passed to UnifiedHandlers.
   * Inconsistency: The REST API's request parameters are less unified and introduce
      specialized, often redundant, fields for specific search types. This creates a
      less consistent API experience and requires additional mapping logic.


  2.5. Response Structure and Metadata

   * gRPC `VectorOperationResponse`: Provides a rich and structured response,
     including metrics, results (a SearchResult protobuf message containing
     SearchVectorRecords), vector_ids, error_message, error_code, and result_info.
     Each SearchVectorRecord is detailed, encompassing id, vector, metadata, score,
     similarity, version, and timestamp.
   * REST `ProgressiveSearchResponse`: Offers a simplified response structure with
     results (a Vec<SearchResult>), processing_time_ms, and stages_executed. The
     REST SearchResult is minimal, often lacking the comprehensive metadata and
     versioning information available in the gRPC SearchVectorRecord.
   * Inconsistency: The REST search response is less granular and less structured
     than its gRPC counterpart. This can force REST clients to make additional
     requests or infer information that is readily available in the gRPC response,
     leading to less efficient client-side implementations.

  3. Recommended Alignment Strategy: Protobuf-First

  The overarching strategy for achieving API consistency is to adopt a
  "Protobuf-First" design. This means that Protocol Buffers will serve as the
  definitive API contract for all operations, regardless of the underlying
  transport protocol (gRPC or REST).

  Core Principle:
   * Protobuf as the Single Source of Truth: All API requests, responses, and error
     structures will be defined in .proto files.
   * Thin Protocol Wrappers: Both gRPC and REST handlers will act as minimal
     wrappers, primarily responsible for:
       * Deserializing incoming requests into protobuf messages.
       * Calling the appropriate method in the UnifiedHandlers layer.
       * Serializing the protobuf responses from UnifiedHandlers into the
         respective wire format (gRPC or JSON).
       * Translating internal errors into a standardized, protocol-agnostic error
         type that can then be mapped to protocol-specific error representations.

  This approach ensures that any changes to the API contract are made once in the
  .proto files and automatically propagate to both gRPC and REST, significantly
  reducing the risk of divergence and simplifying API evolution.

  4. Phased Implementation Plan

  The alignment process will be executed in three distinct phases, each with
  specific tasks and expected outcomes.

  Phase 1: Foundational Alignment

  Goal: Establish a consistent DTO and error handling foundation across REST and
  gRPC by leveraging existing protobuf definitions.

   * Task 1.1: Standardize REST Request DTOs
       * Description: Modify REST API handlers to directly accept and deserialize
         incoming JSON requests into the corresponding protobuf request messages
         (e.g., VectorSearchRequest, CollectionRequest). This eliminates the need
         for custom REST-specific request DTOs.
       * Implementation Details:
           * For each REST endpoint, identify the equivalent gRPC protobuf request
             message.
           * Update axum extractors (e.g., ExtractJson) to directly target the
             protobuf message type.
           * Implement From<JsonValue> for ProtobufMessage or similar conversion
             logic within the REST handler layer if direct deserialization is not
             feasible (e.g., for complex nested structures).
           * Remove redundant custom REST request DTO structs (e.g.,
             ProgressiveSearchRequest).
       * Expected Outcome: REST requests are directly mapped to protobuf messages,
         reducing DTO duplication.

   * Task 1.2: Standardize REST Response DTOs
       * Description: Modify REST API handlers to directly serialize the protobuf
         response messages received from UnifiedHandlers into JSON. This ensures
         that the REST response structure mirrors the gRPC response structure.
       * Implementation Details:
           * For each REST endpoint, identify the equivalent gRPC protobuf response
             message (e.g., VectorOperationResponse, CollectionResponse).
           * Update axum response types to Json<ProtobufMessage>.
           * Ensure that UnifiedHandlers methods return these protobuf messages
             directly.
           * Remove redundant custom REST response DTO structs (e.g.,
             ProgressiveSearchResponse, SearchResult, SearchResultDto).
       * Expected Outcome: REST responses are directly mapped from protobuf
         messages, ensuring structural consistency with gRPC.

   * Task 1.3: Implement Unified Error Handling
       * Description: Introduce a single, internal ApiError type that encapsulates
         all possible application-level errors. This ApiError will then be
         converted to tonic::Status for gRPC and axum::http::StatusCode with a
         standardized JSON error body for REST.
       * Implementation Details:
           * Define a custom enum ApiError in a shared module (e.g., src/errors.rs)
             with variants for common error scenarios (e.g., CollectionNotFound,
             InvalidArgument, InternalError, Unauthorized).
           * Implement From<ApiError> for tonic::Status to map ApiError variants to
             appropriate gRPC status codes and messages.
           * Implement IntoResponse for ApiError (or a custom axum extractor) to map
              ApiError variants to HTTP status codes and a consistent JSON error
             payload for REST.
           * Modify UnifiedHandlers methods to return Result<T, ApiError>.
           * Update gRPC and REST handlers to catch ApiError and convert it to
             their respective protocol-specific error representations.
       * Expected Outcome: Consistent and centralized error handling logic across
         both protocols, simplifying error management.

  Phase 2: Deep Integration & Refinement

  Goal: Optimize handler logic and ensure full feature parity and consistent API
  behavior.

   * Task 2.1: Refactor REST Handlers for Direct UnifiedHandlers Usage
       * Description: Eliminate any remaining custom logic within REST handlers that
          re-processes or re-structures data received from UnifiedHandlers. The REST
          handlers should become thin wrappers that directly pass requests to and
         return responses from UnifiedHandlers.
       * Implementation Details:
           * Review progressive_search_handler.rs and remove the manual flattening
             and conversion of proto_results. The run_progressive_search function
             should directly return the VectorOperationResponse from
             unified_search.
           * Ensure that any specialized REST-only parameters are correctly mapped
             into the search_params field of the VectorSearchRequest protobuf
             message within the REST handler, before calling UnifiedHandlers.
       * Expected Outcome: REST handlers are simplified, reducing code complexity
         and ensuring full adherence to the UnifiedHandlers contract.

   * Task 2.2: Review and Align Feature Parity
       * Description: Conduct a comprehensive review of all REST endpoints against
         the gRPC service definition to identify any functional gaps or redundant
         features.
       * Implementation Details:
           * For any REST-only features, evaluate if they should be exposed via
             gRPC. If so, update the .proto definition and implement the
             corresponding gRPC service method.
           * For any gRPC-only features, evaluate if they should be exposed via
             REST. If so, create the corresponding REST endpoint and integrate it
             with UnifiedHandlers.
           * Ensure that all parameters and response fields are consistently
             available and behave identically across both protocols.
       * Expected Outcome: Full feature parity between REST and gRPC APIs,
         providing a consistent experience for all clients.

   * Task 2.3: Enhance UnifiedHandlers for REST-Specific Needs (if any)
       * Description: If specific REST API requirements (e.g., simplified response
         formats for certain endpoints) cannot be directly met by protobuf
         structures without compromising gRPC, consider adding minimal,
         configurable options within UnifiedHandlers to support these.
       * Implementation Details:
           * This task should only be undertaken if absolutely necessary and after
             careful consideration of the trade-offs.
           * Any such enhancements should be designed to be optional and not impact
             the core protobuf-first principle.
       * Expected Outcome: UnifiedHandlers remains the central logic, with minimal,
         controlled adaptations for specific protocol needs.

  Phase 3: Validation & Documentation

  Goal: Ensure the aligned APIs are robust, performant, and well-documented.

   * Task 3.1: Implement Comprehensive Integration Tests
       * Description: Develop a suite of integration tests that verify the
         consistency and correctness of both REST and gRPC APIs. These tests should
         cover:
           * Request/response mapping for all endpoints.
           * Error handling scenarios.
           * Feature parity and consistent behavior.
           * Performance characteristics.
       * Implementation Details:
           * Use a common test framework (e.g., tokio::test with reqwest for REST
             and tonic::test for gRPC).
           * Automate testing in CI/CD pipelines.
       * Expected Outcome: High confidence in the correctness and consistency of
         the aligned APIs.

   * Task 3.2: Update API Documentation
       * Description: Revise all API documentation (e.g., OpenAPI/Swagger for REST,
         Protobuf/gRPC documentation) to reflect the unified API contract.
       * Implementation Details:
           * Clearly state that protobuf definitions are the source of truth.
           * Provide examples for both gRPC and REST requests/responses,
             highlighting their structural similarity.
           * Document the standardized error handling.
       * Expected Outcome: Clear, accurate, and consistent API documentation for
         all users.

   * Task 3.3: Performance Benchmarking
       * Description: Conduct performance benchmarks for both REST and gRPC APIs to
         measure the impact of the alignment efforts.
       * Implementation Details:
           * Measure latency, throughput, and resource utilization for key
             operations.
           * Compare performance before and after alignment to quantify
             improvements.
       * Expected Outcome: Quantifiable performance metrics demonstrating the
         benefits of the unified approach.

  5. Expected Benefits

  Implementing this phased roadmap will yield significant benefits for ProximaDB:

   * Reduced Code Duplication: Eliminates redundant DTOs and conversion logic,
     leading to a smaller, more concise codebase.
   * Improved Maintainability: Changes to the API contract are made once in
     protobuf, automatically propagating to both gRPC and REST, simplifying
     maintenance and reducing the risk of bugs.
   * Enhanced Consistency: Both APIs will offer the same features, request/response
     structures, and error handling, providing a seamless and predictable experience
      for client developers.
   * Clearer API Contract: Protobuf becomes the single, unambiguous source of truth
     for the API, improving clarity and reducing ambiguity.
   * Better Performance: Eliminating intermediate JSON conversions and leveraging
     protobuf's efficiency can lead to reduced overhead and improved performance,
     especially for high-throughput operations.
   * Faster Feature Development: New API features can be implemented once in
     UnifiedHandlers and exposed via both protocols with minimal additional effort.
   * Simplified Client Development: Clients can interact with ProximaDB using their
     preferred protocol, knowing that the underlying API contract and behavior are
     consistent.
