use std::{
    env,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};

use anyhow::{anyhow, Result};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

#[derive(Clone, Debug, Serialize)]
pub struct PtyDataEvent {
    #[serde(rename = "sessionId")]
    pub session_id: u64,
    pub data: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct PtyExitEvent {
    #[serde(rename = "sessionId")]
    pub session_id: u64,
}

pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn portable_pty::Child + Send>,
    session_id: u64,
    shell_name: String,
}

impl PtySession {
    pub fn spawn(app: AppHandle, session_id: u64, cols: usize, rows: usize) -> Result<Self> {
        let size = pty_size(cols, rows);
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(size)?;

        let (shell, shell_args) = default_shell();
        let shell_name = shell_name(&shell);
        let mut cmd = CommandBuilder::new(shell);
        for arg in shell_args {
            cmd.arg(arg);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "Twitty");
        cmd.env("FORCE_COLOR", "1");
        cmd.env("CLICOLOR_FORCE", "1");
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer()?));

        thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            let mut decoder = Utf8ChunkDecoder::default();

            loop {
                let count = match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => count,
                    Err(_) => break,
                };

                for data in decoder.push(&buffer[..count]) {
                    let _ = app.emit("terminal://data", PtyDataEvent { session_id, data });
                }
            }

            if let Some(data) = decoder.finish() {
                let _ = app.emit("terminal://data", PtyDataEvent { session_id, data });
            }

            let _ = app.emit("terminal://exited", PtyExitEvent { session_id });
        });

        Ok(Self {
            master: pair.master,
            writer,
            child,
            session_id,
            shell_name,
        })
    }

    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    pub fn shell_name(&self) -> &str {
        &self.shell_name
    }

    pub fn write(&self, input: &str) -> Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("PTY writer lock poisoned"))?;
        writer.write_all(input.as_bytes())?;
        writer.flush()?;
        Ok(())
    }

    pub fn interrupt(&self) -> Result<()> {
        // Writing ETX works across Unix PTYs and Windows ConPTY without fragile
        // process-group console attachment side effects.
        self.write("\u{3}")
    }

    pub fn resize(&mut self, cols: usize, rows: usize) -> Result<()> {
        self.master.resize(pty_size(cols, rows))?;
        Ok(())
    }

    pub fn shutdown(&mut self) -> Result<()> {
        let kill_result = self.child.kill();
        let wait_result = self.child.wait();

        match (kill_result, wait_result) {
            (_, Ok(_)) => Ok(()),
            (Ok(()), Err(wait_error)) => Err(wait_error.into()),
            (Err(kill_error), Err(wait_error)) => Err(anyhow!(
                "failed to stop PTY child ({kill_error}); wait also failed ({wait_error})"
            )),
        }
    }
}

pub fn clamp_cols(cols: usize) -> usize {
    cols.clamp(1, 500)
}

pub fn clamp_rows(rows: usize) -> usize {
    rows.clamp(1, 300)
}

fn pty_size(cols: usize, rows: usize) -> PtySize {
    let cols = clamp_cols(cols) as u16;
    let rows = clamp_rows(rows) as u16;
    PtySize {
        rows,
        cols,
        pixel_width: cols.saturating_mul(8),
        pixel_height: rows.saturating_mul(18),
    }
}

#[derive(Default)]
struct Utf8ChunkDecoder {
    pending: Vec<u8>,
}

impl Utf8ChunkDecoder {
    fn push(&mut self, input: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(input);
        let mut output = Vec::new();

        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(valid) => {
                    if !valid.is_empty() {
                        output.push(valid.to_string());
                    }
                    self.pending.clear();
                    break;
                }
                Err(error) => {
                    let valid_up_to = error.valid_up_to();
                    if valid_up_to > 0 {
                        let valid = std::str::from_utf8(&self.pending[..valid_up_to])
                            .expect("UTF-8 validator returned a valid prefix")
                            .to_string();
                        output.push(valid);
                        self.pending.drain(..valid_up_to);
                        continue;
                    }

                    if let Some(error_len) = error.error_len() {
                        output.push("\u{FFFD}".to_string());
                        self.pending.drain(..error_len);
                        continue;
                    }

                    // Incomplete multibyte sequence at the end; keep it for the next read.
                    break;
                }
            }
        }

        output
    }

    fn finish(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }

        let output = String::from_utf8_lossy(&self.pending).to_string();
        self.pending.clear();
        Some(output)
    }
}

fn default_shell() -> (String, Vec<String>) {
    if let Ok(shell) = env::var("TWITTY_SHELL") {
        if !shell.trim().is_empty() {
            return (shell, vec![]);
        }
    }

    #[cfg(windows)]
    {
        if command_exists("pwsh.exe") {
            return (
                "pwsh.exe".to_string(),
                vec!["-NoLogo".to_string(), "-NoProfile".to_string()],
            );
        }
        if command_exists("powershell.exe") {
            return (
                "powershell.exe".to_string(),
                vec!["-NoLogo".to_string(), "-NoProfile".to_string()],
            );
        }
        if let Ok(comspec) = env::var("COMSPEC") {
            if !comspec.trim().is_empty() {
                return (comspec, vec![]);
            }
        }
        ("cmd.exe".to_string(), vec![])
    }

    #[cfg(not(windows))]
    {
        if let Ok(shell) = env::var("SHELL") {
            if Path::new(&shell).exists() {
                return (shell, vec![]);
            }
        }

        #[cfg(target_os = "macos")]
        {
            if Path::new("/bin/zsh").exists() {
                return ("/bin/zsh".to_string(), vec![]);
            }
        }

        if Path::new("/bin/bash").exists() {
            return ("/bin/bash".to_string(), vec![]);
        }
        ("/bin/sh".to_string(), vec![])
    }
}

fn shell_name(shell: &str) -> String {
    Path::new(shell)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or(shell)
        .to_string()
}

#[cfg(windows)]
fn command_exists(command: &str) -> bool {
    if Path::new(command).components().count() > 1 {
        return Path::new(command).exists();
    }

    let path = match env::var_os("PATH") {
        Some(path) => path,
        None => return false,
    };
    let pathext = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    let command_path = Path::new(command);
    let has_extension = command_path.extension().is_some();

    for dir in env::split_paths(&path) {
        if has_extension && dir.join(command).exists() {
            return true;
        }
        for ext in pathext.split(';') {
            let ext = ext.trim();
            if ext.is_empty() {
                continue;
            }
            let mut candidate = PathBuf::from(&dir);
            candidate.push(format!("{command}{ext}"));
            if candidate.exists() {
                return true;
            }
        }
    }

    false
}

#[cfg(not(windows))]
#[allow(dead_code)]
fn command_exists(command: &str) -> bool {
    if Path::new(command).components().count() > 1 {
        return Path::new(command).exists();
    }

    env::var_os("PATH")
        .map(|path| env::split_paths(&path).any(|dir| dir.join(command).exists()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{clamp_cols, clamp_rows, shell_name, Utf8ChunkDecoder};

    #[test]
    fn clamps_terminal_dimensions() {
        assert_eq!(clamp_cols(0), 1);
        assert_eq!(clamp_cols(80), 80);
        assert_eq!(clamp_cols(900), 500);
        assert_eq!(clamp_rows(0), 1);
        assert_eq!(clamp_rows(24), 24);
        assert_eq!(clamp_rows(900), 300);
    }

    #[test]
    fn extracts_shell_name_from_paths() {
        assert_eq!(shell_name("pwsh.exe"), "pwsh");
        assert_eq!(
            shell_name(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            "pwsh"
        );
        assert_eq!(shell_name("/bin/bash"), "bash");
    }

    #[test]
    fn decodes_utf8_split_across_chunks() {
        let mut decoder = Utf8ChunkDecoder::default();
        let bytes = "مرحبا".as_bytes();
        assert!(decoder.push(&bytes[..1]).is_empty());

        let mut output = decoder.push(&bytes[1..5]);
        output.extend(decoder.push(&bytes[5..]));
        assert_eq!(output.concat(), "مرحبا");
        assert_eq!(decoder.finish(), None);
    }

    #[test]
    fn preserves_valid_text_around_invalid_utf8() {
        let mut decoder = Utf8ChunkDecoder::default();
        let output = decoder.push(&[b'o', b'k', 0xff, b'!']).concat();
        assert_eq!(output, "ok\u{FFFD}!");
    }

    #[test]
    fn finishes_incomplete_utf8_with_replacement_character() {
        let mut decoder = Utf8ChunkDecoder::default();
        assert!(decoder.push(&[0xd9]).is_empty());
        assert_eq!(decoder.finish().as_deref(), Some("\u{FFFD}"));
        assert_eq!(decoder.finish(), None);
    }
}
