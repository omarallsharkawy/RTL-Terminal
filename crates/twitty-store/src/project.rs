//! Project identity helpers and `memory_exclude` glob/path matching.
//!
//! When a working directory or resolved project root matches any configured exclusion pattern,
//! nothing is recorded to history/memory and nothing is assembled into AI context.

use std::path::{Path, PathBuf};

/// Return `true` if `path` matches any glob or path prefix in `patterns`.
pub fn is_path_excluded(path: &Path, patterns: &[String]) -> bool {
    if patterns.is_empty() || path.as_os_str().is_empty() {
        return false;
    }
    let home = current_home_dir();
    if is_path_excluded_with_home(path, patterns, home.as_deref()) {
        return true;
    }
    if let Ok(canon) = std::fs::canonicalize(path) {
        if canon != path && is_path_excluded_with_home(&canon, patterns, home.as_deref()) {
            return true;
        }
    }
    false
}

/// Return `true` if either `cwd` or its resolved `project_root` is excluded by `patterns`.
pub fn is_project_excluded(cwd: &Path, project_root: &Path, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return false;
    }
    is_path_excluded(cwd, patterns)
        || (project_root != cwd && is_path_excluded(project_root, patterns))
}

/// Pure, environment-free version of [`is_path_excluded`] using an explicit `home` directory for
/// `~` expansion.
pub fn is_path_excluded_with_home(path: &Path, patterns: &[String], home: Option<&Path>) -> bool {
    if patterns.is_empty() || path.as_os_str().is_empty() {
        return false;
    }
    let norm_path = normalize_path_str(&path.to_string_lossy());
    if norm_path.is_empty() {
        return false;
    }
    let home_norm = home.map(|h| normalize_path_str(&h.to_string_lossy()));

    for raw in patterns {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let expanded = expand_tilde(trimmed, home_norm.as_deref());
        let norm_pat = normalize_path_str(&expanded);
        if norm_pat.is_empty() {
            continue;
        }
        if matches_single_pattern(&norm_path, &norm_pat) {
            return true;
        }
    }
    false
}

fn current_home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }
}

fn expand_tilde(pattern: &str, home: Option<&str>) -> String {
    let Some(home) = home.filter(|h| !h.is_empty()) else {
        return pattern.to_owned();
    };
    if pattern == "~" {
        return home.to_owned();
    }
    if let Some(rest) = pattern
        .strip_prefix("~/")
        .or_else(|| pattern.strip_prefix("~\\"))
    {
        return format!("{home}/{rest}");
    }
    pattern.to_owned()
}

fn normalize_path_str(raw: &str) -> String {
    let replaced = raw.replace('\\', "/");
    let mut out = String::with_capacity(replaced.len());
    let mut prev_slash = false;
    for ch in replaced.chars() {
        if ch == '/' {
            if !prev_slash {
                out.push('/');
            }
            prev_slash = true;
        } else {
            out.push(ch);
            prev_slash = false;
        }
    }
    while out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    out
}

fn has_glob_meta(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?')
}

fn matches_single_pattern(path: &str, pattern: &str) -> bool {
    if !has_glob_meta(pattern) {
        if is_absolute_norm(pattern) {
            return path == pattern
                || (pattern == "/" && path.starts_with('/'))
                || path
                    .strip_prefix(pattern)
                    .is_some_and(|rest| rest.starts_with('/'));
        }
        // Relative literal: match a path segment or contiguous sub-path.
        if !pattern.contains('/') {
            return path.split('/').any(|seg| seg == pattern);
        }
        return path == pattern
            || path.ends_with(&format!("/{pattern}"))
            || path.contains(&format!("/{pattern}/"));
    }

    // Glob pattern without `/` matches any individual path component (e.g. `*-secret`, `*.private`).
    if !pattern.contains('/') {
        let pat_chars: Vec<char> = pattern.chars().collect();
        return path
            .split('/')
            .filter(|seg| !seg.is_empty())
            .any(|seg| segment_glob_match(&pat_chars, &seg.chars().collect::<Vec<_>>()));
    }

    // Path glob: check `path` and every directory prefix (ancestor) so `/clients/*` also excludes
    // `/clients/acme/repo`.
    let pat_segs = split_path_segments(pattern);
    let path_segs = split_path_segments(path);
    let anchored = is_absolute_norm(pattern);

    if anchored {
        for end in 1..=path_segs.len() {
            if segments_match(&pat_segs, &path_segs[..end]) {
                return true;
            }
        }
        false
    } else {
        // Relative path glob (e.g. `**/secrets/**` or `work/*`): match against full path or any
        // suffix/prefix slice of path segments.
        for start in 0..path_segs.len() {
            for end in (start + 1)..=path_segs.len() {
                if segments_match(&pat_segs, &path_segs[start..end]) {
                    return true;
                }
            }
        }
        false
    }
}

fn is_absolute_norm(path: &str) -> bool {
    if path.starts_with('/') {
        return true;
    }
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/'
}

fn split_path_segments(s: &str) -> Vec<&str> {
    let trimmed = s.strip_prefix('/').unwrap_or(s);
    if trimmed.is_empty() {
        return Vec::new();
    }
    trimmed.split('/').collect()
}

/// DP matching over path segments where `**` matches 0 or more segments and `*`/`?` match within a
/// single segment. Bounded in time and memory by `(pat.len() + 1) * (path.len() + 1)`.
fn segments_match(pat: &[&str], path: &[&str]) -> bool {
    let m = pat.len();
    let n = path.len();
    let mut dp = vec![false; n + 1];
    dp[0] = true;

    for &p in pat.iter().take(m) {
        let mut next = vec![false; n + 1];
        if p == "**" {
            let mut any_true = false;
            for j in 0..=n {
                if dp[j] {
                    any_true = true;
                }
                next[j] = any_true;
            }
        } else {
            let p_chars: Vec<char> = p.chars().collect();
            for j in 0..n {
                if dp[j] {
                    let s_chars: Vec<char> = path[j].chars().collect();
                    if segment_glob_match(&p_chars, &s_chars) {
                        next[j + 1] = true;
                    }
                }
            }
        }
        dp = next;
    }
    dp[n]
}

/// Iterative glob matcher for a single path segment (`*` matches 0+ chars, `?` matches 1 char).
fn segment_glob_match(pat: &[char], text: &[char]) -> bool {
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star_pi: Option<usize> = None;
    let mut star_ti = 0usize;

    while ti < text.len() {
        if pi < pat.len() && (pat[pi] == '?' || pat[pi] == text[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pat.len() && pat[pi] == '*' {
            star_pi = Some(pi);
            pi += 1;
            star_ti = ti;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == '*' {
        pi += 1;
    }
    pi == pat.len()
}
