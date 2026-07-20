// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Neutral usage-event emission (ADR-067 / TD-SANDHI-1 embeddings; TD-SANDHI-2 generation LLM).
//!
//! Serializes to the AnvaiOps/`sandhi` `usage-event.v1` schema — the neutral, cross-repo
//! boundary object emitted once per external model call. **Measured units only: NO dollars,
//! NO tier/SKU names** (OSS emits units, AnvaiOps prices — ADR-067 / ADR-060).
//!
//! Transport is the ADR-067 "in-process emit": a direct crate call that logs the event through
//! `tracing` (operators ship logs to their sink of record). It **complements** the KEU meter
//! (`consumption_metrics`), never replaces it (AnvaiOps ADR-0020 D6), and is **default-inert** —
//! nothing is emitted unless `PROXIMADB_EMIT_USAGE_EVENTS` is set truthy. Local BGE ONNX has no
//! external egress and does not emit (it keeps its compute-based KEU accrual).
//!
//! Dependency direction is one-way (ProximaDB → `sandhi`, ADR-060): the event *struct* mirrors the
//! wire schema locally (embedding path, TD-SANDHI-1), and the generation path additionally adopts
//! `sandhi-core`'s single-sourced, fixture-proven usage *parsers* (TD-SANDHI-2 / ADR-0047 D10a) —
//! rather than hand-rolling a per-provider extractor.

use serde::Serialize;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// The `tracing` target external tooling filters on to collect neutral usage events.
pub const USAGE_EVENT_TARGET: &str = "proximadb::usage_event";

/// Env flag gating emission. Unset/false ⇒ fully inert (no allocation, no log).
const EMIT_FLAG_ENV: &str = "PROXIMADB_EMIT_USAGE_EVENTS";

/// Backend classification (schema `backend` enum).
pub const BACKEND_EXTERNAL: &str = "external";
/// Self-hosted backend (local inference — Ollama / vLLM). Tokens are display-only there; the
/// cost basis is GPU-hours (AnvaiOps ADR-0020 D4), which ProximaDB does not measure here.
pub const BACKEND_SELF_HOSTED: &str = "self_hosted";

fn emission_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var(EMIT_FLAG_ENV)
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    })
}

fn next_request_id(kind: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let millis = chrono::Utc::now().timestamp_millis();
    format!("keu-{kind}-{millis}-{n}")
}

/// The neutral usage event — `sandhi` `usage-event.v1`. `additionalProperties:false`, so this
/// struct carries exactly the schema's fields. Optional attribution fields are omitted when
/// `None` (a valid encoding of the nullable schema fields).
#[derive(Debug, Clone, Serialize)]
pub struct UsageEvent {
    pub schema_version: &'static str,
    pub request_id: String,
    /// RFC 3339, set when the usage was finalized.
    pub occurred_at: String,
    /// Neutral provider slug (`azure_openai` / `openai` / `cohere` / `byo`). Never a tier/SKU.
    pub provider: String,
    /// Model id as sent to the provider.
    pub model: String,
    /// `external` for provider APIs (billed in tokens).
    pub backend: &'static str,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub virtual_key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
    /// The team/tenant the call is attributed to. ProximaDB maps its `tenant_id` here (the drainer
    /// knows the tenant/account, not the individual user).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,

    /// Fresh (non-cached) input tokens — the provider's real usage count.
    pub tokens_in: u64,
    /// Completion tokens. Always 0 for embeddings (they emit vectors, not completion tokens).
    pub tokens_out: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_seconds: Option<f64>,
}

impl UsageEvent {
    /// Build the event for one external **embedding** call, metered from the provider's real
    /// input-token count. `route` is the operation label (e.g. `"embed_batch"`).
    pub fn external_embedding(
        provider: impl Into<String>,
        model: impl Into<String>,
        tenant_id: Option<&str>,
        tokens_in: u64,
        route: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: "1",
            request_id: next_request_id("emb"),
            occurred_at: chrono::Utc::now().to_rfc3339(),
            provider: provider.into(),
            model: model.into(),
            backend: BACKEND_EXTERNAL,
            virtual_key_id: None,
            subject_id: None,
            group_id: tenant_id.map(str::to_string),
            route: Some(route.into()),
            session_id: None,
            tokens_in,
            tokens_out: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            gpu_seconds: None,
        }
    }

    /// Build the event for one **generation-LLM** call, with the full token breakdown incl. the
    /// prompt-cache split (TD-SANDHI-2 / ADR-0047 D10a). `backend` is `BACKEND_EXTERNAL` for
    /// provider APIs or `BACKEND_SELF_HOSTED` for local inference; `route` is the operation label.
    #[allow(clippy::too_many_arguments)]
    pub fn generation_llm(
        provider: impl Into<String>,
        model: impl Into<String>,
        tenant_id: Option<&str>,
        tokens_in: u64,
        tokens_out: u64,
        cache_creation_tokens: u64,
        cache_read_tokens: u64,
        route: impl Into<String>,
        backend: &'static str,
    ) -> Self {
        Self {
            schema_version: "1",
            request_id: next_request_id("gen"),
            occurred_at: chrono::Utc::now().to_rfc3339(),
            provider: provider.into(),
            model: model.into(),
            backend,
            virtual_key_id: None,
            subject_id: None,
            group_id: tenant_id.map(str::to_string),
            route: Some(route.into()),
            session_id: None,
            tokens_in,
            tokens_out,
            cache_creation_tokens,
            cache_read_tokens,
            gpu_seconds: None,
        }
    }

    /// Emit the event through `tracing` — **best-effort, off the hot path, default-inert**. No-op
    /// unless `PROXIMADB_EMIT_USAGE_EVENTS` is truthy. Never fails the caller.
    pub fn emit(&self) {
        if !emission_enabled() {
            return;
        }
        match serde_json::to_string(self) {
            Ok(json) => tracing::info!(target: USAGE_EVENT_TARGET, usage_event = %json),
            Err(e) => tracing::debug!("usage-event serialize failed (ignored): {e}"),
        }
    }
}

/// Build (without emitting) the neutral usage event for one generation-LLM call, **adopting the
/// fixture-proven `sandhi-core` usage parser** for `provider` (single-sourced metering trust —
/// AnvaiOps ADR-0047 D10a / Sandhi ADR-0003). Returns `None` if the body doesn't parse or carries
/// no usage. Split out from [`emit_generation_usage`] so the parser-dispatch + backend mapping is
/// unit-testable without the env-gated emit.
fn build_generation_event(
    provider: &str,
    model: &str,
    tenant_id: Option<&str>,
    raw_response_body: &str,
    route: &str,
) -> Option<UsageEvent> {
    let value: serde_json::Value = serde_json::from_str(raw_response_body).ok()?;
    use sandhi_core::usage::{
        parse_anthropic_usage, parse_cohere_usage, parse_ollama_usage, parse_openai_usage,
    };
    // OpenAI-compatible providers (OpenAI / Azure / vLLM / HuggingFace TGI) share the usage shape;
    // Anthropic / Cohere / Ollama have their own. All extractors are single-sourced in sandhi-core
    // and W1-fixture-proven (Sandhi TD-0001).
    let usage = match provider {
        "anthropic" => parse_anthropic_usage(&value),
        "cohere" => parse_cohere_usage(&value),
        "ollama" => parse_ollama_usage(&value),
        _ => parse_openai_usage(&value),
    }?;
    let backend = match provider {
        "ollama" | "vllm" => BACKEND_SELF_HOSTED,
        _ => BACKEND_EXTERNAL,
    };
    Some(UsageEvent::generation_llm(
        provider,
        model,
        tenant_id,
        usage.tokens_in,
        usage.tokens_out,
        usage.cache_creation_tokens,
        usage.cache_read_tokens,
        route,
        backend,
    ))
}

/// Emit the neutral usage event for one **generation-LLM** call. `provider` is the neutral slug
/// (`anthropic` / `openai` / `azure_openai` / `cohere` / `ollama` / `vllm` / `huggingface`).
/// **Best-effort + default-inert**: no parse work and no emit unless `PROXIMADB_EMIT_USAGE_EVENTS`
/// is set; never raises into the caller (the provider hot path).
pub fn emit_generation_usage(
    provider: &str,
    model: &str,
    tenant_id: Option<&str>,
    raw_response_body: &str,
    route: &str,
) {
    if !emission_enabled() {
        return;
    }
    if let Some(ev) = build_generation_event(provider, model, tenant_id, raw_response_body, route) {
        ev.emit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_to_neutral_schema_shape() {
        let ev = UsageEvent::external_embedding(
            "azure_openai",
            "text-embedding-3-small",
            Some("tenant-abc"),
            1234,
            "embed_batch",
        );
        let v: serde_json::Value = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["schema_version"], "1");
        assert_eq!(v["backend"], "external");
        assert_eq!(v["provider"], "azure_openai");
        assert_eq!(v["tokens_in"], 1234);
        assert_eq!(v["tokens_out"], 0);
        assert_eq!(v["group_id"], "tenant-abc");
        assert_eq!(v["route"], "embed_batch");
        // Neutral contract: units only, never dollars/tiers.
        assert!(v.get("cost").is_none() && v.get("usd").is_none() && v.get("tier").is_none());
        // Omitted optional attribution is absent (a valid nullable encoding).
        assert!(v.get("subject_id").is_none());
        assert!(v["request_id"].as_str().unwrap().starts_with("keu-emb-"));
    }

    #[test]
    fn generation_event_anthropic_carries_cache_split() {
        // Adopts sandhi-core::parse_anthropic_usage — the cache split the provider DTO dropped.
        let body = serde_json::json!({
            "model": "claude-3-5-sonnet",
            "content": [{ "type": "text", "text": "hi" }],
            "usage": {
                "input_tokens": 100, "output_tokens": 20,
                "cache_creation_input_tokens": 30, "cache_read_input_tokens": 40
            }
        })
        .to_string();
        let ev = build_generation_event(
            "anthropic",
            "claude-3-5-sonnet",
            Some("tenant-x"),
            &body,
            "query",
        )
        .unwrap();
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["provider"], "anthropic");
        assert_eq!(v["backend"], "external");
        assert_eq!(v["tokens_in"], 100);
        assert_eq!(v["tokens_out"], 20);
        assert_eq!(v["cache_creation_tokens"], 30);
        assert_eq!(v["cache_read_tokens"], 40);
        assert_eq!(v["group_id"], "tenant-x");
        assert!(v["request_id"].as_str().unwrap().starts_with("keu-gen-"));
    }

    #[test]
    fn generation_event_openai_family_and_selfhosted_backend() {
        let body = serde_json::json!({ "usage": { "prompt_tokens": 50, "completion_tokens": 10 } })
            .to_string();
        // OpenAI-compat family (default arm) → external.
        let ext = build_generation_event("azure_openai", "gpt-4o", None, &body, "query").unwrap();
        assert_eq!(serde_json::to_value(&ext).unwrap()["backend"], "external");
        // vLLM parses via the same OpenAI shape but classifies as self_hosted.
        let sh = build_generation_event("vllm", "llama", None, &body, "query").unwrap();
        let v = serde_json::to_value(&sh).unwrap();
        assert_eq!(v["backend"], "self_hosted");
        assert_eq!(v["tokens_in"], 50);
        assert_eq!(v["cache_creation_tokens"], 0);
    }

    #[test]
    fn generation_event_none_on_missing_usage_or_bad_json() {
        assert!(
            build_generation_event("openai", "m", None, r#"{"choices":[]}"#, "query").is_none()
        );
        assert!(build_generation_event("openai", "m", None, "not json", "query").is_none());
    }

    #[test]
    fn emit_is_inert_without_flag() {
        // Just assert it never panics when the flag is unset (default posture).
        UsageEvent::external_embedding("openai", "text-embedding-3-large", None, 10, "embed_batch")
            .emit();
    }
}
