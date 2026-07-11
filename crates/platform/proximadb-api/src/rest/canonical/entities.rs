//! # Vector and Entity Handlers (compatibility stubs)
//!
//! The v1 vector router (`/api/v1/search`, `/api/v1/vectors/*`) and its handler
//! functions were removed in the API standardization hard-rename. Vector
//! similarity search and single-vector record operations are now served by the
//! canonical v2 router (`/api/v2/collections/:id/search`, `/records/*`).
//!
//! Only the handler stub types remain, kept for re-export compatibility with the
//! aggregating `crate::rest` / `crate::lib` handler surfaces.

/// Entity handler stub (kept for re-export compatibility).
pub struct EntityHandler;

impl EntityHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EntityHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Vector handler stub (kept for re-export compatibility).
pub struct VectorHandler;

impl VectorHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for VectorHandler {
    fn default() -> Self {
        Self::new()
    }
}
