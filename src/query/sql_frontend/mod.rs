//! SQL frontend module - converts external SQL to internal AST

pub mod lowering;
pub mod parser;

#[cfg(test)]
mod tests;

pub use parser::SqlFrontendParser;
