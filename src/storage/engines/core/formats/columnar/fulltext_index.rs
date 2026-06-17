//! Full-Text Search Index for TEXT Columns
//!
//! Provides inverted index-based full-text search with BM25 scoring for RAG applications.
//! This module integrates with the existing TextStorage infrastructure to enable
//! efficient text retrieval and ranking.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    Full-Text Search Index                        │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  ┌─────────────────┐  ┌──────────────────┐  ┌────────────────┐ │
//! │  │   Tokenizers    │  │  Inverted Index  │  │  BM25 Scorer   │ │
//! │  │  - Standard     │  │  - Term → Docs   │  │  - TF-IDF      │ │
//! │  │  - Whitespace   │  │  - Positions     │  │  - Doc lengths │ │
//! │  │  - N-gram       │  │  - Frequencies   │  │  - Avg length  │ │
//! │  └─────────────────┘  └──────────────────┘  └────────────────┘ │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  FullTextIndex │ TextStatistics │ FulltextSearchResult │ IndexBuilder  │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Features
//!
//! - **Tokenization**: Multiple tokenizer options (standard, whitespace, n-gram)
//! - **Inverted Index**: Efficient term-to-document mapping with position information
//! - **BM25 Scoring**: Industry-standard ranking algorithm with tunable parameters
//! - **Statistics Collection**: Document length, term frequencies, IDF values
//! - **RAG Integration**: Works with ChunkingConfig for per-chunk indexing
//!
//! ## Example Usage
//!
//! ```rust,ignore
//! use proximadb::storage::engines::core::formats::columnar::fulltext_index::*;
//!
//! // Create index with standard tokenizer
//! let mut index = FullTextIndex::new(TokenizerConfig::default());
//!
//! // Add documents
//! index.add_document("doc_001", "The quick brown fox jumps over the lazy dog");
//! index.add_document("doc_002", "A quick brown dog runs across the field");
//!
//! // Search with BM25 scoring
//! let results = index.search("quick brown", 10);
//! for result in results {
//!     println!("Doc: {}, Score: {:.4}", result.doc_id, result.score);
//! }
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};
use thiserror::Error;

/// Errors that can occur during full-text indexing
#[derive(Error, Debug)]
pub enum FullTextIndexError {
    /// Invalid tokenizer configuration
    #[error("Invalid tokenizer configuration: {0}")]
    InvalidTokenizerConfig(String),

    /// Document already exists
    #[error("Document already exists: {0}")]
    DocumentExists(String),

    /// Document not found
    #[error("Document not found: {0}")]
    DocumentNotFound(String),

    /// Index serialization error
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Index is read-only
    #[error("Index is read-only, cannot modify")]
    ReadOnlyIndex,
}

// =============================================================================
// Tokenization
// =============================================================================

/// Tokenizer type selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TokenizerType {
    /// Standard tokenizer: lowercase, remove punctuation, split on whitespace
    #[default]
    Standard,
    /// Whitespace-only tokenizer: split on whitespace, preserve case
    Whitespace,
    /// Simple tokenizer: lowercase, split on non-alphanumeric
    Simple,
    /// N-gram tokenizer: generate character n-grams
    Ngram,
    /// Edge n-gram tokenizer: n-grams from word beginnings
    EdgeNgram,
    /// Keyword tokenizer: entire input as single token
    Keyword,
}

/// Configuration for tokenization
#[derive(Debug, Clone)]
pub struct TokenizerConfig {
    /// Type of tokenizer to use
    pub tokenizer_type: TokenizerType,

    /// Minimum token length to index
    pub min_token_length: usize,

    /// Maximum token length to index
    pub max_token_length: usize,

    /// Stop words to filter out
    pub stop_words: HashSet<String>,

    /// Whether to apply stemming (Porter stemmer)
    pub enable_stemming: bool,

    /// Whether to convert to lowercase
    pub lowercase: bool,

    /// N-gram minimum size (for n-gram tokenizers)
    pub ngram_min: usize,

    /// N-gram maximum size (for n-gram tokenizers)
    pub ngram_max: usize,

    /// Custom token patterns (regex)
    pub token_patterns: Option<Vec<String>>,
}

impl Default for TokenizerConfig {
    fn default() -> Self {
        Self {
            tokenizer_type: TokenizerType::Standard,
            min_token_length: 1,
            max_token_length: 256,
            stop_words: Self::default_stop_words(),
            enable_stemming: false,
            lowercase: true,
            ngram_min: 3,
            ngram_max: 4,
            token_patterns: None,
        }
    }
}

impl TokenizerConfig {
    /// Create a new tokenizer config with specified type
    pub fn new(tokenizer_type: TokenizerType) -> Self {
        Self {
            tokenizer_type,
            ..Default::default()
        }
    }

    /// Create config for semantic search (minimal preprocessing)
    pub fn for_semantic_search() -> Self {
        Self {
            tokenizer_type: TokenizerType::Simple,
            enable_stemming: false,
            stop_words: HashSet::new(), // Keep stop words for semantic meaning
            ..Default::default()
        }
    }

    /// Create config for keyword search (aggressive normalization)
    pub fn for_keyword_search() -> Self {
        Self {
            tokenizer_type: TokenizerType::Standard,
            enable_stemming: true,
            ..Default::default()
        }
    }

    /// Create config for autocomplete (edge n-grams)
    pub fn for_autocomplete() -> Self {
        Self {
            tokenizer_type: TokenizerType::EdgeNgram,
            ngram_min: 2,
            ngram_max: 10,
            stop_words: HashSet::new(),
            ..Default::default()
        }
    }

    /// Create config for fuzzy matching (n-grams)
    pub fn for_fuzzy_matching() -> Self {
        Self {
            tokenizer_type: TokenizerType::Ngram,
            ngram_min: 3,
            ngram_max: 4,
            stop_words: HashSet::new(),
            ..Default::default()
        }
    }

    /// Builder: set minimum token length
    pub fn with_min_token_length(mut self, len: usize) -> Self {
        self.min_token_length = len;
        self
    }

    /// Builder: set maximum token length
    pub fn with_max_token_length(mut self, len: usize) -> Self {
        self.max_token_length = len;
        self
    }

    /// Builder: add stop words
    pub fn with_stop_words(mut self, words: Vec<String>) -> Self {
        self.stop_words.extend(words);
        self
    }

    /// Builder: clear stop words
    pub fn without_stop_words(mut self) -> Self {
        self.stop_words.clear();
        self
    }

    /// Builder: enable/disable stemming
    pub fn with_stemming(mut self, enable: bool) -> Self {
        self.enable_stemming = enable;
        self
    }

    /// Builder: set lowercase flag
    pub fn with_lowercase(mut self, lowercase: bool) -> Self {
        self.lowercase = lowercase;
        self
    }

    /// Builder: set n-gram range
    pub fn with_ngram_range(mut self, min: usize, max: usize) -> Self {
        self.ngram_min = min;
        self.ngram_max = max;
        self
    }

    /// Default English stop words
    pub fn default_stop_words() -> HashSet<String> {
        [
            "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "has", "he", "in",
            "is", "it", "its", "of", "on", "or", "that", "the", "to", "was", "were", "will",
            "with", "the", "this", "but", "they", "have", "had", "what", "when", "where", "who",
            "which", "why", "how", "all", "each", "every", "both", "few", "more", "most", "other",
            "some", "such", "no", "not", "only", "own", "same", "so", "than", "too", "very", "can",
            "just", "should", "now",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }
}

/// A token with position information
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// The token text
    pub text: String,
    /// Position in the document (token index)
    pub position: u32,
    /// Character start offset
    pub start_offset: u32,
    /// Character end offset
    pub end_offset: u32,
}

/// Tokenizer that processes text into tokens
#[derive(Debug, Clone)]
pub struct Tokenizer {
    config: TokenizerConfig,
}

impl Tokenizer {
    /// Create a new tokenizer with given configuration
    pub fn new(config: TokenizerConfig) -> Self {
        Self { config }
    }

    /// Get the tokenizer configuration
    pub fn config(&self) -> &TokenizerConfig {
        &self.config
    }

    /// Tokenize text into tokens with positions
    pub fn tokenize(&self, text: &str) -> Vec<Token> {
        match self.config.tokenizer_type {
            TokenizerType::Standard => self.tokenize_standard(text),
            TokenizerType::Whitespace => self.tokenize_whitespace(text),
            TokenizerType::Simple => self.tokenize_simple(text),
            TokenizerType::Ngram => self.tokenize_ngram(text),
            TokenizerType::EdgeNgram => self.tokenize_edge_ngram(text),
            TokenizerType::Keyword => self.tokenize_keyword(text),
        }
    }

    /// Tokenize into just the token strings (for quick queries)
    pub fn tokenize_to_strings(&self, text: &str) -> Vec<String> {
        self.tokenize(text).into_iter().map(|t| t.text).collect()
    }

    /// Standard tokenization: lowercase, split on non-alphanumeric, filter
    fn tokenize_standard(&self, text: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        let mut position = 0u32;
        let _start = 0;
        let mut in_token = false;
        let mut token_start = 0;

        let text_lower = if self.config.lowercase {
            text.to_lowercase()
        } else {
            text.to_string()
        };

        for (i, c) in text_lower.char_indices() {
            if c.is_alphanumeric() || c == '_' {
                if !in_token {
                    token_start = i;
                    in_token = true;
                }
            } else if in_token {
                let token_text = &text_lower[token_start..i];
                if let Some(token) = self.process_token(token_text, position, token_start, i) {
                    tokens.push(token);
                    position += 1;
                }
                in_token = false;
            }
        }

        // Handle last token
        if in_token {
            let token_text = &text_lower[token_start..];
            if let Some(token) =
                self.process_token(token_text, position, token_start, text_lower.len())
            {
                tokens.push(token);
            }
        }

        tokens
    }

    /// Whitespace tokenization: split on whitespace only
    fn tokenize_whitespace(&self, text: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        let mut position = 0u32;

        for word in text.split_whitespace() {
            let text_to_use = if self.config.lowercase {
                word.to_lowercase()
            } else {
                word.to_string()
            };

            if let Some(token) = self.process_token(&text_to_use, position, 0, text_to_use.len()) {
                tokens.push(token);
                position += 1;
            }
        }

        tokens
    }

    /// Simple tokenization: lowercase, split on non-alphanumeric
    fn tokenize_simple(&self, text: &str) -> Vec<Token> {
        self.tokenize_standard(text)
    }

    /// N-gram tokenization: character n-grams
    fn tokenize_ngram(&self, text: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        let text_to_use = if self.config.lowercase {
            text.to_lowercase()
        } else {
            text.to_string()
        };

        let chars: Vec<char> = text_to_use.chars().collect();
        let mut position = 0u32;

        for n in self.config.ngram_min..=self.config.ngram_max {
            if chars.len() >= n {
                for i in 0..=(chars.len() - n) {
                    let ngram: String = chars[i..i + n].iter().collect();
                    if self.is_valid_token(&ngram) {
                        tokens.push(Token {
                            text: ngram,
                            position,
                            start_offset: i as u32,
                            end_offset: (i + n) as u32,
                        });
                        position += 1;
                    }
                }
            }
        }

        tokens
    }

    /// Edge n-gram tokenization: n-grams from word beginnings
    fn tokenize_edge_ngram(&self, text: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        let text_to_use = if self.config.lowercase {
            text.to_lowercase()
        } else {
            text.to_string()
        };

        let mut position = 0u32;

        for word in text_to_use.split_whitespace() {
            let chars: Vec<char> = word.chars().collect();
            for n in self.config.ngram_min..=self.config.ngram_max.min(chars.len()) {
                let ngram: String = chars[..n].iter().collect();
                if self.is_valid_token(&ngram) {
                    tokens.push(Token {
                        text: ngram,
                        position,
                        start_offset: 0,
                        end_offset: n as u32,
                    });
                    position += 1;
                }
            }
        }

        tokens
    }

    /// Keyword tokenization: entire input as single token
    fn tokenize_keyword(&self, text: &str) -> Vec<Token> {
        let text_to_use = if self.config.lowercase {
            text.to_lowercase()
        } else {
            text.to_string()
        };

        let trimmed = text_to_use.trim();
        if !trimmed.is_empty() && self.is_valid_token(trimmed) {
            vec![Token {
                text: trimmed.to_string(),
                position: 0,
                start_offset: 0,
                end_offset: trimmed.len() as u32,
            }]
        } else {
            Vec::new()
        }
    }

    /// Process a token: apply stemming, check validity
    fn process_token(&self, text: &str, position: u32, start: usize, end: usize) -> Option<Token> {
        if !self.is_valid_token(text) {
            return None;
        }

        let token_text = if self.config.enable_stemming {
            self.stem(text)
        } else {
            text.to_string()
        };

        Some(Token {
            text: token_text,
            position,
            start_offset: start as u32,
            end_offset: end as u32,
        })
    }

    /// Check if a token is valid (length, stop words)
    fn is_valid_token(&self, text: &str) -> bool {
        let len = text.len();
        if len < self.config.min_token_length || len > self.config.max_token_length {
            return false;
        }
        if self.config.stop_words.contains(text) {
            return false;
        }
        true
    }

    /// Simple Porter-like stemming (basic suffix removal)
    fn stem(&self, word: &str) -> String {
        let mut result = word.to_string();

        // Basic suffix rules (simplified Porter stemmer)
        let suffixes = [
            ("ational", "ate"),
            ("tional", "tion"),
            ("enci", "ence"),
            ("anci", "ance"),
            ("izer", "ize"),
            ("isation", "ize"),
            ("ization", "ize"),
            ("ation", "ate"),
            ("ator", "ate"),
            ("alism", "al"),
            ("iveness", "ive"),
            ("fulness", "ful"),
            ("ousness", "ous"),
            ("aliti", "al"),
            ("iviti", "ive"),
            ("biliti", "ble"),
            ("alli", "al"),
            ("entli", "ent"),
            ("eli", "e"),
            ("ousli", "ous"),
            ("ing", ""),
            ("ed", ""),
            ("ly", ""),
            ("es", ""),
            ("s", ""),
        ];

        for (suffix, replacement) in suffixes {
            if result.ends_with(suffix) && result.len() > suffix.len() + 2 {
                result = format!("{}{}", &result[..result.len() - suffix.len()], replacement);
                break;
            }
        }

        result
    }
}

impl Default for Tokenizer {
    fn default() -> Self {
        Self::new(TokenizerConfig::default())
    }
}

// =============================================================================
// Inverted Index
// =============================================================================

/// Posting for a term occurrence
#[derive(Debug, Clone)]
pub struct Posting {
    /// Document ID
    pub doc_id: String,
    /// Term frequency in this document
    pub term_frequency: u32,
    /// Positions of the term in the document
    pub positions: Vec<u32>,
}

/// Backwards-compat alias for [`FulltextPostingList`].
pub type PostingList = FulltextPostingList;

/// Posting list for a term
#[derive(Debug, Clone, Default)]
pub struct FulltextPostingList {
    /// Document frequency (number of documents containing this term)
    pub doc_frequency: u32,
    /// Total occurrences across all documents
    pub total_frequency: u64,
    /// Individual postings
    pub postings: Vec<Posting>,
}

impl FulltextPostingList {
    /// Create a new empty posting list
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a posting to the list
    pub fn add_posting(&mut self, doc_id: String, positions: Vec<u32>) {
        let term_frequency = positions.len() as u32;
        self.total_frequency += term_frequency as u64;
        self.doc_frequency += 1;
        self.postings.push(Posting {
            doc_id,
            term_frequency,
            positions,
        });
    }

    /// Get posting for a specific document
    pub fn get_posting(&self, doc_id: &str) -> Option<&Posting> {
        self.postings.iter().find(|p| p.doc_id == doc_id)
    }
}

/// Document metadata stored in the index
#[derive(Debug, Clone)]
pub struct DocumentMetadata {
    /// Number of tokens in the document
    pub token_count: u32,
    /// Original text length in characters
    pub char_length: u32,
    /// Timestamp when document was indexed
    pub indexed_at: i64,
    /// Custom metadata fields
    pub metadata: HashMap<String, String>,
}

// =============================================================================
// Text Statistics
// =============================================================================

/// Statistics about the indexed text corpus
#[derive(Debug, Clone, Default)]
pub struct TextStatistics {
    /// Total number of documents
    pub total_documents: u64,
    /// Total number of tokens across all documents
    pub total_tokens: u64,
    /// Average document length (in tokens)
    pub avg_document_length: f64,
    /// Number of unique terms
    pub unique_terms: u64,
    /// Maximum document length
    pub max_document_length: u32,
    /// Minimum document length (non-zero)
    pub min_document_length: u32,
    /// Term frequency distribution (for top terms)
    pub top_terms: Vec<(String, u64)>,
}

impl TextStatistics {
    /// Update statistics after adding a document
    pub fn update_after_add(&mut self, token_count: u32) {
        self.total_documents += 1;
        self.total_tokens += token_count as u64;
        self.avg_document_length = self.total_tokens as f64 / self.total_documents.max(1) as f64;

        if token_count > self.max_document_length {
            self.max_document_length = token_count;
        }
        if self.min_document_length == 0 || token_count < self.min_document_length {
            self.min_document_length = token_count;
        }
    }

    /// Update statistics after removing a document
    pub fn update_after_remove(&mut self, token_count: u32) {
        self.total_documents = self.total_documents.saturating_sub(1);
        self.total_tokens = self.total_tokens.saturating_sub(token_count as u64);
        if self.total_documents > 0 {
            self.avg_document_length = self.total_tokens as f64 / self.total_documents as f64;
        } else {
            self.avg_document_length = 0.0;
        }
    }

    /// Update unique term count
    pub fn set_unique_terms(&mut self, count: u64) {
        self.unique_terms = count;
    }
}

// =============================================================================
// BM25 Scoring
// =============================================================================

/// BM25 scoring configuration
#[derive(Debug, Clone)]
pub struct BM25Config {
    /// k1 parameter: term frequency saturation (typical: 1.2-2.0)
    pub k1: f64,
    /// b parameter: document length normalization (typical: 0.75)
    pub b: f64,
    /// delta parameter for BM25+ variant (typical: 1.0)
    pub delta: f64,
    /// Whether to use BM25+ variant
    pub use_bm25_plus: bool,
}

impl Default for BM25Config {
    fn default() -> Self {
        Self {
            k1: 1.2,
            b: 0.75,
            delta: 1.0,
            use_bm25_plus: false,
        }
    }
}

impl BM25Config {
    /// Create config optimized for short documents
    pub fn for_short_documents() -> Self {
        Self {
            k1: 1.5,
            b: 0.5, // Less length normalization for short docs
            ..Default::default()
        }
    }

    /// Create config optimized for long documents
    pub fn for_long_documents() -> Self {
        Self {
            k1: 1.0,
            b: 0.9, // More length normalization for long docs
            ..Default::default()
        }
    }

    /// Create BM25+ config (better for exact match queries)
    pub fn bm25_plus() -> Self {
        Self {
            use_bm25_plus: true,
            delta: 1.0,
            ..Default::default()
        }
    }
}

/// BM25 scorer for ranking search results
#[derive(Debug, Clone)]
pub struct BM25Scorer {
    config: BM25Config,
    /// Average document length
    avgdl: f64,
    /// Total number of documents
    n_docs: u64,
}

impl BM25Scorer {
    /// Create a new BM25 scorer
    pub fn new(config: BM25Config, avgdl: f64, n_docs: u64) -> Self {
        Self {
            config,
            avgdl,
            n_docs,
        }
    }

    /// Calculate IDF (Inverse Document Frequency) for a term
    ///
    /// IDF = log((N - n + 0.5) / (n + 0.5) + 1)
    /// where N = total documents, n = documents containing term
    pub fn idf(&self, doc_frequency: u32) -> f64 {
        let n = doc_frequency as f64;
        let n_docs = self.n_docs as f64;

        // Smoothed IDF to avoid negative values
        ((n_docs - n + 0.5) / (n + 0.5) + 1.0).ln()
    }

    /// Calculate BM25 score for a single term in a document
    ///
    /// score = IDF * (tf * (k1 + 1)) / (tf + k1 * (1 - b + b * dl/avgdl))
    pub fn term_score(&self, term_frequency: u32, doc_frequency: u32, doc_length: u32) -> f64 {
        let tf = term_frequency as f64;
        let dl = doc_length as f64;
        let k1 = self.config.k1;
        let b = self.config.b;
        let avgdl = self.avgdl.max(1.0);

        let idf = self.idf(doc_frequency);

        let numerator = tf * (k1 + 1.0);
        let denominator = tf + k1 * (1.0 - b + b * dl / avgdl);

        let score = idf * numerator / denominator;

        if self.config.use_bm25_plus {
            score + self.config.delta
        } else {
            score
        }
    }

    /// Calculate TF-IDF score (alternative to BM25)
    pub fn tfidf_score(&self, term_frequency: u32, doc_frequency: u32, doc_length: u32) -> f64 {
        let tf = (term_frequency as f64).ln() + 1.0; // Log-normalized TF
        let idf = self.idf(doc_frequency);
        let length_norm = 1.0 / (doc_length as f64).sqrt(); // L2 normalization

        tf * idf * length_norm
    }

    /// Update statistics (call when index changes)
    pub fn update_stats(&mut self, avgdl: f64, n_docs: u64) {
        self.avgdl = avgdl;
        self.n_docs = n_docs;
    }
}

// =============================================================================
// Search Results
// =============================================================================

/// A search result with score
#[derive(Debug, Clone)]
pub struct FulltextSearchResult {
    /// Document ID
    pub doc_id: String,
    /// Relevance score
    pub score: f64,
    /// Matching terms
    pub matched_terms: Vec<String>,
    /// Term frequency breakdown per term
    pub term_frequencies: HashMap<String, u32>,
    /// Highlight positions (term -> positions)
    pub highlight_positions: HashMap<String, Vec<u32>>,
}

impl FulltextSearchResult {
    /// Create a new search result
    pub fn new(doc_id: String, score: f64) -> Self {
        Self {
            doc_id,
            score,
            matched_terms: Vec::new(),
            term_frequencies: HashMap::new(),
            highlight_positions: HashMap::new(),
        }
    }

    /// Add matched term information
    pub fn add_term_match(&mut self, term: String, frequency: u32, positions: Vec<u32>) {
        self.matched_terms.push(term.clone());
        self.term_frequencies.insert(term.clone(), frequency);
        self.highlight_positions.insert(term, positions);
    }
}

/// Search options for query configuration
#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// Maximum number of results
    pub limit: usize,
    /// Minimum score threshold
    pub min_score: f64,
    /// Whether to compute highlights
    pub include_highlights: bool,
    /// Whether all query terms must match (AND) or any (OR)
    pub require_all_terms: bool,
    /// Boost factors for specific terms
    pub term_boosts: HashMap<String, f64>,
    /// Field weights (if searching multiple fields)
    pub field_weights: HashMap<String, f64>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            limit: 10,
            min_score: 0.0,
            include_highlights: false,
            require_all_terms: false,
            term_boosts: HashMap::new(),
            field_weights: HashMap::new(),
        }
    }
}

impl SearchOptions {
    /// Create options for top-k search
    pub fn top_k(k: usize) -> Self {
        Self {
            limit: k,
            ..Default::default()
        }
    }

    /// Builder: set minimum score
    pub fn with_min_score(mut self, min_score: f64) -> Self {
        self.min_score = min_score;
        self
    }

    /// Builder: enable highlights
    pub fn with_highlights(mut self) -> Self {
        self.include_highlights = true;
        self
    }

    /// Builder: require all terms (AND query)
    pub fn require_all(mut self) -> Self {
        self.require_all_terms = true;
        self
    }

    /// Builder: add term boost
    pub fn with_term_boost(mut self, term: String, boost: f64) -> Self {
        self.term_boosts.insert(term, boost);
        self
    }
}

// =============================================================================
// Full-Text Index
// =============================================================================

/// Full-text search index with inverted index and BM25 scoring
#[derive(Debug)]
pub struct FullTextIndex {
    /// Tokenizer for processing text
    tokenizer: Tokenizer,
    /// Inverted index: term -> posting list
    inverted_index: BTreeMap<String, FulltextPostingList>,
    /// Document metadata
    documents: HashMap<String, DocumentMetadata>,
    /// Text statistics
    statistics: TextStatistics,
    /// BM25 scorer
    scorer: BM25Scorer,
    /// BM25 configuration
    bm25_config: BM25Config,
    /// Whether the index is read-only
    read_only: bool,
}

impl FullTextIndex {
    /// Create a new full-text index
    pub fn new(tokenizer_config: TokenizerConfig) -> Self {
        Self {
            tokenizer: Tokenizer::new(tokenizer_config),
            inverted_index: BTreeMap::new(),
            documents: HashMap::new(),
            statistics: TextStatistics::default(),
            scorer: BM25Scorer::new(BM25Config::default(), 0.0, 0),
            bm25_config: BM25Config::default(),
            read_only: false,
        }
    }

    /// Create with custom BM25 configuration
    pub fn with_bm25_config(mut self, config: BM25Config) -> Self {
        self.bm25_config = config.clone();
        self.scorer = BM25Scorer::new(config, 0.0, 0);
        self
    }

    /// Get the tokenizer
    pub fn tokenizer(&self) -> &Tokenizer {
        &self.tokenizer
    }

    /// Get text statistics
    pub fn statistics(&self) -> &TextStatistics {
        &self.statistics
    }

    /// Get number of indexed documents
    pub fn document_count(&self) -> usize {
        self.documents.len()
    }

    /// Get number of unique terms
    pub fn term_count(&self) -> usize {
        self.inverted_index.len()
    }

    /// Check if a document exists
    pub fn contains_document(&self, doc_id: &str) -> bool {
        self.documents.contains_key(doc_id)
    }

    /// Make the index read-only
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    /// Add a document to the index
    pub fn add_document(&mut self, doc_id: &str, text: &str) -> Result<(), FullTextIndexError> {
        if self.read_only {
            return Err(FullTextIndexError::ReadOnlyIndex);
        }

        if self.documents.contains_key(doc_id) {
            return Err(FullTextIndexError::DocumentExists(doc_id.to_string()));
        }

        let tokens = self.tokenizer.tokenize(text);
        let token_count = tokens.len() as u32;

        // Build term frequency map with positions
        let mut term_positions: HashMap<String, Vec<u32>> = HashMap::new();
        for token in &tokens {
            term_positions
                .entry(token.text.clone())
                .or_default()
                .push(token.position);
        }

        // Add to inverted index
        for (term, positions) in term_positions {
            self.inverted_index
                .entry(term)
                .or_default()
                .add_posting(doc_id.to_string(), positions);
        }

        // Store document metadata
        self.documents.insert(
            doc_id.to_string(),
            DocumentMetadata {
                token_count,
                char_length: text.len() as u32,
                indexed_at: chrono::Utc::now().timestamp_millis(),
                metadata: HashMap::new(),
            },
        );

        // Update statistics
        self.statistics.update_after_add(token_count);
        self.statistics
            .set_unique_terms(self.inverted_index.len() as u64);

        // Update scorer
        self.scorer.update_stats(
            self.statistics.avg_document_length,
            self.statistics.total_documents,
        );

        Ok(())
    }

    /// Add a document with metadata
    pub fn add_document_with_metadata(
        &mut self,
        doc_id: &str,
        text: &str,
        metadata: HashMap<String, String>,
    ) -> Result<(), FullTextIndexError> {
        self.add_document(doc_id, text)?;

        // Update metadata
        if let Some(doc_meta) = self.documents.get_mut(doc_id) {
            doc_meta.metadata = metadata;
        }

        Ok(())
    }

    /// Remove a document from the index
    pub fn remove_document(&mut self, doc_id: &str) -> Result<(), FullTextIndexError> {
        if self.read_only {
            return Err(FullTextIndexError::ReadOnlyIndex);
        }

        let doc_meta = self
            .documents
            .remove(doc_id)
            .ok_or_else(|| FullTextIndexError::DocumentNotFound(doc_id.to_string()))?;

        // Remove from inverted index
        let mut empty_terms = Vec::new();
        for (term, posting_list) in &mut self.inverted_index {
            if let Some(idx) = posting_list
                .postings
                .iter()
                .position(|p| p.doc_id == doc_id)
            {
                let removed = posting_list.postings.remove(idx);
                posting_list.doc_frequency -= 1;
                posting_list.total_frequency -= removed.term_frequency as u64;

                if posting_list.postings.is_empty() {
                    empty_terms.push(term.clone());
                }
            }
        }

        // Remove empty terms
        for term in empty_terms {
            self.inverted_index.remove(&term);
        }

        // Update statistics
        self.statistics.update_after_remove(doc_meta.token_count);
        self.statistics
            .set_unique_terms(self.inverted_index.len() as u64);

        // Update scorer
        self.scorer.update_stats(
            self.statistics.avg_document_length,
            self.statistics.total_documents,
        );

        Ok(())
    }

    /// Search the index with default options
    pub fn search(&self, query: &str, limit: usize) -> Vec<FulltextSearchResult> {
        self.search_with_options(query, SearchOptions::top_k(limit))
    }

    /// Search the index with custom options
    pub fn search_with_options(
        &self,
        query: &str,
        options: SearchOptions,
    ) -> Vec<FulltextSearchResult> {
        if self.documents.is_empty() {
            return Vec::new();
        }

        // Tokenize query
        let query_tokens = self.tokenizer.tokenize_to_strings(query);
        if query_tokens.is_empty() {
            return Vec::new();
        }

        // Build candidate documents
        let mut doc_scores: HashMap<
            String,
            (
                f64,
                Vec<String>,
                HashMap<String, u32>,
                HashMap<String, Vec<u32>>,
            ),
        > = HashMap::new();

        let query_terms: HashSet<_> = query_tokens.iter().cloned().collect();
        let n_query_terms = query_terms.len();

        for term in &query_terms {
            if let Some(posting_list) = self.inverted_index.get(term) {
                let term_boost = options.term_boosts.get(term).copied().unwrap_or(1.0);

                for posting in &posting_list.postings {
                    let doc_meta = match self.documents.get(&posting.doc_id) {
                        Some(meta) => meta,
                        None => continue,
                    };

                    let term_score = self.scorer.term_score(
                        posting.term_frequency,
                        posting_list.doc_frequency,
                        doc_meta.token_count,
                    ) * term_boost;

                    let entry = doc_scores
                        .entry(posting.doc_id.clone())
                        .or_insert_with(|| (0.0, Vec::new(), HashMap::new(), HashMap::new()));

                    entry.0 += term_score;
                    entry.1.push(term.clone());
                    entry.2.insert(term.clone(), posting.term_frequency);
                    if options.include_highlights {
                        entry.3.insert(term.clone(), posting.positions.clone());
                    }
                }
            }
        }

        // Filter and collect results
        let mut results: Vec<FulltextSearchResult> = doc_scores
            .into_iter()
            .filter(|(_, (score, matched_terms, _, _))| {
                // Filter by minimum score
                if *score < options.min_score {
                    return false;
                }

                // Filter by require_all_terms
                if options.require_all_terms && matched_terms.len() < n_query_terms {
                    return false;
                }

                true
            })
            .map(
                |(doc_id, (score, matched_terms, term_frequencies, highlight_positions))| {
                    FulltextSearchResult {
                        doc_id,
                        score,
                        matched_terms,
                        term_frequencies,
                        highlight_positions,
                    }
                },
            )
            .collect();

        // Sort by score descending
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Limit results
        results.truncate(options.limit);

        results
    }

    /// Get document frequency for a term
    pub fn get_document_frequency(&self, term: &str) -> u32 {
        self.inverted_index
            .get(term)
            .map_or(0, |pl| pl.doc_frequency)
    }

    /// Get term frequency for a term in a specific document
    pub fn get_term_frequency(&self, term: &str, doc_id: &str) -> u32 {
        self.inverted_index
            .get(term)
            .and_then(|pl| pl.get_posting(doc_id))
            .map_or(0, |p| p.term_frequency)
    }

    /// Get IDF for a term
    pub fn get_idf(&self, term: &str) -> f64 {
        let doc_freq = self.get_document_frequency(term);
        self.scorer.idf(doc_freq)
    }

    /// Get document metadata
    pub fn get_document_metadata(&self, doc_id: &str) -> Option<&DocumentMetadata> {
        self.documents.get(doc_id)
    }

    /// Get top terms by document frequency
    pub fn get_top_terms(&self, limit: usize) -> Vec<(String, u32)> {
        let mut terms: Vec<_> = self
            .inverted_index
            .iter()
            .map(|(term, pl)| (term.clone(), pl.doc_frequency))
            .collect();

        terms.sort_by_key(|t| std::cmp::Reverse(t.1));
        terms.truncate(limit);
        terms
    }

    /// Get terms matching a prefix (for autocomplete)
    pub fn get_terms_with_prefix(&self, prefix: &str, limit: usize) -> Vec<String> {
        self.inverted_index
            .range(prefix.to_string()..)
            .take_while(|(term, _)| term.starts_with(prefix))
            .take(limit)
            .map(|(term, _)| term.clone())
            .collect()
    }

    /// Clear the entire index
    pub fn clear(&mut self) -> Result<(), FullTextIndexError> {
        if self.read_only {
            return Err(FullTextIndexError::ReadOnlyIndex);
        }

        self.inverted_index.clear();
        self.documents.clear();
        self.statistics = TextStatistics::default();
        self.scorer.update_stats(0.0, 0);

        Ok(())
    }

    /// Merge another index into this one
    pub fn merge(&mut self, other: FullTextIndex) -> Result<(), FullTextIndexError> {
        if self.read_only {
            return Err(FullTextIndexError::ReadOnlyIndex);
        }

        // Merge documents
        for (doc_id, meta) in other.documents {
            self.documents.entry(doc_id).or_insert(meta);
        }

        // Merge inverted index
        for (term, other_pl) in other.inverted_index {
            let pl = self.inverted_index.entry(term).or_default();

            for posting in other_pl.postings {
                if pl.get_posting(&posting.doc_id).is_none() {
                    pl.postings.push(posting.clone());
                    pl.doc_frequency += 1;
                    pl.total_frequency += posting.term_frequency as u64;
                }
            }
        }

        // Recompute statistics
        self.recompute_statistics();

        Ok(())
    }

    /// Recompute statistics from scratch
    fn recompute_statistics(&mut self) {
        self.statistics = TextStatistics::default();
        for meta in self.documents.values() {
            self.statistics.update_after_add(meta.token_count);
        }
        self.statistics
            .set_unique_terms(self.inverted_index.len() as u64);
        self.scorer.update_stats(
            self.statistics.avg_document_length,
            self.statistics.total_documents,
        );
    }
}

// =============================================================================
// Index Builder for Batch Operations
// =============================================================================

/// Builder for creating full-text indices efficiently
pub struct FullTextIndexBuilder {
    tokenizer_config: TokenizerConfig,
    bm25_config: BM25Config,
    /// Buffer of pending documents
    pending_docs: Vec<(String, String, HashMap<String, String>)>,
    /// Maximum documents to buffer before building
    batch_size: usize,
}

impl FullTextIndexBuilder {
    /// Create a new index builder
    pub fn new() -> Self {
        Self {
            tokenizer_config: TokenizerConfig::default(),
            bm25_config: BM25Config::default(),
            pending_docs: Vec::new(),
            batch_size: 10000,
        }
    }

    /// Set tokenizer configuration
    pub fn with_tokenizer(mut self, config: TokenizerConfig) -> Self {
        self.tokenizer_config = config;
        self
    }

    /// Set BM25 configuration
    pub fn with_bm25(mut self, config: BM25Config) -> Self {
        self.bm25_config = config;
        self
    }

    /// Set batch size
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    /// Add a document to be indexed
    pub fn add_document(&mut self, doc_id: String, text: String) {
        self.pending_docs.push((doc_id, text, HashMap::new()));
    }

    /// Add a document with metadata
    pub fn add_document_with_metadata(
        &mut self,
        doc_id: String,
        text: String,
        metadata: HashMap<String, String>,
    ) {
        self.pending_docs.push((doc_id, text, metadata));
    }

    /// Build the index from all pending documents
    pub fn build(self) -> Result<FullTextIndex, FullTextIndexError> {
        let mut index =
            FullTextIndex::new(self.tokenizer_config).with_bm25_config(self.bm25_config);

        for (doc_id, text, metadata) in self.pending_docs {
            if metadata.is_empty() {
                index.add_document(&doc_id, &text)?;
            } else {
                index.add_document_with_metadata(&doc_id, &text, metadata)?;
            }
        }

        Ok(index)
    }
}

impl Default for FullTextIndexBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Integration with ChunkingConfig
// =============================================================================

/// Extension trait for integrating with RAG chunking
pub trait ChunkIndexing {
    /// Index chunks from a document
    fn index_chunks(
        &mut self,
        parent_doc_id: &str,
        chunks: &[super::text_storage::TextChunk],
    ) -> Result<(), FullTextIndexError>;

    /// Search across chunks
    fn search_chunks(&self, query: &str, limit: usize) -> Vec<ChunkSearchResult>;
}

/// Search result specific to chunk-level search
#[derive(Debug, Clone)]
pub struct ChunkSearchResult {
    /// Chunk ID
    pub chunk_id: String,
    /// Parent document ID
    pub parent_doc_id: String,
    /// Chunk index within parent
    pub chunk_index: u32,
    /// Relevance score
    pub score: f64,
    /// Matched terms
    pub matched_terms: Vec<String>,
}

impl ChunkIndexing for FullTextIndex {
    fn index_chunks(
        &mut self,
        parent_doc_id: &str,
        chunks: &[super::text_storage::TextChunk],
    ) -> Result<(), FullTextIndexError> {
        for chunk in chunks {
            let mut metadata = HashMap::new();
            metadata.insert("parent_doc_id".to_string(), parent_doc_id.to_string());
            metadata.insert("chunk_index".to_string(), chunk.chunk_index.to_string());
            metadata.insert("start_offset".to_string(), chunk.start_offset.to_string());
            metadata.insert("end_offset".to_string(), chunk.end_offset.to_string());

            self.add_document_with_metadata(&chunk.chunk_id, &chunk.content, metadata)?;
        }
        Ok(())
    }

    fn search_chunks(&self, query: &str, limit: usize) -> Vec<ChunkSearchResult> {
        let results = self.search(query, limit);

        results
            .into_iter()
            .filter_map(|result| {
                let meta = self.get_document_metadata(&result.doc_id)?;
                let parent_doc_id = meta.metadata.get("parent_doc_id")?.clone();
                let chunk_index = meta.metadata.get("chunk_index")?.parse().ok()?;

                Some(ChunkSearchResult {
                    chunk_id: result.doc_id,
                    parent_doc_id,
                    chunk_index,
                    score: result.score,
                    matched_terms: result.matched_terms,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenizer_standard() {
        let tokenizer = Tokenizer::new(TokenizerConfig::default());
        let tokens = tokenizer.tokenize("Hello, World! This is a test.");

        // Should have lowercase tokens, no punctuation
        let texts: Vec<_> = tokens.iter().map(|t| t.text.as_str()).collect();
        assert!(texts.contains(&"hello"));
        assert!(texts.contains(&"world"));
        assert!(texts.contains(&"test"));
        // Stop words should be filtered
        assert!(!texts.contains(&"this"));
        assert!(!texts.contains(&"is"));
        assert!(!texts.contains(&"a"));
    }

    #[test]
    fn test_tokenizer_whitespace() {
        let tokenizer = Tokenizer::new(TokenizerConfig::new(TokenizerType::Whitespace));
        let tokens = tokenizer.tokenize("Hello World");

        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].text, "hello");
        assert_eq!(tokens[1].text, "world");
    }

    #[test]
    fn test_tokenizer_ngram() {
        let config = TokenizerConfig::new(TokenizerType::Ngram)
            .with_ngram_range(2, 3)
            .without_stop_words();
        let tokenizer = Tokenizer::new(config);
        let tokens = tokenizer.tokenize("test");

        // Should have: te, es, st, tes, est
        let texts: Vec<_> = tokens.iter().map(|t| t.text.as_str()).collect();
        assert!(texts.contains(&"te"));
        assert!(texts.contains(&"es"));
        assert!(texts.contains(&"st"));
        assert!(texts.contains(&"tes"));
        assert!(texts.contains(&"est"));
    }

    #[test]
    fn test_fulltext_index_basic() {
        let mut index = FullTextIndex::new(TokenizerConfig::default());

        index
            .add_document("doc1", "The quick brown fox")
            .expect("Failed to add doc1");
        index
            .add_document("doc2", "A lazy brown dog")
            .expect("Failed to add doc2");
        index
            .add_document("doc3", "The quick blue bird")
            .expect("Failed to add doc3");

        assert_eq!(index.document_count(), 3);
        assert!(index.term_count() > 0);
    }

    #[test]
    fn test_fulltext_search() {
        let mut index = FullTextIndex::new(TokenizerConfig::default());

        index
            .add_document("doc1", "The quick brown fox jumps over")
            .expect("Failed to add doc1");
        index
            .add_document("doc2", "A lazy brown dog sleeps")
            .expect("Failed to add doc2");
        index
            .add_document("doc3", "The quick blue bird flies")
            .expect("Failed to add doc3");

        let results = index.search("quick brown", 10);

        assert!(!results.is_empty());
        // doc1 should rank highest (has both "quick" and "brown")
        assert_eq!(results[0].doc_id, "doc1");
    }

    #[test]
    fn test_bm25_scoring() {
        let scorer = BM25Scorer::new(BM25Config::default(), 100.0, 1000);

        let idf = scorer.idf(100); // Term in 100 of 1000 docs
        assert!(idf > 0.0);

        let score = scorer.term_score(5, 100, 150);
        assert!(score > 0.0);
    }

    #[test]
    fn test_remove_document() {
        let mut index = FullTextIndex::new(TokenizerConfig::default());

        index
            .add_document("doc1", "Hello world")
            .expect("Failed to add doc1");
        index
            .add_document("doc2", "Hello there")
            .expect("Failed to add doc2");

        assert_eq!(index.document_count(), 2);

        index
            .remove_document("doc1")
            .expect("Failed to remove doc1");
        assert_eq!(index.document_count(), 1);
        assert!(!index.contains_document("doc1"));
        assert!(index.contains_document("doc2"));
    }

    #[test]
    fn test_index_builder() {
        let mut builder =
            FullTextIndexBuilder::new().with_tokenizer(TokenizerConfig::for_keyword_search());

        builder.add_document("doc1".to_string(), "First document text".to_string());
        builder.add_document("doc2".to_string(), "Second document text".to_string());

        let index = builder.build().expect("Failed to build index");
        assert_eq!(index.document_count(), 2);
    }

    #[test]
    fn test_document_frequency() {
        let mut index = FullTextIndex::new(TokenizerConfig::default());

        index
            .add_document("doc1", "hello world")
            .expect("Failed to add doc1");
        index
            .add_document("doc2", "hello there")
            .expect("Failed to add doc2");
        index
            .add_document("doc3", "goodbye world")
            .expect("Failed to add doc3");

        let hello_df = index.get_document_frequency("hello");
        assert_eq!(hello_df, 2);

        let world_df = index.get_document_frequency("world");
        assert_eq!(world_df, 2);

        let goodbye_df = index.get_document_frequency("goodbye");
        assert_eq!(goodbye_df, 1);
    }

    #[test]
    fn test_search_with_options() {
        let mut index = FullTextIndex::new(TokenizerConfig::default());

        index
            .add_document("doc1", "quick brown fox")
            .expect("Failed to add doc1");
        index
            .add_document("doc2", "quick")
            .expect("Failed to add doc2");
        index
            .add_document("doc3", "slow brown tortoise")
            .expect("Failed to add doc3");

        // Require all terms
        let results =
            index.search_with_options("quick brown", SearchOptions::top_k(10).require_all());

        // Only doc1 has both terms
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_id, "doc1");
    }

    #[test]
    fn test_prefix_terms() {
        let mut index = FullTextIndex::new(TokenizerConfig::default());

        index
            .add_document("doc1", "testing tested tester")
            .expect("Failed to add doc1");

        let terms = index.get_terms_with_prefix("test", 10);
        assert!(!terms.is_empty());
        for term in terms {
            assert!(term.starts_with("test"));
        }
    }

    #[test]
    fn test_statistics() {
        let mut index = FullTextIndex::new(TokenizerConfig::default());

        index
            .add_document("doc1", "one two three")
            .expect("Failed to add doc1");
        index
            .add_document("doc2", "four five six seven eight")
            .expect("Failed to add doc2");

        let stats = index.statistics();
        assert_eq!(stats.total_documents, 2);
        assert!(stats.avg_document_length > 0.0);
        assert!(stats.unique_terms > 0);
    }
}
