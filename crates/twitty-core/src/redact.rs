//! Linear-time secret redaction applied to text before persistence, logging, syncing, or
//! transmission to an AI provider.
//!
//! Replaces recognized secret shapes with `[REDACTED:<kind>]` while leaving ordinary text
//! (including Arabic/UTF-8, git commit hashes, UUIDs, and generic base64) untouched.
//! Returns [`Cow::Borrowed`] with zero heap allocation when no secret is present.

use std::borrow::Cow;

use memchr::memchr;

/// Redact recognized secrets from `text`, replacing each secret value with `[REDACTED:<kind>]`.
///
/// Scanning is strictly bounded and linear in `text.len()` (no regex engine, no backtracking).
/// When no secret is found, returns `Cow::Borrowed(text)` without allocating.
pub fn redact(text: &str) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    if len < 6 {
        return Cow::Borrowed(text);
    }

    let mut out: Option<String> = None;
    let mut copied = 0usize;
    let mut i = 0usize;

    // Tracks the command executable of the current shell command segment so `-p<pass>` is only
    // redacted when unambiguously belonging to mysql/mariadb clients.
    let mut cmd_state = CmdState::Start;

    while i < len {
        let b = bytes[i];

        // 1. PEM private-key blocks (`-----BEGIN ... PRIVATE KEY-----`).
        if b == b'-' && bytes[i..].starts_with(b"-----BEGIN ") {
            if let Some(end) = match_pem_private_key(bytes, i) {
                push_redacted(&mut out, text, &mut copied, i, end, "pem_private_key");
                i = end;
                cmd_state = CmdState::Start;
                continue;
            }
        }

        // 2. Track shell command separators for unambiguous CLI flag context.
        if b == b'\n' || b == b'\r' || b == b';' || b == b'|' || b == b'&' {
            cmd_state = CmdState::Start;
            i += 1;
            continue;
        }
        if b == b' ' || b == b'\t' {
            i += 1;
            continue;
        }

        // Check if we are at a token boundary (start of input or preceded by a delimiter).
        let at_boundary = i == 0 || is_token_boundary(bytes[i - 1]);
        let at_line_or_ws = i == 0 || is_assignment_prefix_boundary(bytes[i - 1]);

        // 3. `Authorization: Bearer|Basic <token>` header (case-insensitive, including
        //    `Proxy-Authorization:`).
        if (at_boundary || (i > 0 && bytes[i - 1] == b'-')) && (b == b'A' || b == b'a') {
            if let Some((val_start, val_end)) = match_auth_header(bytes, i) {
                push_redacted(
                    &mut out,
                    text,
                    &mut copied,
                    val_start,
                    val_end,
                    "auth_header",
                );
                i = val_end;
                continue;
            }
        }

        // 4. Known prefixed tokens at token boundaries.
        if at_boundary {
            if let Some((end, kind)) = match_prefixed_token(bytes, i) {
                push_redacted(&mut out, text, &mut copied, i, end, kind);
                i = end;
                cmd_state.advance_non_cmd();
                continue;
            }
        }

        // 5. CLI password flags (`--password=...`, `--password ...`, and mysql `-p<pass>`).
        if at_line_or_ws && b == b'-' {
            if let Some((val_start, val_end)) = match_cli_password(bytes, i, cmd_state) {
                push_redacted(
                    &mut out,
                    text,
                    &mut copied,
                    val_start,
                    val_end,
                    "cli_password",
                );
                i = val_end;
                cmd_state.advance_non_cmd();
                continue;
            }
        }

        // 6. `.env`-style or inline `KEY=VALUE` / `aws_secret_access_key = VALUE`.
        if at_line_or_ws && (b.is_ascii_alphabetic() || b == b'_') {
            if let Some((val_start, val_end, kind)) = match_env_assignment(bytes, i) {
                push_redacted(&mut out, text, &mut copied, val_start, val_end, kind);
                i = val_end;
                // An env assignment before a command (`FOO_TOKEN=x mysql ...`) leaves command
                // executable detection ready for the next word.
                continue;
            }
        }

        // 7. Passwords in URLs (`scheme://user:pass@host`).
        if b == b':' && bytes[i..].starts_with(b"://") {
            if let Some((pass_start, pass_end, after_at)) = match_url_password(bytes, i) {
                push_redacted(
                    &mut out,
                    text,
                    &mut copied,
                    pass_start,
                    pass_end,
                    "url_password",
                );
                i = after_at;
                cmd_state.advance_non_cmd();
                continue;
            }
        }

        // Advance command-name tracking at token start (bounded O(1) slice check).
        if at_line_or_ws && matches!(cmd_state, CmdState::Start) {
            let word_end = scan_shell_word_end(bytes, i);
            let word = &bytes[i..word_end];
            if is_shell_prefix_keyword(word) || word.contains(&b'=') {
                // `env`, `sudo`, `export`, or `VAR=val` precedes the real command.
                cmd_state = CmdState::Start;
            } else if is_mysql_client_command(word) {
                cmd_state = CmdState::MysqlClient;
            } else {
                cmd_state = CmdState::Other;
            }
        }

        // Fast-forward over non-ASCII UTF-8 continuation bytes or plain text cleanly.
        i += 1;
    }

    match out {
        Some(mut buf) => {
            buf.push_str(&text[copied..]);
            Cow::Owned(buf)
        }
        None => Cow::Borrowed(text),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CmdState {
    Start,
    MysqlClient,
    Other,
}

impl CmdState {
    fn advance_non_cmd(&mut self) {
        if *self == Self::Start {
            *self = Self::Other;
        }
    }
}

#[inline]
fn push_redacted(
    out: &mut Option<String>,
    text: &str,
    copied: &mut usize,
    start: usize,
    end: usize,
    kind: &str,
) {
    let buf = out.get_or_insert_with(|| String::with_capacity(text.len()));
    buf.push_str(&text[*copied..start]);
    buf.push_str("[REDACTED:");
    buf.push_str(kind);
    buf.push(']');
    *copied = end;
}

#[inline]
fn is_token_boundary(b: u8) -> bool {
    !(b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[inline]
fn is_assignment_prefix_boundary(b: u8) -> bool {
    matches!(
        b,
        b' ' | b'\t' | b'\n' | b'\r' | b';' | b'|' | b'&' | b'(' | b'{'
    )
}

#[inline]
fn is_base64url(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

#[inline]
fn is_key_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[inline]
fn is_token_char_dash(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

fn scan_shell_word_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    let limit = (start + 128).min(bytes.len());
    while i < limit && !matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r' | b';' | b'|' | b'&') {
        i += 1;
    }
    i
}

fn is_shell_prefix_keyword(word: &[u8]) -> bool {
    matches!(
        word,
        b"env" | b"sudo" | b"doas" | b"command" | b"time" | b"nohup" | b"exec" | b"export"
    )
}

fn is_mysql_client_command(word: &[u8]) -> bool {
    let base = match word.iter().rposition(|&b| b == b'/' || b == b'\\') {
        Some(pos) => &word[pos + 1..],
        None => word,
    };
    matches!(
        base,
        b"mysql"
            | b"mysqldump"
            | b"mysqladmin"
            | b"mysqlshow"
            | b"mariadb"
            | b"mariadb-dump"
            | b"mariadb-admin"
            | b"mysql.exe"
            | b"mysqldump.exe"
            | b"mariadb.exe"
    )
}

/// Match `-----BEGIN [TYPE ]PRIVATE KEY[ BLOCK]-----` through its matching END line.
fn match_pem_private_key(bytes: &[u8], start: usize) -> Option<usize> {
    const PREFIX: &[u8] = b"-----BEGIN ";
    let rest = &bytes[start + PREFIX.len()..];
    let line_end_rel = memchr(b'\n', rest).unwrap_or(rest.len()).min(128);
    let header_line = &rest[..line_end_rel];
    let header_trimmed = trim_ascii_end(header_line);
    if !header_trimmed.ends_with(b"-----") {
        return None;
    }
    let label = &header_trimmed[..header_trimmed.len() - 5];
    // Match `PRIVATE KEY` or `PRIVATE KEY BLOCK` (not `PUBLIC KEY` or `CERTIFICATE`).
    if !(label == b"PRIVATE KEY"
        || label.ends_with(b" PRIVATE KEY")
        || label.ends_with(b" PRIVATE KEY BLOCK"))
    {
        return None;
    }
    // Find matching `-----END <label>-----`.
    let body_start = start + PREFIX.len() + line_end_rel;
    let mut search = body_start;
    while search < bytes.len() {
        let Some(rel) = memchr(b'-', &bytes[search..]) else {
            break;
        };
        let pos = search + rel;
        if bytes[pos..].starts_with(b"-----END ") {
            let after_end = pos + b"-----END ".len();
            if bytes[after_end..].starts_with(label) {
                let after_label = after_end + label.len();
                if bytes[after_label..].starts_with(b"-----") {
                    return Some(after_label + 5);
                }
            }
        }
        search = pos + 1;
    }
    // Truncated PEM private key block: redact contiguous base64/header lines after BEGIN.
    let mut pos = body_start;
    let mut saw_body = false;
    while pos < bytes.len() {
        if bytes[pos] == b'\n' {
            pos += 1;
            continue;
        }
        let next_nl = memchr(b'\n', &bytes[pos..]).map_or(bytes.len(), |r| pos + r);
        let line = trim_ascii_end(&bytes[pos..next_nl]);
        if line.is_empty() {
            break;
        }
        let is_pem_body_line = line.iter().all(|&c| {
            c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'=' | b':' | b' ' | b'-' | b',')
        });
        if !is_pem_body_line {
            break;
        }
        saw_body = true;
        pos = next_nl;
    }
    if saw_body {
        Some(pos)
    } else {
        // Even a bare header line with nothing after it is safer redacted only if body exists;
        // avoid redacting documentation mentions of the header alone without a body or END.
        None
    }
}

fn trim_ascii_end(mut slice: &[u8]) -> &[u8] {
    while let Some(&last) = slice.last() {
        if matches!(last, b' ' | b'\t' | b'\r') {
            slice = &slice[..slice.len() - 1];
        } else {
            break;
        }
    }
    slice
}

/// Match `Authorization: Bearer <token>` or `Authorization: Basic <token>` (case-insensitive).
fn match_auth_header(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    const HDR: &[u8] = b"authorization";
    if bytes.len() - start < HDR.len() + 2 {
        return None;
    }
    if !bytes[start..start + HDR.len()].eq_ignore_ascii_case(HDR) {
        return None;
    }
    let mut p = start + HDR.len();
    while p < bytes.len() && (bytes[p] == b' ' || bytes[p] == b'\t') {
        p += 1;
    }
    if p >= bytes.len() || bytes[p] != b':' {
        return None;
    }
    p += 1;
    while p < bytes.len() && (bytes[p] == b' ' || bytes[p] == b'\t') {
        p += 1;
    }
    // Check `Bearer` or `Basic` (case-insensitive) followed by whitespace.
    let scheme_len = if bytes[p..].len() >= 6 && bytes[p..p + 6].eq_ignore_ascii_case(b"bearer") {
        6
    } else if bytes[p..].len() >= 5 && bytes[p..p + 5].eq_ignore_ascii_case(b"basic") {
        5
    } else {
        return None;
    };
    p += scheme_len;
    if p >= bytes.len() || !(bytes[p] == b' ' || bytes[p] == b'\t') {
        return None;
    }
    while p < bytes.len() && (bytes[p] == b' ' || bytes[p] == b'\t') {
        p += 1;
    }
    let val_start = p;
    if bytes[val_start..].starts_with(b"[REDACTED:") {
        return None;
    }
    let mut val_end = p;
    while val_end < bytes.len() && !is_header_value_stop(bytes[val_end]) {
        val_end += 1;
    }
    if val_end <= val_start {
        return None;
    }
    let val = &bytes[val_start..val_end];
    if is_placeholder_or_var(val) {
        return None;
    }
    Some((val_start, val_end))
}

#[inline]
fn is_header_value_stop(b: u8) -> bool {
    matches!(
        b,
        b' ' | b'\t'
            | b'\r'
            | b'\n'
            | b'\''
            | b'"'
            | b','
            | b';'
            | b')'
            | b']'
            | b'}'
            | b'<'
            | b'>'
    )
}

/// Match known prefixed secrets starting at `start` (caller verified token boundary before `start`).
fn match_prefixed_token(bytes: &[u8], start: usize) -> Option<(usize, &'static str)> {
    let tail = &bytes[start..];
    match tail[0] {
        b'g' => {
            // GitHub classic / OAuth / user-to-server / server-to-server / refresh tokens.
            if tail.len() >= 34
                && matches!(&tail[..4], b"ghp_" | b"gho_" | b"ghu_" | b"ghs_" | b"ghr_")
            {
                let end = scan_while(bytes, start + 4, is_key_char);
                if end - (start + 4) >= 30 && ends_at_token_boundary(bytes, end) {
                    return Some((end, "github_token"));
                }
            }
            // GitHub fine-grained personal access token (`github_pat_...`).
            if tail.starts_with(b"github_pat_") {
                let end = scan_while(bytes, start + 11, is_key_char);
                if end - (start + 11) >= 20 && ends_at_token_boundary(bytes, end) {
                    return Some((end, "github_token"));
                }
            }
            None
        }
        b's' => {
            // Anthropic API keys (`sk-ant-...`). Check before OpenAI `sk-`.
            if tail.starts_with(b"sk-ant-") {
                let end = scan_while(bytes, start + 7, is_token_char_dash);
                if end - (start + 7) >= 20 && ends_at_token_boundary(bytes, end) {
                    return Some((end, "anthropic_key"));
                }
                return None;
            }
            // Stripe live keys (`sk_live_...`).
            if tail.starts_with(b"sk_live_") {
                let end = scan_while(bytes, start + 8, |c| c.is_ascii_alphanumeric());
                if end - (start + 8) >= 16 && ends_at_token_boundary(bytes, end) {
                    return Some((end, "stripe_key"));
                }
                return None;
            }
            // OpenAI project / service-account / admin / legacy keys (`sk-proj-...`, `sk-...`).
            if tail.starts_with(b"sk-") {
                let (prefix_len, specific_prefix) = if tail.starts_with(b"sk-proj-") {
                    (8, true)
                } else if tail.starts_with(b"sk-svcacct-") {
                    (11, true)
                } else if tail.starts_with(b"sk-admin-") {
                    (9, true)
                } else if tail.starts_with(b"sk-None-") {
                    (8, true)
                } else {
                    (3, false)
                };
                let end = scan_while(bytes, start + prefix_len, is_token_char_dash);
                let body = &bytes[start + prefix_len..end];
                if body.len() >= 20
                    && (specific_prefix || has_digit_and_letter(body))
                    && ends_at_token_boundary(bytes, end)
                {
                    return Some((end, "openai_key"));
                }
            }
            None
        }
        b'r' => {
            // Stripe restricted live keys (`rk_live_...`).
            if tail.starts_with(b"rk_live_") {
                let end = scan_while(bytes, start + 8, |c| c.is_ascii_alphanumeric());
                if end - (start + 8) >= 16 && ends_at_token_boundary(bytes, end) {
                    return Some((end, "stripe_key"));
                }
            }
            None
        }
        b'A' => {
            // Google API keys (`AIza` + 35 base64url chars; accept 32..=45 for robustness).
            if tail.starts_with(b"AIza") {
                let end = scan_while(bytes, start + 4, is_base64url);
                let body_len = end - (start + 4);
                if (32..=45).contains(&body_len) && ends_at_token_boundary(bytes, end) {
                    return Some((end, "google_api_key"));
                }
            }
            // AWS Access Key IDs (`AKIA` / `ASIA` + 16 uppercase alphanumeric chars = 20 total).
            if tail.len() >= 20 && (tail.starts_with(b"AKIA") || tail.starts_with(b"ASIA")) {
                let end = scan_while(bytes, start + 4, |c| {
                    c.is_ascii_uppercase() || c.is_ascii_digit()
                });
                if end - start == 20 && ends_at_token_boundary(bytes, end) {
                    return Some((end, "aws_access_key"));
                }
            }
            None
        }
        b'x' => {
            // Slack tokens (`xox[abprs]-...`).
            if tail.len() >= 15
                && tail[0..3] == *b"xox"
                && matches!(tail[3], b'a' | b'b' | b'p' | b'r' | b's')
                && tail[4] == b'-'
            {
                let end = scan_while(bytes, start + 5, |c| c.is_ascii_alphanumeric() || c == b'-');
                let body = &bytes[start + 5..end];
                if body.len() >= 10
                    && body.iter().any(|c| c.is_ascii_digit())
                    && ends_at_token_boundary(bytes, end)
                {
                    return Some((end, "slack_token"));
                }
            }
            None
        }
        b'e' => {
            // JWTs: three base64url segments separated by `.`, first segment starts with `eyJ`.
            if tail.starts_with(b"eyJ") {
                let seg1_end = scan_while(bytes, start + 3, is_base64url);
                if seg1_end - start < 10 || seg1_end >= bytes.len() || bytes[seg1_end] != b'.' {
                    return None;
                }
                let seg2_start = seg1_end + 1;
                let seg2_end = scan_while(bytes, seg2_start, is_base64url);
                if seg2_end - seg2_start < 10 || seg2_end >= bytes.len() || bytes[seg2_end] != b'.'
                {
                    return None;
                }
                let seg3_start = seg2_end + 1;
                let seg3_end = scan_while(bytes, seg3_start, is_base64url);
                if seg3_end - seg3_start >= 10 && ends_at_jwt_boundary(bytes, seg3_end) {
                    return Some((seg3_end, "jwt"));
                }
            }
            None
        }
        _ => None,
    }
}

#[inline]
fn scan_while(bytes: &[u8], mut i: usize, pred: impl Fn(u8) -> bool) -> usize {
    while i < bytes.len() && pred(bytes[i]) {
        i += 1;
    }
    i
}

#[inline]
fn ends_at_token_boundary(bytes: &[u8], end: usize) -> bool {
    end >= bytes.len() || !(bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
}

#[inline]
fn ends_at_jwt_boundary(bytes: &[u8], end: usize) -> bool {
    end >= bytes.len() || !(is_base64url(bytes[end]) || bytes[end] == b'.')
}

fn has_digit_and_letter(slice: &[u8]) -> bool {
    let mut digit = false;
    let mut letter = false;
    for &b in slice {
        if b.is_ascii_digit() {
            digit = true;
        } else if b.is_ascii_alphabetic() {
            letter = true;
        }
        if digit && letter {
            return true;
        }
    }
    false
}

/// Match `--password=<val>`, `--password <val>`, `--passwd=<val>`, `--passwd <val>`,
/// or `-p<pass>` when inside a mysql/mariadb client command.
fn match_cli_password(bytes: &[u8], start: usize, cmd_state: CmdState) -> Option<(usize, usize)> {
    let tail = &bytes[start..];
    for flag in [b"--password".as_slice(), b"--passwd".as_slice()] {
        if tail.starts_with(flag) {
            let after = start + flag.len();
            if after >= bytes.len() {
                return None;
            }
            if bytes[after] == b'=' {
                return scan_cli_value(bytes, after + 1);
            }
            if bytes[after] == b' ' || bytes[after] == b'\t' {
                let mut p = after;
                while p < bytes.len() && (bytes[p] == b' ' || bytes[p] == b'\t') {
                    p += 1;
                }
                if p < bytes.len() && bytes[p] != b'-' {
                    return scan_cli_value(bytes, p);
                }
            }
            return None;
        }
    }
    // `-p<pass>` with no separating space: only for mysql/mariadb clients, where `-p<pass>`
    // unambiguously supplies the password (`-p` followed by space prompts interactively).
    if cmd_state == CmdState::MysqlClient
        && tail.len() >= 3
        && tail[0] == b'-'
        && tail[1] == b'p'
        && !matches!(
            tail[2],
            b' ' | b'\t' | b'\n' | b'\r' | b';' | b'|' | b'&' | b'-'
        )
    {
        return scan_cli_value(bytes, start + 2);
    }
    None
}

fn scan_cli_value(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    if start >= bytes.len() {
        return None;
    }
    let (val_start, val_end) = if bytes[start] == b'"' || bytes[start] == b'\'' {
        let quote = bytes[start];
        let inner_start = start + 1;
        let mut p = inner_start;
        while p < bytes.len() && bytes[p] != quote && bytes[p] != b'\n' {
            if quote == b'"' && bytes[p] == b'\\' && p + 1 < bytes.len() {
                p += 2;
            } else {
                p += 1;
            }
        }
        (inner_start, p)
    } else {
        let mut p = start;
        while p < bytes.len()
            && !matches!(
                bytes[p],
                b' ' | b'\t' | b'\r' | b'\n' | b';' | b'|' | b'&' | b')'
            )
        {
            p += 1;
        }
        (start, p)
    };
    if val_end <= val_start {
        return None;
    }
    let val = &bytes[val_start..val_end];
    if is_placeholder_or_var(val) {
        return None;
    }
    Some((val_start, val_end))
}

/// Match `.env`-style / INI-style / inline `KEY=VALUE` assignments whose key contains a
/// secret indicator (`TOKEN`, `SECRET`, `PASSWORD`, `PASSWD`, `API_KEY`, `PRIVATE_KEY`,
/// `CREDENTIAL`, or `AWS_SECRET_ACCESS_KEY`).
fn match_env_assignment(bytes: &[u8], start: usize) -> Option<(usize, usize, &'static str)> {
    // Bounded identifier scan (at most 64 bytes).
    let max_key_end = (start + 64).min(bytes.len());
    let mut p = start;
    while p < max_key_end && (bytes[p].is_ascii_alphanumeric() || bytes[p] == b'_') {
        p += 1;
    }
    let key = &bytes[start..p];
    if key.len() < 5 {
        return None;
    }
    let kind = classify_secret_key(key)?;

    // Optional spaces before `=` (INI / `~/.aws/credentials` style: `aws_secret_access_key = ...`).
    let mut eq = p;
    while eq < bytes.len() && (bytes[eq] == b' ' || bytes[eq] == b'\t') {
        eq += 1;
    }
    if eq >= bytes.len() || bytes[eq] != b'=' {
        return None;
    }
    // Avoid `==` comparisons in code snippets.
    if eq + 1 < bytes.len() && bytes[eq + 1] == b'=' {
        return None;
    }
    let mut v = eq + 1;
    // Allow spaces after `=` only when spaces were also allowed before `=` (INI style) or at
    // start of line (`.env` with spaces).
    let line_start = is_start_of_line(bytes, start);
    if line_start || eq > p {
        while v < bytes.len() && (bytes[v] == b' ' || bytes[v] == b'\t') {
            v += 1;
        }
    }
    if v >= bytes.len() || bytes[v] == b'\n' || bytes[v] == b'\r' {
        return None;
    }

    let (val_start, val_end) = if bytes[v] == b'"' || bytes[v] == b'\'' {
        let quote = bytes[v];
        let inner_start = v + 1;
        let mut end = inner_start;
        while end < bytes.len() && bytes[end] != quote && bytes[end] != b'\n' && bytes[end] != b'\r'
        {
            if quote == b'"' && bytes[end] == b'\\' && end + 1 < bytes.len() {
                end += 2;
            } else {
                end += 1;
            }
        }
        (inner_start, end)
    } else {
        let mut end = v;
        while end < bytes.len()
            && !matches!(
                bytes[end],
                b' ' | b'\t' | b'\r' | b'\n' | b';' | b'|' | b'&' | b'#' | b')'
            )
        {
            end += 1;
        }
        (v, end)
    };

    if val_end <= val_start {
        return None;
    }
    let val = &bytes[val_start..val_end];
    if is_placeholder_or_var(val) {
        return None;
    }
    Some((val_start, val_end, kind))
}

fn is_start_of_line(bytes: &[u8], pos: usize) -> bool {
    let mut i = pos;
    while i > 0 {
        let b = bytes[i - 1];
        if b == b'\n' || b == b'\r' {
            return true;
        }
        if b == b' ' || b == b'\t' {
            i -= 1;
            continue;
        }
        // Also treat `export KEY=...` as line-start assignment.
        if i >= 6 && &bytes[i - 6..i] == b"export" {
            let before_export = i - 6;
            if before_export == 0
                || matches!(bytes[before_export - 1], b'\n' | b'\r' | b' ' | b'\t')
            {
                return true;
            }
        }
        return false;
    }
    true
}

fn classify_secret_key(key: &[u8]) -> Option<&'static str> {
    // Uppercase into a fixed stack buffer (key.len() <= 64).
    let mut upper = [0u8; 64];
    let n = key.len().min(upper.len());
    for (dst, &src) in upper[..n].iter_mut().zip(&key[..n]) {
        *dst = src.to_ascii_uppercase();
    }
    let u = &upper[..n];
    if u == b"AWS_SECRET_ACCESS_KEY" {
        return Some("aws_secret_key");
    }
    const NEEDLES: &[&[u8]] = &[
        b"TOKEN",
        b"SECRET",
        b"PASSWORD",
        b"PASSWD",
        b"API_KEY",
        b"PRIVATE_KEY",
        b"CREDENTIAL",
    ];
    if NEEDLES.iter().any(|needle| contains_subslice(u, needle)) {
        return Some("env_secret");
    }
    None
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Match `scheme://user:pass@host` at `colon_pos` (where `bytes[colon_pos..]` starts with `://`).
/// Returns `(pass_start, pass_end, after_at)`.
fn match_url_password(bytes: &[u8], colon_pos: usize) -> Option<(usize, usize, usize)> {
    // Verify valid URI scheme immediately before `://` (at most 32 bytes).
    let scheme_min = colon_pos.saturating_sub(32);
    let mut s = colon_pos;
    while s > scheme_min
        && (bytes[s - 1].is_ascii_alphanumeric() || matches!(bytes[s - 1], b'+' | b'-' | b'.'))
    {
        s -= 1;
    }
    if s == colon_pos || !bytes[s].is_ascii_alphabetic() {
        return None;
    }

    // Scan authority after `://` for `user:pass@host`. Stop at `/`, `?`, `#`, whitespace, quotes.
    let auth_start = colon_pos + 3;
    let max_auth = (auth_start + 512).min(bytes.len());
    let mut colon_in_auth: Option<usize> = None;
    let mut at_pos: Option<usize> = None;
    let mut p = auth_start;
    while p < max_auth {
        let b = bytes[p];
        if b == b'@' {
            at_pos = Some(p);
            break;
        }
        if matches!(
            b,
            b'/' | b'?' | b'#' | b' ' | b'\t' | b'\r' | b'\n' | b'\'' | b'"' | b'<' | b'>'
        ) {
            return None;
        }
        if b == b':' && colon_in_auth.is_none() {
            colon_in_auth = Some(p);
        }
        // RFC 3986 userinfo never contains `[` or `]`; this also avoids re-matching
        // `https://[REDACTED:github_token]@host` as `user:pass@host`.
        if colon_in_auth.is_none() && (b == b'[' || b == b']') {
            return None;
        }
        p += 1;
    }

    let at = at_pos?;
    let user_colon = colon_in_auth?;
    let user_len = user_colon - auth_start;
    let pass_start = user_colon + 1;
    let pass_end = at;
    if user_len == 0 || pass_end <= pass_start {
        return None;
    }
    // Host must start after `@` with a plausible host byte (alphanumeric, `[`, `_`, `-`).
    let host_start = at + 1;
    if host_start >= bytes.len()
        || !(bytes[host_start].is_ascii_alphanumeric()
            || matches!(bytes[host_start], b'[' | b'_' | b'-'))
    {
        return None;
    }
    let pass = &bytes[pass_start..pass_end];
    if is_placeholder_or_var(pass) {
        return None;
    }
    Some((pass_start, pass_end, host_start))
}

fn is_placeholder_or_var(val: &[u8]) -> bool {
    if val.is_empty() {
        return true;
    }
    // Already redacted or shell variable reference like `$TOKEN` / `${TOKEN}`.
    if val.starts_with(b"[REDACTED:") && val.ends_with(b"]") {
        return true;
    }
    if val[0] == b'$' {
        return true;
    }
    false
}
