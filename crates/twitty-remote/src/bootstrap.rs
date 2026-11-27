// Adapted from cmux-gtk cmux/src/remote/bootstrap.rs (AGPL-3.0-or-later,
// Copyright (c) 2026 Douglas Soares de Andrade). Changes: Probe and install a sibling Rust binary over SSH with SHA-256 verification.

//! Bootstrap inside the pane's PTY so OpenSSH can prompt on its controlling terminal.

use std::fmt;
use std::path::Path;

use super::Target;

#[derive(Debug)]
pub struct Error {
    pub disconnected: bool,
    message: String,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            disconnected: false,
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

pub trait SshRunner {
    fn run(
        &mut self,
        target: &Target,
        command: &str,
        upload: Option<&Path>,
    ) -> Result<String, Error>;
}

fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\"'\"'"))
}

fn home_path(relative_path: &str) -> String {
    format!("\"$HOME\"/{}", quote(relative_path))
}

fn relative_binary() -> String {
    format!(
        ".cache/twitty/bin/twitty-remote-{}",
        env!("CARGO_PKG_VERSION")
    )
}

/// Paths are rendered into one SSH command. OpenSSH joins command arguments with spaces
/// before the remote shell parses them, even if the local process was spawned with argv.
pub fn attach_args(target: &Target, cols: u16, rows: u16) -> Vec<String> {
    let path = home_path(&relative_binary());
    let name = quote(target.name());
    vec![
        "-t".into(),
        "--".into(),
        target.destination().into(),
        format!("env TWITTY_REMOTE_MARKER=1 {path} attach {name} --cols {cols} --rows {rows}"),
    ]
}

fn linux_platform(output: &str, architecture: &str) -> Result<(), Error> {
    let machine = match architecture {
        "x86" => ["i386", "i486", "i586", "i686"].as_slice(),
        "x86_64" => &["x86_64", "amd64"],
        "aarch64" => &["aarch64", "arm64"],
        "arm" => &["armv7l", "armv8l"],
        "powerpc64" if cfg!(target_endian = "little") => &["ppc64le"],
        "powerpc64" => &["ppc64"],
        "riscv64" => &["riscv64"],
        other => &[other],
    };
    let mut words = output.split_whitespace();
    if words.next() == Some("Linux")
        && words.next().is_some_and(|arch| machine.contains(&arch))
        && words.next().is_none()
    {
        Ok(())
    } else {
        let output: String = output.trim().chars().take(100).collect();
        Err(Error::new(format!(
            "SSH requires a Linux host matching this build's architecture ({architecture}); uname -sm reported {output:?}"
        )))
    }
}

fn hash_word(output: &str) -> Option<&str> {
    let hash = output.split_whitespace().next()?;
    (hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(hash)
}

fn sh(script: &str) -> String {
    format!("sh -c {}", quote(script))
}

/// Probe the host, check its cache, and upload only when the executable or digest differs.
pub fn bootstrap(
    runner: &mut impl SshRunner,
    target: &Target,
    binary: &Path,
    expected_sha256: &str,
    architecture: &str,
) -> Result<(), Error> {
    linux_platform(&runner.run(target, "uname -sm", None)?, architecture)?;
    if hash_word(expected_sha256) != Some(expected_sha256) {
        return Err(Error::new("invalid local SHA-256 digest"));
    }

    let path = home_path(&relative_binary());
    let check = sh(&format!("if test -x {path}; then sha256sum < {path}; fi"));
    let matches = |output: &str| hash_word(output) == Some(expected_sha256);
    if matches(&runner.run(target, &check, None)?) {
        return Ok(());
    }

    // The remote host allocates the temp file so simultaneous bootstraps can both finish.
    // Validate the staged file before replacing the known path, then check the cache again.
    let dir = home_path(".cache/twitty/bin");
    let upload = sh(&format!(
        "set -eu\numask 077\nmkdir -p {dir}\nchmod 700 {dir}\n\
         tmp=$(mktemp {dir}/.twitty-remote.XXXXXXXX)\n\
         trap 'rm -f -- \"$tmp\"' EXIT\n\
         cat > \"$tmp\"\n\
         digest=$(sha256sum < \"$tmp\")\n\
         test \"${{digest%% *}}\" = '{expected_sha256}'\n\
         chmod 700 \"$tmp\"\n\
         mv -fT -- \"$tmp\" {path}"
    ));
    runner.run(target, &upload, Some(binary))?;
    if !matches(&runner.run(target, &check, None)?) {
        return Err(Error::new(
            "remote helper SHA-256 verification failed after upload",
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
struct OpenSsh;

#[cfg(target_os = "linux")]
impl SshRunner for OpenSsh {
    fn run(
        &mut self,
        target: &Target,
        command: &str,
        upload: Option<&Path>,
    ) -> Result<String, Error> {
        use std::fs::File;
        use std::process::{Command, Stdio};

        let stdin = match upload {
            Some(path) => Stdio::from(File::open(path).map_err(|err| Error::new(err.to_string()))?),
            None => Stdio::null(),
        };
        let output = Command::new("ssh")
            .args(["-T", "--", target.destination(), command])
            .stdin(stdin)
            .stderr(Stdio::inherit())
            .output()
            .map_err(|err| Error::new(format!("could not run ssh: {err}")))?;
        if !output.status.success() {
            return Err(Error {
                disconnected: output.status.code() == Some(super::SSH_DISCONNECTED as i32),
                message: format!("SSH bootstrap command exited with {}", output.status),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// Internal entry point of the local pane process. Replace it with SSH after bootstrap.
#[cfg(target_os = "linux")]
pub fn run_in_pane(target: &Target, cols: u16, rows: u16) -> Result<(), Error> {
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    let binary = std::env::current_exe()
        .map_err(|err| Error::new(format!("could not locate Twitty: {err}")))?
        .with_file_name("twitty-remote");
    let bytes = std::fs::read(&binary).map_err(|err| {
        Error::new(format!(
            "could not open sibling helper {}: {err}",
            binary.display()
        ))
    })?;
    let digest = twitty_core::sha256::sha256_hex(&bytes);
    drop(bytes);
    bootstrap(
        &mut OpenSsh,
        target,
        &binary,
        &digest,
        std::env::consts::ARCH,
    )?;
    let err = Command::new("ssh")
        .args(attach_args(target, cols, rows))
        .exec();
    Err(Error::new(format!("could not run ssh: {err}")))
}

#[cfg(not(target_os = "linux"))]
pub fn run_in_pane(_target: &Target, _cols: u16, _rows: u16) -> Result<(), Error> {
    Err(Error::new(
        "persistent SSH tabs require the Linux build of Twitty",
    ))
}
