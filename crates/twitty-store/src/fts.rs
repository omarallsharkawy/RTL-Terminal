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
pub fn escape_fts5_query(input: &str) -> Option<String> {
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
        // Truncate safely on a UTF-8 char boundary.
        let truncated = truncate_utf8(&cleaned, MAX_TERM_BYTES);
        // Strip leading/trailing punctuation outside the token so e.g. `NEAR(` searches for
        // `NEAR` safely as a quoted prefix rather than passing unbalanced punctuation, while
        // preserving internal punctuation (like `foo-bar`, `src/main.rs`) inside quotes.
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
