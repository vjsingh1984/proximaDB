//! Vector module alias for symmetric imports with graph::query.
//! The canonical SQL frontend + orchestration remains in crate::query.

pub mod query {
    pub use crate::query::*;
}

