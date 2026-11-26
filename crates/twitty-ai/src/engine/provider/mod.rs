//! Transport-independent provider request, response and cancellation types.

mod api;
mod cli;
mod http;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::de::{Error, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};
use twitty_store::SecretStore;

use super::journal::{sanitized, scrub};
use super::model::{ProviderKind, Usage};
use super::tools::Tool;
use crate::ai::AiCancel;

pub use api::{build_request, parse_stream};
pub use cli::parse_cli;
pub use http::secret_account;

type Interrupt = Box<dyn FnOnce() + Send>;

/// Shared cancellation signal. Providers may attach an interrupt for blocking I/O.
#[derive(Clone, Default)]
pub struct Cancellation {
    flag: AiCancel,
    interrupt: Arc<Mutex<Option<Interrupt>>>,
}

impl Cancellation {
    pub fn is_cancelled(&self) -> bool {
        self.flag.is_cancelled()
    }

    pub fn cancel(&self) {
        self.flag.cancel();
        if let Ok(mut interrupt) = self.interrupt.lock() {
            if let Some(interrupt) = interrupt.take() {
                interrupt();
            }
        }
    }

    pub fn interrupt_with(&self, interrupt: impl FnOnce() + Send + 'static) {
        if let Ok(mut slot) = self.interrupt.lock() {
            if self.is_cancelled() {
                interrupt();
            } else {
                *slot = Some(Box::new(interrupt));
            }
        }
    }
}

// All tool parameters are strings. Reading the map directly rejects duplicate keys before
// serde_json could silently replace the first value with a later one.
struct Arguments(Value);

impl<'de> Deserialize<'de> for Arguments {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StringMap;

        impl<'de> Visitor<'de> for StringMap {
            type Value = Arguments;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an object of unique string parameters")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Arguments, M::Error> {
                let mut values = serde_json::Map::new();
                while let Some((key, value)) = map.next_entry::<String, String>()? {
                    if values.insert(key, Value::String(value)).is_some() {
                        return Err(M::Error::custom("duplicate tool parameter"));
                    }
                }
                Ok(Arguments(Value::Object(values)))
            }
        }

        deserializer.deserialize_map(StringMap)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

impl ToolCall {
    pub fn new(name: impl Into<String>, arguments: Value) -> Self {
        Self {
            id: "call_0".into(),
            name: name.into(),
            arguments,
        }
    }

    pub(super) fn decode(&self) -> Option<Tool> {
        serde_json::from_value(json!({"name": self.name, "arguments": self.arguments})).ok()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProviderTurn {
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub usage: Usage,
    /// Gemini model parts, including any opaque thought signature needed on the next request.
    pub native_parts: Vec<Value>,
}

impl ProviderTurn {
    pub(super) fn sanitize(&mut self, credential: Option<&str>) {
        self.text = sanitized(&self.text, credential);
        for call in &mut self.calls {
            call.id = sanitized(&call.id, credential);
            call.name = sanitized(&call.name, credential);
            let before = call.arguments.clone();
            scrub(&mut call.arguments, credential);
            if before != call.arguments
                && matches!(call.name.as_str(), "run_command" | "apply_patch")
            {
                // A redacted action may have a different effect than the action proposed.
                call.arguments = Value::Null;
            }
        }
        for part in &mut self.native_parts {
            scrub(part, credential);
        }
    }

    pub(super) fn valid_calls(&self) -> bool {
        !self.calls.is_empty()
            && self.calls.len() <= 32
            && self.calls.iter().enumerate().all(|(index, call)| {
                !call.id.is_empty()
                    && call.id.len() <= 256
                    && call.decode().is_some()
                    && self.calls[..index].iter().all(|prev| prev.id != call.id)
            })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    User(String),
    Assistant(ProviderTurn),
    ToolResult {
        id: String,
        name: String,
        text: String,
    },
}

#[derive(Clone, Debug)]
pub struct ProviderRequest {
    pub kind: ProviderKind,
    pub model: String,
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<Value>,
    pub max_output_tokens: u32,
    pub timeout: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderError {
    Cancelled,
    Timeout,
    Configuration,
    Credential,
    Unavailable,
    Protocol,
    TooLarge,
}

impl ProviderError {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::Cancelled => "Provider request cancelled / أُلغي طلب المزود",
            Self::Timeout => "Provider request timed out / انتهت مهلة طلب المزود",
            Self::Configuration => "Invalid provider settings / إعدادات المزود غير صالحة",
            Self::Credential => "Provider key unavailable / مفتاح المزود غير متاح",
            Self::Unavailable => "Provider request failed / تعذّر طلب المزود",
            Self::Protocol => {
                "Invalid or incomplete provider response / رد المزود ناقص أو غير صالح"
            }
            Self::TooLarge => "Provider response too large / رد المزود أكبر من الحد المسموح",
        }
    }
}

/// One synchronous provider call. Text callbacks contain the latest cumulative safe preview.
/// The engine handles all side effects and only acts on completed, validated tool calls.
pub trait Provider: Send + Sync + 'static {
    fn complete(
        &self,
        request: &ProviderRequest,
        cancel: &Cancellation,
        text: &mut dyn FnMut(String),
    ) -> Result<ProviderTurn, ProviderError>;
}

pub(super) struct BuiltInProvider {
    pub secrets: Arc<dyn SecretStore + Send + Sync>,
}

impl Provider for BuiltInProvider {
    fn complete(
        &self,
        request: &ProviderRequest,
        cancel: &Cancellation,
        text: &mut dyn FnMut(String),
    ) -> Result<ProviderTurn, ProviderError> {
        match &request.kind {
            ProviderKind::Cli { program } => cli::complete(program, request, cancel),
            _ => http::complete(request, self.secrets.as_ref(), cancel, text),
        }
    }
}
