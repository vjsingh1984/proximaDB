// Document-specific compression
//
// JSON documents often have repetitive structures that benefit from
// specialized compression techniques:
// - Dictionary encoding for repeated string values
// - Schema-based compression for consistent document structures
// - Delta encoding for similar documents

use std::collections::HashMap;

use anyhow::Result;

/// Document compressor for JSON documents
pub struct DocumentCompressor {
    /// String dictionary for repeated values
    string_dictionary: HashMap<String, u32>,
    /// Next dictionary ID
    next_dict_id: u32,
    /// Minimum string length for dictionary encoding
    min_dict_length: usize,
}

impl DocumentCompressor {
    /// Create a new document compressor
    pub fn new() -> Self {
        Self {
            string_dictionary: HashMap::new(),
            next_dict_id: 0,
            min_dict_length: 4,
        }
    }

    /// Compress a batch of documents
    pub fn compress(&mut self, documents: &[Vec<u8>]) -> Result<CompressedBatch> {
        // Step 1: Build dictionary from common strings
        self.build_dictionary(documents);

        // Step 2: Apply dictionary encoding
        let encoded: Vec<Vec<u8>> = documents
            .iter()
            .map(|doc| self.dictionary_encode(doc))
            .collect();

        // Step 3: Apply general compression (LZ4 or Zstd)
        let compressed = self.apply_compression(&encoded)?;

        Ok(CompressedBatch {
            dictionary: self.string_dictionary.clone(),
            data: compressed,
            document_count: documents.len(),
        })
    }

    /// Decompress a batch of documents
    pub fn decompress(&self, batch: &CompressedBatch) -> Result<Vec<Vec<u8>>> {
        // Step 1: Decompress
        let encoded = self.apply_decompression(&batch.data, batch.document_count)?;

        // Step 2: Decode dictionary references
        let decoded: Vec<Vec<u8>> = encoded
            .iter()
            .map(|doc| self.dictionary_decode(doc, &batch.dictionary))
            .collect();

        Ok(decoded)
    }

    /// Build string dictionary from documents
    fn build_dictionary(&mut self, documents: &[Vec<u8>]) {
        let mut string_counts: HashMap<String, usize> = HashMap::new();

        for doc in documents {
            // Simple string extraction - look for quoted strings
            self.extract_strings(doc, &mut string_counts);
        }

        // Add frequently occurring strings to dictionary
        for (s, count) in string_counts {
            if count >= 2
                && s.len() >= self.min_dict_length
                && !self.string_dictionary.contains_key(&s)
            {
                self.string_dictionary.insert(s, self.next_dict_id);
                self.next_dict_id += 1;
            }
        }
    }

    /// Extract strings from a JSON document
    fn extract_strings(&self, doc: &[u8], counts: &mut HashMap<String, usize>) {
        // Simple extraction - look for quoted strings
        let mut i = 0;
        while i < doc.len() {
            if doc[i] == b'"' {
                i += 1;
                let start = i;
                while i < doc.len() && doc[i] != b'"' {
                    if doc[i] == b'\\' && i + 1 < doc.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                if i > start
                    && let Ok(s) = std::str::from_utf8(&doc[start..i])
                {
                    *counts.entry(s.to_string()).or_insert(0) += 1;
                }
            }
            i += 1;
        }
    }

    /// Dictionary encoding marker byte (0xFF followed by 4-byte dict ID)
    const DICT_MARKER: u8 = 0xFF;

    /// Apply dictionary encoding to a document
    ///
    /// Scans for quoted strings that appear in the dictionary and replaces them
    /// with a compact marker + dictionary ID reference.
    fn dictionary_encode(&self, doc: &[u8]) -> Vec<u8> {
        if self.string_dictionary.is_empty() {
            return doc.to_vec();
        }

        let mut result = Vec::with_capacity(doc.len());
        let mut i = 0;

        while i < doc.len() {
            if doc[i] == b'"' {
                // Try to extract the quoted string and look it up in the dictionary
                let start = i + 1;
                let mut end = start;
                while end < doc.len() && doc[end] != b'"' {
                    if doc[end] == b'\\' && end + 1 < doc.len() {
                        end += 2;
                    } else {
                        end += 1;
                    }
                }
                if end < doc.len()
                    && let Ok(s) = std::str::from_utf8(&doc[start..end])
                    && let Some(&dict_id) = self.string_dictionary.get(s)
                {
                    // Replace with marker + dict ID
                    result.push(Self::DICT_MARKER);
                    result.extend_from_slice(&dict_id.to_le_bytes());
                    i = end + 1; // skip past closing quote
                    continue;
                }
                // No dictionary hit — copy the character and continue
                result.push(doc[i]);
                i += 1;
            } else {
                result.push(doc[i]);
                i += 1;
            }
        }

        result
    }

    /// Decode dictionary references back to original strings
    fn dictionary_decode(&self, doc: &[u8], dict: &HashMap<String, u32>) -> Vec<u8> {
        if dict.is_empty() {
            return doc.to_vec();
        }

        // Build reverse dictionary (id -> string)
        let reverse: HashMap<u32, &str> = dict.iter().map(|(s, id)| (*id, s.as_str())).collect();

        let mut result = Vec::with_capacity(doc.len());
        let mut i = 0;

        while i < doc.len() {
            if doc[i] == Self::DICT_MARKER && i + 4 < doc.len() {
                let dict_id = u32::from_le_bytes([doc[i + 1], doc[i + 2], doc[i + 3], doc[i + 4]]);
                if let Some(s) = reverse.get(&dict_id) {
                    // Restore the original quoted string
                    result.push(b'"');
                    result.extend_from_slice(s.as_bytes());
                    result.push(b'"');
                    i += 5; // marker + 4 bytes
                    continue;
                }
            }
            result.push(doc[i]);
            i += 1;
        }

        result
    }

    /// Apply LZ4 compression to length-prefixed concatenated documents
    fn apply_compression(&self, documents: &[Vec<u8>]) -> Result<Vec<u8>> {
        // Concatenate documents with length prefixes
        let mut combined = Vec::new();
        for doc in documents {
            combined.extend_from_slice(&(doc.len() as u32).to_le_bytes());
            combined.extend_from_slice(doc);
        }

        // Apply LZ4 block compression
        let compressed = lz4_flex::compress_prepend_size(&combined);
        Ok(compressed)
    }

    /// Decompress LZ4 data and split into individual documents
    fn apply_decompression(&self, data: &[u8], count: usize) -> Result<Vec<Vec<u8>>> {
        // Decompress LZ4 block
        let decompressed = lz4_flex::decompress_size_prepended(data)
            .map_err(|e| anyhow::anyhow!("LZ4 decompression failed: {}", e))?;

        // Split into individual documents using length prefixes
        let mut documents = Vec::with_capacity(count);
        let mut i = 0;

        while i + 4 <= decompressed.len() && documents.len() < count {
            let len = u32::from_le_bytes([
                decompressed[i],
                decompressed[i + 1],
                decompressed[i + 2],
                decompressed[i + 3],
            ]) as usize;
            i += 4;

            if i + len <= decompressed.len() {
                documents.push(decompressed[i..i + len].to_vec());
                i += len;
            } else {
                break;
            }
        }

        Ok(documents)
    }
}

impl Default for DocumentCompressor {
    fn default() -> Self {
        Self::new()
    }
}

/// Compressed batch of documents
pub struct CompressedBatch {
    /// String dictionary
    pub dictionary: HashMap<String, u32>,
    /// Compressed data
    pub data: Vec<u8>,
    /// Number of documents
    pub document_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compressor_roundtrip() {
        let mut compressor = DocumentCompressor::new();

        let documents = vec![
            br#"{"name":"Alice","age":30}"#.to_vec(),
            br#"{"name":"Bob","age":25}"#.to_vec(),
            br#"{"name":"Charlie","age":35}"#.to_vec(),
        ];

        let compressed = compressor.compress(&documents).unwrap();
        let decompressed = compressor.decompress(&compressed).unwrap();

        assert_eq!(documents.len(), decompressed.len());
        for (original, recovered) in documents.iter().zip(decompressed.iter()) {
            assert_eq!(original, recovered);
        }
    }

    #[test]
    fn test_lz4_compression_reduces_size() {
        let mut compressor = DocumentCompressor::new();

        // Create highly compressible data (repeated JSON structures)
        let documents: Vec<Vec<u8>> = (0..100)
            .map(|i| format!(r#"{{"user_id":{},"status":"active","role":"engineer","department":"engineering"}}"#, i).into_bytes())
            .collect();

        let compressed = compressor.compress(&documents).unwrap();
        let total_raw: usize = documents.iter().map(|d| d.len()).sum();

        // LZ4 should compress repeated structures significantly
        assert!(
            compressed.data.len() < total_raw,
            "compressed {} should be smaller than raw {}",
            compressed.data.len(),
            total_raw
        );

        // Verify roundtrip
        let decompressed = compressor.decompress(&compressed).unwrap();
        assert_eq!(documents, decompressed);
    }

    #[test]
    fn test_dictionary_encoding_roundtrip() {
        let mut compressor = DocumentCompressor::new();

        // Documents with repeated string values trigger dictionary building
        let documents = vec![
            br#"{"department":"engineering","status":"active"}"#.to_vec(),
            br#"{"department":"engineering","status":"active"}"#.to_vec(),
            br#"{"department":"engineering","status":"inactive"}"#.to_vec(),
        ];

        let compressed = compressor.compress(&documents).unwrap();
        assert!(
            !compressed.dictionary.is_empty(),
            "dictionary should be non-empty"
        );

        let decompressed = compressor.decompress(&compressed).unwrap();
        assert_eq!(documents, decompressed);
    }

    #[test]
    fn test_empty_documents() {
        let mut compressor = DocumentCompressor::new();
        let documents: Vec<Vec<u8>> = vec![];

        let compressed = compressor.compress(&documents).unwrap();
        let decompressed = compressor.decompress(&compressed).unwrap();

        assert!(decompressed.is_empty());
    }

    #[test]
    fn test_single_document() {
        let mut compressor = DocumentCompressor::new();
        let documents = vec![br#"{"hello":"world"}"#.to_vec()];

        let compressed = compressor.compress(&documents).unwrap();
        let decompressed = compressor.decompress(&compressed).unwrap();

        assert_eq!(documents, decompressed);
    }
}
