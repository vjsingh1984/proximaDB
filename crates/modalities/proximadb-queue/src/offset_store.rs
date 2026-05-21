//! Per-partition committed-offset metadata file.
//!
//! Stored at `{queue_root}/{topic}/{partition_id}/offset.meta` as a tiny
//! JSON document `{"group": "...", "committed_offset": <u64>}`. A successful
//! `Consumer::ack` writes this file; consumer restart reads it to resume.
//!
//! ## Phase 1B scaffold
//!
//! Real read/write via FilesystemFactory lands in a follow-up commit. In
//! this scaffold the offset stays in memory (lost on restart), which is
//! acceptable until the disk tier is wired.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OffsetMeta {
    pub group: String,
    pub committed_offset: u64,
}
