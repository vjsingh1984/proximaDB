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
            if count >= 2 && s.len() >= self.min_dict_length {
                if !self.string_dictionary.contains_key(&s) {
                    self.string_dictionary.insert(s, self.next_dict_id);
                    self.next_dict_id += 1;
                }
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
                if i > start {
                    if let Ok(s) = std::str::from_utf8(&doc[start..i]) {
                        *counts.entry(s.to_string()).or_insert(0) += 1;
                    }
                }
            }
            i += 1;
        }
    }

    /// Apply dictionary encoding to a document
    fn dictionary_encode(&self, _doc: &[u8]) -> Vec<u8> {
        // TODO: Implement dictionary encoding
        // For now, return original
        _doc.to_vec()
    }

    /// Decode dictionary references
    fn dictionary_decode(&self, _doc: &[u8], _dict: &HashMap<String, u32>) -> Vec<u8> {
        // TODO: Implement dictionary decoding
        _doc.to_vec()
    }

    /// Apply compression to encoded documents
    fn apply_compression(&self, documents: &[Vec<u8>]) -> Result<Vec<u8>> {
        // Concatenate documents with length prefixes
        let mut combined = Vec::new();
        for doc in documents {
            combined.extend_from_slice(&(doc.len() as u32).to_le_bytes());
            combined.extend_from_slice(doc);
        }

        // Apply LZ4 compression
        // TODO: Use lz4 crate for actual compression
        Ok(combined)
    }

    /// Decompress documents
    fn apply_decompression(&self, data: &[u8], count: usize) -> Result<Vec<Vec<u8>>> {
        // TODO: Apply LZ4 decompression first
        let decompressed = data;

        // Split into individual documents
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
}
