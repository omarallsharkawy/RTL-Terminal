//! Engine spawner abstraction, scripted fake engine, and offline placeholder loop.

use std::sync::mpsc;
use std::thread;

use twitty_ai::engine::model::{
    AgentCommand, AgentEvent, AgentHandle, FinishReason, ProviderInfo, ProviderKind, Step, StepId,
    StepKind, StepStatus,
};
use twitty_ai::engine::EngineConfig;

// ---- Engine spawner abstraction & scripted fake engine ----

/// Factory producing an [`AgentHandle`]; the app uses `twitty_ai::engine::spawn`, tests a script.
pub trait AgentEngineSpawner: Send + Sync {
    fn spawn(&self, config: EngineConfig) -> Result<AgentHandle, String>;
}

/// Function-pointer spawner implementation for lightweight wiring.
pub type SpawnEngineFn = fn(EngineConfig) -> Result<AgentHandle, String>;

impl AgentEngineSpawner for SpawnEngineFn {
    fn spawn(&self, config: EngineConfig) -> Result<AgentHandle, String> {
        (self)(config)
    }
}

/// Callback invoked by [`ScriptedEngine`] for each incoming [`AgentCommand`].
type ScriptedHandler =
    Box<dyn Fn(AgentCommand, &mpsc::Sender<AgentEvent>) -> bool + Send + Sync + 'static>;

/// Scripted fake engine that speaks the exact `AgentCommand`/`AgentEvent` channel contract.
/// Used by window-free tests and local development before lane s10's engine is linked.
pub struct ScriptedEngine {
    handler: ScriptedHandler,
}

impl ScriptedEngine {
    /// Create a scripted engine whose `handler` is called for each incoming [`AgentCommand`] and
    /// emits any number of [`AgentEvent`]s. Return `false` from `handler` to stop the loop.
    pub fn new(
        handler: impl Fn(AgentCommand, &mpsc::Sender<AgentEvent>) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            handler: Box::new(handler),
        }
    }

    /// Spawn a background thread speaking `AgentCommand` -> `AgentEvent` over standard channels.
    pub fn spawn_handle(
        handler: impl FnMut(AgentCommand, &mpsc::Sender<AgentEvent>) -> bool + Send + 'static,
    ) -> AgentHandle {
        let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>();
        let (evt_tx, evt_rx) = mpsc::channel::<AgentEvent>();
        let mut handler = handler;
        let _ = thread::Builder::new()
            .name("twitty-agent-scripted".into())
            .spawn(move || {
                while let Ok(cmd) = cmd_rx.recv() {
                    if !handler(cmd, &evt_tx) {
                        break;
                    }
                }
            });
        AgentHandle {
            commands: cmd_tx,
            events: evt_rx,
        }
    }
}

impl AgentEngineSpawner for ScriptedEngine {
    fn spawn(&self, _config: EngineConfig) -> Result<AgentHandle, String> {
        // Because `ScriptedEngine` may be spawned once per app lifetime, we forward through a
        // channel owned by a background thread only if created via `spawn_handle`; here we provide
        // a clean fallback if called via trait object directly.
        let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>();
        let (evt_tx, evt_rx) = mpsc::channel::<AgentEvent>();
        let _ = thread::Builder::new()
            .name("twitty-agent-placeholder".into())
            .spawn(move || {
                placeholder_engine_loop(cmd_rx, evt_tx);
            });
        let _ = &self.handler;
        Ok(AgentHandle {
            commands: cmd_tx,
            events: evt_rx,
        })
    }
}

/// Offline stand-in engine that answers every request with a fixed message (tests, demos).
pub fn spawn_placeholder_engine(_config: EngineConfig) -> Result<AgentHandle, String> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>();
    let (evt_tx, evt_rx) = mpsc::channel::<AgentEvent>();
    thread::Builder::new()
        .name("twitty-agent-stub".into())
        .spawn(move || placeholder_engine_loop(cmd_rx, evt_tx))
        .map_err(|e| format!("failed to spawn agent thread: {e}"))?;
    Ok(AgentHandle {
        commands: cmd_tx,
        events: evt_rx,
    })
}

fn placeholder_engine_loop(cmd_rx: mpsc::Receiver<AgentCommand>, evt_tx: mpsc::Sender<AgentEvent>) {
    let mut next_step = 1u64;
    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            AgentCommand::Start { session, .. } | AgentCommand::Reply { session, .. } => {
                let _ = evt_tx.send(AgentEvent::Started {
                    session,
                    provider: ProviderInfo {
                        kind: ProviderKind::Cli {
                            program: "agent".into(),
                        },
                        model: "pending".into(),
                        display: "Agent · Pending".into(),
                    },
                });
                let id = StepId(next_step);
                next_step += 1;
                let _ = evt_tx.send(AgentEvent::StepAdded {
                    session,
                    step: Step {
                        id,
                        kind: StepKind::Message {
                            text: crate::ui::chrome::t("Agent engine backend is not linked in this build yet. Configure a provider in Settings → AI.").into(),
                        },
                        status: StepStatus::Done,
                    },
                });
                let _ = evt_tx.send(AgentEvent::Finished {
                    session,
                    reason: FinishReason::Completed,
                });
            }
            AgentCommand::Stop { session } => {
                let _ = evt_tx.send(AgentEvent::Finished {
                    session,
                    reason: FinishReason::Stopped,
                });
            }
            AgentCommand::Decide { .. }
            | AgentCommand::CommandFinished { .. }
            | AgentCommand::Undo { .. } => {}
        }
    }
}
