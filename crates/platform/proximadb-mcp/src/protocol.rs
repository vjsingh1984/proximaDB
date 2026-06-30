// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! JSON-RPC 2.0 + minimal MCP wire types for the reference surface.
//!
//! Deliberately small and dependency-light (serde + serde_json only): MCP is one
//! *projection* of the engine's affordances, not a new authority. If a different
//! agent protocol displaces it, only this layer is re-skinned; the surfaces it
//! projects (stats/describe/explain/search) do not move.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Standard JSON-RPC 2.0 error codes (plus MCP usage).
pub mod error_code {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
}

/// An incoming JSON-RPC 2.0 request (or notification when `id` is absent).
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl JsonRpcRequest {
    /// A request with no `id` is a notification — it expects no response.
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// A JSON-RPC 2.0 response (exactly one of `result`/`error` is set).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// An MCP tool descriptor (returned by `tools/list`).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Tool {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool's arguments.
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// Parameters of a `tools/call` request.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolCallParams {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

/// Wrap a tool's structured result as MCP tool-call content (a single text block
/// carrying the JSON payload — the conformant, transport-stable shape).
pub fn tool_result_content(payload: &Value) -> Value {
    serde_json::json!({
        "content": [
            { "type": "text", "text": payload.to_string() }
        ],
        "isError": false
    })
}
