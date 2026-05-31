use std::{io::{Read, Write}, process::Command, sync::{Arc, Mutex}, thread, time::Duration};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use anyhow::{anyhow, Result};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tauri::{AppHandle, Emitter};

use crate::{ansi::AnsiParser, terminal::TerminalGrid};

pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn portable_pty::Child + Send>,
    grid: Arc<Mutex<TerminalGrid>>,
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

        let shell = default_shell();
        let mut cmd = CommandBuilder::new(shell);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "Twitty");
        cmd.env("FORCE_COLOR", "1");
        cmd.env("CLICOLOR_FORCE", "1");
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer()?));
        let grid = Arc::new(Mutex::new(TerminalGrid::new(cols, rows)));
        let read_grid = Arc::clone(&grid);

        thread::spawn(move || {
            let mut parser = AnsiParser::default();
            let mut buffer = [0_u8; 8192];
            loop {
                let count = match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => count,
                    Err(_) => break,
                };
                let chunk = String::from_utf8_lossy(&buffer[..count]).to_string();
                let _ = app.emit("terminal://data", chunk.clone());
                let frame = {
                    let mut grid = read_grid.lock().expect("terminal grid poisoned");
                    parser.push(&chunk, &mut grid);
                    grid.frame()
                };
                let _ = app.emit("terminal://frame", frame);
            }
        });

        Ok(Self { master: pair.master, writer, child, grid })
    }

    pub fn write(&self, input: &str) -> Result<()> {
        let mut writer = self.writer.lock().map_err(|_| anyhow!("PTY writer lock poisoned"))?;
        writer.write_all(input.as_bytes())?;
        writer.flush()?;
        Ok(())
    }

    pub fn interrupt(&self) -> Result<()> {
        self.write("\u{3}")?;

        #[cfg(windows)]
        if let Some(shell_pid) = self.child.process_id() {
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(650));
                kill_descendants(shell_pid);
            });
        }

        Ok(())
    }

    pub fn resize(&mut self, cols: usize, rows: usize) -> Result<()> {
        self.master.resize(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: (cols as u16).saturating_mul(8),
            pixel_height: (rows as u16).saturating_mul(18),
        })?;
        if let Ok(mut grid) = self.grid.lock() {
            grid.resize(cols, rows);
        }
        Ok(())
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

fn default_shell() -> String {
    #[cfg(windows)]
    {
        "powershell.exe".to_string()
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
    }
}

#[cfg(windows)]
fn kill_descendants(root_pid: u32) {
    let script = format!(
        r#"
$log = 'C:\Users\Administrator\.gemini\antigravity\brain\3793a041-eaad-4f9d-86b0-80e2815ce2a2\debug_kill.log'
"--- Kill attempt at $(Get-Date) ---" | Out-File -FilePath $log -Append
$root = {root_pid}
"Root PID: $root" | Out-File -FilePath $log -Append
try {{
  $children = Get-CimInstance Win32_Process -Filter "ParentProcessId=$root" -ErrorAction Stop
  "Found $($children.Count) direct children" | Out-File -FilePath $log -Append
  $children | ForEach-Object {{
    "Child: $($_.Name) ($($_.ProcessId))" | Out-File -FilePath $log -Append
    if ($_.Name -ne 'conhost.exe' -and $_.Name -ne 'openconsole.exe' -and $_.ProcessId -ne $root) {{
      "Killing $($_.Name) ($($_.ProcessId)) using taskkill" | Out-File -FilePath $log -Append
      $out = taskkill /F /T /PID $_.ProcessId 2>&1
      "Taskkill output: $out" | Out-File -FilePath $log -Append
    }} else {{
      "Skipping $($_.Name) ($($_.ProcessId)) (filtered out)" | Out-File -FilePath $log -Append
    }}
  }}
}} catch {{
  "Error during query or execution: $_" | Out-File -FilePath $log -Append
}}
"#
    );

    let _ = Command::new("powershell.exe")
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .status();
}



