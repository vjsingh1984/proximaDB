//! Span / answer-quality benchmark (TD-SELECTOR-1, gate 4).
//!
//! Gate 2 asks "which selector keeps the most relevant documents?". This gate asks
//! the question that actually matters downstream: **does that difference reach the
//! answer?** It therefore consumes the *same* `SelectorResult` rankings the selector
//! harness produces, so the two gates compose rather than measuring different worlds.
//!
//! # What is measured
//!
//! For each question, an [`AnswerExtractor`] reads the ranking a selector emitted,
//! pulls a span out of the retrieved documents, and the span is scored against gold:
//!
//! - **Exact match** — SQuAD-style normalisation (lowercase, strip punctuation and
//!   articles, collapse whitespace) then string equality. Not "F1 == 1.0": a bag of
//!   the same tokens in the wrong order is not an exact match.
//! - **Token F1 / precision / recall** — over token *bags*, not sets, so a repeated
//!   token cannot be double-credited.
//! - **Span IoU and exact-boundary accuracy** — character offsets into the source
//!   document, for extractors that report them.
//!
//! Statistics reuse [`crate::bench_selector_benchmark::stats`]: percentile bootstrap
//! intervals and a paired sign-flip test across selectors on identical questions.
//!
//! # Honesty boundary
//!
//! The corpus is synthetic (`answer_quality_data`) and the extractors are lexical,
//! not model-driven. This measures **whether the retrieval-to-answer coupling is
//! real and how much of it survives each selector**, not ProximaDB's answer quality.
//! No number here belongs in `BENCHMARK_EVIDENCE.toml`.

#![allow(dead_code)]

use std::collections::HashMap;
use std::time::Instant;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::bench_selector_benchmark::{SelectorResult, stats};

/// Deterministic seed for the resampling statistics in this gate.
const STATS_SEED: u64 = 0x5E1E_C704_0000_0004;

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AnswerType {
    /// A single entity or fact.
    Factoid,
    /// Several items that must all appear.
    List,
    /// Yes / no.
    Boolean,
    /// Long-form prose.
    Descriptive,
    /// Requires joining two documents.
    MultiHop,
}

impl AnswerType {
    pub fn label(self) -> &'static str {
        match self {
            AnswerType::Factoid => "factoid",
            AnswerType::List => "list",
            AnswerType::Boolean => "boolean",
            AnswerType::Descriptive => "descriptive",
            AnswerType::MultiHop => "multi_hop",
        }
    }
}

/// A character span within one document.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextSpan {
    pub doc_id: String,
    pub start: usize,
    pub end: usize,
}

impl TextSpan {
    /// Intersection over union with another span. Spans in different documents
    /// never overlap, so they score 0.
    pub fn iou(&self, other: &TextSpan) -> f64 {
        if self.doc_id != other.doc_id {
            return 0.0;
        }
        let lo = self.start.max(other.start);
        let hi = self.end.min(other.end);
        let intersection = hi.saturating_sub(lo) as f64;
        let union = (self.end.max(other.end) - self.start.min(other.start)) as f64;
        if union == 0.0 {
            0.0
        } else {
            intersection / union
        }
    }
}

#[derive(Debug, Clone)]
pub struct GoldAnswer {
    pub text: String,
    /// Where the answer lives, when the gold data pins it down.
    pub span: Option<TextSpan>,
}

/// One question, its acceptable answers, and the document that contains them.
#[derive(Debug, Clone)]
pub struct QAPair {
    pub query_id: String,
    pub query_text: String,
    /// Any one of these counts as correct — the standard multi-reference protocol.
    pub answers: Vec<GoldAnswer>,
    pub answer_type: AnswerType,
    /// The document a correct extractor must reach. If the selector drops it, no
    /// extractor can recover — this is the coupling gate 4 exists to measure.
    pub gold_doc_id: String,
}

/// `doc_id -> document text`. Extractors read spans out of this.
pub type Corpus = HashMap<String, String>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExtractionMethod {
    /// The whole top-ranked document, verbatim.
    WholeDocument,
    /// The best-matching sentence inside the retrieved documents.
    SentenceSelection,
}

#[derive(Debug, Clone)]
pub struct ExtractedAnswer {
    pub answer_text: String,
    pub span: Option<TextSpan>,
    pub source_doc_ids: Vec<String>,
    pub method: ExtractionMethod,
}

// ---------------------------------------------------------------------------
// Extractors
// ---------------------------------------------------------------------------

/// Reads a selector's ranking and produces an answer span.
///
/// Implementations must consume the ranking *as given* — reordering it here would
/// hide the selector's effect, which is the whole point of this gate.
/// Named metric column, kept out of line so the extractor/comparison tables stay
/// readable.
type MetricFn<T> = (&'static str, fn(&T) -> f64);

pub trait AnswerExtractor {
    fn name(&self) -> &str;
    /// `read_depth` is how many ranked documents the extractor may look at.
    fn extract(
        &self,
        query: &str,
        ranking: &[SelectorResult],
        corpus: &Corpus,
    ) -> Option<ExtractedAnswer>;
}

/// Baseline: return the top-ranked document verbatim. Recall-perfect when the gold
/// document ranks first, hopeless at precision — the floor everything else beats.
pub struct WholeTopDocument;

impl AnswerExtractor for WholeTopDocument {
    fn name(&self) -> &str {
        "whole_top_document"
    }

    fn extract(
        &self,
        _query: &str,
        ranking: &[SelectorResult],
        corpus: &Corpus,
    ) -> Option<ExtractedAnswer> {
        let top = ranking.first()?;
        let text = corpus.get(&top.doc_id)?;
        Some(ExtractedAnswer {
            answer_text: text.clone(),
            span: Some(TextSpan {
                doc_id: top.doc_id.clone(),
                start: 0,
                end: text.len(),
            }),
            source_doc_ids: vec![top.doc_id.clone()],
            method: ExtractionMethod::WholeDocument,
        })
    }
}

/// Scans the top `read_depth` documents and returns the sentence with the highest
/// normalised token overlap with the query. Lexical, deterministic, no model — the
/// point is to hold extraction fixed so differences are attributable to the selector.
pub struct BestSentence {
    pub read_depth: usize,
}

impl BestSentence {
    pub fn new(read_depth: usize) -> Self {
        Self {
            read_depth: read_depth.max(1),
        }
    }
}

impl AnswerExtractor for BestSentence {
    fn name(&self) -> &str {
        "best_sentence"
    }

    fn extract(
        &self,
        query: &str,
        ranking: &[SelectorResult],
        corpus: &Corpus,
    ) -> Option<ExtractedAnswer> {
        let query_tokens = normalized_tokens(query);
        let mut best: Option<(f64, TextSpan, String)> = None;

        for result in ranking.iter().take(self.read_depth) {
            let Some(text) = corpus.get(&result.doc_id) else {
                continue;
            };
            for (start, end) in sentence_bounds(text) {
                let sentence = &text[start..end];
                let overlap = token_overlap(&query_tokens, &normalized_tokens(sentence));
                // Strict `>` keeps the earliest sentence on a tie, so the choice is
                // deterministic rather than dependent on scan order.
                if best.as_ref().is_none_or(|(score, _, _)| overlap > *score) {
                    best = Some((
                        overlap,
                        TextSpan {
                            doc_id: result.doc_id.clone(),
                            start,
                            end,
                        },
                        sentence.to_string(),
                    ));
                }
            }
        }

        let (_, span, sentence) = best?;
        Some(ExtractedAnswer {
            answer_text: sentence,
            source_doc_ids: vec![span.doc_id.clone()],
            span: Some(span),
            method: ExtractionMethod::SentenceSelection,
        })
    }
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// SQuAD-style normalisation: lowercase, drop punctuation, drop leading articles,
/// collapse whitespace. Applied to both sides before every comparison.
pub fn normalize_answer(text: &str) -> String {
    let lowered: String = text
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect();
    lowered
        .split_whitespace()
        .filter(|token| !matches!(*token, "a" | "an" | "the"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn normalized_tokens(text: &str) -> Vec<String> {
    normalize_answer(text)
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Bag-of-tokens precision / recall / F1.
///
/// Counted as multisets: a prediction repeating "paris" three times gets credit for
/// it once if gold contains it once. A set-based intersection (the previous
/// revision) silently inflated both sides.
pub fn token_prf(predicted: &[String], gold: &[String]) -> (f64, f64, f64) {
    if predicted.is_empty() && gold.is_empty() {
        return (1.0, 1.0, 1.0);
    }
    if predicted.is_empty() || gold.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let mut gold_counts: HashMap<&str, usize> = HashMap::new();
    for token in gold {
        *gold_counts.entry(token.as_str()).or_insert(0) += 1;
    }
    let mut overlap = 0usize;
    for token in predicted {
        if let Some(remaining) = gold_counts.get_mut(token.as_str())
            && *remaining > 0
        {
            *remaining -= 1;
            overlap += 1;
        }
    }
    if overlap == 0 {
        return (0.0, 0.0, 0.0);
    }
    let precision = overlap as f64 / predicted.len() as f64;
    let recall = overlap as f64 / gold.len() as f64;
    (
        precision,
        recall,
        2.0 * precision * recall / (precision + recall),
    )
}

fn token_overlap(query: &[String], sentence: &[String]) -> f64 {
    let (_, recall, _) = token_prf(sentence, query);
    recall
}

/// Split on `.`, `!`, `?` and return `(start, end)` byte offsets of each sentence,
/// trimmed of surrounding whitespace. Offsets index the original string so spans
/// stay comparable with gold.
pub fn sentence_bounds(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut bounds = Vec::new();
    let mut start = 0usize;
    for (i, byte) in bytes.iter().enumerate() {
        if matches!(byte, b'.' | b'!' | b'?') {
            push_trimmed(text, start, i + 1, &mut bounds);
            start = i + 1;
        }
    }
    if start < text.len() {
        push_trimmed(text, start, text.len(), &mut bounds);
    }
    bounds
}

fn push_trimmed(text: &str, start: usize, end: usize, out: &mut Vec<(usize, usize)>) {
    let slice = &text[start..end];
    let leading = slice.len() - slice.trim_start().len();
    let trailing = slice.len() - slice.trim_end().len();
    let (s, e) = (start + leading, end - trailing);
    if e > s {
        out.push((s, e));
    }
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

/// One (selector, extractor, question) measurement.
#[derive(Debug, Clone)]
pub struct AnswerMetrics {
    pub query_id: String,
    pub answer_type: AnswerType,
    pub exact_match: f64,
    pub f1: f64,
    pub precision: f64,
    pub recall: f64,
    /// 1.0 when the predicted span's boundaries equal gold's exactly.
    pub span_exact: f64,
    /// Intersection-over-union of the predicted and gold spans.
    pub span_iou: f64,
    /// 1.0 when the gold document appeared anywhere in the ranking the extractor read.
    pub gold_doc_retrieved: f64,
    pub latency_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateQuality {
    pub selector: String,
    pub extractor: String,
    pub num_questions: usize,
    pub mean_exact_match: f64,
    pub mean_f1: f64,
    pub mean_precision: f64,
    pub mean_recall: f64,
    pub mean_span_exact: f64,
    pub mean_span_iou: f64,
    /// The retrieval ceiling on answer quality: no extractor can beat this.
    pub mean_gold_doc_retrieved: f64,
    pub mean_latency_ms: f64,
    pub exact_match_ci: (f64, f64),
    pub f1_ci: (f64, f64),
    pub by_answer_type: HashMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityComparison {
    pub extractor: String,
    pub selector_a: String,
    pub selector_b: String,
    pub metric: String,
    pub mean_difference: f64,
    pub difference_ci: (f64, f64),
    pub p_value: f64,
    pub statistically_significant: bool,
    pub winner: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnswerQualityResults {
    /// Keyed `"{selector}/{extractor}"`.
    pub aggregates: HashMap<String, AggregateQuality>,
    pub comparisons: Vec<QualityComparison>,
}

impl AnswerQualityResults {
    pub fn get(&self, selector: &str, extractor: &str) -> Result<&AggregateQuality> {
        self.aggregates
            .get(&format!("{selector}/{extractor}"))
            .ok_or_else(|| anyhow!("no answer-quality aggregate for {selector}/{extractor}"))
    }

    pub fn comparison(
        &self,
        extractor: &str,
        a: &str,
        b: &str,
        metric: &str,
    ) -> Option<&QualityComparison> {
        self.comparisons.iter().find(|c| {
            c.extractor == extractor
                && c.metric == metric
                && ((c.selector_a == a && c.selector_b == b)
                    || (c.selector_a == b && c.selector_b == a))
        })
    }

    pub fn render(&self) -> String {
        let mut rows: Vec<&AggregateQuality> = self.aggregates.values().collect();
        rows.sort_by(|a, b| {
            b.mean_f1
                .partial_cmp(&a.mean_f1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut out = String::new();
        out.push_str(
            "selector/extractor                        EM      F1      [95% CI F1]        \
             span_IoU  gold_doc  ms\n",
        );
        for row in rows {
            out.push_str(&format!(
                "{:<40} {:.4}  {:.4}  [{:.4}, {:.4}]  {:.4}    {:.4}    {:.2}\n",
                format!("{}/{}", row.selector, row.extractor),
                row.mean_exact_match,
                row.mean_f1,
                row.f1_ci.0,
                row.f1_ci.1,
                row.mean_span_iou,
                row.mean_gold_doc_retrieved,
                row.mean_latency_ms
            ));
        }
        out
    }
}

/// Rankings produced by one selector, keyed by query id.
pub type SelectorRankings = HashMap<String, Vec<SelectorResult>>;

pub struct AnswerQualityEvaluator {
    extractors: Vec<Box<dyn AnswerExtractor>>,
    resamples: usize,
}

impl AnswerQualityEvaluator {
    pub fn new(extractors: Vec<Box<dyn AnswerExtractor>>) -> Self {
        Self {
            extractors,
            resamples: 1000,
        }
    }

    pub fn with_resamples(mut self, resamples: usize) -> Self {
        self.resamples = resamples.max(1);
        self
    }

    /// Evaluate every extractor against every selector's rankings on the same
    /// questions, then compare selectors pairwise per extractor.
    pub fn evaluate(
        &self,
        qa_pairs: &[QAPair],
        corpus: &Corpus,
        rankings_by_selector: &[(String, SelectorRankings)],
    ) -> Result<AnswerQualityResults> {
        if self.extractors.is_empty() {
            return Err(anyhow!("answer-quality evaluation needs an extractor"));
        }
        if qa_pairs.is_empty() {
            return Err(anyhow!("answer-quality evaluation needs a question"));
        }

        // key -> per-question metrics, ordered by qa_pairs so pairing is positional-safe.
        let mut per_run: HashMap<String, Vec<AnswerMetrics>> = HashMap::new();
        for (selector, rankings) in rankings_by_selector {
            for extractor in &self.extractors {
                let mut metrics = Vec::with_capacity(qa_pairs.len());
                for qa in qa_pairs {
                    let ranking = rankings
                        .get(&qa.query_id)
                        .ok_or_else(|| {
                            anyhow!("selector '{selector}' has no ranking for '{}'", qa.query_id)
                        })?
                        .as_slice();
                    metrics.push(self.score_one(qa, ranking, corpus, extractor.as_ref()));
                }
                per_run.insert(format!("{selector}/{}", extractor.name()), metrics);
            }
        }

        let mut aggregates = HashMap::new();
        for (selector, _) in rankings_by_selector {
            for extractor in &self.extractors {
                let key = format!("{selector}/{}", extractor.name());
                if let Some(metrics) = per_run.get(&key) {
                    aggregates.insert(
                        key.clone(),
                        self.aggregate(selector, extractor.name(), metrics),
                    );
                }
            }
        }

        let comparisons = self.compare(rankings_by_selector, &per_run);
        Ok(AnswerQualityResults {
            aggregates,
            comparisons,
        })
    }

    fn score_one(
        &self,
        qa: &QAPair,
        ranking: &[SelectorResult],
        corpus: &Corpus,
        extractor: &dyn AnswerExtractor,
    ) -> AnswerMetrics {
        let start = Instant::now();
        let extracted = extractor.extract(&qa.query_text, ranking, corpus);
        let latency_ms = start.elapsed().as_secs_f64() * 1000.0;

        let gold_doc_retrieved = if ranking.iter().any(|r| r.doc_id == qa.gold_doc_id) {
            1.0
        } else {
            0.0
        };

        let Some(answer) = extracted else {
            return AnswerMetrics {
                query_id: qa.query_id.clone(),
                answer_type: qa.answer_type,
                exact_match: 0.0,
                f1: 0.0,
                precision: 0.0,
                recall: 0.0,
                span_exact: 0.0,
                span_iou: 0.0,
                gold_doc_retrieved,
                latency_ms,
            };
        };

        // Multi-reference: score against every acceptable answer, keep the best F1.
        let predicted_norm = normalize_answer(&answer.answer_text);
        let predicted_tokens = normalized_tokens(&answer.answer_text);
        let mut best = (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
        for gold in &qa.answers {
            let (precision, recall, f1) =
                token_prf(&predicted_tokens, &normalized_tokens(&gold.text));
            let exact_match = if predicted_norm == normalize_answer(&gold.text) {
                1.0
            } else {
                0.0
            };
            let (span_exact, span_iou) = match (&answer.span, &gold.span) {
                (Some(p), Some(g)) => (if p == g { 1.0 } else { 0.0 }, p.iou(g)),
                // Gold pins no span, so span scoring does not apply; treat as 0 and
                // read `mean_span_iou` only on beds that carry gold spans.
                _ => (0.0, 0.0),
            };
            if f1 > best.2 || (f1 == best.2 && exact_match > best.0) {
                best = (exact_match, precision, f1, recall, span_exact, span_iou);
            }
        }

        AnswerMetrics {
            query_id: qa.query_id.clone(),
            answer_type: qa.answer_type,
            exact_match: best.0,
            precision: best.1,
            f1: best.2,
            recall: best.3,
            span_exact: best.4,
            span_iou: best.5,
            gold_doc_retrieved,
            latency_ms,
        }
    }

    fn aggregate(
        &self,
        selector: &str,
        extractor: &str,
        metrics: &[AnswerMetrics],
    ) -> AggregateQuality {
        let column = |f: fn(&AnswerMetrics) -> f64| -> Vec<f64> { metrics.iter().map(f).collect() };
        let exact_match = column(|m| m.exact_match);
        let f1 = column(|m| m.f1);

        let mut by_answer_type: HashMap<String, Vec<f64>> = HashMap::new();
        for m in metrics {
            by_answer_type
                .entry(m.answer_type.label().to_string())
                .or_default()
                .push(m.f1);
        }

        AggregateQuality {
            selector: selector.to_string(),
            extractor: extractor.to_string(),
            num_questions: metrics.len(),
            mean_exact_match: stats::mean(&exact_match),
            mean_f1: stats::mean(&f1),
            mean_precision: stats::mean(&column(|m| m.precision)),
            mean_recall: stats::mean(&column(|m| m.recall)),
            mean_span_exact: stats::mean(&column(|m| m.span_exact)),
            mean_span_iou: stats::mean(&column(|m| m.span_iou)),
            mean_gold_doc_retrieved: stats::mean(&column(|m| m.gold_doc_retrieved)),
            mean_latency_ms: stats::mean(&column(|m| m.latency_ms)),
            exact_match_ci: stats::bootstrap_ci(&exact_match, self.resamples, STATS_SEED),
            f1_ci: stats::bootstrap_ci(&f1, self.resamples, STATS_SEED),
            by_answer_type: by_answer_type
                .into_iter()
                .map(|(label, values)| (label, stats::mean(&values)))
                .collect(),
        }
    }

    fn compare(
        &self,
        rankings_by_selector: &[(String, SelectorRankings)],
        per_run: &HashMap<String, Vec<AnswerMetrics>>,
    ) -> Vec<QualityComparison> {
        let metrics: [MetricFn<AnswerMetrics>; 3] = [
            ("F1", |m| m.f1),
            ("ExactMatch", |m| m.exact_match),
            ("GoldDocRetrieved", |m| m.gold_doc_retrieved),
        ];

        let mut comparisons = Vec::new();
        for extractor in &self.extractors {
            for (i, (a, _)) in rankings_by_selector.iter().enumerate() {
                for (b, _) in rankings_by_selector.iter().skip(i + 1) {
                    let (Some(ma), Some(mb)) = (
                        per_run.get(&format!("{a}/{}", extractor.name())),
                        per_run.get(&format!("{b}/{}", extractor.name())),
                    ) else {
                        continue;
                    };
                    for (metric, extract) in metrics {
                        let index_b: HashMap<&str, f64> = mb
                            .iter()
                            .map(|m| (m.query_id.as_str(), extract(m)))
                            .collect();
                        let diffs: Vec<f64> = ma
                            .iter()
                            .filter_map(|m| {
                                index_b.get(m.query_id.as_str()).map(|v| extract(m) - *v)
                            })
                            .collect();
                        if diffs.is_empty() {
                            continue;
                        }
                        let mean_difference = stats::mean(&diffs);
                        let p_value = stats::sign_flip_p(&diffs, self.resamples, STATS_SEED);
                        let significant = p_value < 0.05;
                        comparisons.push(QualityComparison {
                            extractor: extractor.name().to_string(),
                            selector_a: a.clone(),
                            selector_b: b.clone(),
                            metric: metric.to_string(),
                            mean_difference,
                            difference_ci: stats::bootstrap_ci(&diffs, self.resamples, STATS_SEED),
                            p_value,
                            statistically_significant: significant,
                            winner: if !significant || mean_difference == 0.0 {
                                None
                            } else if mean_difference > 0.0 {
                                Some(a.clone())
                            } else {
                                Some(b.clone())
                            },
                        });
                    }
                }
            }
        }
        comparisons
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalisation_strips_case_punctuation_and_articles() {
        assert_eq!(
            normalize_answer("The Capital, of France!"),
            "capital of france"
        );
        assert_eq!(normalize_answer("  a  Boat  "), "boat");
        assert_eq!(normalize_answer("Paris"), normalize_answer("paris."));
    }

    #[test]
    fn exact_match_is_not_merely_token_set_equality() {
        // Same token bag, different order: F1 is 1.0 but this is NOT an exact match.
        let a = normalized_tokens("Paris France");
        let b = normalized_tokens("France Paris");
        let (_, _, f1) = token_prf(&a, &b);
        assert_eq!(f1, 1.0);
        assert_ne!(
            normalize_answer("Paris France"),
            normalize_answer("France Paris")
        );
    }

    #[test]
    fn token_f1_counts_bags_not_sets() {
        // Predicting "paris" three times must not earn triple credit.
        let predicted = normalized_tokens("paris paris paris");
        let gold = normalized_tokens("paris");
        let (precision, recall, _) = token_prf(&predicted, &gold);
        assert!(
            (precision - 1.0 / 3.0).abs() < 1e-12,
            "precision was {precision}"
        );
        assert_eq!(recall, 1.0);
    }

    #[test]
    fn disjoint_answers_score_zero() {
        let (p, r, f1) = token_prf(&normalized_tokens("berlin"), &normalized_tokens("paris"));
        assert_eq!((p, r, f1), (0.0, 0.0, 0.0));
    }

    #[test]
    fn sentence_bounds_index_the_original_string() {
        let text = "One. Two! Three?";
        let bounds = sentence_bounds(text);
        let slices: Vec<&str> = bounds.iter().map(|(s, e)| &text[*s..*e]).collect();
        assert_eq!(slices, vec!["One.", "Two!", "Three?"]);
    }

    #[test]
    fn sentence_bounds_handles_trailing_text_without_terminator() {
        let text = "First. dangling tail";
        let bounds = sentence_bounds(text);
        assert_eq!(bounds.len(), 2);
        assert_eq!(&text[bounds[1].0..bounds[1].1], "dangling tail");
    }

    #[test]
    fn span_iou_is_zero_across_documents() {
        let a = TextSpan {
            doc_id: "d1".into(),
            start: 0,
            end: 10,
        };
        let b = TextSpan {
            doc_id: "d2".into(),
            start: 0,
            end: 10,
        };
        assert_eq!(a.iou(&b), 0.0);
        assert_eq!(a.iou(&a), 1.0);

        let c = TextSpan {
            doc_id: "d1".into(),
            start: 5,
            end: 15,
        };
        assert!((a.iou(&c) - 5.0 / 15.0).abs() < 1e-12);
    }
}
