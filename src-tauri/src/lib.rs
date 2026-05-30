mod ansi;
mod bidi;
mod pty;
mod terminal;

use std::sync::Mutex;

use serde::Serialize;
use tauri::{Manager, State};

use crate::{bidi::visual_line, pty::PtySession, terminal::{CellStyle, TerminalFrame}};

#[derive(Default)]
struct AppState {
    session: Mutex<Option<PtySession>>,
}

#[derive(Debug, Serialize)]
struct CommandError {
    message: String,
}

impl From<anyhow::Error> for CommandError {
    fn from(error: anyhow::Error) -> Self {
        Self { message: error.to_string() }
    }
}

#[tauri::command]
async fn start_terminal(app: tauri::AppHandle, state: State<'_, AppState>, cols: usize, rows: usize) -> Result<(), CommandError> {
    let mut session = state.session.lock().expect("session lock poisoned");
    if let Some(existing) = session.as_mut() {
        existing.kill();
    }
    *session = Some(PtySession::spawn(app, cols, rows)?);
    Ok(())
}

#[tauri::command]
async fn write_terminal(state: State<'_, AppState>, input: String) -> Result<(), CommandError> {
    let session = state.session.lock().expect("session lock poisoned");
    if let Some(session) = session.as_ref() {
        session.write(&input)?;
    }
    Ok(())
}

#[tauri::command]
async fn resize_terminal(state: State<'_, AppState>, cols: usize, rows: usize) -> Result<(), CommandError> {
    let mut session = state.session.lock().expect("session lock poisoned");
    if let Some(session) = session.as_mut() {
        session.resize(cols, rows)?;
    }
    Ok(())
}

#[tauri::command]
async fn stop_terminal(state: State<'_, AppState>) -> Result<(), CommandError> {
    let mut session = state.session.lock().expect("session lock poisoned");
    if let Some(session) = session.as_mut() {
        session.kill();
    }
    *session = None;
    Ok(())
}

#[tauri::command]
async fn shape_preview(text: String) -> TerminalFrame {
    let style = CellStyle { fg: Some([228, 238, 247]), ..CellStyle::default() };
    let logical: Vec<_> = text.chars().map(|ch| (ch, style.clone())).collect();
    TerminalFrame {
        cols: logical.len().max(1),
        rows: 1,
        cursor_col: 0,
        cursor_row: 0,
        cursor_visible: false,
        title: "BiDi preview".to_string(),
        lines: vec![visual_line(0, &logical, true)],
    }
}

pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                #[cfg(target_os = "macos")]
                window.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)))?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![start_terminal, write_terminal, resize_terminal, stop_terminal, shape_preview])
        .run(tauri::generate_context!())
        .expect("error while running RTL Terminal");
}
