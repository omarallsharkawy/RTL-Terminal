//! SSH session identity and reconnect policy, independent of the window and renderer.

pub mod bootstrap;
#[cfg(target_os = "linux")]
pub mod daemon;

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub use twitty_core::remote::{
    validate_destination, validate_name, ATTACH_MARKER, BOOTSTRAP_FAILED, SSH_DISCONNECTED,
};

pub const PANE_WORKER: &str = "__ssh-pane";

/// Detect the attach acknowledgment even when it spans multiple PTY reads.
#[derive(Default)]
pub struct AttachMarker(usize);

impl AttachMarker {
    pub fn push(&mut self, bytes: &[u8]) -> bool {
        if self.0 == ATTACH_MARKER.len() {
            return false;
        }
        for &byte in bytes {
            if byte == ATTACH_MARKER[self.0] {
                self.0 += 1;
                if self.0 == ATTACH_MARKER.len() {
                    return true;
                }
            } else {
                self.0 = usize::from(byte == ATTACH_MARKER[0]);
            }
        }
        false
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "SessionFields")]
pub struct Target {
    destination: String,
    name: String,
}

#[derive(Deserialize)]
struct SessionFields {
    destination: String,
    name: String,
}

impl TryFrom<SessionFields> for Target {
    type Error = String;

    fn try_from(fields: SessionFields) -> Result<Self, Self::Error> {
        Self::new(&fields.destination, &fields.name)
    }
}

impl Target {
    pub fn new(destination: &str, name: &str) -> Result<Self, String> {
        validate_destination(destination)?;
        validate_name(name)?;
        Ok(Self {
            destination: destination.to_owned(),
            name: name.to_owned(),
        })
    }

    pub fn for_tab(destination: &str, name: Option<&str>) -> Result<Self, String> {
        if !cfg!(target_os = "linux") {
            return Err("persistent SSH tabs require the Linux build of Twitty".into());
        }
        validate_destination(destination)?;
        if let Some(name) = name {
            return Self::new(destination, name);
        }

        use std::io::Read;
        let mut bytes = [0; 3];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut random| random.read_exact(&mut bytes))
            .map_err(|err| format!("could not generate an SSH session name: {err}"))?;
        let [a, b, c] = bytes;
        Self::new(destination, &format!("twitty-{a:02x}{b:02x}{c:02x}"))
    }

    pub fn destination(&self) -> &str {
        &self.destination
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

pub fn retry_delay(attempt: usize) -> Option<Duration> {
    [1, 2, 4, 8, 16]
        .get(attempt)
        .copied()
        .map(Duration::from_secs)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Disconnected,
}

impl ConnectionState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Disconnected => "Disconnected",
        }
    }
}

pub struct RemotePane {
    pub target: Target,
    pub state: ConnectionState,
    pub waiting_for_enter: bool,
    attempts: usize,
    retry_generation: u64,
    retry_at: Option<(Instant, u64)>,
}

impl RemotePane {
    pub fn new(target: Target) -> Self {
        Self {
            target,
            state: ConnectionState::Connecting,
            waiting_for_enter: false,
            attempts: 0,
            retry_generation: 0,
            retry_at: None,
        }
    }

    pub fn connected(&mut self) {
        self.state = ConnectionState::Connected;
        self.attempts = 0;
        self.retry_generation = self.retry_generation.wrapping_add(1);
        self.retry_at = None;
    }

    pub fn disconnected(&mut self, retry: bool) -> Option<Duration> {
        self.state = ConnectionState::Disconnected;
        self.retry_generation = self.retry_generation.wrapping_add(1);
        self.retry_at = None;
        let delay = retry.then(|| retry_delay(self.attempts)).flatten();
        self.waiting_for_enter = delay.is_none();
        if delay.is_some() {
            self.attempts += 1;
        }
        delay
    }

    /// Schedule a reconnect retry at `now + delay` tagged with a fresh generation token.
    pub fn schedule_retry(&mut self, now: Instant, delay: Duration) -> u64 {
        self.retry_generation = self.retry_generation.wrapping_add(1);
        self.retry_at = Some((now + delay, self.retry_generation));
        self.retry_generation
    }

    /// Whether a scheduled retry with `generation` is still current.
    pub fn may_retry(&self, generation: u64) -> bool {
        self.state == ConnectionState::Disconnected && self.retry_generation == generation
    }

    /// Returns the retry generation once the scheduled retry deadline has elapsed at `now`.
    pub fn take_due_retry(&mut self, now: Instant) -> Option<u64> {
        let (at, generation) = self.retry_at?;
        if !self.may_retry(generation) {
            self.retry_at = None;
            return None;
        }
        if now >= at {
            self.retry_at = None;
            Some(generation)
        } else {
            None
        }
    }

    /// The pending retry deadline for `ControlFlow::WaitUntil`, if still current.
    pub fn next_retry_deadline(&self) -> Option<Instant> {
        let (at, generation) = self.retry_at?;
        self.may_retry(generation).then_some(at)
    }

    pub fn connecting(&mut self) {
        if self.waiting_for_enter {
            self.attempts = 0;
            self.waiting_for_enter = false;
        }
        self.state = ConnectionState::Connecting;
        self.retry_generation = self.retry_generation.wrapping_add(1);
        self.retry_at = None;
    }
}
