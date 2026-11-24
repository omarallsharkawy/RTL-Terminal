//! Signals that shells and coding agents send through OSC sequences `alacritty_terminal`
//! ignores: the working directory (OSC 7), desktop notifications (OSC 9, OSC 777, OSC 99) and
//! command boundaries (OSC 133, and VS Code's OSC 633).
//!
//! `Terminal::process_bytes` hands every finished OSC payload to [`parse_osc`]; the ones that
//! are signals become [`ShellEvent`]s, stamped with where the cursor was when the sequence
//! arrived. The app drains them after each batch of output (`Terminal::take_shell_events`) and
//! feeds the sidebar (cwd, notifications) and the blocks (marks).
//!
//! Everything a program sends here is untrusted text that ends up on screen and in desktop
//! notifications, so strings are cleaned the same way titles are: no control or bidi-override
//! characters, and a bounded length.

use std::path::PathBuf;

/// Longest OSC payload looked at, in bytes. Longer ones (OSC 52 clipboard copies, big
/// hyperlinks) are never signals and are not copied.
pub const MAX_SIGNAL_LEN: usize = 4096;

/// Longest notification title or body kept, in characters.
pub const MAX_NOTIFY_CHARS: usize = 512;

/// Longest command line kept from OSC 633;E, in characters.
pub const MAX_COMMAND_CHARS: usize = 1024;

/// Events kept per terminal until the app drains them; older ones are dropped first, so a
/// program flooding notifications cannot grow memory.
pub const MAX_PENDING_EVENTS: usize = 256;

/// What a shell or program reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signal {
    /// OSC 7: the shell's working directory (local `file://` URLs only).
    Cwd(PathBuf),
    /// OSC 9, OSC 777;notify or OSC 99: a desktop notification, e.g. an agent that finished or
    /// is waiting for input.
    Notify { title: Option<String>, body: String },
    /// OSC 133 / OSC 633 A, B, C, D: a command boundary.
    Mark(MarkKind),
    /// OSC 633;E: the command line about to run, as the shell reports it.
    CommandLine(String),
}

/// Command boundaries in the FinalTerm / OSC 133 model:
/// `A` prompt … `B` typed command … `C` output … `D;exit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    PromptStart,
    CommandStart,
    OutputStart,
    CommandEnd { exit: Option<i32> },
}

/// A signal and where the cursor was when it arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellEvent {
    pub signal: Signal,
    /// Cursor line in grid coordinates (0 = top of the screen) when the sequence ended.
    pub line: i32,
    /// Cursor column at that moment.
    pub column: usize,
    /// Lines in scrollback at that moment. Together with `line` it places the mark in the
    /// scrollback; see `blocks` for how marks follow the text as it scrolls.
    pub history: usize,
}

/// The signal in one OSC payload (the bytes between `ESC ]` and the terminator), if it is one.
pub fn parse_osc(payload: &[u8]) -> Option<Signal> {
    let split = payload.iter().position(|&b| b == b';')?;
    let (number, rest) = (&payload[..split], &payload[split + 1..]);
    match number {
        b"7" => parse_cwd(rest),
        b"9" => parse_osc9(rest),
        b"777" => parse_osc777(rest),
        b"99" => parse_osc99(rest),
        b"133" | b"633" => parse_mark(rest, number == b"633"),
        _ => None,
    }
}

fn parse_cwd(rest: &[u8]) -> Option<Signal> {
    let url = std::str::from_utf8(rest).ok()?;
    // kitty's shell integration uses its own scheme for the same thing.
    let url = match url.strip_prefix("kitty-shell-cwd://") {
        Some(tail) => format!("file://{tail}"),
        None => url.to_string(),
    };
    let path = crate::links::local_file_url(&url)?;
    let text = path.to_str()?;
    if text.chars().any(|c| c.is_control()) {
        return None;
    }
    Some(Signal::Cwd(path))
}

/// OSC 9 is a notification (iTerm2, Windows Terminal), except ConEmu's numbered commands
/// (`9;4;…` progress, `9;1;…` sleep and so on), which are not.
fn parse_osc9(rest: &[u8]) -> Option<Signal> {
    let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
    if digits > 0 && matches!(rest.get(digits), None | Some(b';')) {
        return None;
    }
    let body = clean_text(&String::from_utf8_lossy(rest), MAX_NOTIFY_CHARS);
    (!body.is_empty()).then_some(Signal::Notify { title: None, body })
}

/// `777;notify;TITLE;BODY` (rxvt, used by many agents). The body may contain `;`.
fn parse_osc777(rest: &[u8]) -> Option<Signal> {
    let text = String::from_utf8_lossy(rest);
    let mut parts = text.splitn(3, ';');
    if parts.next()? != "notify" {
        return None;
    }
    let title = clean_text(parts.next().unwrap_or(""), MAX_NOTIFY_CHARS);
    let body = clean_text(parts.next().unwrap_or(""), MAX_NOTIFY_CHARS);
    if title.is_empty() && body.is_empty() {
        return None;
    }
    Some(Signal::Notify {
        title: (!title.is_empty()).then_some(title),
        body,
    })
}

/// kitty's `99;METADATA;PAYLOAD`. Only single-part, plain-text notifications: chunked ones
/// (`d=0`), base64 payloads (`e=1`) and non-text parts are ignored.
fn parse_osc99(rest: &[u8]) -> Option<Signal> {
    let text = String::from_utf8_lossy(rest);
    let (meta, payload) = text.split_once(';')?;
    let mut is_body = false;
    for pair in meta.split(':').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match (key, value) {
            ("d", "0") | ("e", "1") => return None,
            ("p", "body") => is_body = true,
            ("p", "title") => is_body = false,
            ("p", _) => return None,
            _ => {}
        }
    }
    let payload = clean_text(payload, MAX_NOTIFY_CHARS);
    if payload.is_empty() {
        return None;
    }
    Some(if is_body {
        Signal::Notify {
            title: None,
            body: payload,
        }
    } else {
        Signal::Notify {
            title: Some(payload),
            body: String::new(),
        }
    })
}

fn parse_mark(rest: &[u8], vscode: bool) -> Option<Signal> {
    let (&kind, tail) = rest.split_first()?;
    // Parameters after the letter, e.g. `D;0` or `A;cl=m;aid=12`.
    let params = match tail.split_first() {
        None => &[][..],
        Some((b';', params)) => params,
        Some(_) => return None,
    };
    let mark = match kind {
        b'A' => MarkKind::PromptStart,
        b'B' => MarkKind::CommandStart,
        b'C' => MarkKind::OutputStart,
        b'D' => {
            let first = params.split(|&b| b == b';').next().unwrap_or(&[]);
            let exit = std::str::from_utf8(first).ok().and_then(|s| s.parse().ok());
            MarkKind::CommandEnd { exit }
        }
        b'E' if vscode => {
            let first = params.split(|&b| b == b';').next().unwrap_or(&[]);
            let line = unescape_633(&String::from_utf8_lossy(first));
            let line = clean_text(&line, MAX_COMMAND_CHARS);
            return (!line.is_empty()).then_some(Signal::CommandLine(line));
        }
        _ => return None,
    };
    Some(Signal::Mark(mark))
}

/// OSC 633;E escapes `;`, `\` and control characters as `\xHH` and `\\`.
fn unescape_633(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('x') => {
                let hex: String = chars.by_ref().take(2).collect();
                if let Some(b) = u8::from_str_radix(&hex, 16).ok().map(char::from) {
                    out.push(b);
                }
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Program text as shown in the UI: no control or bidi-override characters, trimmed, at most
/// `max` characters.
pub fn clean_text(text: &str, max: usize) -> String {
    use unicode_segmentation::UnicodeSegmentation;

    let text = bounded_text(
        text.chars().filter(|&c| {
            !c.is_control()
                && !crate::terminal::is_bidi_control(c)
                && !matches!(
                    c,
                    '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}' | '\u{2029}'
                )
        }),
        max,
    );
    let mut graphemes = text.grapheme_indices(true);
    let Some((start, first)) = graphemes.find(|(_, g)| !g.chars().all(char::is_whitespace)) else {
        return String::new();
    };
    let end = graphemes
        .rfind(|(_, g)| !g.chars().all(char::is_whitespace))
        .map_or(start + first.len(), |(i, g)| i + g.len());
    text[start..end].to_owned()
}

pub(crate) fn bounded_text(chars: impl Iterator<Item = char>, max: usize) -> String {
    use unicode_segmentation::UnicodeSegmentation;

    // Look one scalar past the limit to tell whether the final grapheme continues.
    let mut text: String = chars.take(max.saturating_add(1)).collect();
    if let Some((limit, _)) = text.char_indices().nth(max) {
        let end = text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .take_while(|&i| i <= limit)
            .last()
            .unwrap_or(0);
        text.truncate(end);
    }
    text
}
