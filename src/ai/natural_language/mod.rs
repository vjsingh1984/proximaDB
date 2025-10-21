//! Natural Language Query Translation Module
//!
//! This module provides natural language to SQL translation capabilities
//! as specified in the task_1_ai_implementation_design.adoc

pub mod prompt_builder;
pub mod schema_context;
pub mod sql_validator;
pub mod translator;

pub use prompt_builder::{PromptBuilder, PromptTemplate};
pub use schema_context::{SchemaContext, SchemaContextBuilder};
pub use sql_validator::{SQLValidator, ValidationResult};
pub use translator::{NLQueryTranslator, TranslationError, TranslationResult};
