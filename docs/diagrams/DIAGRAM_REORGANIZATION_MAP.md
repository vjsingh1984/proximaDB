# Diagram Reorganization Map

## Category Classification and Renaming Strategy

### MONOLITHIC Diagrams (Consolidated/Complete Views)
These provide comprehensive overviews but are replaced with modular abstractions in documentation:

#### Business Monolithic → Business Overview (suffix: `-overview`)
- business-architecture-consolidated.mmd → business-architecture-overview.mmd
- proximadb-architecture-consolidated.mmd → technical-architecture-overview.mmd  
- compression-architecture-consolidated.mmd → compression-architecture-overview.mmd
- query-flow-complete.mmd → query-processing-overview.mmd

### MODULAR Diagrams (Focused/Specific Components)

#### Use Case Diagrams (suffix: `-usecase`)
- usecase-vector-operations.mmd → vector-operations-usecase.mmd
- usecase-module-overview.mmd → module-integration-usecase.mmd
- usecase-collection-management.mmd → collection-management-usecase.mmd
- proximadb-use-cases.mmd → business-applications-usecase.mmd

#### Activity Diagrams (suffix: `-activity`)
- vector-search-activity.mmd → vector-search-activity.mmd ✓ (already semantic)
- vector-insert-workflow.mmd → vector-insert-activity.mmd
- vector-search-workflow.mmd → vector-search-workflow-activity.mmd
- collection-management-workflow.mmd → collection-management-activity.mmd

#### Sequence Diagrams (suffix: `-sequence`)
- initialization-sequence.mmd → system-initialization-sequence.mmd
- vector-insert-sequence.mmd → vector-insert-sequence.mmd ✓ (already semantic)
- sst_viper_search_sequence.mmd → storage-engine-search-sequence.mmd
- search_orchestration_swimlanes.mmd → search-orchestration-sequence.mmd

#### State Diagrams (suffix: `-state`)
- vector-lifecycle-state.mmd → vector-lifecycle-state.mmd ✓ (already semantic)

#### Class Diagrams (suffix: `-class`)  
- proximadb-core-classes.mmd → core-components-class.mmd
- class-module-overview.mmd → module-structure-class.mmd
- class-vector-operations-module.mmd → vector-operations-class.mmd
- class-storage-module.mmd → storage-components-class.mmd
- storage-engine-classes.mmd → storage-engine-class.mmd

#### Architecture Diagrams (suffix: `-architecture`)
- technical-storage-architecture.mmd → storage-components-architecture.mmd
- technical-cache-architecture.mmd → cache-system-architecture.mmd
- hierarchical-sst-architecture.mmd → sst-hierarchical-architecture.mmd
- bloom-filter-architecture.mmd → bloom-filter-architecture.mmd ✓ (already semantic)
- axis-tiering-architecture.mmd → axis-tiering-architecture.mmd ✓ (already semantic)
- security-architecture.mmd → security-components-architecture.mmd
- multi-tenancy-architecture.mmd → multi-tenant-architecture.mmd
- distributed-architecture.mmd → distributed-system-architecture.mmd
- sql-acceleration-architecture.mmd → sql-acceleration-architecture.mmd ✓ (already semantic)

#### Service Diagrams (suffix: `-service`)
- service-interactions.mmd → service-interactions-service.mmd
- service-interactions-unified.mmd → unified-service-interactions-service.mmd
- vector-operations-service.mmd → vector-operations-service.mmd ✓ (already semantic)

#### Data Flow Diagrams (suffix: `-flow`)
- technical-data-flow.mmd → system-data-flow.mmd
- axis-tiering-search-flow.mmd → axis-search-flow.mmd
- comprehensive_search_flow.mmd → comprehensive-search-flow.mmd

#### Strategy/Pattern Diagrams (suffix: `-pattern`)
- write-ahead-log-strategy-pattern.mmd → wal-strategy-pattern.mmd
- engine-quantization-strategy.mmd → quantization-strategy-pattern.mmd
- compression-decision-tree.mmd → compression-decision-pattern.mmd

#### Format/Detail Diagrams (suffix: `-detail`)
- sst1-format.mmd → sst-format-detail.mmd
- sst-adaptive-precision-detail.mmd → sst-precision-detail.mmd
- sst-sorting-architecture.mmd → sst-sorting-detail.mmd

#### System Integration Diagrams (suffix: `-integration`)
- unified-handlers-architecture.mmd → api-handlers-integration.mmd
- unified-parquet-reader-architecture.mmd → parquet-reader-integration.mmd
- unified_quantization_system.mmd → quantization-system-integration.mmd
- test-infrastructure-flow.mmd → test-infrastructure-integration.mmd
- two-stage-search.mmd → two-stage-search-integration.mmd

#### Deployment Diagrams (suffix: `-deployment`)
- proximadb-deployment.mmd → system-deployment.mmd
- deployment-architecture.mmd → production-deployment.mmd
- aws-landing-zone.mmd → aws-deployment.mmd
- azure-landing-zone.mmd → azure-deployment.mmd
- gcp-landing-zone.mmd → gcp-deployment.mmd
- axis-multi-az-architecture.mmd → multi-az-deployment.mmd
- multi_disk_architecture.mmd → multi-disk-deployment.mmd

#### Capability/Feature Diagrams (suffix: `-capability`)
- business-capabilities.mmd → business-capabilities-capability.mmd
- technical-hardware-acceleration.mmd → hardware-acceleration-capability.mmd
- technical-api-protocols.mmd → api-protocols-capability.mmd
- technical-index-algorithms.mmd → index-algorithms-capability.mmd
- chunking-strategies.mmd → text-chunking-capability.mmd

#### Business/Market Diagrams (suffix: `-business`)
- business-overview.mmd → business-overview-business.mmd
- business-use-cases.mmd → business-applications-business.mmd
- business-competitive-advantages.mmd → competitive-advantages-business.mmd
- business-roi-metrics.mmd → roi-metrics-business.mmd

#### Module/Component Diagrams (suffix: `-module`)
- module-interactions-overview.mmd → module-interactions-module.mmd

#### Technical Architecture Diagrams (suffix: `-technical`)
- technical-architecture-unified.mmd → unified-technical-architecture.mmd

## Documentation Update Strategy

After renaming, AsciiDoc files will be updated to use:
1. **Overview diagrams** for high-level introductions
2. **Modular diagrams** for detailed explanations
3. **Specific suffix-based diagrams** for technical deep-dives

Example transformation:
```adoc
// OLD - Monolithic
image::proximadb-architecture-consolidated.svg[Architecture, width=900]

// NEW - Modular
.System Overview
image::system-data-flow.svg[Data Flow, width=450]
image::service-interactions-service.svg[Services, width=450]
```