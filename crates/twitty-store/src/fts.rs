//! Safe FTS5 query escaping.
//!
//! Turns arbitrary user input (including hostile FTS5 operators such as `"`, `*`, `NEAR(`, `-`,
//! `OR`, `NOT`, `col:term`, and Arabic text) into a parameterised FTS5 phrase/prefix query that
//! cannot trigger syntax errors or unintended operator injection.

/// Maximum number of whitespace-separated terms kept in one FTS5 query.
const MAX_TERMS: usize = 32;
/// Maximum UTF-8 bytes of a single query term before truncation.
const MAX_TERM_BYTES: usize = 128;

/// Convert raw user search text into a safe SQLite FTS5 `MATCH` expression.
///
/// Each whitespace-separated token containing at least one alphanumeric character is stripped of
/// control characters, double-quote escaped (`"` -> `""`), wrapped in double quotes (neutralising
/// all FTS5 operators like `NEAR`, `AND`, `OR`, `NOT`, `-`, `:`, `^`, `*`), and suffixed with `*`
/// for prefix matching.
///
/// Returns `None` when `input` contains no searchable alphanumeric tokens.
/// Like [`escape_fts5_query`], joining terms with `OR` so natural-language AI requests retrieve
/// relevant memories even when a memory matches only a subset of request keywords (BM25 still
/// ranks multi-term matches highest).
pub fn escape_fts5_or_query(input: &str) -> Option<String> {
    let terms = collect_fts5_terms(input);
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

fn collect_fts5_terms(input: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for raw in input.split_whitespace() {
        if terms.len() >= MAX_TERMS {
            break;
        }
        let cleaned: String = raw
            .chars()
            .filter(|c| !c.is_control() && *c != '\0')
            .collect();
        if !cleaned.chars().any(char::is_alphanumeric) {
            continue;
        }
        let truncated = truncate_utf8(&cleaned, MAX_TERM_BYTES);
        let escaped = truncated.replace('"', "\"\"");
        if !escaped.chars().any(char::is_alphanumeric) {
            continue;
        }
        let mut term = String::with_capacity(escaped.len() + 3);
        term.push('"');
        term.push_str(&escaped);
        term.push('"');
        term.push('*');
        terms.push(term);
    }
    terms
}

pub fn escape_fts5_query(input: &str) -> Option<String> {
    let terms = collect_fts5_terms(input);
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" "))
    }
}

fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
