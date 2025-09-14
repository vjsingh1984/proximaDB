//! Natural Language Query Translation Module
//!
//! This module provides natural language to SQL translation capabilities
//! as specified in the task_1_ai_implementation_design.adoc

pub mod translator;
pub mod schema_context;
pub mod sql_validator;
pub mod prompt_builder;

pub use translator::{NLQueryTranslator, TranslationResult, TranslationError};
pub use schema_context::{SchemaContext, SchemaContextBuilder};
pub use sql_validator::{SQLValidator, ValidationResult};
pub use prompt_builder::{PromptBuilder, PromptTemplate};