//! Bring-your-own (BYO) embedding endpoint client. Enterprise tier feature.
//!
//! AnvaiOps customers can register their own HTTPS embedding endpoint in the
//! tenant registry. This client invokes it with the contract documented in
//! `docs/architecture/EMBEDDING.md` §5 (Anvaiops repo).

use crate::config::ByoAuth;
use crate::{EmbeddingError, Result};

pub struct ByoClient {
    url: String,
    auth: ByoAuth,
    batch_size: usize,
    timeout_ms: u64,
}

impl ByoClient {
    pub fn new(url: String, auth: ByoAuth, batch_size: usize, timeout_ms: u64) -> Self {
        let batch_size = batch_size.max(1);
        let timeout_ms = timeout_ms.clamp(500, 60_000);
        Self {
            url,
            auth,
            batch_size,
            timeout_ms,
        }
    }

    pub fn embed_batch(&self, texts: &[String], declared_dim: usize) -> Result<Vec<Vec<f32>>> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_millis(self.timeout_ms))
            .build()
            .map_err(|e| EmbeddingError::Other(anyhow::anyhow!(e)))?;

        let mut out: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(self.batch_size) {
            let body = serde_json::json!({
                "texts": chunk,
                "model_hint": "anvaiops-byo",
            });
            let mut req = client.post(&self.url).json(&body);
            req = match &self.auth {
                ByoAuth::Bearer { secret_ref } => req.bearer_auth(secret_ref),
                ByoAuth::Mtls { .. } => req, // mTLS handled at the transport layer
                ByoAuth::None => req,
            };
            let resp = req
                .send()
                .map_err(|e| EmbeddingError::ByoEndpoint(e.to_string()))?;
            if !resp.status().is_success() {
                return Err(EmbeddingError::ByoEndpoint(format!(
                    "BYO endpoint returned {}",
                    resp.status()
                )));
            }
            let body: ByoResponse = resp
                .json()
                .map_err(|e| EmbeddingError::ByoEndpoint(e.to_string()))?;
            for v in body.embeddings {
                if v.len() != declared_dim {
                    return Err(EmbeddingError::DimMismatch {
                        expected: declared_dim,
                        actual: v.len(),
                    });
                }
                out.push(v);
            }
        }
        Ok(out)
    }
}

#[derive(serde::Deserialize)]
struct ByoResponse {
    embeddings: Vec<Vec<f32>>,
    #[allow(dead_code)]
    #[serde(default)]
    model_version: Option<String>,
}
