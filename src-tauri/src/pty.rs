use std::{io::{Read, Write}, sync::{Arc, Mutex}, thread};

#[cfg(windows)]
use std::time::Duration;

use anyhow::{anyhow, Result};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tauri::{AppHandle, Emitter};

pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn portable_pty::Child + Send>,
}

impl PtySession {
    pub fn spawn(app: AppHandle, cols: usize, rows: usize) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: (cols as u16).saturating_mul(8),
            pixel_height: (rows as u16).saturating_mul(18),
        })?;

        let (shell, shell_args) = default_shell();
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
            loop {
                let count = match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => count,
                    Err(_) => break,
                };
                // Emit raw PTY bytes; xterm.js handles ANSI parsing and rendering.
                let chunk = String::from_utf8_lossy(&buffer[..count]).to_string();
                let _ = app.emit("terminal://data", chunk);
            }
            // Shell exited - notify frontend so it can auto-respawn
            let _ = app.emit("terminal://exited", ());
        });

        Ok(Self { master: pair.master, writer, child })
    }

    pub fn write(&self, input: &str) -> Result<()> {
        let mut writer = self.writer.lock().map_err(|_| anyhow!("PTY writer lock poisoned"))?;
        writer.write_all(input.as_bytes())?;
        writer.flush()?;
        Ok(())
    }

    pub fn interrupt(&self) -> Result<()> {
        #[cfg(windows)]
        if let Some(shell_pid) = self.child.process_id() {
            unsafe {
                send_ctrl_c(shell_pid);
            }
        }
        #[cfg(not(windows))]
        self.write("\u{3}")?;

        Ok(())
    }

    pub fn resize(&mut self, cols: usize, rows: usize) -> Result<()> {
        self.master.resize(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: (cols as u16).saturating_mul(8),
            pixel_height: (rows as u16).saturating_mul(18),
        })?;
        Ok(())
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

fn default_shell() -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        ("powershell.exe".to_string(), vec!["-NoLogo".to_string()])
    }
    #[cfg(not(windows))]
    {
        (std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()), vec![])
    }
}

#[cfg(windows)]
extern "system" {
    fn AttachConsole(dwProcessId: u32) -> i32;
    fn FreeConsole() -> i32;
    fn GenerateConsoleCtrlEvent(dwCtrlEvent: u32, dwProcessGroupId: u32) -> i32;
    fn SetConsoleCtrlHandler(HandlerRoutine: Option<unsafe extern "system" fn(u32) -> i32>, Add: i32) -> i32;
}

#[cfg(windows)]
unsafe fn send_ctrl_c(pid: u32) {
    FreeConsole();
    if AttachConsole(pid) != 0 {
        SetConsoleCtrlHandler(None, 1);
        GenerateConsoleCtrlEvent(0, 0); // 0 is CTRL_C_EVENT
        thread::sleep(Duration::from_millis(50));
        FreeConsole();
        SetConsoleCtrlHandler(None, 0);
    }
}
