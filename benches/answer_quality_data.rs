//! Synthetic QA bed for the span/answer-quality gate (TD-SELECTOR-1, gate 4).
//!
//! Builds a corpus, a question set with gold spans, and — crucially — the *same*
//! `CandidatePool` shape the selector harness consumes, so a selector's real ranking
//! can be fed straight into answer extraction. That is what makes gate 4 a
//! measurement of the retrieval→answer coupling rather than a second simulation.
//!
//! # Construction
//!
//! Each question owns a private topic token, so exactly one sentence in one document
//! answers it. Distractor documents carry *near-miss* sentences that mention the
//! topic without answering it, so extraction degrades gradually rather than
//! collapsing to a binary — a bed where a wrong document scores 0.0 cannot tell a
//! mediocre extractor from a broken one.
//!
//! The gold document's depth in each retriever's ranking is drawn explicitly rather
//! than falling out of a score model. That is the independent variable: a selector
//! that truncates at depth D can only answer the questions whose gold document
//! landed above D.

#![allow(dead_code)]

use std::collections::HashMap;

use rand::rngs::SmallRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};

use crate::bench_answer_quality::{
    AnswerType, Corpus, GoldAnswer, QAPair, TextSpan, sentence_bounds,
};
use crate::bench_selector_benchmark::{Candidate, CandidatePool, GroundTruth, Query, ResultSource};

#[derive(Debug, Clone)]
pub struct QaBedConfig {
    pub num_questions: usize,
    /// Distractor documents in the corpus, shared across questions.
    pub num_distractor_docs: usize,
    /// Candidates each retriever returns.
    pub pool_size: usize,
    pub sentences_per_doc: usize,
    /// Deepest rank the gold document may be drawn to in a retriever's ranking.
    /// Larger values put more questions out of reach of a truncating selector.
    pub max_gold_depth: usize,
    pub seed: u64,
}

impl Default for QaBedConfig {
    fn default() -> Self {
        Self {
            num_questions: 60,
            num_distractor_docs: 400,
            pool_size: 200,
            sentences_per_doc: 6,
            max_gold_depth: 150,
            seed: 0x0A57_0000_0000_0004,
        }
    }
}

/// Everything gate 4 needs, plus the selector-harness inputs so the two compose.
pub struct QaBed {
    pub qa_pairs: Vec<QAPair>,
    pub corpus: Corpus,
    pub ground_truth: Vec<GroundTruth>,
    pub candidate_pools: HashMap<String, CandidatePool>,
    pub queries: Vec<Query>,
    /// Per query, the rank the gold document was placed at in the vector ranking.
    pub gold_depth: HashMap<String, usize>,
}

const ANSWER_TYPES: [AnswerType; 5] = [
    AnswerType::Factoid,
    AnswerType::List,
    AnswerType::Boolean,
    AnswerType::Descriptive,
    AnswerType::MultiHop,
];

pub fn generate_qa_bed(config: &QaBedConfig) -> QaBed {
    let mut rng = SmallRng::seed_from_u64(config.seed);

    let mut corpus: Corpus = HashMap::new();
    let mut qa_pairs = Vec::with_capacity(config.num_questions);
    let mut ground_truth = Vec::with_capacity(config.num_questions);
    let mut queries = Vec::with_capacity(config.num_questions);
    let mut gold_depth = HashMap::with_capacity(config.num_questions);

    // Distractor documents: filler prose plus near-miss sentences that mention a
    // topic without answering it.
    let distractor_ids: Vec<String> = (0..config.num_distractor_docs.max(1))
        .map(|i| format!("distractor{i}"))
        .collect();
    for (i, doc_id) in distractor_ids.iter().enumerate() {
        let mut sentences = Vec::with_capacity(config.sentences_per_doc);
        for s in 0..config.sentences_per_doc.max(1) {
            if s % 3 == 0 && config.num_questions > 0 {
                let topic = rng.gen_range(0..config.num_questions);
                // Mentions the topic, does not answer it.
                sentences.push(format!(
                    "Background remarks about topic{topic} appear in section {s} of note {i}."
                ));
            } else {
                sentences.push(format!(
                    "Filler statement {s} in note {i} covers unrelated material."
                ));
            }
        }
        corpus.insert(doc_id.clone(), sentences.join(" "));
    }

    let mut candidate_pools = HashMap::with_capacity(config.num_questions);

    for q in 0..config.num_questions {
        let query_id = format!("qa{q}");
        let gold_doc_id = format!("gold{q}");
        let answer_type = ANSWER_TYPES[q % ANSWER_TYPES.len()];

        // The one sentence that answers this question, placed at a random index so
        // an extractor cannot win by always taking the first sentence.
        let answer_sentence =
            format!("The topic{q} attribute{q} is recorded as value{q} in the register.");
        let answer_index = rng.gen_range(0..config.sentences_per_doc.max(1));
        let mut sentences: Vec<String> = (0..config.sentences_per_doc.max(1))
            .map(|s| format!("Section {s} of the topic{q} dossier lists procedural detail."))
            .collect();
        sentences[answer_index] = answer_sentence.clone();
        let gold_text = sentences.join(" ");

        // Locate the answer sentence's byte span in the assembled document, using the
        // same splitter the extractor uses so gold and predictions are comparable.
        let gold_span = sentence_bounds(&gold_text)
            .into_iter()
            .find(|(s, e)| gold_text[*s..*e] == answer_sentence)
            .map(|(start, end)| TextSpan {
                doc_id: gold_doc_id.clone(),
                start,
                end,
            });

        corpus.insert(gold_doc_id.clone(), gold_text);

        let query_text = format!("What is the topic{q} attribute{q}?");
        qa_pairs.push(QAPair {
            query_id: query_id.clone(),
            query_text: query_text.clone(),
            answers: vec![GoldAnswer {
                text: answer_sentence,
                span: gold_span,
            }],
            answer_type,
            gold_doc_id: gold_doc_id.clone(),
        });
        ground_truth.push(GroundTruth {
            query_id: query_id.clone(),
            relevant_docs: vec![gold_doc_id.clone()],
        });

        let depth = rng.gen_range(0..config.max_gold_depth.max(1).min(config.pool_size.max(1)));
        gold_depth.insert(query_id.clone(), depth);
        candidate_pools.insert(
            query_id.clone(),
            build_pool(
                &mut rng,
                &query_id,
                &gold_doc_id,
                &distractor_ids,
                depth,
                config,
            ),
        );

        queries.push(Query {
            query_id,
            query_text,
            query_vector: None,
        });
    }

    QaBed {
        qa_pairs,
        corpus,
        ground_truth,
        candidate_pools,
        queries,
        gold_depth,
    }
}

/// Build both retrievers' rankings with the gold document pinned to `gold_depth`
/// in the vector ranking and to an independently drawn depth in the BM25 ranking.
///
/// Scores are assigned monotonically from the rank, so ordering is exactly the
/// depth we chose — the experiment's independent variable is not muddied by noise.
fn build_pool(
    rng: &mut SmallRng,
    query_id: &str,
    gold_doc_id: &str,
    distractor_ids: &[String],
    gold_depth: usize,
    config: &QaBedConfig,
) -> CandidatePool {
    let pool_size = config.pool_size.max(1);

    // Draw both depths up front: the ranking builder borrows `rng`, so drawing
    // between the two calls would conflict with that borrow.
    let bm25_depth = rng.gen_range(0..config.max_gold_depth.max(1).min(pool_size));
    let vector_results = make_ranking(
        rng,
        gold_doc_id,
        distractor_ids,
        gold_depth,
        pool_size,
        ResultSource::Vector,
    );
    let bm25_results = make_ranking(
        rng,
        gold_doc_id,
        distractor_ids,
        bm25_depth,
        pool_size,
        ResultSource::BM25,
    );

    let mut union: HashMap<&str, Candidate> = HashMap::new();
    for candidate in vector_results.iter().chain(bm25_results.iter()) {
        union
            .entry(candidate.doc_id.as_str())
            .and_modify(|existing| {
                if candidate.score > existing.score {
                    existing.score = candidate.score;
                }
                existing.source = ResultSource::Hybrid;
            })
            .or_insert_with(|| candidate.clone());
    }
    let mut union_pool: Vec<Candidate> = union.into_values().collect();
    sort_desc(&mut union_pool);

    CandidatePool {
        query_id: query_id.to_string(),
        vector_results,
        bm25_results,
        union_pool,
    }
}

/// One retriever's ranking: `pool_size - 1` shuffled distractors with the gold
/// document spliced in at `depth`, scored strictly decreasing in rank.
fn make_ranking(
    rng: &mut SmallRng,
    gold_doc_id: &str,
    distractor_ids: &[String],
    depth: usize,
    pool_size: usize,
    source: ResultSource,
) -> Vec<Candidate> {
    let mut picked: Vec<&String> = distractor_ids.iter().collect();
    picked.shuffle(rng);
    let mut docs: Vec<String> = picked
        .into_iter()
        .take(pool_size.saturating_sub(1))
        .cloned()
        .collect();
    docs.insert(depth.min(docs.len()), gold_doc_id.to_string());

    docs.into_iter()
        .enumerate()
        .map(|(rank, doc_id)| Candidate {
            doc_id,
            // Strictly decreasing in rank, and bounded in (0, 1].
            score: 1.0 - (rank as f64 / (pool_size + 1) as f64),
            source: source.clone(),
        })
        .collect()
}

fn sort_desc(candidates: &mut [Candidate]) {
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.doc_id.cmp(&b.doc_id))
    });
}

/// Fraction of questions whose gold document sits at or above `depth` in the vector
/// ranking — the share any selector truncating at `depth` could still answer.
pub fn reachable_fraction(bed: &QaBed, depth: usize) -> f64 {
    if bed.gold_depth.is_empty() {
        return 0.0;
    }
    let reachable = bed.gold_depth.values().filter(|d| **d < depth).count();
    reachable as f64 / bed.gold_depth.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_question_has_a_gold_document_and_a_locatable_span() {
        let bed = generate_qa_bed(&QaBedConfig::default());
        assert_eq!(bed.qa_pairs.len(), bed.gold_depth.len());
        for qa in &bed.qa_pairs {
            let text = bed
                .corpus
                .get(&qa.gold_doc_id)
                .expect("gold document is in the corpus");
            let gold = qa.answers.first().expect("one gold answer");
            let span = gold.span.as_ref().expect("gold span was located");
            assert_eq!(
                &text[span.start..span.end],
                gold.text,
                "the gold span must index the gold answer inside the gold document"
            );
        }
    }

    #[test]
    fn the_gold_document_sits_at_the_depth_it_was_assigned() {
        let bed = generate_qa_bed(&QaBedConfig::default());
        for qa in &bed.qa_pairs {
            let pool = bed
                .candidate_pools
                .get(&qa.query_id)
                .expect("pool for every question");
            let position = pool
                .vector_results
                .iter()
                .position(|c| c.doc_id == qa.gold_doc_id)
                .expect("gold document is in the vector ranking");
            assert_eq!(position, bed.gold_depth[&qa.query_id]);
        }
    }

    #[test]
    fn generation_is_deterministic_for_a_seed() {
        let a = generate_qa_bed(&QaBedConfig::default());
        let b = generate_qa_bed(&QaBedConfig::default());
        assert_eq!(a.gold_depth, b.gold_depth);
        for (id, text) in &a.corpus {
            assert_eq!(b.corpus.get(id), Some(text));
        }
    }

    #[test]
    fn gold_depths_span_a_useful_range() {
        // A bed where every gold document ranks first cannot show truncation cost.
        let bed = generate_qa_bed(&QaBedConfig::default());
        assert!(reachable_fraction(&bed, 10) < 0.35);
        assert!(reachable_fraction(&bed, 150) > 0.95);
    }
}
