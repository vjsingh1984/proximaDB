//! Startup recovery — replay disk segments past the per-partition committed
//! offset back into the memory tier so consumers resume seamlessly across
//! process restarts.
//!
//! ## Phase 1B scaffold
//!
//! Wired against the disk tier in a follow-up commit. Until then,
//! `recover()` is a no-op; in-flight memory-tier messages are lost on
//! restart but ack'd messages are also memory-only so the contract isn't
//! violated (Lazy mode semantics by default at this stage).

/// Replay sealed disk segments + the active segment past the committed
/// offset back into the memory tier. Returns the number of messages
/// re-enqueued.
pub async fn recover() -> crate::Result<usize> {
    Ok(0)
}
