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
//! Per-request query plumbing:
//! v1 stores the query text in an `Arc<RwLock<Arc<str>>>` interior
//! field with an `update_query_text` setter. The request handler
//! updates it before driving the second-phase scorer. This works
//! correctly for single-request-at-a-time deployments but races on
//! concurrent requests sharing the same extractor instance. The
//! proper fix (widen `SecondPhaseScorer::rescore` to take a
//! `QueryContext`) is **R-5b.1.3** — explicitly deferred so this slice
//! ships smaller. Deployments that need concurrent reranking should
//! either (a) instantiate a fresh extractor per request, or (b)
//! serialize requests at the rank route until R-5b.1.3 lands.
//!
//! Spec: roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md §4.4.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use proximadb_rank_core::{DocHandle, RankError, RankResult};

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
/// - `query_text`: initial query text. Update via `update_query_text`
///   per request.
/// - `max_seq_len`: hard cap on tokenized sequence length. Padding +
///   truncation happen at this boundary so the resulting batch is
///   rectangular (a hard requirement for the ONNX session — the
///   tensor shape must be known at run time).
/// - `emit_token_type_ids`: when true, produce `token_type_ids`
///   (segment ids) — required by BERT-base/MiniLM-L-12-v2 style
///   models; some MiniLM-derived models don't take them.
pub struct BertPairTokenizingDocFeatureExtractor {
    tokenizer: Arc<tokenizers::Tokenizer>,
    doc_text_source: Arc<dyn DocTextSource>,
    query_text: Arc<RwLock<Arc<str>>>,
    max_seq_len: usize,
    emit_token_type_ids: bool,
}

impl BertPairTokenizingDocFeatureExtractor {
    /// Construct with an initial query text. Update per request via
    /// `update_query_text`.
    pub fn new(
        tokenizer: Arc<tokenizers::Tokenizer>,
        doc_text_source: Arc<dyn DocTextSource>,
        query_text: impl Into<Arc<str>>,
        max_seq_len: usize,
        emit_token_type_ids: bool,
    ) -> Self {
        Self {
            tokenizer,
            doc_text_source,
            query_text: Arc::new(RwLock::new(query_text.into())),
            max_seq_len: max_seq_len.max(1),
            emit_token_type_ids,
        }
    }

    /// Replace the query text used on the next `extract_batch` call.
    /// Concurrent callers race — the v1 deployment contract is one
    /// rerank at a time per extractor. R-5b.1.3 makes this safe.
    pub fn update_query_text(&self, query_text: impl Into<Arc<str>>) -> RankResult<()> {
        let mut guard = self
            .query_text
            .write()
            .map_err(|e| RankError::ModelInference {
                model_id: "bert_pair_extractor".into(),
                reason: format!("query_text RwLock poisoned: {e}"),
            })?;
        *guard = query_text.into();
        Ok(())
    }

    /// Read the current query text. Exposed for tests + debug logging.
    pub fn query_text(&self) -> RankResult<Arc<str>> {
        Ok(self
            .query_text
            .read()
            .map_err(|e| RankError::ModelInference {
                model_id: "bert_pair_extractor".into(),
                reason: format!("query_text RwLock poisoned: {e}"),
            })?
            .clone())
    }
}

impl TokenizedDocFeatureExtractor for BertPairTokenizingDocFeatureExtractor {
    fn extract_batch(&self, docs: &[DocHandle]) -> RankResult<TokenizedBatch> {
        if docs.is_empty() {
            return Ok(TokenizedBatch::default());
        }
        let query: Arc<str> = self.query_text()?;
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

    fn extractor_with(
        query: &str,
        max_seq_len: usize,
        emit_token_type_ids: bool,
        doc_texts: &[(u32, &str)],
    ) -> BertPairTokenizingDocFeatureExtractor {
        BertPairTokenizingDocFeatureExtractor::new(
            synthetic_tokenizer(),
            doc_text_source(doc_texts),
            query.to_string(),
            max_seq_len,
            emit_token_type_ids,
        )
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
        let e = extractor_with("query", 16, false, &[(1, "doc one")]);
        let b = e.extract_batch(&[]).unwrap();
        assert_eq!(b.batch_size(), 0);
        assert_eq!(b.seq_len(), 0);
    }

    #[test]
    fn extractor_produces_rectangular_batch_at_max_seq_len() {
        let e = extractor_with("alpha beta", 6, false, &[
            (1, "doc one"),
            (2, "doc two"),
            (3, "doc three with extra tokens to truncate"),
        ]);
        let b = e
            .extract_batch(&[DocHandle(1), DocHandle(2), DocHandle(3)])
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
        let e = extractor_with("query", 8, true, &[(1, "doc")]);
        let b = e.extract_batch(&[DocHandle(1)]).unwrap();
        assert!(b.token_type_ids.is_some());
        let tti = b.token_type_ids.unwrap();
        assert_eq!(tti.len(), 1);
        assert_eq!(tti[0].len(), 8);
    }

    #[test]
    fn extractor_omits_token_type_ids_by_default() {
        let e = extractor_with("query", 8, false, &[(1, "doc")]);
        let b = e.extract_batch(&[DocHandle(1)]).unwrap();
        assert!(b.token_type_ids.is_none());
    }

    #[test]
    fn extractor_handles_missing_doc_text_as_empty_string() {
        // DocHandle(99) has no text in the source — should be treated
        // as the empty string and still produce a row in the batch
        // (the model scores it; the score reflects an empty document).
        let e = extractor_with("query", 8, false, &[(1, "the real doc")]);
        let b = e
            .extract_batch(&[DocHandle(1), DocHandle(99)])
            .unwrap();
        assert_eq!(b.batch_size(), 2);
        // Both rows have full max_seq_len width.
        assert_eq!(b.input_ids[1].len(), 8);
    }

    #[test]
    fn extractor_uses_current_query_text() {
        let e = extractor_with("initial", 8, false, &[(1, "doc")]);
        assert_eq!(e.query_text().unwrap().as_ref(), "initial");
        e.update_query_text("updated").unwrap();
        assert_eq!(e.query_text().unwrap().as_ref(), "updated");
        // Sanity: extracting after update doesn't error.
        let _ = e.extract_batch(&[DocHandle(1)]).unwrap();
    }

    #[test]
    fn extractor_pads_to_at_least_seq_len_one() {
        // Defensive: passing max_seq_len = 0 at construction clamps to 1
        // so the resulting batch is never zero-width (zero-width
        // tensors confuse downstream ort inference).
        let e = BertPairTokenizingDocFeatureExtractor::new(
            synthetic_tokenizer(),
            doc_text_source(&[(1, "doc")]),
            "query".to_string(),
            0,
            false,
        );
        let b = e.extract_batch(&[DocHandle(1)]).unwrap();
        assert!(b.seq_len() >= 1);
    }

    #[test]
    fn extractor_doc_text_lookup_errors_propagate() {
        // A doc_text_source that always errors must short-circuit the
        // extract_batch call rather than silently dropping rows.
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
            "query".to_string(),
            4,
            false,
        );
        match e.extract_batch(&[DocHandle(1)]) {
            Err(RankError::ModelInference { reason, .. }) => {
                assert!(reason.contains("attribute store unavailable"));
            }
            other => panic!("expected ModelInference, got {other:?}"),
        }
    }

    #[test]
    fn extractor_is_dyn_compatible_as_doc_feature_extractor() {
        // Compile-time check: the production type must satisfy the
        // public trait surface so the second-phase scorer can hold it
        // as `Arc<dyn TokenizedDocFeatureExtractor>`.
        let e = extractor_with("q", 4, false, &[(1, "doc")]);
        let dyn_e: Arc<dyn TokenizedDocFeatureExtractor> = Arc::new(e);
        let b = dyn_e.extract_batch(&[DocHandle(1)]).unwrap();
        assert_eq!(b.batch_size(), 1);
    }
}
