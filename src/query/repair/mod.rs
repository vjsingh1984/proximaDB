//! # Retrieval Repair Controller (LLD §9)
//!
//! Two primitives this module exports:
//!
//!   - `sure_aggregator` — turns pair-level (claim, evidence) verifier outputs
//!     into the five set-level signals from arXiv 2605.03534 (SURE-RAG).
//!   - `decision` — maps the SURE signals + the current candidate set's
//!     shape into a typed RepairAction the runtime can execute. Anchored on
//!     Doctor-RAG arXiv 2604.00865 (prefix reuse) and Skill-RAG arXiv
//!     2604.15771 (bounded skill set: query-rewrite, decompose, focus, exit).
//!
//! The pair-level verifier itself runs outside ProximaDB (AnvaiOps calls
//! whatever model the tenant configured). The data-plane only ever sees
//! the aggregated signals — keeping this primitive testable without an
//! LLM in the loop.

pub mod decision;
pub mod sure_aggregator;

pub use decision::{RepairAction, RepairBudget, RepairDecision, decide};
pub use sure_aggregator::{PairVerification, RelationLabel, SureSignals, aggregate};
