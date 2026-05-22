// Workload category classifier — heuristic inference for the
// per_category_policy lookup.
//
// `PerCategoryPolicy::lookup(category)` expects a category tag the
// caller supplies. The LLD §6.2 says this tag is "tenant-supplied", but
// most tenants won't bother configuring one — the gateway needs a
// default that's better than the safe-fallback "unknown" bucket.
//
// This module ships a heuristic classifier that infers the category
// from observable request shape:
//
//   - `code`         — query has heavy ASCII-punctuation / code-marker
//                      tokens (parens, braces, semicolons, underscores).
//   - `docs`         — query is long-form prose, mixed-case, low symbol
//                      density.
//   - `conversational` — short, low-symbol, often interrogative
//                        ("how do I", "what is").
//   - `specialized`  — query carries domain-specific tokens (jargon
//                      indicator: rare-character ratio).
//   - `volatile`     — explicit freshness hint = "strict" on the
//                      request, or "stock"/"price"/"news"/"latest"
//                      keyword presence.
//   - `unknown`      — nothing matched; the safe-fallback policy
//                      applies.
//
// Heuristics are intentionally simple — the goal is a default that
// outperforms "unknown" without needing per-tenant configuration. The
// LLD's planner v2 (Phase 7) will replace this with a learned head
// trained on `anvaiops_search_plan_traces` rows.

/// Bounded category set the classifier emits. Matches the PerCategoryPolicy
/// default table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Code,
    Docs,
    Conversational,
    Specialized,
    Volatile,
    Unknown,
}

impl Category {
    /// Bounded label — matches the `prom_label` field on
    /// `per_category_policy::CategoryPolicy`.
    pub const fn label(self) -> &'static str {
        match self {
            Category::Code => "code",
            Category::Docs => "docs",
            Category::Conversational => "conversational",
            Category::Specialized => "specialized",
            Category::Volatile => "volatile",
            Category::Unknown => "unknown",
        }
    }
}

/// Optional request-level hint the caller can supply. Strict-freshness
/// requests always classify as `Volatile` regardless of text shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshnessHint {
    Strict,
    Standard,
    Relaxed,
}

/// Inputs the classifier consumes. The classifier is text-shape-based
/// only — no embedding-model inference is attempted (that would be a
/// Phase 7 follow-up). Callers can supply a freshness hint to short-
/// circuit to `Volatile`.
#[derive(Debug, Clone)]
pub struct ClassifierInputs<'a> {
    pub query_text: &'a str,
    pub freshness_hint: Option<FreshnessHint>,
}

/// A set of low-cardinality keyword groups the classifier uses. Bounded
/// arrays so the classifier allocates nothing on the hot path.
const VOLATILE_KEYWORDS: &[&str] = &[
    "stock",
    "price",
    "today",
    "tonight",
    "now",
    "latest",
    "breaking",
    "live",
    "current",
    "headline",
    "intraday",
    "real-time",
    "realtime",
];

const CONVERSATIONAL_PREFIXES: &[&str] = &[
    "how do i",
    "how to",
    "what is",
    "what's",
    "whats",
    "can you",
    "could you",
    "please",
    "tell me",
    "explain",
    "show me",
];

/// Classify a single request. Always returns a `Category` — defaults to
/// `Unknown` so the safe-fallback policy from
/// `per_category_policy::CategoryPolicy::fallback()` applies.
pub fn classify(inputs: &ClassifierInputs<'_>) -> Category {
    // 1. Strict freshness short-circuits everything else. Even a code-
    //    looking query becomes Volatile if the tenant asked for strict
    //    freshness — the cache must re-verify.
    if matches!(inputs.freshness_hint, Some(FreshnessHint::Strict)) {
        return Category::Volatile;
    }

    let text = inputs.query_text.trim();
    if text.is_empty() {
        return Category::Unknown;
    }
    let lower = text.to_lowercase();

    // 2. Volatile keywords win next — even a long prose query about
    //    "stock prices today" is Volatile.
    if VOLATILE_KEYWORDS.iter().any(|kw| word_present(&lower, kw)) {
        return Category::Volatile;
    }

    // 3. Code detection: high density of code-marker punctuation.
    if looks_like_code(text) {
        return Category::Code;
    }

    // 4. Conversational: short or starts with a conversational prefix.
    let word_count = lower.split_whitespace().count();
    if word_count <= 4 || CONVERSATIONAL_PREFIXES.iter().any(|p| lower.starts_with(p)) {
        return Category::Conversational;
    }

    // 5. Specialized: high rare-character ratio (numbers, hyphens,
    //    abbreviation-shaped tokens) without code markers.
    if specialized_ratio(text) >= 0.20 {
        return Category::Specialized;
    }

    // 6. Long prose with low symbol density → Docs.
    if word_count >= 8 && symbol_density(text) < 0.05 {
        return Category::Docs;
    }

    Category::Unknown
}

/// Heuristic: a query "looks like code" when ≥3 code-marker characters
/// appear inside the trimmed text or any single line contains
/// `() { } ;` patterns.
fn looks_like_code(text: &str) -> bool {
    let markers = b"(){}[];=<>";
    let count = text
        .as_bytes()
        .iter()
        .filter(|b| markers.contains(b))
        .count();
    if count >= 3 {
        return true;
    }
    // CamelCase or snake_case identifier alongside parens — typical
    // code-search shape.
    text.contains("()")
        || text.contains("::")
        || (text.contains('_') && text.contains('(') && text.contains(')'))
}

/// Fraction of characters that are non-alphanumeric, non-space symbols.
fn symbol_density(text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    let total = text.chars().count() as f64;
    let symbols = text
        .chars()
        .filter(|c| !c.is_alphanumeric() && !c.is_whitespace())
        .count() as f64;
    symbols / total
}

/// Fraction of tokens that look "specialized" — numeric-ish, contain
/// hyphens, contain dots, or are all-caps acronyms.
fn specialized_ratio(text: &str) -> f64 {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.is_empty() {
        return 0.0;
    }
    let n_specialized = tokens
        .iter()
        .filter(|t| {
            let has_digit = t.chars().any(|c| c.is_ascii_digit());
            let has_hyphen = t.contains('-');
            let has_dot = t.contains('.');
            let all_caps = t
                .chars()
                .all(|c| !c.is_ascii_alphabetic() || c.is_ascii_uppercase())
                && t.chars().any(|c| c.is_ascii_alphabetic())
                && t.len() >= 2;
            has_digit || has_hyphen || has_dot || all_caps
        })
        .count();
    n_specialized as f64 / tokens.len() as f64
}

/// Whether `keyword` appears as a whole word in the (already-lowercased)
/// haystack. Avoids `"stock"` matching `"stockholm"`. Hyphens in both
/// sides are stripped before tokenizing so a keyword like `"real-time"`
/// matches `"real-time inventory levels"` (the tokenizer would otherwise
/// split the hyphen and lose the multi-token shape).
fn word_present(haystack: &str, keyword: &str) -> bool {
    let normalize = |s: &str| s.replace('-', "");
    let h = normalize(haystack);
    let k = normalize(keyword);
    h.split(|c: char| !c.is_alphanumeric()).any(|w| w == k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ci(text: &str) -> ClassifierInputs<'_> {
        ClassifierInputs {
            query_text: text,
            freshness_hint: None,
        }
    }

    fn ci_with(text: &str, hint: FreshnessHint) -> ClassifierInputs<'_> {
        ClassifierInputs {
            query_text: text,
            freshness_hint: Some(hint),
        }
    }

    #[test]
    fn empty_text_is_unknown() {
        assert_eq!(classify(&ci("")), Category::Unknown);
        assert_eq!(classify(&ci("   ")), Category::Unknown);
    }

    #[test]
    fn strict_freshness_short_circuits_to_volatile() {
        // Even a code-looking query becomes Volatile under strict freshness.
        let c = classify(&ci_with(
            "fn parse_args() { let mut args = vec![]; }",
            FreshnessHint::Strict,
        ));
        assert_eq!(c, Category::Volatile);
    }

    #[test]
    fn non_strict_freshness_does_not_force_volatile() {
        let c = classify(&ci_with(
            "How do I configure the cache",
            FreshnessHint::Standard,
        ));
        assert_eq!(c, Category::Conversational);
        let c = classify(&ci_with(
            "How do I configure the cache",
            FreshnessHint::Relaxed,
        ));
        assert_eq!(c, Category::Conversational);
    }

    #[test]
    fn volatile_keywords_classify_volatile() {
        for kw in [
            "what is the stock price today",
            "latest news about AI",
            "live headlines from finance",
            "real-time inventory levels",
        ] {
            assert_eq!(classify(&ci(kw)), Category::Volatile, "input: {kw}");
        }
    }

    #[test]
    fn volatile_keyword_partial_match_does_not_trigger() {
        // "stockholm" should NOT trigger the "stock" volatile keyword.
        let c = classify(&ci(
            "describe the geographic features of stockholm in scandinavia",
        ));
        assert_ne!(c, Category::Volatile, "stockholm must not match 'stock'");
    }

    #[test]
    fn code_shape_classifies_code() {
        for code in [
            "fn parse_args() -> Result<(), Error> { ... }",
            "obj.method(arg1, arg2);",
            "namespace::Class::method()",
            "snake_case_func(x, y)",
        ] {
            assert_eq!(classify(&ci(code)), Category::Code, "input: {code}");
        }
    }

    #[test]
    fn conversational_short_query() {
        for prose in ["what is rust", "how to configure", "explain that"] {
            assert_eq!(
                classify(&ci(prose)),
                Category::Conversational,
                "input: {prose}"
            );
        }
    }

    #[test]
    fn conversational_prefixed_long_query() {
        // Long-form query that starts with a conversational prefix still
        // classifies as Conversational.
        let q = "how do i set up the production monitoring pipeline for the new tenant";
        assert_eq!(classify(&ci(q)), Category::Conversational);
    }

    #[test]
    fn specialized_jargon_classifies_specialized() {
        // Tokens with hyphens, numbers, dots — domain-specific shape.
        let q = "CVE-2026-12345 patch for openssl-3.2.1 affects rhel-9.4";
        assert_eq!(classify(&ci(q)), Category::Specialized);
    }

    #[test]
    fn long_prose_with_low_symbol_density_is_docs() {
        let q = "describe the lifecycle of a search request from gateway to response in detail";
        assert_eq!(classify(&ci(q)), Category::Docs);
    }

    #[test]
    fn unknown_when_no_heuristic_matches() {
        // Mid-length text, no code markers, no conversational prefix,
        // no volatile keywords, no jargon. The classifier should land
        // on Unknown so the safe-fallback policy applies.
        // Choose text that is 5-7 words (above conversational threshold,
        // below docs minimum word count of 8) with normal punctuation.
        let q = "find recent incident reports for payments";
        assert_eq!(classify(&ci(q)), Category::Unknown);
    }

    #[test]
    fn labels_are_bounded_lowercase_snake_case() {
        let labels = [
            Category::Code.label(),
            Category::Docs.label(),
            Category::Conversational.label(),
            Category::Specialized.label(),
            Category::Volatile.label(),
            Category::Unknown.label(),
        ];
        for l in &labels {
            assert!(!l.is_empty());
            assert!(l.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
        }
        // All distinct.
        let unique: std::collections::HashSet<_> = labels.iter().copied().collect();
        assert_eq!(unique.len(), labels.len());
    }

    #[test]
    fn label_matches_per_category_policy_default_table() {
        // The classifier's bounded label set must include every key in
        // the PerCategoryPolicy default table so lookups never miss.
        use crate::query::cache::per_category_policy::default_table;
        let table = default_table();
        for label in [
            Category::Code.label(),
            Category::Docs.label(),
            Category::Conversational.label(),
            Category::Specialized.label(),
            Category::Volatile.label(),
        ] {
            assert!(
                table.contains_key(label),
                "label {} missing from per_category_policy::default_table",
                label
            );
        }
    }

    #[test]
    fn volatile_dominates_code_when_both_signals_present() {
        // A query like "stock price method()" has volatile keyword AND
        // code-shape parens. Volatile wins because the staleness signal
        // outweighs the shape signal.
        let q = "stock price calc()";
        assert_eq!(classify(&ci(q)), Category::Volatile);
    }
}
