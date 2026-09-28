mod pty;

use std::sync::Mutex;

use serde::Serialize;
#[cfg(target_os = "macos")]
use tauri::Manager;
use tauri::State;

use crate::pty::{clamp_cols, clamp_rows, PtySession};

#[derive(Default)]
struct AppState {
    session: Mutex<Option<PtySession>>,
}

#[derive(Debug, Serialize)]
struct CommandError {
    message: String,
}

#[derive(Debug, Serialize)]
struct StartTerminalResult {
    #[serde(rename = "sessionId")]
    session_id: u64,
    shell: String,
}

impl From<anyhow::Error> for CommandError {
    fn from(error: anyhow::Error) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

fn session_is_current(active_session_id: Option<u64>, requested_session_id: u64) -> bool {
    active_session_id == Some(requested_session_id)
}

fn stale_session_error(requested_session_id: u64) -> CommandError {
    CommandError {
        message: format!("terminal session {requested_session_id} is no longer active"),
    }
}

#[tauri::command]
async fn start_terminal(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    cols: usize,
    rows: usize,
    session_id: u64,
) -> Result<StartTerminalResult, CommandError> {
    let mut session = state.session.lock().map_err(|_| CommandError {
        message: "terminal session lock poisoned".to_string(),
    })?;
    if let Some(mut existing) = session.take() {
        existing.shutdown()?;
    }
    let next = PtySession::spawn(app, session_id, clamp_cols(cols), clamp_rows(rows))?;
    let result = StartTerminalResult {
        session_id: next.session_id(),
        shell: next.shell_name().to_string(),
    };
    *session = Some(next);
    Ok(result)
}

#[tauri::command]
async fn write_terminal(
    state: State<'_, AppState>,
    session_id: u64,
    input: String,
) -> Result<(), CommandError> {
    let session = state.session.lock().map_err(|_| CommandError {
        message: "terminal session lock poisoned".to_string(),
    })?;
    if !session_is_current(session.as_ref().map(PtySession::session_id), session_id) {
        return Err(stale_session_error(session_id));
    }
    if let Some(session) = session.as_ref() {
        session.write(&input)?;
    }
    Ok(())
}

#[tauri::command]
async fn interrupt_terminal(
    state: State<'_, AppState>,
    session_id: u64,
) -> Result<(), CommandError> {
    let session = state.session.lock().map_err(|_| CommandError {
        message: "terminal session lock poisoned".to_string(),
    })?;
    if !session_is_current(session.as_ref().map(PtySession::session_id), session_id) {
        return Err(stale_session_error(session_id));
    }
    if let Some(session) = session.as_ref() {
        session.interrupt()?;
    }
    Ok(())
}

#[tauri::command]
async fn resize_terminal(
    state: State<'_, AppState>,
    session_id: u64,
    cols: usize,
    rows: usize,
) -> Result<(), CommandError> {
    let mut session = state.session.lock().map_err(|_| CommandError {
        message: "terminal session lock poisoned".to_string(),
    })?;
    if !session_is_current(session.as_ref().map(PtySession::session_id), session_id) {
        return Err(stale_session_error(session_id));
    }
    if let Some(session) = session.as_mut() {
        session.resize(clamp_cols(cols), clamp_rows(rows))?;
    }
    Ok(())
}

#[tauri::command]
async fn stop_terminal(state: State<'_, AppState>, session_id: u64) -> Result<(), CommandError> {
    let mut session = state.session.lock().map_err(|_| CommandError {
        message: "terminal session lock poisoned".to_string(),
    })?;
    if !session_is_current(session.as_ref().map(PtySession::session_id), session_id) {
        return Err(stale_session_error(session_id));
    }
    if let Some(mut session) = session.take() {
        session.shutdown()?;
    }
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .setup(|_app| {
            #[cfg(target_os = "macos")]
            if let Some(window) = _app.get_webview_window("main") {
                window.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)))?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_terminal,
            write_terminal,
            interrupt_terminal,
            resize_terminal,
            stop_terminal
        ])
        .run(tauri::generate_context!())
        .expect("error while running RTL Terminal");
}

#[cfg(test)]
mod tests {
    use super::session_is_current;

    #[test]
    fn only_the_active_session_matches() {
        assert!(session_is_current(Some(7), 7));
        assert!(!session_is_current(Some(7), 6));
        assert!(!session_is_current(None, 7));
    }
}
