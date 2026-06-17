//! Server-side chunking. Splits a raw document into chunks ready for
//! embedding. Strategy is configured per-collection via [`ChunkConfig`].
//!
//! Phase 1 implements: FixedWindow, SlidingWindow, Paragraph. Heading-aware
//! splitting lands in Phase 3 alongside the SDK simplification.

use crate::Result;
use crate::config::{ChunkConfig, ChunkStrategy};
use crate::tokenizer::SharedTokenizer;

/// A single chunk produced by the chunker. The `id` is derived from the
/// original document id with the chunk index appended (`{doc_id}:chunk:{n}`).
#[derive(Debug, Clone)]
pub struct Chunk {
    pub id: String,
    pub text: String,
    pub chunk_index: usize,
}

pub struct Chunker<'a> {
    tokenizer: &'a SharedTokenizer,
    config: &'a ChunkConfig,
}

impl<'a> Chunker<'a> {
    pub fn new(tokenizer: &'a SharedTokenizer, config: &'a ChunkConfig) -> Self {
        Self { tokenizer, config }
    }

    pub fn chunk(&self, doc_id: &str, text: &str) -> Result<Vec<Chunk>> {
        match self.config.strategy {
            ChunkStrategy::FixedWindow => self.fixed_window(doc_id, text),
            ChunkStrategy::SlidingWindow => self.sliding_window(doc_id, text),
            ChunkStrategy::Paragraph => self.paragraph(doc_id, text),
            ChunkStrategy::Heading => self.paragraph(doc_id, text), // fallback in Phase 1
        }
    }

    fn fixed_window(&self, doc_id: &str, text: &str) -> Result<Vec<Chunk>> {
        let target = self.config.size_tokens.max(16);
        // Approximate by character window first; refine with token count for
        // boundaries. ~4 chars per token is the conservative ratio.
        let char_window = target * 4;
        let mut chunks = Vec::new();
        let mut idx = 0;
        let mut pos = 0;
        let chars: Vec<char> = text.chars().collect();
        while pos < chars.len() {
            let end = (pos + char_window).min(chars.len());
            let body: String = chars[pos..end].iter().collect();
            chunks.push(Chunk {
                id: format!("{}:chunk:{}", doc_id, idx),
                text: body,
                chunk_index: idx,
            });
            pos = end;
            idx += 1;
        }
        Ok(chunks)
    }

    fn sliding_window(&self, doc_id: &str, text: &str) -> Result<Vec<Chunk>> {
        let target = self.config.size_tokens.max(16);
        let char_window = target * 4;
        let overlap_chars = (char_window as f32 * self.config.overlap_pct) as usize;
        let stride = char_window.saturating_sub(overlap_chars).max(1);

        let mut chunks = Vec::new();
        let mut idx = 0;
        let mut pos = 0;
        let chars: Vec<char> = text.chars().collect();
        while pos < chars.len() {
            let end = (pos + char_window).min(chars.len());
            let body: String = chars[pos..end].iter().collect();
            chunks.push(Chunk {
                id: format!("{}:chunk:{}", doc_id, idx),
                text: body,
                chunk_index: idx,
            });
            if end == chars.len() {
                break;
            }
            pos += stride;
            idx += 1;
        }
        Ok(chunks)
    }

    fn paragraph(&self, doc_id: &str, text: &str) -> Result<Vec<Chunk>> {
        let cap_tokens = self.config.size_tokens.max(16);
        let mut current = String::new();
        let mut current_tokens = 0;
        let mut chunks = Vec::new();
        let mut idx = 0;

        for paragraph in text.split("\n\n") {
            let p = paragraph.trim();
            if p.is_empty() {
                continue;
            }
            let p_tokens = self.tokenizer.count_tokens(p).unwrap_or(p.len() / 4);
            if current_tokens + p_tokens > cap_tokens && !current.is_empty() {
                // Emit current accumulator.
                chunks.push(Chunk {
                    id: format!("{}:chunk:{}", doc_id, idx),
                    text: std::mem::take(&mut current),
                    chunk_index: idx,
                });
                idx += 1;
                current_tokens = 0;
            }
            if !current.is_empty() {
                current.push_str("\n\n");
            }
            current.push_str(p);
            current_tokens += p_tokens;
        }
        if !current.is_empty() {
            chunks.push(Chunk {
                id: format!("{}:chunk:{}", doc_id, idx),
                text: current,
                chunk_index: idx,
            });
        }
        Ok(chunks)
    }
}
