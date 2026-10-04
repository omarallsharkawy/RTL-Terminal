use std::env;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
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
        cmd.env("LANG", "en_US.UTF-8");

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn shell in PTY")?;
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .context("Failed to clone PTY reader")?;
        let writer = Arc::new(Mutex::new(
            pair.master
                .take_writer()
                .context("Failed to take PTY writer")?,
        ));

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

    pub fn spawn_command<F, E>(
        cols: u16,
        rows: u16,
        command: &str,
        args: &[String],
        mut on_data: F,
        mut on_exit: E,
    ) -> Result<Self>
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

        let mut cmd = CommandBuilder::new(command);
        for arg in args {
            cmd.arg(arg);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "Twitty");
        cmd.env("LANG", "en_US.UTF-8");

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn command in PTY")?;
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .context("Failed to clone PTY reader")?;
        let writer = Arc::new(Mutex::new(
            pair.master
                .take_writer()
                .context("Failed to take PTY writer")?,
        ));

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
        if let Ok(mut writer) = self.writer.lock() {
            writer.write_all(data)?;
            writer.flush()?;
        }
        Ok(())
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
        if !shell.trim().is_empty() {
            return (shell, vec![]);
        }
    }

    #[cfg(windows)]
    {
        if let Ok(comspec) = env::var("COMSPEC") {
            if !comspec.trim().is_empty() {
                return (comspec, vec![]);
            }
        }
        ("powershell.exe".to_string(), vec!["-NoLogo".to_string()])
    }

    #[cfg(not(windows))]
    {
        if let Ok(shell) = env::var("SHELL") {
            if !shell.trim().is_empty() {
                return (shell, vec!["-l".to_string()]);
            }
        }
        ("/bin/bash".to_string(), vec!["-l".to_string()])
    }
}
