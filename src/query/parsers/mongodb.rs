// Re-export shim — mongodb parser extracted to proximadb-mongodb-parser crate.
//
// The parser core (lexer, AST, conversion to DocumentFilter) now lives in the
// dedicated `proximadb-mongodb-parser` crate. This module re-exports all of its
// public surface so existing call sites (`proximadb::query::parsers::mongodb::*`,
// and the `pub use mongodb::{...}` in the parent `parsers::mod`) resolve
// unchanged.
//
// The two trait impls below bind the extracted types to root-local traits
// (`QueryParser`, `ToFilter`) that are shared with the Cypher parser and live in
// `super`. They cannot live in the extracted crate (that would require the crate
// to depend on the root, forming a build cycle), so they are kept here at the
// seam.
pub use proximadb_mongodb_parser::*;

use anyhow::{Result, anyhow};
use serde_json::Value as JsonValue;

use super::{QueryParser, ToFilter};
use crate::proto::proximadb_v1::DocumentFilter;

impl QueryParser for MongoDBParser {
    type Output = MongoDBParseResult;

    fn parse(&self, input: &str) -> Result<Self::Output> {
        // Try to determine if this is a pipeline or a query
        let json: JsonValue = serde_json::from_str(input)?;

        match &json {
            JsonValue::Array(_) => {
                // This is a pipeline
                let pipeline = self.parse_pipeline(input)?;
                Ok(MongoDBParseResult {
                    query: None,
                    pipeline: Some(pipeline),
                })
            }
            JsonValue::Object(_) => {
                // This is a query
                let expr = self.parse_query(input)?;
                Ok(MongoDBParseResult {
                    query: Some(MongoDBQuery {
                        filter: Some(expr),
                        projection: None,
                        sort: None,
                        limit: None,
                        skip: None,
                    }),
                    pipeline: None,
                })
            }
            _ => Err(anyhow!("MongoDB query must be an object or array")),
        }
    }
}

impl ToFilter for MongoDBExpression {
    fn to_filter(&self) -> Result<DocumentFilter> {
        self.to_document_filter()
    }
}
