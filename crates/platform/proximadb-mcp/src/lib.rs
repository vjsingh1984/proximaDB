// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! # ProximaDB reference MCP surface (the agent-facing catalog reference-MCP design)
//!
//! A thin, OSS, **unprivileged** Model Context Protocol projection of surfaces the
//! engine already exposes: catalog `list_collections`/`describe`, the statistics
//! `stats` envelope, the ADR-004 `explain`, REST v2 `search`, and the ADR-022
//! `memory`/`checkpoint`/`event` agent-state services. It is **adoption fuel and
//! the conformance target** for the boundary object — deliberately stopping at the
//! open-core line: no rate card, account entitlement, data redaction, or
//! governance gating live here (those are AnvaiOps's governed gateway, ADR-0021,
//! which *composes* this surface). The unprivileged invariant is enforced by the
//! catalog guard test below.
//!
//! This crate is the **protocol + catalog + dispatch** layer over a
//! [`ReferenceBackend`] trait; an adapter wires the trait to the engine's
//! REST/gRPC surfaces (and an stdio loop drives [`handle_request`]). Keeping the
//! engine out of this crate makes the surface independently testable.

pub mod protocol;
pub mod tools;

pub use protocol::{JsonRpcRequest, JsonRpcResponse, Tool};
pub use tools::{BackendError, ReferenceBackend, dispatch_tool, reference_tools, tool_name};

use protocol::{ToolCallParams, error_code, tool_result_content};
use serde_json::{Value, json};

/// The MCP protocol version this reference surface implements.
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

fn server_info() -> Value {
    json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "proximadb-reference-mcp",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

/// Handle one JSON-RPC request against the backend. Returns `None` for
/// notifications (requests without an `id`), which expect no response.
pub async fn handle_request(
    backend: &dyn ReferenceBackend,
    req: JsonRpcRequest,
) -> Option<JsonRpcResponse> {
    let id = req.id.clone()?; // notification → no response

    let response = match req.method.as_str() {
        "initialize" => JsonRpcResponse::ok(id, server_info()),
        "tools/list" => JsonRpcResponse::ok(id, json!({ "tools": reference_tools() })),
        "tools/call" => match serde_json::from_value::<ToolCallParams>(req.params) {
            Ok(params) => match dispatch_tool(backend, &params.name, &params.arguments).await {
                Ok(payload) => JsonRpcResponse::ok(id, tool_result_content(&payload)),
                Err(e) => JsonRpcResponse::err(id, e.code, e.message),
            },
            Err(e) => JsonRpcResponse::err(
                id,
                error_code::INVALID_PARAMS,
                format!("invalid tools/call params: {e}"),
            ),
        },
        other => JsonRpcResponse::err(
            id,
            error_code::METHOD_NOT_FOUND,
            format!("unknown method: {other}"),
        ),
    };
    Some(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    /// A canned backend that echoes which tool was called.
    struct MockBackend;

    #[async_trait]
    impl ReferenceBackend for MockBackend {
        async fn list_collections(&self, _: &Value) -> Result<Value, BackendError> {
            Ok(json!({ "collections": ["a", "b"] }))
        }
        async fn describe(&self, args: &Value) -> Result<Value, BackendError> {
            Ok(json!({ "describe": args }))
        }
        async fn stats(&self, args: &Value) -> Result<Value, BackendError> {
            Ok(json!({ "envelope_version": "1.0.0", "for": args }))
        }
        async fn explain(&self, _: &Value) -> Result<Value, BackendError> {
            Ok(json!({ "plan": "explain" }))
        }
        async fn search(&self, _: &Value) -> Result<Value, BackendError> {
            Ok(json!({ "hits": [] }))
        }
        async fn memory(&self, _: &Value) -> Result<Value, BackendError> {
            Ok(json!({ "memory": "ok" }))
        }
        async fn checkpoint(&self, _: &Value) -> Result<Value, BackendError> {
            Ok(json!({ "checkpoint": "ok" }))
        }
        async fn event(&self, _: &Value) -> Result<Value, BackendError> {
            Ok(json!({ "event": "ok" }))
        }
    }

    fn req(id: Value, method: &str, params: Value) -> JsonRpcRequest {
        serde_json::from_value(json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        }))
        .expect("valid request")
    }

    #[tokio::test]
    async fn initialize_returns_server_info() {
        let resp = handle_request(&MockBackend, req(json!(1), "initialize", json!({})))
            .await
            .expect("response");
        let result = resp.result.expect("result");
        assert_eq!(result["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_eq!(result["serverInfo"]["name"], "proximadb-reference-mcp");
    }

    #[tokio::test]
    async fn tools_list_returns_the_eight_reference_tools() {
        let resp = handle_request(&MockBackend, req(json!(2), "tools/list", json!({})))
            .await
            .expect("response");
        let tools = resp.result.expect("result")["tools"].clone();
        let names: Vec<String> = tools
            .as_array()
            .expect("array")
            .iter()
            .map(|t| t["name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(names.len(), 8);
        for expected in [
            "list_collections",
            "describe",
            "stats",
            "explain",
            "search",
            "memory",
            "checkpoint",
            "event",
        ] {
            assert!(
                names.contains(&expected.to_string()),
                "missing tool {expected}"
            );
        }
    }

    #[tokio::test]
    async fn tools_call_dispatches_and_wraps_content() {
        let params = json!({ "name": "stats", "arguments": { "collection_id": "incidents" } });
        let resp = handle_request(&MockBackend, req(json!(3), "tools/call", params))
            .await
            .expect("response");
        let result = resp.result.expect("result");
        // MCP content envelope with a text block carrying the JSON payload.
        let text = result["content"][0]["text"].as_str().expect("text block");
        assert!(text.contains("envelope_version"));
        assert_eq!(result["isError"], false);
    }

    #[tokio::test]
    async fn unknown_tool_is_method_not_found() {
        let params = json!({ "name": "delete_everything", "arguments": {} });
        let resp = handle_request(&MockBackend, req(json!(4), "tools/call", params))
            .await
            .expect("response");
        assert_eq!(
            resp.error.expect("error").code,
            error_code::METHOD_NOT_FOUND
        );
    }

    #[tokio::test]
    async fn unknown_method_is_method_not_found() {
        let resp = handle_request(&MockBackend, req(json!(5), "resources/list", json!({})))
            .await
            .expect("response");
        assert_eq!(
            resp.error.expect("error").code,
            error_code::METHOD_NOT_FOUND
        );
    }

    #[tokio::test]
    async fn notifications_get_no_response() {
        // No `id` → notification → no response.
        let notif = serde_json::from_value(json!({
            "jsonrpc": "2.0", "method": "notifications/initialized", "params": {}
        }))
        .expect("valid notification");
        assert!(handle_request(&MockBackend, notif).await.is_none());
    }

    /// the agent-facing catalog reference-MCP design / open-core guard: the reference surface stays
    /// unprivileged — no pricing, governance, entitlement, redaction, or
    /// per-account policy may leak into the tool catalog the server exposes. This
    /// checks the runtime catalog (names + descriptions), the actual surface,
    /// rather than explanatory source comments. (A complementary source-symbol CI
    /// grep — mirroring the ADR-030 billing-never-gated guard — is the production
    /// hook to add.)
    #[test]
    fn unprivileged_catalog_carries_no_pricing_or_governance() {
        const FORBIDDEN: &[&str] = &[
            "price",
            "pricing",
            "bill",
            "invoice",
            "rate card",
            "rate_card",
            "entitle",
            "governance",
            "redact",
            "pii",
            "per-account",
            "per account",
            "$",
        ];
        for tool in reference_tools() {
            let haystack = format!("{} {}", tool.name, tool.description).to_lowercase();
            for bad in FORBIDDEN {
                assert!(
                    !haystack.contains(bad),
                    "reference tool '{}' leaks forbidden term '{bad}' (open-core boundary)",
                    tool.name
                );
            }
        }
    }
}
