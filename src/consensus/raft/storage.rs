// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Raft storage implementation

use std::sync::{Arc, Mutex};
use raft::storage::Storage;
use raft::Error as StorageError;
use raft::eraftpb::{Entry, HardState, Snapshot, ConfState, SnapshotMetadata};
use raft::RaftState;

use crate::core::VectorDBError;

type Result<T> = std::result::Result<T, VectorDBError>;
type ProximaDBError = VectorDBError;

/// Raft storage implementation for ProximaDB
#[derive(Debug, Clone)]
pub struct RaftStorage {
    /// Core storage state
    core: Arc<Mutex<StorageCore>>,
}

#[derive(Debug)]
struct StorageCore {
    /// Raft hard state (term, vote, commit)
    hard_state: HardState,
    
    /// Log entries
    entries: Vec<Entry>,
    
    /// Current snapshot
    snapshot: Snapshot,
    
    /// Configuration state
    conf_state: ConfState,
    
    /// Applied index
    applied_index: u64,
}

impl RaftStorage {
    /// Create a new Raft storage instance
    pub async fn new() -> Result<Self> {
        let core = StorageCore {
            hard_state: HardState::default(),
            entries: Vec::new(),
            snapshot: Snapshot::default(),
            conf_state: ConfState::default(),
            applied_index: 0,
        };
        
        Ok(Self {
            core: Arc::new(Mutex::new(core)),
        })
    }
    
    /// Append new entries to the log
    pub fn append_entries(&self, entries: &[Entry]) -> Result<()> {
        let mut core = self.core.lock().map_err(|_| {
            ProximaDBError::Consensus(crate::core::ConsensusError::Raft("Failed to acquire storage lock".to_string()))
        })?;
        
        // Validate entries are consecutive and from valid term
        if !entries.is_empty() {
            let first_index = entries[0].index;
            let last_stored_index = core.entries.last().map(|e| e.index).unwrap_or(0);
            
            if first_index != last_stored_index + 1 {
                return Err(ProximaDBError::Consensus(
                    crate::core::ConsensusError::Raft(format!("Non-consecutive entries: expected {}, got {}", 
                           last_stored_index + 1, first_index))
                ));
            }
        }
        
        core.entries.extend_from_slice(entries);
        Ok(())
    }
    
    /// Set the hard state
    pub fn set_hard_state(&self, hard_state: HardState) -> Result<()> {
        let mut core = self.core.lock().map_err(|_| {
            ProximaDBError::Consensus(crate::core::ConsensusError::Raft("Failed to acquire storage lock".to_string()))
        })?;
        
        core.hard_state = hard_state;
        Ok(())
    }
    
    /// Create a snapshot at the given index
    pub fn create_snapshot(&self, index: u64, conf_state: ConfState, data: Vec<u8>) -> Result<()> {
        let mut core = self.core.lock().map_err(|_| {
            ProximaDBError::Consensus(crate::core::ConsensusError::Raft("Failed to acquire storage lock".to_string()))
        })?;
        
        // Find the term for the given index
        let term = core.entries.iter()
            .find(|e| e.index == index)
            .map(|e| e.term)
            .unwrap_or(0);
        
        let mut snapshot_metadata = SnapshotMetadata::default();
        snapshot_metadata.index = index;
        snapshot_metadata.term = term;
        snapshot_metadata.conf_state = Some(conf_state.clone()).into();
        
        core.snapshot.metadata = Some(snapshot_metadata).into();
        core.snapshot.data = data.into();
        core.conf_state = conf_state;
        
        // Remove entries that are now included in the snapshot
        core.entries.retain(|e| e.index > index);
        
        Ok(())
    }
    
    /// Compact log entries up to the given index
    pub fn compact(&self, compact_index: u64) -> Result<()> {
        let mut core = self.core.lock().map_err(|_| {
            ProximaDBError::Consensus(crate::core::ConsensusError::Raft("Failed to acquire storage lock".to_string()))
        })?;
        
        if compact_index <= core.snapshot.metadata.as_ref().map_or(0, |m| m.index) {
            return Err(ProximaDBError::Consensus(
                crate::core::ConsensusError::Raft("Cannot compact to index before snapshot".to_string())
            ));
        }
        
        // Remove entries up to compact_index
        core.entries.retain(|e| e.index > compact_index);
        
        Ok(())
    }
    
    /// Apply entries up to the given index
    pub fn apply(&self, applied_index: u64) -> Result<()> {
        let mut core = self.core.lock().map_err(|_| {
            ProximaDBError::Consensus(crate::core::ConsensusError::Raft("Failed to acquire storage lock".to_string()))
        })?;
        
        core.applied_index = applied_index;
        Ok(())
    }
}

impl Storage for RaftStorage {
    fn initial_state(&self) -> raft::Result<RaftState> {
        let core = self.core.lock().map_err(|_| {
            StorageError::Store(raft::StorageError::Other("Failed to acquire storage lock".into()))
        })?;
        
        Ok(RaftState {
            hard_state: core.hard_state.clone(),
            conf_state: core.conf_state.clone(),
        })
    }
    
    fn entries(&self, low: u64, high: u64, max_size: impl Into<Option<u64>>, _context: raft::GetEntriesContext) -> raft::Result<Vec<Entry>> {
        let core = self.core.lock().map_err(|_| {
            StorageError::Store(raft::StorageError::Other("Failed to acquire storage lock".into()))
        })?;
        
        let max_size = max_size.into().unwrap_or(u64::MAX);
        let mut size = 0;
        let mut result = Vec::new();
        
        for entry in &core.entries {
            if entry.index < low {
                continue;
            }
            if entry.index >= high {
                break;
            }
            
            let entry_size = entry.data.len() as u64;
            if size + entry_size > max_size && !result.is_empty() {
                break;
            }
            
            result.push(entry.clone());
            size += entry_size;
        }
        
        if result.is_empty() && low <= high {
            // Check if the requested range is in the snapshot
            if low <= core.snapshot.metadata.as_ref().map_or(0, |m| m.index) {
                return Err(StorageError::Store(raft::StorageError::Compacted));
            }
            
            // Check if the requested range is beyond available entries
            let last_index = core.entries.last().map(|e| e.index).unwrap_or(0);
            if low > last_index + 1 {
                return Err(StorageError::Store(raft::StorageError::Unavailable));
            }
        }
        
        Ok(result)
    }
    
    fn term(&self, idx: u64) -> raft::Result<u64> {
        let core = self.core.lock().map_err(|_| {
            StorageError::Store(raft::StorageError::Other("Failed to acquire storage lock".into()))
        })?;
        
        // Check snapshot first
        if idx == core.snapshot.metadata.as_ref().map_or(0, |m| m.index) {
            return Ok(core.snapshot.metadata.as_ref().map_or(0, |m| m.term));
        }
        
        if idx < core.snapshot.metadata.as_ref().map_or(0, |m| m.index) {
            return Err(StorageError::Store(raft::StorageError::Compacted));
        }
        
        // Check entries
        for entry in &core.entries {
            if entry.index == idx {
                return Ok(entry.term);
            }
        }
        
        Err(StorageError::Store(raft::StorageError::Unavailable))
    }
    
    fn first_index(&self) -> raft::Result<u64> {
        let core = self.core.lock().map_err(|_| {
            StorageError::Store(raft::StorageError::Other("Failed to acquire storage lock".into()))
        })?;
        
        if let Some(first_entry) = core.entries.first() {
            Ok(first_entry.index)
        } else {
            Ok(core.snapshot.metadata.as_ref().map_or(0, |m| m.index) + 1)
        }
    }
    
    fn last_index(&self) -> raft::Result<u64> {
        let core = self.core.lock().map_err(|_| {
            StorageError::Store(raft::StorageError::Other("Failed to acquire storage lock".into()))
        })?;
        
        if let Some(last_entry) = core.entries.last() {
            Ok(last_entry.index)
        } else {
            Ok(core.snapshot.metadata.as_ref().map_or(0, |m| m.index))
        }
    }
    
    fn snapshot(&self, request_index: u64, _to: u64) -> raft::Result<Snapshot> {
        let core = self.core.lock().map_err(|_| {
            StorageError::Store(raft::StorageError::Other("Failed to acquire storage lock".into()))
        })?;
        
        if core.snapshot.metadata.as_ref().map_or(0, |m| m.index) >= request_index {
            Ok(core.snapshot.clone())
        } else {
            Err(StorageError::Store(raft::StorageError::SnapshotTemporarilyUnavailable))
        }
    }
}