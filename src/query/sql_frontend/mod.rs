//! SQL frontend module - converts external SQL to internal AST

pub mod parser;
pub mod lowering;

#[cfg(test)]
mod tests;

pub use parser::SqlFrontendParser;