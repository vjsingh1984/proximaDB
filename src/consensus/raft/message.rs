// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Raft message types and command handling

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::{CollectionId, VectorId, VectorRecord};

/// Inter-node Raft messages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RaftMessage {
    /// Vote request during leader election
    VoteRequest {
        term: u64,
        candidate_id: u64,
        last_log_index: u64,
        last_log_term: u64,
    },

    /// Vote response
    VoteResponse { term: u64, vote_granted: bool },

    /// Append entries (heartbeat or log replication)
    AppendEntries {
        term: u64,
        leader_id: u64,
        prev_log_index: u64,
        prev_log_term: u64,
        entries: Vec<LogEntry>,
        leader_commit: u64,
    },

    /// Append entries response
    AppendEntriesResponse {
        term: u64,
        success: bool,
        match_index: Option<u64>,
    },

    /// Install snapshot message
    InstallSnapshot {
        term: u64,
        leader_id: u64,
        last_included_index: u64,
        last_included_term: u64,
        data: Vec<u8>,
        done: bool,
    },

    /// Install snapshot response
    InstallSnapshotResponse { term: u64, success: bool },
}

/// Raft log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub term: u64,
    pub index: u64,
    pub command: RaftCommand,
    pub timestamp: DateTime<Utc>,
}

/// Commands that can be proposed to the Raft cluster
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RaftCommand {
    /// Insert a vector into a collection
    VectorInsert {
        collection_id: CollectionId,
        vector_record: VectorRecord,
    },

    /// Delete a vector from a collection
    VectorDelete {
        collection_id: CollectionId,
        vector_id: VectorId,
    },

    /// Create a new collection
    CollectionCreate { collection: CollectionDefinition },

    /// Delete a collection
    CollectionDelete { collection_id: CollectionId },
}

/// Collection definition for Raft commands
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionDefinition {
    pub id: CollectionId,
    pub name: String,
    pub dimension: usize,
    pub schema_type: String,
    pub created_at: DateTime<Utc>,
}

/// Responses from Raft command execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RaftResponse {
    /// Vector insertion result
    VectorInserted { id: VectorId, success: bool },

    /// Vector deletion result
    VectorDeleted { id: VectorId, success: bool },

    /// Collection creation result
    CollectionCreated { id: CollectionId, success: bool },

    /// Collection deletion result
    CollectionDeleted { id: CollectionId, success: bool },

    /// Error response
    Error { message: String, code: String },
}
