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

/// Token pricing rates for a model in thousandths of one micro-USD per token
/// (equivalently, USD per billion tokens, or `usd_per_million * 1000`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelRates {
    pub input_milli_micros: u64,
    pub output_milli_micros: u64,
}

impl ModelRates {
    /// Construct rates from USD per 1M input/output tokens (e.g. `per_million_usd(3.0, 15.0)`).
    pub fn per_million_usd(input_usd: f64, output_usd: f64) -> Self {
        let to_rate = |usd: f64| {
            if usd.is_finite() && usd >= 0.0 {
                (usd * 1000.0).round() as u64
            } else {
                0
            }
        };
        Self {
            input_milli_micros: to_rate(input_usd),
            output_milli_micros: to_rate(output_usd),
        }
    }

    pub fn cost_micros(self, input: u64, output: u64) -> u64 {
        let total = u128::from(input) * u128::from(self.input_milli_micros)
            + u128::from(output) * u128::from(self.output_milli_micros);
        u64::try_from(total.div_ceil(1000)).unwrap_or(u64::MAX)
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
    /// Optional per-model rate overrides keyed by exact model name or prefix.
    pub custom_pricing: HashMap<String, ModelRates>,
    /// Canonical project roots mapped to exact argv prefixes. Empty by default.
    pub allow_lists: HashMap<PathBuf, Vec<Vec<String>>>,
    pub journal_dir: PathBuf,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            provider: ProviderKind::Cli {
                program: "codex".into(),
            },
            models: ProviderModels::default(),
            timeout: Duration::from_secs(60),
            max_steps: 25,
            max_consecutive_tool_calls: 8,
            max_output_tokens: 4096,
            token_cap: None,
            cost_cap_micros_usd: None,
            custom_pricing: HashMap::new(),
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
    /// Estimate micro-USD cost using `self.custom_pricing` overrides first, then the built-in table.
    pub fn estimate_cost_micros(
        &self,
        kind: &ProviderKind,
        model: &str,
        input: u64,
        output: u64,
    ) -> Option<u64> {
        if let Some(rates) = self.custom_pricing.get(model) {
            return Some(rates.cost_micros(input, output));
        }
        let mut best_prefix: Option<(usize, ModelRates)> = None;
        for (prefix, &rates) in &self.custom_pricing {
            if !prefix.is_empty()
                && model.starts_with(prefix)
                && best_prefix.is_none_or(|(len, _)| prefix.len() > len)
            {
                best_prefix = Some((prefix.len(), rates));
            }
        }
        if let Some((_, rates)) = best_prefix {
            return Some(rates.cost_micros(input, output));
        }
        estimated_cost_micros(kind, model, input, output)
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
    let m = model.to_ascii_lowercase();
    let (input_rate, output_rate) = match (kind, m.as_str()) {
        (ProviderKind::Anthropic, m) if m.starts_with("claude-opus-") || m.contains("-opus") => {
            (15000_u64, 75000_u64)
        }
        (ProviderKind::Anthropic, m)
            if m.starts_with("claude-sonnet-") || m.contains("-sonnet") =>
        {
            (3000, 15000)
        }
        (ProviderKind::Anthropic, m) if m.starts_with("claude-haiku-") || m.contains("-haiku") => {
            (800, 4000)
        }
        (ProviderKind::OpenAi, m) if m.starts_with("gpt-4.1-mini") => (400, 1600),
        (ProviderKind::OpenAi, m) if m.starts_with("gpt-4.1-nano") => (100, 400),
        (ProviderKind::OpenAi, m) if m.starts_with("gpt-4.1") => (2000, 8000),
        (ProviderKind::OpenAi, m) if m.starts_with("gpt-4o-mini") => (150, 600),
        (ProviderKind::OpenAi, m) if m.starts_with("gpt-4o") => (2500, 10000),
        (ProviderKind::OpenAi, m) if m.starts_with("gpt-5-mini") => (250, 2000),
        (ProviderKind::OpenAi, m) if m.starts_with("gpt-5") => (1250, 10000),
        (ProviderKind::OpenAi, m) if m.starts_with("o4-mini") || m.starts_with("o3-mini") => {
            (1100, 4400)
        }
        (ProviderKind::OpenAi, m) if m.starts_with("o3") => (2000, 8000),
        (ProviderKind::Gemini, m) if m.starts_with("gemini-") && m.contains("flash") => (300, 2500),
        (ProviderKind::Gemini, m) if m.starts_with("gemini-") && m.contains("pro") => (1250, 10000),
        (ProviderKind::Ollama { .. }, _) => (0, 0),
        _ => return None,
    };
    Some(
        ModelRates {
            input_milli_micros: input_rate,
            output_milli_micros: output_rate,
        }
        .cost_micros(input, output),
    )
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
