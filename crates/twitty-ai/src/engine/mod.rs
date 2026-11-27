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

const MAX_RETAINED_FINISHED_SESSIONS: usize = 8;

struct SessionSlot {
    session: Session,
    last_used: u64,
}

fn run(
    config: EngineConfig,
    provider: Arc<dyn Provider>,
    requests: mpsc::Receiver<AgentCommand>,
    events: &mpsc::Sender<AgentEvent>,
) {
    let (notice_tx, notice_rx) = mpsc::channel();
    let mut sessions = HashMap::<SessionId, SessionSlot>::new();
    let mut tick: u64 = 0;
    loop {
        let busy = sessions.values().any(|slot| slot.session.busy());
        let received = if busy {
            requests.recv_timeout(Duration::from_millis(10))
        } else {
            requests
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        };
        match received {
            Ok(command) => {
                handle(command, &config, events, &mut sessions, &mut tick);
                for command in requests.try_iter().take(64) {
                    handle(command, &config, events, &mut sessions, &mut tick);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        for (id, generation, notice) in notice_rx.try_iter().take(64) {
            if let Some(slot) = sessions.get_mut(&id) {
                tick = tick.wrapping_add(1);
                slot.last_used = tick;
                slot.session
                    .provider_notice(generation, notice, &config, events);
            }
        }
        for slot in sessions.values_mut() {
            slot.session.advance(&config, &provider, &notice_tx, events);
        }
        prune_finished_sessions(&mut sessions);
    }
}

fn prune_finished_sessions(sessions: &mut HashMap<SessionId, SessionSlot>) {
    let finished_count = sessions
        .values()
        .filter(|slot| slot.session.is_finished())
        .count();
    if finished_count <= MAX_RETAINED_FINISHED_SESSIONS {
        return;
    }
    let mut finished: Vec<(SessionId, u64)> = sessions
        .iter()
        .filter(|(_, slot)| slot.session.is_finished())
        .map(|(&id, slot)| (id, slot.last_used))
        .collect();
    finished.sort_by_key(|&(_, last_used)| last_used);
    let to_remove = finished.len() - MAX_RETAINED_FINISHED_SESSIONS;
    for (id, _) in finished.into_iter().take(to_remove) {
        sessions.remove(&id);
    }
}

fn handle(
    command: AgentCommand,
    config: &EngineConfig,
    events: &mpsc::Sender<AgentEvent>,
    sessions: &mut HashMap<SessionId, SessionSlot>,
    tick: &mut u64,
) {
    *tick = tick.wrapping_add(1);
    let now = *tick;
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
                        entry.insert(SessionSlot {
                            session: state,
                            last_used: now,
                        });
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
            if let Some(slot) = sessions.get_mut(&session) {
                slot.last_used = now;
                slot.session.reply(&text, events);
            }
        }
        AgentCommand::Decide {
            session,
            step,
            decision,
        } => {
            if let Some(slot) = sessions.get_mut(&session) {
                slot.last_used = now;
                slot.session.decide(step, decision, config, events);
            }
        }
        AgentCommand::CommandFinished {
            session,
            step,
            exit,
            duration_ms,
            output,
        } => {
            if let Some(slot) = sessions.get_mut(&session) {
                slot.last_used = now;
                slot.session
                    .command_finished(step, exit, duration_ms, &output, events);
            }
        }
        AgentCommand::Undo { session, step } => {
            if let Some(slot) = sessions.get_mut(&session) {
                slot.last_used = now;
                slot.session.undo(step, events);
            }
        }
        AgentCommand::Stop { session } => {
            if let Some(slot) = sessions.get_mut(&session) {
                slot.last_used = now;
                slot.session.stop(events);
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
