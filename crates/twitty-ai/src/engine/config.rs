use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use super::model::{ProviderInfo, ProviderKind, Usage};

pub const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";

#[derive(Clone, Debug)]
pub struct ProviderModels {
    /// Empty uses the selected CLI's own default.
    pub cli: String,
    pub anthropic: String,
    pub openai: String,
    pub gemini: String,
    pub ollama: String,
}

impl Default for ProviderModels {
    fn default() -> Self {
        Self {
            cli: String::new(),
            anthropic: "claude-sonnet-4-6".into(),
            openai: "gpt-4.1-mini".into(),
            gemini: "gemini-3.5-flash-lite".into(),
            ollama: "qwen2.5".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub provider: ProviderKind,
    pub models: ProviderModels,
    pub timeout: Duration,
    pub max_steps: u32,
    pub max_consecutive_tool_calls: u32,
    pub max_output_tokens: u32,
    /// Usage budget per request, extended only after approval.
    pub token_cap: Option<u64>,
    /// Estimated micro-USD budget per request, extended only after approval.
    pub cost_cap_micros_usd: Option<u64>,
    /// Canonical project roots mapped to exact argv prefixes. Empty by default.
    pub allow_lists: HashMap<PathBuf, Vec<Vec<String>>>,
    pub journal_dir: PathBuf,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            provider: ProviderKind::Cli {
                program: "claude".into(),
            },
            models: ProviderModels::default(),
            timeout: Duration::from_secs(60),
            max_steps: 25,
            max_consecutive_tool_calls: 8,
            max_output_tokens: 4096,
            token_cap: None,
            cost_cap_micros_usd: None,
            allow_lists: HashMap::new(),
            journal_dir: super::journal::default_dir(),
        }
    }
}

impl EngineConfig {
    pub fn provider_info(&self) -> ProviderInfo {
        let mut kind = self.provider.clone();
        let (model, display) = match &mut kind {
            ProviderKind::Cli { program } => {
                let name = std::path::Path::new(program)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("CLI");
                (
                    &self.models.cli,
                    format!("{name} · {}", twitty_core::language::t("CLI")),
                )
            }
            ProviderKind::Anthropic => (&self.models.anthropic, "Claude · API".into()),
            ProviderKind::OpenAi => (&self.models.openai, "OpenAI · API".into()),
            ProviderKind::Gemini => (&self.models.gemini, "Gemini · API".into()),
            ProviderKind::Ollama { base_url } => {
                if base_url.is_empty() {
                    *base_url = DEFAULT_OLLAMA_URL.into();
                }
                (
                    &self.models.ollama,
                    format!("{} · Ollama", self.models.ollama),
                )
            }
        };
        ProviderInfo {
            kind,
            model: model.clone(),
            display,
        }
    }
}

/// Approximate standard list rates in thousandths of one micro-USD per token. Cached tokens,
/// negotiated prices, long-context tiers, and provider-specific fees can differ.
pub fn estimated_cost_micros(
    kind: &ProviderKind,
    model: &str,
    input: u64,
    output: u64,
) -> Option<u64> {
    let (input_rate, output_rate) = match (kind, model) {
        (ProviderKind::Anthropic, "claude-sonnet-4-6") => (3000_u64, 15000_u64),
        (ProviderKind::OpenAi, "gpt-4.1-mini" | "gpt-4.1-mini-2025-04-14") => (400, 1600),
        (ProviderKind::OpenAi, "gpt-4.1" | "gpt-4.1-2025-04-14") => (2000, 8000),
        (ProviderKind::Gemini, "gemini-3.5-flash-lite") => (300, 2500),
        (ProviderKind::Gemini, "gemini-2.5-flash") => (300, 2500),
        (ProviderKind::Ollama { .. }, _) => (0, 0),
        _ => return None,
    };
    let total =
        u128::from(input) * u128::from(input_rate) + u128::from(output) * u128::from(output_rate);
    Some(u64::try_from(total.div_ceil(1000)).unwrap_or(u64::MAX))
}

pub(super) fn total_usage(previous: Usage, next: Usage) -> Usage {
    Usage {
        input_tokens: previous.input_tokens.saturating_add(next.input_tokens),
        output_tokens: previous.output_tokens.saturating_add(next.output_tokens),
        cost_micros_usd: match (previous.cost_micros_usd, next.cost_micros_usd) {
            (Some(a), Some(b)) => Some(a.saturating_add(b)),
            (None, Some(b)) if previous == Usage::default() => Some(b),
            _ => None,
        },
    }
}
