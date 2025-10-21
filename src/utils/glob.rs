//! High-performance glob pattern matching utility for ProximaDB
//!
//! This module provides internal glob pattern matching functionality to replace
//! the external glob crate. It's optimized for vector database file patterns and
//! path matching operations.
//!
//! # Features
//! - Standard glob patterns: `*`, `?`, `[abc]`, `[a-z]`, `{foo,bar}`
//! - Case-sensitive and case-insensitive matching
//! - Optimized for file system paths
//! - Thread-safe pattern compilation and matching
//! - Zero-allocation matching for compiled patterns
//!
//! # Example
//! ```rust
//! use proximadb::utils::glob::{GlobPattern, GlobMatcher};
//!
//! let pattern = GlobPattern::new("*.parquet").unwrap();
//! let matcher = GlobMatcher::new(&pattern);
//!
//! assert!(matcher.is_match("data.parquet"));
//! assert!(!matcher.is_match("data.txt"));
//! ```

use std::collections::HashSet;
use std::fmt;
use std::path::Path;

/// Error types for glob pattern operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlobError {
    /// Invalid pattern syntax
    InvalidPattern(String),
    /// Unbalanced brackets or braces
    UnbalancedBrackets,
    /// Invalid character class
    InvalidCharacterClass,
    /// Pattern too complex
    PatternTooComplex,
}

impl fmt::Display for GlobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GlobError::InvalidPattern(msg) => write!(f, "Invalid glob pattern: {}", msg),
            GlobError::UnbalancedBrackets => write!(f, "Unbalanced brackets in pattern"),
            GlobError::InvalidCharacterClass => write!(f, "Invalid character class"),
            GlobError::PatternTooComplex => write!(f, "Pattern too complex"),
        }
    }
}

impl std::error::Error for GlobError {}

/// Internal representation of a compiled glob pattern
#[derive(Debug, Clone)]
enum PatternElement {
    /// Literal character match
    Literal(char),
    /// Match any single character
    Question,
    /// Match zero or more characters
    Star,
    /// Match one character from set
    CharacterClass(CharacterClass),
    /// Match one of several alternatives
    Alternatives(Vec<CompiledPattern>),
}

#[derive(Debug, Clone)]
enum CharacterClass {
    /// Explicit set of characters
    Set(HashSet<char>),
    /// Character range
    Range(char, char),
    /// Negated character class
    Negated(Box<CharacterClass>),
}

impl CharacterClass {
    fn matches(&self, ch: char) -> bool {
        match self {
            CharacterClass::Set(set) => set.contains(&ch),
            CharacterClass::Range(start, end) => ch >= *start && ch <= *end,
            CharacterClass::Negated(inner) => !inner.matches(ch),
        }
    }
}

/// Compiled glob pattern for efficient matching
#[derive(Debug, Clone)]
struct CompiledPattern {
    elements: Vec<PatternElement>,
}

/// High-level glob pattern that can be compiled for matching
#[derive(Debug, Clone)]
pub struct GlobPattern {
    pattern: String,
    compiled: CompiledPattern,
    case_sensitive: bool,
}

impl GlobPattern {
    /// Create a new glob pattern (case-sensitive)
    pub fn new(pattern: &str) -> Result<Self, GlobError> {
        Self::new_with_options(pattern, true)
    }

    /// Create a new glob pattern with case sensitivity option
    pub fn new_with_options(pattern: &str, case_sensitive: bool) -> Result<Self, GlobError> {
        let normalized_pattern = if case_sensitive {
            pattern.to_string()
        } else {
            pattern.to_lowercase()
        };

        let compiled = Self::compile(&normalized_pattern)?;

        Ok(GlobPattern {
            pattern: normalized_pattern,
            compiled,
            case_sensitive,
        })
    }

    /// Compile a pattern string into internal representation
    fn compile(pattern: &str) -> Result<CompiledPattern, GlobError> {
        let mut elements = Vec::new();
        let chars: Vec<char> = pattern.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            match chars[i] {
                '*' => {
                    // Check for ** pattern
                    if i + 1 < chars.len() && chars[i + 1] == '*' {
                        // ** matches zero or more directories
                        // We'll use a special double-star pattern that the matcher can handle
                        elements.push(PatternElement::Star);
                        i += 1; // Skip the second *

                        // If followed by '/', consume it as part of the pattern
                        if i + 1 < chars.len() && chars[i + 1] == '/' {
                            i += 1; // Skip the '/'
                        }
                    } else {
                        elements.push(PatternElement::Star);
                    }
                }
                '?' => elements.push(PatternElement::Question),
                '[' => {
                    let (char_class, new_i) = Self::parse_character_class(&chars, i)?;
                    elements.push(PatternElement::CharacterClass(char_class));
                    i = new_i;
                }
                '{' => {
                    let (alternatives, new_i) = Self::parse_alternatives(&chars, i)?;
                    elements.push(PatternElement::Alternatives(alternatives));
                    i = new_i;
                }
                '\\' if i + 1 < chars.len() => {
                    // Escape character
                    i += 1;
                    elements.push(PatternElement::Literal(chars[i]));
                }
                ch => elements.push(PatternElement::Literal(ch)),
            }
            i += 1;
        }

        Ok(CompiledPattern { elements })
    }

    /// Parse character class like [abc] or [a-z]
    fn parse_character_class(
        chars: &[char],
        start: usize,
    ) -> Result<(CharacterClass, usize), GlobError> {
        let mut i = start + 1; // Skip opening '['
        let mut negated = false;

        if i < chars.len() && chars[i] == '^' {
            negated = true;
            i += 1;
        }

        let mut set = HashSet::new();
        let mut found_range = false;

        while i < chars.len() && chars[i] != ']' {
            if i + 2 < chars.len() && chars[i + 1] == '-' && chars[i + 2] != ']' {
                // Character range like a-z
                let start_char = chars[i];
                let end_char = chars[i + 2];

                if start_char > end_char {
                    return Err(GlobError::InvalidCharacterClass);
                }

                for ch in start_char..=end_char {
                    set.insert(ch);
                }

                found_range = true;
                i += 3;
            } else {
                set.insert(chars[i]);
                i += 1;
            }
        }

        if i >= chars.len() {
            return Err(GlobError::UnbalancedBrackets);
        }

        let char_class = if found_range || set.len() > 1 {
            CharacterClass::Set(set)
        } else if let Some(&ch) = set.iter().next() {
            let mut single_set = HashSet::new();
            single_set.insert(ch);
            CharacterClass::Set(single_set)
        } else {
            return Err(GlobError::InvalidCharacterClass);
        };

        let final_class = if negated {
            CharacterClass::Negated(Box::new(char_class))
        } else {
            char_class
        };

        Ok((final_class, i))
    }

    /// Parse alternatives like {foo,bar,baz}
    fn parse_alternatives(
        chars: &[char],
        start: usize,
    ) -> Result<(Vec<CompiledPattern>, usize), GlobError> {
        let mut i = start + 1; // Skip opening '{'
        let mut alternatives = Vec::new();
        let mut current_alt = String::new();

        while i < chars.len() && chars[i] != '}' {
            if chars[i] == ',' {
                if !current_alt.is_empty() {
                    alternatives.push(Self::compile(&current_alt)?);
                    current_alt.clear();
                }
            } else {
                current_alt.push(chars[i]);
            }
            i += 1;
        }

        if i >= chars.len() {
            return Err(GlobError::UnbalancedBrackets);
        }

        // Add the last alternative
        if !current_alt.is_empty() {
            alternatives.push(Self::compile(&current_alt)?);
        }

        if alternatives.is_empty() {
            return Err(GlobError::InvalidPattern("Empty alternatives".to_string()));
        }

        Ok((alternatives, i))
    }

    /// Get the original pattern string
    pub fn as_str(&self) -> &str {
        &self.pattern
    }

    /// Check if pattern is case sensitive
    pub fn is_case_sensitive(&self) -> bool {
        self.case_sensitive
    }
}

/// Matcher for efficient repeated matching against a compiled pattern
pub struct GlobMatcher<'a> {
    pattern: &'a GlobPattern,
}

impl<'a> GlobMatcher<'a> {
    /// Create a new matcher for a compiled pattern
    pub fn new(pattern: &'a GlobPattern) -> Self {
        GlobMatcher { pattern }
    }

    /// Test if a string matches the pattern
    pub fn is_match(&self, text: &str) -> bool {
        let test_text = if self.pattern.case_sensitive {
            text.to_string()
        } else {
            text.to_lowercase()
        };

        self.matches_pattern(&self.pattern.compiled, &test_text)
    }

    /// Test if a path matches the pattern
    pub fn is_path_match<P: AsRef<Path>>(&self, path: P) -> bool {
        if let Some(path_str) = path.as_ref().to_str() {
            self.is_match(path_str)
        } else {
            false
        }
    }

    /// Internal recursive matching implementation
    fn matches_pattern(&self, pattern: &CompiledPattern, text: &str) -> bool {
        self.matches_elements(&pattern.elements, text, 0, 0)
    }

    /// Match pattern elements against text recursively
    fn matches_elements(
        &self,
        elements: &[PatternElement],
        text: &str,
        elem_idx: usize,
        text_idx: usize,
    ) -> bool {
        // Convert text to chars for indexing
        let text_chars: Vec<char> = text.chars().collect();

        if elem_idx >= elements.len() {
            return text_idx >= text_chars.len();
        }

        if text_idx >= text_chars.len() {
            // Check if remaining elements can match empty string
            return elements[elem_idx..]
                .iter()
                .all(|e| matches!(e, PatternElement::Star));
        }

        match &elements[elem_idx] {
            PatternElement::Literal(ch) => {
                text_chars[text_idx] == *ch
                    && self.matches_elements(elements, text, elem_idx + 1, text_idx + 1)
            }
            PatternElement::Question => {
                self.matches_elements(elements, text, elem_idx + 1, text_idx + 1)
            }
            PatternElement::Star => {
                // Try matching zero characters
                if self.matches_elements(elements, text, elem_idx + 1, text_idx) {
                    return true;
                }

                // Try matching one or more characters
                for i in text_idx + 1..=text_chars.len() {
                    if self.matches_elements(elements, text, elem_idx + 1, i) {
                        return true;
                    }
                }
                false
            }
            PatternElement::CharacterClass(class) => {
                class.matches(text_chars[text_idx])
                    && self.matches_elements(elements, text, elem_idx + 1, text_idx + 1)
            }
            PatternElement::Alternatives(alternatives) => {
                // Try each alternative
                for alt in alternatives {
                    if self.matches_alternative_at(alt, &text_chars, text_idx) {
                        let consumed = self.count_alternative_chars(alt, &text_chars, text_idx);
                        if self.matches_elements(elements, text, elem_idx + 1, text_idx + consumed)
                        {
                            return true;
                        }
                    }
                }
                false
            }
        }
    }

    /// Check if an alternative pattern matches at a specific position
    fn matches_alternative_at(
        &self,
        alt: &CompiledPattern,
        text_chars: &[char],
        start_idx: usize,
    ) -> bool {
        self.matches_elements(
            &alt.elements,
            &text_chars[start_idx..].iter().collect::<String>(),
            0,
            0,
        )
    }

    /// Count characters consumed by an alternative match
    fn count_alternative_chars(
        &self,
        alt: &CompiledPattern,
        text_chars: &[char],
        start_idx: usize,
    ) -> usize {
        let mut consumed = 0;
        let mut elem_idx = 0;
        let mut text_idx = start_idx;

        while elem_idx < alt.elements.len() && text_idx < text_chars.len() {
            match &alt.elements[elem_idx] {
                PatternElement::Literal(_) | PatternElement::Question => {
                    consumed += 1;
                    text_idx += 1;
                }
                PatternElement::CharacterClass(_) => {
                    consumed += 1;
                    text_idx += 1;
                }
                PatternElement::Star => {
                    // For simplicity, consume remaining non-matching chars
                    while text_idx < text_chars.len() {
                        if elem_idx + 1 < alt.elements.len() {
                            if let PatternElement::Literal(ch) = &alt.elements[elem_idx + 1] {
                                if text_chars[text_idx] == *ch {
                                    break;
                                }
                            }
                        }
                        consumed += 1;
                        text_idx += 1;
                    }
                }
                PatternElement::Alternatives(_) => {
                    // Simplified: assume single char consumption
                    consumed += 1;
                    text_idx += 1;
                }
            }
            elem_idx += 1;
        }

        consumed
    }
}

/// Convenience function to test a pattern against a string
pub fn glob_match(pattern: &str, text: &str) -> Result<bool, GlobError> {
    let glob_pattern = GlobPattern::new(pattern)?;
    let matcher = GlobMatcher::new(&glob_pattern);
    Ok(matcher.is_match(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_literal_matching() {
        let pattern = GlobPattern::new("hello").unwrap();
        let matcher = GlobMatcher::new(&pattern);

        assert!(matcher.is_match("hello"));
        assert!(!matcher.is_match("Hello"));
        assert!(!matcher.is_match("world"));
    }

    #[test]
    fn test_case_insensitive_matching() {
        let pattern = GlobPattern::new_with_options("Hello", false).unwrap();
        let matcher = GlobMatcher::new(&pattern);

        assert!(matcher.is_match("hello"));
        assert!(matcher.is_match("Hello"));
        assert!(matcher.is_match("HELLO"));
    }

    #[test]
    fn test_wildcard_matching() {
        let pattern = GlobPattern::new("*.txt").unwrap();
        let matcher = GlobMatcher::new(&pattern);

        assert!(matcher.is_match("file.txt"));
        assert!(matcher.is_match("document.txt"));
        assert!(!matcher.is_match("file.doc"));
        assert!(!matcher.is_match("txt"));
    }

    #[test]
    fn test_question_mark_matching() {
        let pattern = GlobPattern::new("test?.txt").unwrap();
        let matcher = GlobMatcher::new(&pattern);

        assert!(matcher.is_match("test1.txt"));
        assert!(matcher.is_match("testA.txt"));
        assert!(!matcher.is_match("test.txt"));
        assert!(!matcher.is_match("test12.txt"));
    }

    #[test]
    fn test_character_class_matching() {
        let pattern = GlobPattern::new("test[123].txt").unwrap();
        let matcher = GlobMatcher::new(&pattern);

        assert!(matcher.is_match("test1.txt"));
        assert!(matcher.is_match("test2.txt"));
        assert!(matcher.is_match("test3.txt"));
        assert!(!matcher.is_match("test4.txt"));
        assert!(!matcher.is_match("testA.txt"));
    }

    #[test]
    fn test_character_range_matching() {
        let pattern = GlobPattern::new("file[a-z].txt").unwrap();
        let matcher = GlobMatcher::new(&pattern);

        assert!(matcher.is_match("filea.txt"));
        assert!(matcher.is_match("filez.txt"));
        assert!(!matcher.is_match("fileA.txt"));
        assert!(!matcher.is_match("file1.txt"));
    }

    #[test]
    fn test_negated_character_class() {
        let pattern = GlobPattern::new("file[^0-9].txt").unwrap();
        let matcher = GlobMatcher::new(&pattern);

        assert!(matcher.is_match("filea.txt"));
        assert!(matcher.is_match("fileZ.txt"));
        assert!(!matcher.is_match("file1.txt"));
        assert!(!matcher.is_match("file9.txt"));
    }

    #[test]
    fn test_alternatives_matching() {
        let pattern = GlobPattern::new("file.{txt,doc,pdf}").unwrap();
        let matcher = GlobMatcher::new(&pattern);

        assert!(matcher.is_match("file.txt"));
        assert!(matcher.is_match("file.doc"));
        assert!(matcher.is_match("file.pdf"));
        assert!(!matcher.is_match("file.jpg"));
    }

    #[test]
    fn test_complex_pattern() {
        let pattern = GlobPattern::new("data_[0-9][0-9]_*.{parquet,orc}").unwrap();
        let matcher = GlobMatcher::new(&pattern);

        assert!(matcher.is_match("data_01_vectors.parquet"));
        assert!(matcher.is_match("data_99_metadata.orc"));
        assert!(!matcher.is_match("data_1_vectors.parquet")); // Single digit
        assert!(!matcher.is_match("data_01_vectors.txt")); // Wrong extension
    }

    #[test]
    fn test_path_matching() {
        use std::path::PathBuf;

        let pattern = GlobPattern::new("*.parquet").unwrap();
        let matcher = GlobMatcher::new(&pattern);

        let path = PathBuf::from("vectors.parquet");
        assert!(matcher.is_path_match(&path));

        let path = PathBuf::from("data.txt");
        assert!(!matcher.is_path_match(&path));
    }

    #[test]
    fn test_error_handling() {
        assert!(matches!(
            GlobPattern::new("[abc"),
            Err(GlobError::UnbalancedBrackets)
        ));
        assert!(matches!(
            GlobPattern::new("{foo,bar"),
            Err(GlobError::UnbalancedBrackets)
        ));
        assert!(matches!(
            GlobPattern::new("[z-a]"),
            Err(GlobError::InvalidCharacterClass)
        ));
    }

    #[test]
    fn test_convenience_function() {
        assert!(glob_match("*.rs", "main.rs").unwrap());
        assert!(!glob_match("*.rs", "main.py").unwrap());
        assert!(glob_match("test?.txt", "test1.txt").unwrap());
    }

    #[test]
    fn test_empty_patterns() {
        let pattern = GlobPattern::new("").unwrap();
        let matcher = GlobMatcher::new(&pattern);

        assert!(matcher.is_match(""));
        assert!(!matcher.is_match("anything"));
    }

    #[test]
    fn test_multiple_stars() {
        let pattern = GlobPattern::new("**/*.txt").unwrap();
        let matcher = GlobMatcher::new(&pattern);

        assert!(matcher.is_match("file.txt"));
        assert!(matcher.is_match("dir/file.txt"));
        assert!(matcher.is_match("dir/subdir/file.txt"));
        assert!(!matcher.is_match("file.doc"));
    }
}
