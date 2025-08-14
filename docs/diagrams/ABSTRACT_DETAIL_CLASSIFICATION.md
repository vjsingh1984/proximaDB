# Abstract vs Detail Classification for Diagrams

## Classification Strategy

**ABSTRACT**: High-level overviews, simplified views, executive summaries
**DETAIL**: Comprehensive technical details, complete workflows, implementation specifics

## Current Diagrams Requiring Reclassification

### OVERVIEW → ABSTRACT (High-level business/technical overviews)
- business-architecture-overview.mmd → business-architecture-abstract.mmd
- technical-architecture-overview.mmd → technical-architecture-abstract.mmd  
- compression-architecture-overview.mmd → compression-architecture-abstract.mmd
- query-processing-overview.mmd → query-processing-abstract.mmd

### UNIFIED → DETAIL (Comprehensive, detailed views)
- unified-service-interactions-service.mmd → service-interactions-detail.mmd
- unified-technical-architecture.mmd → technical-architecture-detail.mmd

### COMPREHENSIVE → DETAIL (Complete, detailed workflows)
- comprehensive-search-flow.mmd → search-flow-detail.mmd

### Architecture Diagrams - Classify by Complexity
**ABSTRACT (High-level architectural views):**
- axis-tiering-architecture.mmd → axis-tiering-abstract.mmd
- bloom-filter-architecture.mmd → bloom-filter-abstract.mmd
- cache-system-architecture.mmd → cache-system-abstract.mmd
- security-components-architecture.mmd → security-components-abstract.mmd
- storage-components-architecture.mmd → storage-components-abstract.mmd
- multi-tenant-architecture.mmd → multi-tenant-abstract.mmd
- distributed-system-architecture.mmd → distributed-system-abstract.mmd
- sql-acceleration-architecture.mmd → sql-acceleration-abstract.mmd

**DETAIL (Detailed architectural implementations):**
- sst-hierarchical-architecture.mmd → sst-hierarchical-detail.mmd

### Service Diagrams
**ABSTRACT:**
- service-interactions-service.mmd → service-interactions-abstract.mmd
- vector-operations-service.mmd → vector-operations-abstract.mmd

### Flow Diagrams  
**ABSTRACT (High-level flows):**
- system-data-flow.mmd → system-data-flow-abstract.mmd
- axis-search-flow.mmd → axis-search-abstract.mmd

### Capability Diagrams (Keep as abstract by nature)
- business-capabilities-capability.mmd → business-capabilities-abstract.mmd
- hardware-acceleration-capability.mmd → hardware-acceleration-abstract.mmd
- api-protocols-capability.mmd → api-protocols-abstract.mmd
- index-algorithms-capability.mmd → index-algorithms-abstract.mmd
- text-chunking-capability.mmd → text-chunking-abstract.mmd

### Integration Diagrams (Detailed by nature - keep detail suffix)
- api-handlers-integration.mmd → api-handlers-detail.mmd
- parquet-reader-integration.mmd → parquet-reader-detail.mmd
- quantization-system-integration.mmd → quantization-system-detail.mmd
- test-infrastructure-integration.mmd → test-infrastructure-detail.mmd
- two-stage-search-integration.mmd → two-stage-search-detail.mmd

### Sequence Diagrams
**ABSTRACT (Simple sequences):**
- vector-insert-sequence.mmd → vector-insert-abstract.mmd
- vector-lifecycle-state.mmd → vector-lifecycle-abstract.mmd

**DETAIL (Complex sequences):**
- system-initialization-sequence.mmd → system-initialization-detail.mmd
- storage-engine-search-sequence.mmd → storage-engine-search-detail.mmd
- search-orchestration-sequence.mmd → search-orchestration-detail.mmd

### Class Diagrams
**ABSTRACT (High-level class overviews):**
- core-components-class.mmd → core-components-abstract.mmd
- module-structure-class.mmd → module-structure-abstract.mmd
- storage-engine-class.mmd → storage-engine-abstract.mmd

**DETAIL (Detailed class implementations):**
- storage-components-class.mmd → storage-components-detail.mmd
- vector-operations-class.mmd → vector-operations-detail.mmd

### Activity Diagrams (Keep abstract for activities)
- collection-management-activity.mmd → collection-management-abstract.mmd
- vector-insert-activity.mmd → vector-insert-abstract.mmd
- vector-search-activity.mmd → vector-search-abstract.mmd
- vector-search-workflow-activity.mmd → vector-search-workflow-abstract.mmd

### Use Case Diagrams (Abstract by nature)
- business-applications-usecase.mmd → business-applications-abstract.mmd
- collection-management-usecase.mmd → collection-management-abstract.mmd
- module-integration-usecase.mmd → module-integration-abstract.mmd
- vector-operations-usecase.mmd → vector-operations-abstract.mmd

### Business Diagrams (Abstract by nature)
- business-overview-business.mmd → business-overview-abstract.mmd
- business-applications-business.mmd → business-applications-abstract.mmd
- competitive-advantages-business.mmd → competitive-advantages-abstract.mmd
- roi-metrics-business.mmd → roi-metrics-abstract.mmd

### Deployment Diagrams (Keep current names - they're specific by nature)
- aws-deployment.mmd → aws-deployment-detail.mmd
- azure-deployment.mmd → azure-deployment-detail.mmd
- gcp-deployment.mmd → gcp-deployment-detail.mmd
- multi-az-deployment.mmd → multi-az-deployment-detail.mmd
- multi-disk-deployment.mmd → multi-disk-deployment-detail.mmd
- production-deployment.mmd → production-deployment-detail.mmd
- system-deployment.mmd → system-deployment-abstract.mmd

### Pattern Diagrams (Keep as abstract)
- compression-decision-pattern.mmd → compression-decision-abstract.mmd
- quantization-strategy-pattern.mmd → quantization-strategy-abstract.mmd
- wal-strategy-pattern.mmd → wal-strategy-abstract.mmd

### Module Diagrams (Abstract by nature)
- module-interactions-module.mmd → module-interactions-abstract.mmd

## Final Naming Convention

Format: `[component]-[type]-[abstraction_level].mmd`

- **component**: business, technical, storage, vector, etc.
- **type**: architecture, sequence, class, usecase, activity, etc.
- **abstraction_level**: abstract OR detail

Examples:
- `technical-architecture-abstract.mmd` (high-level overview)
- `technical-architecture-detail.mmd` (comprehensive implementation)
- `vector-operations-abstract.mmd` (simple use case view)
- `service-interactions-detail.mmd` (complete sequence diagram)