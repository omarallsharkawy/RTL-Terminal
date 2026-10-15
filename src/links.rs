//! Clickable links: plain-URL detection, the scheme allow-list, and opening a URL.
use std::process::{Command, Stdio};

const SCHEMES: [&str; 5] = ["https://", "http://", "file://", "ftp://", "mailto:"];
const ALLOWED: [&str; 5] = ["http", "https", "file", "mailto", "ftp"];

fn matches_at(chars: &[char], i: usize, scheme: &str) -> bool {
    let n = scheme.chars().count();
    chars.len() >= i + n
        && chars[i..i + n]
            .iter()
            .zip(scheme.chars())
            .all(|(a, b)| a.to_ascii_lowercase() == b)
}

fn is_stop(c: char) -> bool {
    c.is_whitespace()
        || c.is_control()
        || matches!(c, '"' | '\'' | '<' | '>' | '`' | '«' | '»' | '“' | '”')
}

/// Drop trailing punctuation; closing brackets stay only when balanced inside the URL.
fn trimmed_end(chars: &[char], start: usize, mut end: usize) -> usize {
    while end > start {
        let c = chars[end - 1];
        let drop = match c {
            // Latin and Arabic sentence punctuation (، ؛ ؟ ۔) and a closing quote.
            '.' | ',' | ';' | ':' | '!' | '?' | '،' | '؛' | '؟' | '۔' | '’' => true,
            ')' | ']' | '}' => {
                let open = match c {
                    ')' => '(',
                    ']' => '[',
                    _ => '{',
                };
                let slice = &chars[start..end];
                let opens = slice.iter().filter(|&&x| x == open).count();
                let closes = slice.iter().filter(|&&x| x == c).count();
                closes > opens
            }
            _ => false,
        };
        if !drop {
            break;
        }
        end -= 1;
    }
    end
}

/// Find the URL covering `char_index` (an index into `text.chars()`).
/// Returns `(start, end, url)` with `end` exclusive, both in chars.
pub fn find_url_at(text: &str, char_index: usize) -> Option<(usize, usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let boundary = i == 0 || !chars[i - 1].is_alphanumeric();
        let scheme = SCHEMES
            .iter()
            .find(|s| boundary && matches_at(&chars, i, s));
        let Some(scheme) = scheme else {
            i += 1;
            continue;
        };
        let body = i + scheme.chars().count();
        let mut end = body;
        while end < chars.len() && !is_stop(chars[end]) {
            end += 1;
        }
        let end = trimmed_end(&chars, i, end);
        if end > body {
            if char_index >= i && char_index < end {
                return Some((i, end, chars[i..end].iter().collect()));
            }
            i = end;
        } else {
            i = body.max(i + 1);
        }
        if i > char_index {
            return None;
        }
    }
    None
}

/// Only these schemes may be handed to the system opener.
pub fn is_allowed_scheme(url: &str) -> bool {
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let Some((scheme, rest)) = url.split_once(':') else {
        return false;
    };
    ALLOWED.iter().any(|s| scheme.eq_ignore_ascii_case(s)) && !rest.is_empty()
}

/// Open `url` with the system handler, detached. Returns false if rejected or not spawned.
pub fn open_url(url: &str) -> bool {
    if !is_allowed_scheme(url) {
        return false;
    }
    // The URL is always a single argument; no shell is involved.
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let mut cmd = Command::new(program);
    cmd.arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match cmd.spawn() {
        Ok(mut child) => {
            // Reap it off the UI thread so it never becomes a zombie.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}
