//! Analytics utilities that read collection state and report quality metrics.
//!
//! This module hosts read-only analyzers that observe vectors and chunks
//! already in ProximaDB and compute interpretable diagnostics. Analyzers
//! never mutate storage; they are safe to invoke at any time.
//!
//! Currently implemented:
//!
//! - [`entanglement`] — Loghmani 2026's *Entanglement Index* (EI) for
//!   measuring cross-topic overlap in embedding space, the diagnostic
//!   metric behind TD-043 (Semantic Disentanglement Pipeline).

#![allow(missing_docs)]

pub mod entanglement;
