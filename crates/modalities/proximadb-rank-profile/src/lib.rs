//! ProximaDB rank-profile DSL + validator + registry.
//!
//! A *rank profile* is a declarative description of the multi-phase
//! scoring pipeline attached to a collection or invoked per-query. See
//! `roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md` (R-4).
//!
//! Pipeline stages:
//!
//! 1. [`spec`] — typed schema + serde for the TOML DSL.
//! 2. [`dsl`] — TOML → [`spec::RankProfileSpec`] parsing.
//! 3. [`validator`] — validates a spec against a [`proximadb_rank_core::BlueprintFactory`];
//!    resolves single-inheritance with cycle detection.
//! 4. [`compiled`] — [`compiled::CompiledRankProfile`] holds the validated
//!    spec + factory; `materialize(qctx)` builds a fresh
//!    [`proximadb_rank_core::RankPipeline`] per query.
//! 5. [`repository`] — `RankProfileRepository` trait + in-memory
//!    implementation. The xCatalog-backed implementation lives in the
//!    catalog crate (out of scope for R-4).
//! 6. [`registry`] — `ProfileRegistry` with `ArcSwap`-based RCU for
//!    hot-reload without disrupting in-flight queries.

pub mod compiled;
pub mod dsl;
pub mod registry;
pub mod repository;
pub mod spec;
pub mod validator;

pub use compiled::CompiledRankProfile;
pub use dsl::{parse_document, parse_single};
pub use registry::ProfileRegistry;
pub use repository::{InMemoryRankProfileRepository, ProfileEvent, RankProfileRepository};
pub use spec::{
    ConstantSpec, FunctionSpec, GlobalPhaseSpec, PhaseBudgetSpec, PhaseSpec, RankProfileSpec,
};
pub use validator::{resolve_inheritance, validate};
