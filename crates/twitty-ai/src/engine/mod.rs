//! The Twitty agent engine and the channel contract shared by the app and project memory.

mod config;
mod files;
mod journal;
pub mod model;
pub mod patch;
pub mod policy;
pub mod provider;
mod session;
mod shell;
mod tools;

#[doc(hidden)]
pub mod testing;

use std::collections::HashMap;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use twitty_store::SecretStore;

pub use config::{estimated_cost_micros, EngineConfig, ProviderModels, DEFAULT_OLLAMA_URL};
pub use model::AgentHandle;
pub use provider::Provider;

use model::{AgentCommand, AgentEvent, FinishReason, SessionId};
use provider::BuiltInProvider;
use session::{Notice, Session};

/// Start the agent engine. Sending `Start` is the app's explicit opt-in for this request.
pub fn spawn(config: EngineConfig, secrets: Arc<dyn SecretStore + Send + Sync>) -> AgentHandle {
    spawn_provider(config, Arc::new(BuiltInProvider { secrets }))
}

fn spawn_provider(config: EngineConfig, provider: Arc<dyn Provider>) -> AgentHandle {
    let (commands, requests) = mpsc::channel();
    let (send, events) = mpsc::channel();
    thread::Builder::new()
        .name("twitty-agent".into())
        .spawn(move || {
            run(config, provider, requests, &send);
        })
        .expect("could not start Twitty agent engine");
    AgentHandle { commands, events }
}

fn run(
    config: EngineConfig,
    provider: Arc<dyn Provider>,
    requests: mpsc::Receiver<AgentCommand>,
    events: &mpsc::Sender<AgentEvent>,
) {
    let (notice_tx, notice_rx) = mpsc::channel();
    let mut sessions = HashMap::<SessionId, Session>::new();
    loop {
        let busy = sessions.values().any(Session::busy);
        let received = if busy {
            requests.recv_timeout(Duration::from_millis(10))
        } else {
            requests
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        };
        match received {
            Ok(command) => {
                handle(command, &config, events, &mut sessions);
                for command in requests.try_iter().take(64) {
                    handle(command, &config, events, &mut sessions);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        for (id, generation, notice) in notice_rx.try_iter().take(64) {
            if let Some(session) = sessions.get_mut(&id) {
                session.provider_notice(generation, notice, &config, events);
            }
        }
        for session in sessions.values_mut() {
            session.advance(&config, &provider, &notice_tx, events);
        }
    }
}

fn handle(
    command: AgentCommand,
    config: &EngineConfig,
    events: &mpsc::Sender<AgentEvent>,
    sessions: &mut HashMap<SessionId, Session>,
) {
    match command {
        AgentCommand::Start {
            session,
            request,
            context,
            cwd,
        } => {
            if let std::collections::hash_map::Entry::Vacant(entry) = sessions.entry(session) {
                match Session::start(session, request, context, &cwd, config, events) {
                    Ok(state) => {
                        entry.insert(state);
                    }
                    Err(message) => {
                        let _ = events.send(AgentEvent::Finished {
                            session,
                            reason: FinishReason::Error(message.into()),
                        });
                    }
                }
            }
        }
        AgentCommand::Reply { session, text } => {
            if let Some(state) = sessions.get_mut(&session) {
                state.reply(&text, events);
            }
        }
        AgentCommand::Decide {
            session,
            step,
            decision,
        } => {
            if let Some(state) = sessions.get_mut(&session) {
                state.decide(step, decision, config, events);
            }
        }
        AgentCommand::CommandFinished {
            session,
            step,
            exit,
            duration_ms,
            output,
        } => {
            if let Some(state) = sessions.get_mut(&session) {
                state.command_finished(step, exit, duration_ms, &output, events);
            }
        }
        AgentCommand::Undo { session, step } => {
            if let Some(state) = sessions.get_mut(&session) {
                state.undo(step, events);
            }
        }
        AgentCommand::Stop { session } => {
            if let Some(state) = sessions.get_mut(&session) {
                state.stop(events);
            }
        }
    }
}

type Notices = mpsc::Sender<(SessionId, u64, Notice)>;

fn bounded(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn clean(text: &str) -> String {
    // Redact before trimming or removing controls so a token split by the size limit stays hidden.
    twitty_core::redact::redact(text)
        .chars()
        .filter(|ch| {
            (!ch.is_control() || matches!(ch, '\n' | '\r' | '\t'))
                && !twitty_core::terminal::is_bidi_control(*ch)
                && !matches!(ch, '\u{2028}' | '\u{2029}')
        })
        .collect()
}

fn fenced(label: &str, body: &str) -> String {
    let body = clean(body);
    let fence = crate::agent::safe_fence(&body);
    format!("{label} (untrusted data)\n{fence}text\n{body}\n{fence}")
}
