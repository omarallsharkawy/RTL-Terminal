//! Window-free engine entry point and a deterministic provider for app tests.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use super::provider::{Message, ProviderError, ProviderRequest, ProviderTurn, ToolCall};
use super::{provider::Cancellation, AgentHandle, EngineConfig, Provider};

#[derive(Clone)]
pub enum ScriptedReply {
    Turn(ProviderTurn),
    Error(ProviderError),
    Delayed(Duration, ProviderTurn),
}

impl From<ProviderTurn> for ScriptedReply {
    fn from(turn: ProviderTurn) -> Self {
        Self::Turn(turn)
    }
}

/// A shared FIFO of scripted replies. Clone it to inspect requests after attaching it to an engine.
#[derive(Clone, Default)]
pub struct ScriptedProvider(Arc<Mutex<Script>>);

#[derive(Default)]
struct Script {
    replies: VecDeque<ScriptedReply>,
    requests: Vec<ProviderRequest>,
}

impl ScriptedProvider {
    pub fn new(replies: impl IntoIterator<Item = impl Into<ScriptedReply>>) -> Self {
        Self(Arc::new(Mutex::new(Script {
            replies: replies.into_iter().map(Into::into).collect(),
            requests: Vec::new(),
        })))
    }

    pub fn spawn(&self, config: EngineConfig) -> AgentHandle {
        spawn_with_provider(config, Arc::new(self.clone()))
    }

    pub fn requests(&self) -> Vec<ProviderRequest> {
        self.0
            .lock()
            .map(|script| script.requests.clone())
            .unwrap_or_default()
    }
}

impl Provider for ScriptedProvider {
    fn complete(
        &self,
        request: &ProviderRequest,
        cancel: &Cancellation,
        text: &mut dyn FnMut(String),
    ) -> Result<ProviderTurn, ProviderError> {
        let reply = {
            let mut script = self.0.lock().map_err(|_| ProviderError::Unavailable)?;
            script.requests.push(request.clone());
            script
                .replies
                .pop_front()
                .ok_or(ProviderError::Unavailable)?
        };
        let turn = match reply {
            ScriptedReply::Turn(turn) => turn,
            ScriptedReply::Error(error) => return Err(error),
            ScriptedReply::Delayed(delay, turn) => {
                let start = Instant::now();
                while start.elapsed() < delay {
                    if cancel.is_cancelled() {
                        return Err(ProviderError::Cancelled);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                turn
            }
        };
        if !turn.text.is_empty() {
            text(turn.text.clone());
        }
        Ok(turn)
    }
}

pub fn spawn_with_provider(config: EngineConfig, provider: Arc<dyn Provider>) -> AgentHandle {
    super::spawn_provider(config, provider)
}
