use std::env;
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Sender};
use std::thread;

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    /// Writes go through a dedicated thread so a large paste never blocks the UI.
    writer: Sender<Vec<u8>>,
}

impl Pty {
    pub fn spawn<F, E>(cols: u16, rows: u16, mut on_data: F, mut on_exit: E) -> Result<Self>
    where
        F: FnMut(Vec<u8>) + Send + 'static,
        E: FnMut() + Send + 'static,
    {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY pair")?;

        let (shell_cmd, shell_args) = default_shell();
        let mut cmd = CommandBuilder::new(shell_cmd);
        for arg in shell_args {
            cmd.arg(arg);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "Twitty");
        if env::var_os("LANG").is_none() && env::var_os("LC_ALL").is_none() {
            cmd.env("LANG", "en_US.UTF-8");
        }

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn shell in PTY")?;
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .context("Failed to clone PTY reader")?;
        let mut pty_writer = pair
            .master
            .take_writer()
            .context("Failed to take PTY writer")?;
        let (writer, write_rx) = channel::<Vec<u8>>();
        thread::spawn(move || {
            for data in write_rx {
                if pty_writer
                    .write_all(&data)
                    .and_then(|_| pty_writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        });

        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        on_data(buf[..n].to_vec());
                    }
                    Err(_) => break,
                }
            }
            let _ = child.wait();
            on_exit();
        });

        Ok(Self {
            master: pair.master,
            writer,
        })
    }

    pub fn write(&self, data: &[u8]) -> Result<()> {
        self.writer
            .send(data.to_vec())
            .map_err(|_| anyhow::anyhow!("PTY writer closed"))
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to resize PTY")?;
        Ok(())
    }
}

fn default_shell() -> (String, Vec<String>) {
    if let Ok(shell) = env::var("TWITTY_SHELL") {
        let mut parts = shell.split_whitespace().map(str::to_string);
        if let Some(program) = parts.next() {
            return (program, parts.collect());
        }
    }

    #[cfg(windows)]
    {
        // Prefer PowerShell 7, then Windows PowerShell; cmd.exe is only the last resort.
        for candidate in ["pwsh.exe", "powershell.exe"] {
            if find_in_path(candidate).is_some() {
                return (candidate.to_string(), vec!["-NoLogo".to_string()]);
            }
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
            if !shell.trim().is_empty() {
                return (shell, vec![]);
            }
        }
        ("/bin/bash".to_string(), vec![])
    }
}

#[cfg(windows)]
fn find_in_path(program: &str) -> Option<std::path::PathBuf> {
    env::split_paths(&env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|path| path.is_file())
}
