//! `BertPairTokenizingDocFeatureExtractor` — production
//! `TokenizedDocFeatureExtractor` for BERT-family cross-encoders.
//! R-5b.1.2.
//!
//! Pulls `(query_text, doc_text)` pairs and produces a
//! [`TokenizedBatch`] via `tokenizers::Tokenizer::encode_batch`.
//! Closes the last gap between R-5b.1.1's pipeline wiring and a real
//! BERT cross-encoder running end-to-end through REST / gRPC / Arrow
//! Flight.
//!
//! Feature-gated behind `bert-tokenizer` so the default
//! `proximadb-rank-onnx` build stays light. Deployments that opt into
//! cross-encoder reranking rebuild with `--features bert-tokenizer`
//! (typically also `real-onnx`).
//!
//! Per-request query plumbing (R-5b.1.3):
//! Query text flows in via the `QueryContext` parameter on
//! [`TokenizedDocFeatureExtractor::extract_batch`]. The extractor is
//! stateless — one shared `Arc<…>` instance handles concurrent
//! requests, each carrying its own `qctx.query_text`. The earlier
//! `Arc<RwLock<Arc<str>>>` interior-mutability hack from R-5b.1.2 is
//! gone; the one-rerank-at-a-time deployment caveat it carried is
//! resolved.
//!
//! Spec: roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md §4.4.

use std::collections::HashMap;
use std::sync::Arc;

use proximadb_rank_core::{DocHandle, QueryContext, RankError, RankResult};

use crate::tokenized_doc_feature_extractor::TokenizedDocFeatureExtractor;
use crate::tokenized_scorer_session::TokenizedBatch;

// ---------------------------------------------------------------------------
// DocTextSource — pluggable interface for fetching doc text by handle
// ---------------------------------------------------------------------------

/// Resolve a `DocHandle` to its source text. The text source is the
/// boundary between the rank framework and the attribute / storage
/// layer that owns the actual doc payload. Production wiring will
/// plug an attribute-store reader; tests use the in-memory HashMap
/// impl below.
///
/// Returns `None` when the doc has no text (e.g. a numeric-id-only
/// handle from a hybrid retrieval result the framework can't resolve);
/// the extractor treats `None` as the empty string so an unknown doc
/// scores under the same model rather than failing the second phase.
pub trait DocTextSource: Send + Sync {
    fn doc_text(&self, doc: DocHandle) -> RankResult<Option<String>>;
}

/// In-memory text source. The most common test fixture; also useful
/// as a small-cache wrapper for production paths that batch-load doc
/// text into a process-local map.
pub struct HashMapDocTextSource {
    texts: HashMap<DocHandle, String>,
}

impl HashMapDocTextSource {
    pub fn new() -> Self {
        Self {
            texts: HashMap::new(),
        }
    }

    pub fn with_entries<I, S>(entries: I) -> Self
    where
        I: IntoIterator<Item = (DocHandle, S)>,
        S: Into<String>,
    {
        let mut s = Self::new();
        for (d, t) in entries {
            s.insert(d, t);
        }
        s
    }

    pub fn insert(&mut self, doc: DocHandle, text: impl Into<String>) -> &mut Self {
        self.texts.insert(doc, text.into());
        self
    }

    pub fn len(&self) -> usize {
        self.texts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.texts.is_empty()
    }
}

impl Default for HashMapDocTextSource {
    fn default() -> Self {
        Self::new()
    }
}

impl DocTextSource for HashMapDocTextSource {
    fn doc_text(&self, doc: DocHandle) -> RankResult<Option<String>> {
        Ok(self.texts.get(&doc).cloned())
    }
}

// ---------------------------------------------------------------------------
// BertPairTokenizingDocFeatureExtractor
// ---------------------------------------------------------------------------

/// Tokenized doc-feature extractor that runs `tokenizer.encode_batch`
/// over `(query_text, doc_text)` pairs to produce a `TokenizedBatch`
/// suitable for a BERT cross-encoder.
///
/// Construction:
/// - `tokenizer`: shared `Arc<tokenizers::Tokenizer>` (same instance
///   embedding crate uses; one tokenizer file per deployment).
/// - `doc_text_source`: resolves doc text by handle.
/// - `max_seq_len`: hard cap on tokenized sequence length. Padding +
///   truncation happen at this boundary so the resulting batch is
///   rectangular (a hard requirement for the ONNX session — the
///   tensor shape must be known at run time).
/// - `emit_token_type_ids`: when true, produce `token_type_ids`
///   (segment ids) — required by BERT-base/MiniLM-L-12-v2 style
///   models; some MiniLM-derived models don't take them.
///
/// The extractor is stateless across requests — one shared instance
/// serves concurrent reranks. Per-request query text flows in via
/// `extract_batch`'s `QueryContext` argument (R-5b.1.3).
pub struct BertPairTokenizingDocFeatureExtractor {
    tokenizer: Arc<tokenizers::Tokenizer>,
    doc_text_source: Arc<dyn DocTextSource>,
    max_seq_len: usize,
    emit_token_type_ids: bool,
}

impl BertPairTokenizingDocFeatureExtractor {
    pub fn new(
        tokenizer: Arc<tokenizers::Tokenizer>,
        doc_text_source: Arc<dyn DocTextSource>,
        max_seq_len: usize,
        emit_token_type_ids: bool,
    ) -> Self {
        let max_seq_len = max_seq_len.max(1);
        // R-5b.1.4: clone the shared tokenizer and configure
        // `with_padding(Fixed, max_seq_len)` + `with_truncation(max_seq_len)`
        // so `encode_batch` returns Encodings already at the right
        // shape. Encoding rectangularity is now the tokenizer's
        // responsibility; `pad_or_truncate_to_i64` downstream still
        // runs as defense-in-depth (it's a few `Vec::push`es per row,
        // basically free) AND because we still need the u32 → i64
        // conversion the tokenizer doesn't do.
        //
        // We clone the inner Tokenizer rather than mutate the shared
        // Arc — the same `Arc<Tokenizer>` is used by the embedding
        // crate's `SharedTokenizer` for token-count chunking, which
        // wants per-call defaults. Per-extractor cloning isolates the
        // padding/truncation config to the rerank path.
        let configured = configure_tokenizer_for_pair_encoding(
            tokenizer.as_ref().clone(),
            max_seq_len,
        );
        Self {
            tokenizer: Arc::new(configured),
            doc_text_source,
            max_seq_len,
            emit_token_type_ids,
        }
    }
}

/// Apply Fixed-length padding + matching truncation to a cloned
/// tokenizer so `encode_batch` returns rectangular Encodings without
/// the caller having to pre-pad. Padding token id is `0` to match
/// the [`pad_or_truncate_to_i64`] fallback. Both directions truncate
/// from the end ("right" strategy) — matches BERT's documented
/// behaviour for long inputs.
fn configure_tokenizer_for_pair_encoding(
    mut tokenizer: tokenizers::Tokenizer,
    max_seq_len: usize,
) -> tokenizers::Tokenizer {
    use tokenizers::tokenizer::{PaddingDirection, PaddingParams, PaddingStrategy};
    use tokenizers::utils::truncation::{TruncationDirection, TruncationParams, TruncationStrategy};

    tokenizer.with_padding(Some(PaddingParams {
        strategy: PaddingStrategy::Fixed(max_seq_len),
        direction: PaddingDirection::Right,
        pad_to_multiple_of: None,
        pad_id: 0,
        pad_type_id: 0,
        pad_token: "[PAD]".into(),
    }));
    // with_truncation returns Result because the params can be
    // self-inconsistent (max_length < stride etc.). Our config is
    // simple enough that this never fails in practice; on the off
    // chance it does, fall back to the unconfigured tokenizer and
    // let pad_or_truncate_to_i64 do the work downstream.
    let _ = tokenizer.with_truncation(Some(TruncationParams {
        max_length: max_seq_len,
        strategy: TruncationStrategy::LongestFirst,
        stride: 0,
        direction: TruncationDirection::Right,
    }));
    tokenizer
}

impl TokenizedDocFeatureExtractor for BertPairTokenizingDocFeatureExtractor {
    fn extract_batch(
        &self,
        docs: &[DocHandle],
        qctx: &QueryContext,
    ) -> RankResult<TokenizedBatch> {
        if docs.is_empty() {
            return Ok(TokenizedBatch::default());
        }
        // R-5b.1.3: query text comes from the per-request QueryContext.
        // None → empty string; the model still scores the doc against
        // an empty query (the score reflects that mismatch). Callers
        // that want to short-circuit on missing query_text should
        // check qctx upstream.
        let query: Arc<str> = qctx
            .query_text
            .clone()
            .unwrap_or_else(|| Arc::<str>::from(""));
        // Resolve doc text for each handle. Missing docs (None) become
        // the empty string — the model still scores them, the score
        // just reflects an empty document.
        let mut pairs: Vec<(String, String)> = Vec::with_capacity(docs.len());
        for &doc in docs {
            let text = self
                .doc_text_source
                .doc_text(doc)?
                .unwrap_or_default();
            pairs.push((query.to_string(), text));
        }

        // Build the EncodeInput::Dual list. `tokenizers::encode_batch`
        // accepts owned strings via the `Into<EncodeInput>` impl on
        // `(String, String)`.
        let encodings = self
            .tokenizer
            .encode_batch(pairs, true)
            .map_err(|e| RankError::ModelInference {
                model_id: "bert_pair_extractor".into(),
                reason: format!("tokenizer.encode_batch: {e}"),
            })?;

        // Pad / truncate each encoding to `max_seq_len`. Building the
        // batch by hand here (rather than calling
        // tokenizer.with_padding before encoding) keeps padding
        // semantics local to the extractor — production deployments
        // can swap the strategy without re-priming the tokenizer.
        let mut input_ids: Vec<Vec<i64>> = Vec::with_capacity(encodings.len());
        let mut attention_mask: Vec<Vec<i64>> = Vec::with_capacity(encodings.len());
        let mut token_type_ids: Option<Vec<Vec<i64>>> = if self.emit_token_type_ids {
            Some(Vec::with_capacity(encodings.len()))
        } else {
            None
        };

        for enc in &encodings {
            input_ids.push(pad_or_truncate_to_i64(
                enc.get_ids(),
                self.max_seq_len,
                /*pad=*/ 0,
            ));
            attention_mask.push(pad_or_truncate_to_i64(
                enc.get_attention_mask(),
                self.max_seq_len,
                /*pad=*/ 0,
            ));
            if let Some(tti) = token_type_ids.as_mut() {
                tti.push(pad_or_truncate_to_i64(
                    enc.get_type_ids(),
                    self.max_seq_len,
                    /*pad=*/ 0,
                ));
            }
        }

        let batch = TokenizedBatch {
            input_ids,
            attention_mask,
            token_type_ids,
        };
        // Belt-and-braces: the pad/truncate loop guarantees
        // rectangular output, but verify it before returning so a
        // future refactor that breaks the invariant fails loud.
        debug_assert!(batch.validate_rectangular().is_ok());
        Ok(batch)
    }
}

/// Pad a `&[u32]` to exactly `target_len` elements as `i64`. Truncates
/// when input is too long. The extractor needs i64 because the BERT
/// ONNX export expects int64 inputs; `tokenizers` returns u32.
fn pad_or_truncate_to_i64(src: &[u32], target_len: usize, pad: i64) -> Vec<i64> {
    let mut out: Vec<i64> = Vec::with_capacity(target_len);
    let take = src.len().min(target_len);
    for &v in &src[..take] {
        out.push(v as i64);
    }
    while out.len() < target_len {
        out.push(pad);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal synthetic whitespace tokenizer for shape
    /// testing. WordLevel requires the unk_token to be present in the
    /// vocabulary (it's looked up on encode failure), so we seed a
    /// small vocab containing only `[UNK]` and the special pair
    /// tokens. Every other word in the test queries / docs is
    /// unknown and tokenizes to the [UNK] id — fine because these
    /// tests assert shapes, not vocab fidelity.
    fn synthetic_tokenizer() -> Arc<tokenizers::Tokenizer> {
        use std::collections::HashMap;
        use tokenizers::models::wordlevel::WordLevel;
        use tokenizers::pre_tokenizers::whitespace::Whitespace;
        use tokenizers::tokenizer::Tokenizer;
        let mut vocab: HashMap<String, u32> = HashMap::new();
        vocab.insert("[UNK]".to_string(), 0);
        vocab.insert("[CLS]".to_string(), 1);
        vocab.insert("[SEP]".to_string(), 2);
        vocab.insert("[PAD]".to_string(), 3);
        // Seed real vocab entries so the per-request-qctx test can
        // verify that different queries tokenize differently. Without
        // these, every test-input word maps to [UNK] and queries are
        // indistinguishable on the wire.
        for (i, w) in ["alpha", "beta", "gamma", "delta", "doc"]
            .iter()
            .enumerate()
        {
            vocab.insert((*w).to_string(), 10 + i as u32);
        }
        let model = WordLevel::builder()
            .unk_token("[UNK]".to_string())
            .vocab(vocab)
            .build()
            .expect("synthetic WordLevel build");
        let mut tk = Tokenizer::new(model);
        tk.with_pre_tokenizer(Some(Whitespace {}));
        Arc::new(tk)
    }

    fn doc_text_source(entries: &[(u32, &str)]) -> Arc<dyn DocTextSource> {
        Arc::new(HashMapDocTextSource::with_entries(
            entries.iter().map(|(d, t)| (DocHandle(*d), *t)),
        ))
    }

    /// Convenience: build (extractor, qctx) so tests stay readable
    /// despite query text now living on the per-request context.
    fn extractor_with(
        query: &str,
        max_seq_len: usize,
        emit_token_type_ids: bool,
        doc_texts: &[(u32, &str)],
    ) -> (BertPairTokenizingDocFeatureExtractor, QueryContext) {
        let e = BertPairTokenizingDocFeatureExtractor::new(
            synthetic_tokenizer(),
            doc_text_source(doc_texts),
            max_seq_len,
            emit_token_type_ids,
        );
        let qctx = QueryContext {
            query_text: Some(Arc::<str>::from(query)),
            ..QueryContext::default()
        };
        (e, qctx)
    }

    // ---------------- HashMapDocTextSource ----------------

    #[test]
    fn hashmap_source_returns_some_for_known_doc() {
        let s = HashMapDocTextSource::with_entries([(DocHandle(1), "hello world")]);
        assert_eq!(
            s.doc_text(DocHandle(1)).unwrap().as_deref(),
            Some("hello world")
        );
    }

    #[test]
    fn hashmap_source_returns_none_for_unknown_doc() {
        let s = HashMapDocTextSource::new();
        assert!(s.doc_text(DocHandle(99)).unwrap().is_none());
    }

    #[test]
    fn hashmap_source_insert_replaces_existing_value() {
        let mut s = HashMapDocTextSource::new();
        s.insert(DocHandle(1), "first");
        s.insert(DocHandle(1), "second");
        assert_eq!(
            s.doc_text(DocHandle(1)).unwrap().as_deref(),
            Some("second")
        );
        assert_eq!(s.len(), 1);
    }

    // ---------------- pad_or_truncate_to_i64 ----------------

    #[test]
    fn pad_or_truncate_pads_short_input_with_zeros() {
        let out = pad_or_truncate_to_i64(&[1, 2, 3], 6, 0);
        assert_eq!(out, vec![1, 2, 3, 0, 0, 0]);
    }

    #[test]
    fn pad_or_truncate_truncates_long_input() {
        let out = pad_or_truncate_to_i64(&[1, 2, 3, 4, 5, 6], 3, 0);
        assert_eq!(out, vec![1, 2, 3]);
    }

    #[test]
    fn pad_or_truncate_exact_length_is_pass_through() {
        let out = pad_or_truncate_to_i64(&[1, 2, 3], 3, 0);
        assert_eq!(out, vec![1, 2, 3]);
    }

    #[test]
    fn pad_or_truncate_empty_input_emits_all_padding() {
        let out = pad_or_truncate_to_i64(&[], 4, 7);
        assert_eq!(out, vec![7, 7, 7, 7]);
    }

    // ---------------- BertPairTokenizingDocFeatureExtractor ----------------

    #[test]
    fn extractor_empty_docs_returns_empty_batch_without_tokenizer_call() {
        let (e, qctx) = extractor_with("query", 16, false, &[(1, "doc one")]);
        let b = e.extract_batch(&[], &qctx).unwrap();
        assert_eq!(b.batch_size(), 0);
        assert_eq!(b.seq_len(), 0);
    }

    #[test]
    fn extractor_produces_rectangular_batch_at_max_seq_len() {
        let (e, qctx) = extractor_with("alpha beta", 6, false, &[
            (1, "doc one"),
            (2, "doc two"),
            (3, "doc three with extra tokens to truncate"),
        ]);
        let b = e
            .extract_batch(&[DocHandle(1), DocHandle(2), DocHandle(3)], &qctx)
            .unwrap();
        assert_eq!(b.batch_size(), 3);
        for row in &b.input_ids {
            assert_eq!(row.len(), 6, "row must be padded/truncated to max_seq_len");
        }
        for row in &b.attention_mask {
            assert_eq!(row.len(), 6);
        }
        assert!(b.validate_rectangular().is_ok());
    }

    #[test]
    fn extractor_emits_token_type_ids_when_configured() {
        let (e, qctx) = extractor_with("query", 8, true, &[(1, "doc")]);
        let b = e.extract_batch(&[DocHandle(1)], &qctx).unwrap();
        assert!(b.token_type_ids.is_some());
        let tti = b.token_type_ids.unwrap();
        assert_eq!(tti.len(), 1);
        assert_eq!(tti[0].len(), 8);
    }

    #[test]
    fn extractor_omits_token_type_ids_by_default() {
        let (e, qctx) = extractor_with("query", 8, false, &[(1, "doc")]);
        let b = e.extract_batch(&[DocHandle(1)], &qctx).unwrap();
        assert!(b.token_type_ids.is_none());
    }

    #[test]
    fn extractor_handles_missing_doc_text_as_empty_string() {
        let (e, qctx) = extractor_with("query", 8, false, &[(1, "the real doc")]);
        let b = e
            .extract_batch(&[DocHandle(1), DocHandle(99)], &qctx)
            .unwrap();
        assert_eq!(b.batch_size(), 2);
        assert_eq!(b.input_ids[1].len(), 8);
    }

    #[test]
    fn extractor_uses_per_request_query_text_from_qctx() {
        // R-5b.1.3: query text now flows from QueryContext, not from
        // extractor state. Two different qctx values yield two
        // different tokenizations even though the extractor instance
        // is the same — confirms the per-request plumbing.
        let (e, _) = extractor_with("ignored-construction-arg", 8, false, &[(1, "doc")]);
        let qctx_a = QueryContext {
            query_text: Some(Arc::<str>::from("alpha")),
            ..QueryContext::default()
        };
        let qctx_b = QueryContext {
            query_text: Some(Arc::<str>::from("beta gamma delta")),
            ..QueryContext::default()
        };
        let a = e.extract_batch(&[DocHandle(1)], &qctx_a).unwrap();
        let b = e.extract_batch(&[DocHandle(1)], &qctx_b).unwrap();
        // Same doc, same vocab → identical token ids when the query
        // also matches; here the queries differ so at least one
        // input_id position must differ between batches.
        assert_eq!(a.input_ids[0].len(), 8);
        assert_eq!(b.input_ids[0].len(), 8);
        assert_ne!(
            a.input_ids[0], b.input_ids[0],
            "different query_text must produce different tokens"
        );
    }

    #[test]
    fn extractor_treats_missing_query_text_as_empty_string() {
        // qctx.query_text == None → extractor uses "" rather than
        // erroring. The model still scores against an empty query;
        // the score reflects the mismatch.
        let (e, _) = extractor_with("ignored", 8, false, &[(1, "doc")]);
        let qctx = QueryContext::default(); // query_text is None
        let b = e.extract_batch(&[DocHandle(1)], &qctx).unwrap();
        assert_eq!(b.batch_size(), 1);
        assert_eq!(b.input_ids[0].len(), 8);
    }

    #[test]
    fn extractor_pads_to_at_least_seq_len_one() {
        // Defensive: passing max_seq_len = 0 at construction clamps to 1.
        let e = BertPairTokenizingDocFeatureExtractor::new(
            synthetic_tokenizer(),
            doc_text_source(&[(1, "doc")]),
            0,
            false,
        );
        let qctx = QueryContext {
            query_text: Some(Arc::<str>::from("query")),
            ..QueryContext::default()
        };
        let b = e.extract_batch(&[DocHandle(1)], &qctx).unwrap();
        assert!(b.seq_len() >= 1);
    }

    // ---------------- R-5b.1.4: tokenizer pre-padding/truncation ----------------

    #[test]
    fn extractor_constructor_does_not_mutate_shared_tokenizer_state() {
        // The shared Arc<Tokenizer> the caller hands in must NOT have
        // its padding/truncation config touched — other consumers (e.g.
        // the embedding crate's SharedTokenizer) rely on per-call
        // defaults. Verify by encoding the same input twice via the
        // shared Arc before and after constructing the extractor.
        let shared = synthetic_tokenizer();
        let pre = shared.encode("alpha beta", false).unwrap();
        let _e = BertPairTokenizingDocFeatureExtractor::new(
            shared.clone(),
            doc_text_source(&[(1, "doc")]),
            32,
            false,
        );
        let post = shared.encode("alpha beta", false).unwrap();
        assert_eq!(
            pre.get_ids(),
            post.get_ids(),
            "constructor must not mutate the caller's shared tokenizer"
        );
        assert_eq!(pre.get_ids().len(), post.get_ids().len());
    }

    #[test]
    fn extractor_tokenizer_outputs_are_rectangular_at_max_seq_len() {
        // R-5b.1.4: configured tokenizer pads to exactly max_seq_len.
        // The pad_or_truncate_to_i64 pass downstream becomes a no-op
        // for the length dimension (it still does u32 → i64
        // conversion). Verify rectangular output regardless of the
        // input doc lengths.
        let (e, qctx) = extractor_with(
            "alpha beta gamma",
            10,
            false,
            &[
                (1, "doc"),                              // shorter than max
                (2, "alpha beta gamma delta alpha beta"), // longer than max
                (3, ""),                                  // empty doc
            ],
        );
        let b = e
            .extract_batch(&[DocHandle(1), DocHandle(2), DocHandle(3)], &qctx)
            .unwrap();
        assert_eq!(b.batch_size(), 3);
        for row in &b.input_ids {
            assert_eq!(
                row.len(),
                10,
                "tokenizer padding/truncation must pre-shape every row to max_seq_len"
            );
        }
        for row in &b.attention_mask {
            assert_eq!(row.len(), 10);
        }
        assert!(b.validate_rectangular().is_ok());
    }

    #[test]
    fn extractor_attention_mask_marks_padding_positions_zero() {
        // R-5b.1.4: with Fixed padding configured, the attention_mask
        // distinguishes real tokens (1) from padding (0). A short
        // doc + short query → most positions should be padding (0).
        let (e, qctx) = extractor_with(
            "alpha",
            10,
            false,
            &[(1, "doc")],
        );
        let b = e.extract_batch(&[DocHandle(1)], &qctx).unwrap();
        let mask = &b.attention_mask[0];
        let real = mask.iter().filter(|&&x| x == 1).count();
        let pad = mask.iter().filter(|&&x| x == 0).count();
        assert!(real >= 1, "must include at least 1 real token");
        assert!(pad >= 1, "must include at least 1 padding position");
        assert_eq!(real + pad, 10);
    }

    #[test]
    fn extractor_doc_text_lookup_errors_propagate() {
        struct BrokenSource;
        impl DocTextSource for BrokenSource {
            fn doc_text(&self, _: DocHandle) -> RankResult<Option<String>> {
                Err(RankError::ModelInference {
                    model_id: "broken_source".into(),
                    reason: "attribute store unavailable".into(),
                })
            }
        }
        let e = BertPairTokenizingDocFeatureExtractor::new(
            synthetic_tokenizer(),
            Arc::new(BrokenSource),
            4,
            false,
        );
        let qctx = QueryContext {
            query_text: Some(Arc::<str>::from("query")),
            ..QueryContext::default()
        };
        match e.extract_batch(&[DocHandle(1)], &qctx) {
            Err(RankError::ModelInference { reason, .. }) => {
                assert!(reason.contains("attribute store unavailable"));
            }
            other => panic!("expected ModelInference, got {other:?}"),
        }
    }

    #[test]
    fn extractor_is_dyn_compatible_as_doc_feature_extractor() {
        let (e, qctx) = extractor_with("q", 4, false, &[(1, "doc")]);
        let dyn_e: Arc<dyn TokenizedDocFeatureExtractor> = Arc::new(e);
        let b = dyn_e.extract_batch(&[DocHandle(1)], &qctx).unwrap();
        assert_eq!(b.batch_size(), 1);
    }
}
