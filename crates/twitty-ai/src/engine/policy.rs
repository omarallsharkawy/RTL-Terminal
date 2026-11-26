//! Approval policy for project reads, writes and shell command proposals.

use std::io;
use std::path::{Component, Path, PathBuf};

use super::model::DenyReason;
use super::shell::{assignment, lex, Token};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Approval {
    Allow,
    Prompt,
    Deny(DenyReason),
}

fn hard(rule: &str) -> DenyReason {
    DenyReason::HardDeny { rule: rule.into() }
}

fn outside(path: &Path) -> DenyReason {
    DenyReason::OutsideProject { path: path.into() }
}

fn names(path: &Path) -> impl Iterator<Item = String> + '_ {
    path.components().filter_map(|component| match component {
        Component::Normal(name) => Some(name.to_string_lossy().to_ascii_lowercase()),
        _ => None,
    })
}

fn has_git(path: &Path) -> bool {
    names(path).any(|name| name == ".git")
}

fn protected(path: &Path, write: bool) -> Option<DenyReason> {
    let names: Vec<_> = names(path).collect();
    if names
        .iter()
        .any(|name| matches!(name.as_str(), ".ssh" | ".gnupg" | ".aws" | ".netrc"))
        || names.windows(2).any(|p| p == [".config", "gh"])
        || names.windows(2).any(|p| p == [".local", "keyrings"])
        || names.windows(2).any(|p| p == ["share", "keyrings"])
    {
        return Some(hard("protected credentials"));
    }
    if write
        && (names.first().is_some_and(|name| name == "etc") && path.is_absolute()
            || names.windows(2).any(|p| matches!(p, [a, b] if (a == ".config" && b == "twitty") || (a == ".git" && b == "hooks") || (a == "fish" && b == "config.fish")))
            || names.last().is_some_and(|name| {
                matches!(name.as_str(), ".bashrc" | ".bash_profile" | ".bash_login" | ".profile" | ".zshrc" | ".zprofile" | ".zshenv" | ".zlogin" | ".kshrc" | ".cshrc" | ".tcshrc" | ".inputrc")
            }))
    {
        return Some(hard("protected write"));
    }
    None
}

pub(crate) fn sensitive(path: &Path) -> bool {
    names(path).any(|name| {
        name.starts_with(".env")
            || name.ends_with(".pem")
            || name.ends_with(".key")
            || name.starts_with("id_")
            || name.contains("secret")
            || name.starts_with("credentials")
    })
}

/// Resolve existing paths, including symlinks. For a new file, require an existing parent
/// and reject dangling symlinks. A trailing separator and dot components cannot create a file.
pub(crate) fn resolve(path: &Path, new_file: bool) -> io::Result<PathBuf> {
    match path.canonicalize() {
        Err(err) if new_file && err.kind() == io::ErrorKind::NotFound => {
            if !matches!(path.symlink_metadata(), Err(e) if e.kind() == io::ErrorKind::NotFound) {
                return Err(err);
            }
            let parent = path
                .parent()
                .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
            let name = path
                .file_name()
                .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
            Ok(parent.canonicalize()?.join(name))
        }
        result => result,
    }
}

pub(crate) fn path_policy(
    root: &Path,
    path: &Path,
    write: bool,
) -> Result<(PathBuf, Approval), DenyReason> {
    let input = root.join(path);
    if let Some(reason) = protected(&input, write) {
        return Err(reason);
    }
    let resolved = resolve(&input, write).map_err(|_| outside(path))?;
    if let Some(reason) = protected(&resolved, write) {
        return Err(reason);
    }
    if has_git(&input) || has_git(&resolved) || (write && !resolved.starts_with(root)) {
        return Err(outside(path));
    }
    let approval =
        if write || !resolved.starts_with(root) || sensitive(&input) || sensitive(&resolved) {
            Approval::Prompt
        } else {
            Approval::Allow
        };
    Ok((resolved, approval))
}

/// Inspect a project read. Callers must revalidate the path at execution time.
pub fn read_path(root: &Path, path: &Path) -> Approval {
    path_policy(root, path, false).map_or_else(Approval::Deny, |(_, approval)| approval)
}

/// Inspect a patch target. Every valid write still requires approval.
pub fn write_path(root: &Path, path: &Path) -> Approval {
    path_policy(root, path, true).map_or_else(Approval::Deny, |(_, approval)| approval)
}

/// Check a shell proposal using the root and cwd already canonicalised by the session.
pub fn command(
    root: &Path,
    cwd: &Path,
    input: &str,
    prefixes: &[Vec<String>],
    after_untrusted_data: bool,
) -> Approval {
    if input.is_empty()
        || input.chars().any(|c| {
            (c.is_control() && !matches!(c, '\n' | '\t'))
                || twitty_core::terminal::is_bidi_control(c)
        })
    {
        return Approval::Deny(hard("invalid command text"));
    }
    if let Some(reason) = inspect(root, cwd, input, 0) {
        return Approval::Deny(reason);
    }
    let parsed = lex(input);
    let words: Vec<_> = parsed
        .tokens
        .iter()
        .filter_map(|token| match token {
            Token::Word(word) => Some(word.as_str()),
            _ => None,
        })
        .collect();
    if parsed.simple
        && !after_untrusted_data
        && words.first().is_some_and(|word| eligible_direct(word))
        && prefixes.iter().any(|prefix| {
            !prefix.is_empty()
                && words.starts_with(&prefix.iter().map(String::as_str).collect::<Vec<_>>())
        })
    {
        Approval::Allow
    } else {
        Approval::Prompt
    }
}

fn eligible_direct(word: &str) -> bool {
    let program = executable(word);
    !assignment(word)
        && !interpreter(program)
        && !matches!(
            program,
            "env"
                | "command"
                | "exec"
                | "builtin"
                | "nohup"
                | "time"
                | "eval"
                | "source"
                | "."
                | "xargs"
                | "find"
                | "cd"
                | "pushd"
                | "popd"
                | "and"
                | "or"
                | "not"
        )
}

fn executable(word: &str) -> &str {
    word.rsplit(['/', '\\']).next().unwrap_or(word)
}

fn shell(word: &str) -> bool {
    matches!(
        word,
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "ash" | "fish"
    )
}

fn interpreter(word: &str) -> bool {
    shell(word) || word.starts_with("python") || matches!(word, "perl" | "node" | "ruby" | "php")
}

fn unwrapped<'a>(mut words: &'a [&'a str]) -> &'a [&'a str] {
    while let Some((first, trailing)) = words.split_first() {
        match executable(first) {
            "command" | "exec" | "builtin" | "nohup" | "time" | "and" | "or" | "not" | "!" => {
                words = trailing
            }
            "env" => {
                words = trailing;
                while let Some((first, trailing)) = words.split_first() {
                    match *first {
                        "-u" | "--unset" | "-C" | "--chdir" => {
                            words = trailing.get(1..).unwrap_or_default();
                        }
                        "--" => {
                            words = trailing;
                            break;
                        }
                        word if word.starts_with('-') || assignment(word) => words = trailing,
                        _ => break,
                    }
                }
            }
            word if assignment(word) => words = trailing,
            "-p" | "--" => words = trailing,
            _ => break,
        }
    }
    words
}

fn inspect(root: &Path, cwd: &Path, input: &str, shell_depth: usize) -> Option<DenyReason> {
    let compact: String = input.chars().filter(|ch| !ch.is_whitespace()).collect();
    if compact.contains(":(){") {
        return Some(hard("fork bomb"));
    }
    let parsed = lex(input);
    let mut words = Vec::new();
    let mut redirect = None;
    let mut fetched = false;
    let mut cwd = cwd.to_path_buf();

    for token in parsed.tokens.iter().chain([&Token::Break]) {
        match token {
            Token::Word(word) => {
                if let Some(write) = redirect.take() {
                    if let Some(reason) = inspect_path(root, &cwd, word, write) {
                        return Some(reason);
                    }
                } else {
                    words.push(word.as_str());
                }
            }
            Token::Redirect(write) => redirect = Some(*write || redirect.unwrap_or(false)),
            Token::Pipe | Token::Break => {
                if let Some(reason) = inspect_words(root, &cwd, &words, shell_depth, 0, fetched) {
                    return Some(reason);
                }
                fetched = *token == Token::Pipe
                    && (fetched
                        || unwrapped(&words).first().is_some_and(|word| {
                            matches!(executable(word), "curl" | "wget" | "fetch")
                        }));
                if *token != Token::Pipe {
                    if let [directory_command, target, ..] = unwrapped(&words) {
                        if matches!(executable(directory_command), "cd" | "pushd") {
                            let path = cwd.join(expand_home(target));
                            cwd = resolve(&path, false).unwrap_or_else(|_| normalise(&path));
                        }
                    }
                }
                words.clear();
                redirect = None;
            }
        }
    }
    None
}

fn inspect_words(
    root: &Path,
    cwd: &Path,
    words: &[&str],
    shell_depth: usize,
    wrapper_depth: usize,
    fetched: bool,
) -> Option<DenyReason> {
    // Inspect environment wrappers whose command arguments are shell-split by env itself.
    if let Some(index) = words.iter().position(|word| executable(word) == "env") {
        let options = &words[index + 1..];
        if wrapper_depth < 4 {
            for (index, option) in options
                .iter()
                .enumerate()
                .take_while(|(_, option)| option.starts_with('-') || assignment(option))
            {
                let split = match *option {
                    "-S" | "--split-string" => options.get(index + 1).copied(),
                    arg => arg
                        .strip_prefix("--split-string=")
                        .or_else(|| arg.strip_prefix("-S").filter(|value| !value.is_empty())),
                };
                if let Some(command) = split {
                    if shell_depth == 0 {
                        return inspect(root, cwd, command, shell_depth + 1);
                    }
                }
            }
        }
    }
    let words = unwrapped(words);
    let (first, args) = words.split_first()?;
    let program = executable(first);
    let has = |flag| args.contains(&flag);
    let starts = |prefix: &str| args.iter().any(|arg| arg.starts_with(prefix));
    let short = |flag| {
        args.iter()
            .take_while(|arg| **arg != "--")
            .any(|arg| arg.starts_with('-') && !arg.starts_with("--") && arg[1..].contains(flag))
    };
    let git = args
        .iter()
        .position(|arg| matches!(*arg, "push" | "config" | "remote"));
    let rule = match program {
        "sudo" | "doas" | "pkexec" | "su" => Some(program),
        _ if fetched && interpreter(program) => Some("curl|sh"),
        "dd" if starts("of=/dev/") => Some("dd to device"),
        _ if program == "mkfs" || program.starts_with("mkfs.") => Some("mkfs"),
        "secret-tool"
        | "kwallet-query"
        | "gnome-keyring"
        | "gnome-keyring-daemon"
        | "keyctl"
        | "security"
        | "pass"
        | "gopass"
        | "credential-manager"
        | "wincred" => Some("keyring"),
        "gdbus" | "dbus-send" | "busctl"
            if args.iter().any(|arg| {
                arg.contains("org.freedesktop.secrets") || arg.contains("org.kde.kwallet")
            }) =>
        {
            Some("keyring")
        }
        "git" if args.iter().any(|arg| arg.starts_with("credential")) => Some("keyring"),
        "git"
            if git.is_some_and(|i| match args[i] {
                "push" => true,
                "config" => has("--global") || has("--system"),
                "remote" => has("set-url"),
                _ => false,
            }) =>
        {
            Some(if has("push") {
                "git push"
            } else if has("config") {
                "git config --global"
            } else {
                "git remote set-url"
            })
        }
        "pacman" | "apt" | "apt-get" | "brew" | "dnf" | "yum" | "zypper" | "apk" => {
            Some("package manager")
        }
        "cargo" if has("install") => Some("package install"),
        "npm"
            if (short('g') || has("--global") || has("--location=global"))
                && args
                    .iter()
                    .any(|arg| matches!(*arg, "install" | "i" | "add" | "update" | "up")) =>
        {
            Some("package install")
        }
        _ if program.starts_with("pip") && (has("install") || has("uninstall")) => {
            Some("package install")
        }
        _ if program.starts_with("python")
            && (has("pip") || has("pip3"))
            && (has("install") || has("uninstall")) =>
        {
            Some("package install")
        }
        _ => None,
    };
    if let Some(rule) = rule {
        return Some(hard(rule));
    }

    let recursive_delete = program == "rm"
        && (short('r') || short('R') || has("--recursive"))
        && (short('f') || has("--force"));
    let recursive_permissions =
        matches!(program, "chmod" | "chown") && (short('R') || has("--recursive"));
    for arg in args {
        let value = arg.split_once('=').map_or(*arg, |(_, value)| value);
        if recursive_delete || recursive_permissions {
            let parent = Path::new(value);
            let path = cwd.join(expand_home(value));
            let resolved = resolve_ancestor(&path);
            if !value.starts_with('-')
                && (!resolved.starts_with(root)
                    || recursive_delete
                        && (value.starts_with(['~', '$'])
                            || parent
                                .components()
                                .any(|component| component == Component::ParentDir)))
            {
                return Some(hard(if recursive_delete {
                    "rm -rf outside project"
                } else {
                    "recursive permissions outside project"
                }));
            }
        }
        let writing = matches!(
            program,
            "rm" | "mv"
                | "cp"
                | "install"
                | "touch"
                | "tee"
                | "mkdir"
                | "rmdir"
                | "chmod"
                | "chown"
                | "truncate"
                | "unlink"
                | "ln"
                | "dd"
        ) || (matches!(program, "sed" | "perl")
            && (short('i') || starts("--in-place")));
        if let Some(reason) = inspect_path(root, cwd, value, writing) {
            return Some(reason);
        }
    }

    if shell(program) {
        if let Some(script) = args.windows(2).find(|pair| {
            pair[0].starts_with('-') && !pair[0].starts_with("--") && pair[0][1..].contains('c')
        }) {
            if shell_depth == 0 {
                return inspect(root, cwd, script[1], shell_depth + 1);
            }
            return None;
        }
    }
    if wrapper_depth < 4 && program == "xargs" {
        let mut rest = args;
        while let Some((first, tail)) = rest.split_first() {
            rest = match *first {
                "--" => break,
                "-a" | "-d" | "-E" | "-I" | "-L" | "-n" | "-P" | "-s" | "--arg-file"
                | "--delimiter" | "--eof" | "--replace" | "--max-lines" | "--max-args"
                | "--max-procs" | "--max-chars" => tail.get(1..).unwrap_or_default(),
                flag if flag.starts_with('-') => tail,
                _ => {
                    return inspect_words(root, cwd, rest, shell_depth, wrapper_depth + 1, fetched)
                }
            };
        }
        return inspect_words(
            root,
            cwd,
            rest.strip_prefix(&["--"]).unwrap_or(rest),
            shell_depth,
            wrapper_depth + 1,
            fetched,
        );
    }
    if wrapper_depth < 4 && program == "find" {
        for (index, arg) in args.iter().enumerate() {
            if matches!(*arg, "-exec" | "-execdir" | "-ok" | "-okdir") {
                let nested = &args[index + 1..];
                let end = nested
                    .iter()
                    .position(|arg| matches!(*arg, ";" | "+"))
                    .unwrap_or(nested.len());
                if let Some(reason) = inspect_words(
                    root,
                    cwd,
                    &nested[..end],
                    shell_depth,
                    wrapper_depth + 1,
                    fetched,
                ) {
                    return Some(reason);
                }
            }
        }
    }
    None
}

fn expand_home(value: &str) -> PathBuf {
    let stripped = ["~", "$HOME", "${HOME}"].into_iter().find_map(|prefix| {
        value
            .strip_prefix(prefix)
            .filter(|rest| rest.is_empty() || rest.starts_with('/'))
    });
    if let Some(rest) = stripped {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        return home.join(rest.trim_start_matches('/'));
    }
    PathBuf::from(value)
}

fn normalise(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        if component == Component::ParentDir && output.file_name().is_some() {
            output.pop();
        } else {
            output.push(component);
        }
    }
    output
}

// Commands such as mkdir -p can write through a link even when several leaf components
// do not exist yet. Canonicalize the existing ancestor before reattaching those components.
fn resolve_ancestor(path: &Path) -> PathBuf {
    let mut ancestor = path;
    let mut missing = Vec::new();
    loop {
        if let Ok(mut resolved) = ancestor.canonicalize() {
            for component in missing.into_iter().rev() {
                resolved.push(component);
            }
            return normalise(&resolved);
        }
        let (Some(parent), Some(component)) =
            (ancestor.parent(), ancestor.components().next_back())
        else {
            return normalise(path);
        };
        missing.push(component);
        ancestor = parent;
    }
}

fn inspect_path(root: &Path, cwd: &Path, value: &str, write: bool) -> Option<DenyReason> {
    let path = cwd.join(expand_home(value));
    if let Some(reason) = protected(&path, write) {
        return Some(reason);
    }
    let resolved = resolve_ancestor(&path);
    protected(&resolved, write).or_else(|| {
        (write && (has_git(&path) || has_git(&resolved) || !resolved.starts_with(root)))
            .then(|| outside(&path))
    })
}

/// Validate a patch path before joining it to the project.
pub(crate) fn relative_patch_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}
