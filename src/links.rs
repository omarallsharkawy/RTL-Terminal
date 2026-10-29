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
    if scheme.eq_ignore_ascii_case("mailto") && rest.to_ascii_lowercase().contains("attach") {
        // Some mail clients attach the named local file (`?attach=~/.ssh/id_rsa`).
        return false;
    }
    ALLOWED.iter().any(|s| scheme.eq_ignore_ascii_case(s)) && !rest.is_empty()
}

/// Extensions the system opener runs instead of showing (Windows and Linux desktops).
const LAUNCHABLE: &[&str] = &[
    "appimage",
    "application",
    "appref-ms",
    "bat",
    "cmd",
    "com",
    "cpl",
    "desktop",
    "exe",
    "hta",
    "jar",
    "js",
    "jse",
    "lnk",
    "msc",
    "msi",
    "msp",
    "pif",
    "ps1",
    "psm1",
    "reg",
    "run",
    "scr",
    "sh",
    "url",
    "vbe",
    "vbs",
    "ws",
    "wsf",
    "wsh",
];

/// Decode `%XX` escapes; `None` for malformed escapes or a decoded NUL.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            let b = u8::from_str_radix(hex, 16).ok()?;
            if b == 0 {
                return None;
            }
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// This machine's host name, as `ls --hyperlink` writes it into `file://` links.
fn local_hostname() -> Option<String> {
    let name = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok())?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// What a `file://` link may open: the path itself, or for an executable or launcher its folder.
///
/// Programs choose link targets freely (OSC 8 shows any text over any target), and the system
/// opener runs programs, scripts and `.desktop` launchers rather than showing them, so a click
/// on a harmless-looking link must never run one. Links to other hosts are refused: on Windows
/// they make the system connect to that host and send the user's credentials.
pub fn file_link_target(url: &str) -> Option<std::path::PathBuf> {
    let rest = url
        .get(..7)
        .filter(|p| p.eq_ignore_ascii_case("file://"))
        .map(|_| &url[7..])?;
    let (host, path) = rest.split_at(rest.find('/')?);
    let local = host.is_empty()
        || host.eq_ignore_ascii_case("localhost")
        || local_hostname().is_some_and(|h| h.eq_ignore_ascii_case(host));
    if !local {
        return None;
    }
    // Drop a query or fragment, then decode.
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let mut path = percent_decode(path)?;
    if cfg!(windows) {
        // file:///C:/dir -> C:/dir
        if path.len() >= 3 && path.as_bytes()[2] == b':' {
            path.remove(0);
        }
        if path.starts_with("//") || path.starts_with("\\\\") {
            return None;
        }
    }
    let path = std::path::PathBuf::from(path);
    if is_launchable(&path) {
        return path.parent().map(std::path::Path::to_path_buf);
    }
    Some(path)
}

/// True when opening `path` would run it rather than show it.
fn is_launchable(path: &std::path::Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if ext.is_some_and(|e| LAUNCHABLE.contains(&e.as_str())) {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            return meta.is_file() && meta.permissions().mode() & 0o111 != 0;
        }
    }
    false
}

/// Open `url` with the system handler, detached. Returns false if rejected or not spawned.
pub fn open_url(url: &str) -> bool {
    if !is_allowed_scheme(url) {
        return false;
    }
    let file_target;
    let target: &std::ffi::OsStr = if url
        .get(..5)
        .is_some_and(|p| p.eq_ignore_ascii_case("file:"))
    {
        match file_link_target(url) {
            Some(path) => {
                file_target = path;
                file_target.as_os_str()
            }
            None => return false,
        }
    } else {
        std::ffi::OsStr::new(url)
    };
    // The URL is always a single argument; no shell is involved.
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let mut cmd = Command::new(program);
    cmd.arg(target)
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

/// A link target as shown in the status line while it is hovered: control and bidi formatting
/// characters become `�`, so the text can't be made to read as a different address, and very
/// long targets are shortened.
pub fn display_target(url: &str) -> String {
    const MAX: usize = 300;
    let mut out: String = url
        .chars()
        .take(MAX)
        .map(|c| {
            if c.is_control()
                || crate::terminal::is_bidi_control(c)
                || matches!(c, '\u{200E}' | '\u{200F}' | '\u{061C}')
            {
                '\u{FFFD}'
            } else {
                c
            }
        })
        .collect();
    if url.chars().nth(MAX).is_some() {
        out.push('…');
    }
    out
}
