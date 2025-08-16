# EventLog and AXIS Integration Architecture

## Overview

ProximaDB implements a lightweight queue-based integration between storage operations and the AXIS indexing system. This eliminates the 2x write amplification problem while maintaining asynchronous index updates.

## Problem Solved

Previously, the AXIS indexing system caused:
- **2x Write Amplification**: Data was written twice - once to storage, once to AXIS
- **Synchronous Blocking**: Storage operations waited for AXIS processing
- **Serial Compaction**: All compaction blocked if any AXIS operations were pending

## Solution Architecture

### 1. EventLog Queue System

The EventLog acts as a lightweight message queue (600 lines vs 5000+ for Kafka):

```rust
// Storage notifies EventLog synchronously (just metadata)
eventlog.notify_flush(FlushEvent {
    collection_id,
    data_files: vec![file_paths],  // Just paths, not data
    vector_count,
    has_quantized,
    has_fp32,
}).await?;  // Waits for acknowledgment

// AXIS processes asynchronously
axis_consumer.process_events().await;
```

### 2. Granular Compaction

Instead of blocking all compaction, only AXIS-pending files are excluded:

```rust
// Before: All-or-nothing
if axis_has_pending_operations() {
    return; // No compaction at all
}

// After: Granular file filtering
let compactable_files = all_files
    .filter(|f| !eventlog.is_pending(f))
    .collect();
compact(compactable_files);
```

### 3. Self-Healing Behavior

Files automatically become compactable when AXIS completes:

```
Time T1: File A, B, C flushed → EventLog notified
Time T2: File D, E flushed → EventLog notified
Time T3: AXIS completes A, B, C → Now compactable
Time T4: Compaction runs with A, B, C (D, E still pending)
Time T5: AXIS completes D, E → Now compactable
```

## Configuration

### Compaction Configuration

The new compaction system is fully configurable via TOML:

```toml
[storage.compaction_config]
# Level 0 thresholds
l0_file_threshold = 5          # Files before compaction
l0_size_threshold_mb = 256     # MB before compaction

# Higher level configuration
level_multiplier = 2.0          # Threshold multiplier per level
max_levels = 7                  # Maximum compaction levels

# Compaction strategy
strategy = "hybrid"             # count, size, or hybrid
target_file_size_mb = 128       # Output file size

# Engine-specific overrides
[storage.sst_config.compaction_config]
l0_file_threshold = 10          # SST handles more files

[storage.viper_config.compaction_config]
target_file_size_mb = 256       # VIPER prefers larger files
```

### Compaction Strategies

- **count**: Trigger when file count exceeds threshold
- **size**: Trigger when total size exceeds threshold  
- **hybrid**: Trigger when EITHER count OR size exceeds threshold (recommended)

### Level Calculations

For each level L:
- File threshold = `l0_file_threshold × level_multiplier^L`
- Size threshold = `l0_size_threshold_mb × level_multiplier^L`

Example with defaults:
- L0: 5 files or 256MB
- L1: 10 files or 512MB
- L2: 20 files or 1GB
- L3: 40 files or 2GB

## Implementation Details

### EventLog Service

Located in `/src/services/event_log_service.rs`:
- Lightweight in-memory queue with persistence
- Synchronous acknowledgment for flush operations
- Async processing for AXIS consumers

### AXIS Consumer

Located in `/src/index/axis/eventlog_consumer.rs`:
- Polls EventLog for new events
- Extracts vectors based on index requirements
- Updates AXIS indexes asynchronously
- Marks events as processed

### Unified Compaction Framework

Located in `/src/storage/common/compaction_utils.rs`:
- Single source of truth for file discovery
- EventLog-aware file filtering
- Consistent behavior across SST and VIPER engines

## Benefits

1. **Eliminated Write Amplification**: Data written once, metadata queued
2. **Non-Blocking Storage**: Storage ops only wait for queue acknowledgment
3. **Granular Compaction**: Only pending files excluded, not all compaction
4. **Self-Healing**: Files automatically become available when AXIS completes
5. **Configurable Thresholds**: Full control via TOML configuration
6. **Unified Logic**: Single implementation for all storage engines

## Monitoring

### Debug Logging

Extensive debug logging throughout the pipeline:

```
[AXIS Consumer] Starting processing of FLUSH event abc123
  Collection: my_collection
  Files: ["file1.sst", "file2.sst"]
  Vector Count: 1000
  Has Quantized: true
  Has FP32: true
  Storage Engine: SST

[AXIS Consumer] Extraction complete:
  FP32 vectors: 500
  Quantized vectors: 500
  Processing time: 125ms
```

### Compaction Metrics

```
COMPACTION: Discovery complete for collection my_collection
  Total files: 10
  Compactable: 7
  Pending AXIS: 3

SELF-HEALING: File file1.sst for collection my_collection is now ready for compaction
```

## Future Enhancements

1. **Priority Queues**: High-priority collections processed first
2. **Batch Processing**: Group multiple events for efficiency
3. **Circuit Breakers**: Automatic fallback if AXIS is overloaded
4. **Distributed Queue**: Multi-node EventLog for HA deployments
5. **Dead Letter Queue**: Handle failed events gracefully

---
*Last Updated: 2025-01-15*