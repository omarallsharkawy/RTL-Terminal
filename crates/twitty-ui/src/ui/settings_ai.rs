//! Window-free state, constants, keyring helpers, and layout model for Settings → AI
//! (`docs/design/AGENT.md` §1.6).
//!
//! Only UI state + config keys + `SecretStore` integration live here; provider HTTP/CLI
//! validation belongs to lane s10.

use twitty_store::{SecretStore, SecretValue};

use crate::config::TwittyConfig;
use crate::layout::Rect;
use crate::ui::chrome::UiLanguage;

/// Default local Ollama server URL (AGENT.md §1.6).
pub const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";

/// API providers with keyring-backed keys in Settings → AI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApiProviderSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// Keyring account name under service `"twitty"`.
    pub keyring_account: &'static str,
}

pub const API_PROVIDERS: [ApiProviderSpec; 3] = [
    ApiProviderSpec {
        id: "anthropic",
        label: "Anthropic (Claude)",
        keyring_account: "anthropic-api-key",
    },
    ApiProviderSpec {
        id: "openai",
        label: "OpenAI",
        keyring_account: "openai-api-key",
    },
    ApiProviderSpec {
        id: "gemini",
        label: "Google Gemini",
        keyring_account: "gemini-api-key",
    },
];

/// Default provider selector choices.
/// (id, label); only "Auto" is translated, the rest are product names.
pub const DEFAULT_PROVIDER_CHOICES: [(&str, &str); 6] = [
    ("", twitty_core::en!("Auto")),
    ("cli", "CLI"),
    ("anthropic", "Anthropic"),
    ("openai", "OpenAI"),
    ("gemini", "Gemini"),
    ("ollama", "Ollama"),
];

/// Read-only hard-deny list displayed in Settings → AI (AGENT.md §1.6 & §2.2).
pub const HARD_DENY_SUMMARY: [&str; 8] = [
    twitty_core::en!("sudo / doas / pkexec / su"),
    twitty_core::en!("curl | sh, wget | bash (network pipe to shell)"),
    twitty_core::en!("rm -rf outside project root, /, ~, $HOME, .."),
    twitty_core::en!("chmod -R / chown -R outside project root"),
    twitty_core::en!("dd of=/dev/…, mkfs, fork bombs"),
    twitty_core::en!("Writes to shell rc, ~/.ssh, ~/.gnupg, ~/.config/twitty, git hooks, /etc"),
    twitty_core::en!("Reads from ~/.ssh, ~/.gnupg, ~/.aws, ~/.config/gh, .netrc, keyrings"),
    twitty_core::en!("git push / --force / config --global, package installs (pip, npm -g, cargo install, apt, pacman, brew)"),
];

/// Active text input inside Settings → AI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiTextField {
    OllamaUrl,
    CostCap,
    AllowListAdd,
    ApiKey(usize),
}

/// Window-free UI state for Settings → AI.
#[derive(Clone, Debug, Default)]
pub struct AiSettingsState {
    /// Coding agent CLIs detected on PATH when Settings opened.
    pub detected_cli_agents: Vec<String>,
    /// Active project root for per-project allow-list & rules file approvals.
    pub active_project_root: Option<String>,
    /// Presence flags for API keys in the system keyring (never stores actual keys).
    pub saved_keys: [bool; 3],
    /// Currently active text input inside Settings → AI.
    pub active_field: Option<AiTextField>,
    /// Temporary masked buffer while typing an API key (scrubbed immediately on save/cancel).
    pub key_draft: String,
    /// Draft text for monthly cost cap in USD (blank = off).
    pub cost_cap_draft: String,
    /// Draft argv prefix for adding to the per-project allow-list.
    pub allow_list_draft: String,
    /// Inline status/feedback banner `(is_error, message)`.
    pub notice: Option<(bool, String)>,
    /// Confirmation state for "Clear agent history".
    pub history_cleared: bool,
}

impl AiSettingsState {
    /// Refresh keyring presence flags for all API providers without exposing or caching secrets.
    pub fn refresh_key_flags(&mut self, store: &dyn SecretStore) {
        for (i, spec) in API_PROVIDERS.iter().enumerate() {
            self.saved_keys[i] =
                matches!(store.get(spec.keyring_account), Ok(Some(v)) if !v.is_empty());
        }
    }

    /// Initialize drafts from `config`.
    pub fn sync_from_config(&mut self, config: &TwittyConfig) {
        self.cost_cap_draft = config
            .ai_monthly_cost_cap_usd
            .map(|v| format!("{v:.2}"))
            .unwrap_or_default();
    }

    /// Scrub and clear the temporary API key draft buffer.
    pub fn clear_key_draft(&mut self) {
        let mut raw = std::mem::take(&mut self.key_draft).into_bytes();
        raw.fill(0);
        std::hint::black_box(&raw);
        if matches!(self.active_field, Some(AiTextField::ApiKey(_))) {
            self.active_field = None;
        }
    }

    /// Take the key draft as a [`SecretValue`] ready to be passed to [`SecretStore::set`].
    pub fn take_key_secret(&mut self, provider_idx: usize) -> Option<(String, SecretValue)> {
        let spec = API_PROVIDERS.get(provider_idx)?;
        let trimmed = self.key_draft.trim().to_owned();
        self.clear_key_draft();
        if trimmed.is_empty() {
            return None;
        }
        Some((spec.keyring_account.to_owned(), SecretValue::new(trimmed)))
    }

    /// Store `secret` for `account` directly into `store` (used by app & tests).
    pub fn apply_save_secret(
        &mut self,
        store: &dyn SecretStore,
        account: &str,
        secret: &SecretValue,
        language: UiLanguage,
    ) {
        match store.set(account, secret.expose()) {
            Ok(()) => {
                self.refresh_key_flags(store);
                self.notice = Some((
                    false,
                    language.t("API key saved to system keyring.").to_owned(),
                ));
            }
            Err(err) => {
                self.notice = Some((true, format!("{err}")));
            }
        }
    }

    /// Delete the stored API key for `account` from `store`.
    pub fn apply_delete_secret(
        &mut self,
        store: &dyn SecretStore,
        account: &str,
        language: UiLanguage,
    ) {
        match store.delete(account) {
            Ok(_) => {
                self.refresh_key_flags(store);
                self.notice = Some((
                    false,
                    language.t("API key removed from keyring.").to_owned(),
                ));
            }
            Err(err) => {
                self.notice = Some((true, format!("{err}")));
            }
        }
    }

    /// Validate and parse `cost_cap_draft` into `config.ai_monthly_cost_cap_usd`.
    pub fn commit_cost_cap(&mut self, config: &mut TwittyConfig, language: UiLanguage) -> bool {
        let trimmed = self.cost_cap_draft.trim();
        if trimmed.is_empty() {
            if config.ai_monthly_cost_cap_usd.take().is_some() {
                self.notice = None;
                return true;
            }
            return false;
        }
        match trimmed.parse::<f64>() {
            Ok(v) if v.is_finite() && v > 0.0 => {
                let clamped = (v * 100.0).round() / 100.0;
                config.ai_monthly_cost_cap_usd = Some(clamped);
                self.cost_cap_draft = format!("{clamped:.2}");
                self.notice = None;
                true
            }
            _ => {
                self.notice = Some((
                    true,
                    language
                        .t("Cost cap must be a positive USD number (or blank for off).")
                        .to_owned(),
                ));
                false
            }
        }
    }

    /// Project key used for per-project allow-list and rules approvals.
    pub fn project_key(&self) -> &str {
        self.active_project_root.as_deref().unwrap_or("default")
    }

    /// Add `allow_list_draft` to the current project's allow-list, rejecting shell metacharacters
    /// per AGENT.md §2.2.
    pub fn add_allow_list_entry(
        &mut self,
        config: &mut TwittyConfig,
        language: UiLanguage,
    ) -> bool {
        let trimmed = self.allow_list_draft.trim();
        if trimmed.is_empty() {
            return false;
        }
        if contains_shell_metachar(trimmed) {
            self.notice = Some((
                true,
                language
                    .t("Allow-list prefixes cannot contain shell metacharacters (;, &&, ||, |, $, >, <).")
                    .to_owned(),
            ));
            return false;
        }
        let key = self.project_key().to_owned();
        let entry = trimmed.to_owned();
        let list = config.ai_project_allow_list.entry(key).or_default();
        if !list.iter().any(|existing| existing == &entry) {
            list.push(entry);
            self.allow_list_draft.clear();
            self.notice = None;
            return true;
        }
        self.allow_list_draft.clear();
        false
    }

    pub fn remove_allow_list_entry(&mut self, index: usize, config: &mut TwittyConfig) -> bool {
        let key = self.project_key().to_owned();
        if let Some(list) = config.ai_project_allow_list.get_mut(&key) {
            if index < list.len() {
                list.remove(index);
                if list.is_empty() {
                    config.ai_project_allow_list.remove(&key);
                }
                return true;
            }
        }
        false
    }
}

/// Detect installed coding-agent CLIs on `$PATH` (non-blocking, bounded).
pub fn detect_cli_agents_on_path() -> Vec<String> {
    let candidates = twitty_ai::ai::AI_KNOWN_AGENTS;
    let Some(path_var) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    let dirs: Vec<_> = std::env::split_paths(&path_var).collect();
    let mut found = Vec::new();
    for &name in candidates {
        let present = dirs.iter().any(|dir| {
            let candidate = dir.join(name);
            candidate.is_file()
        });
        if present {
            found.push(name.to_owned());
        }
    }
    found
}

fn contains_shell_metachar(s: &str) -> bool {
    s.contains(';')
        || s.contains("&&")
        || s.contains("||")
        || s.contains('|')
        || s.contains('`')
        || s.contains("$(")
        || s.contains('>')
        || s.contains('<')
        || s.contains('\n')
        || s.contains('\r')
}

/// Physical-pixel layout of the Settings → AI section.
#[derive(Clone, Debug)]
pub struct AiSectionLayout {
    pub default_provider_buttons: Vec<(usize, Rect)>,
    pub cli_section: Rect,
    pub api_rows: Vec<AiApiRowLayout>,
    pub ollama_input: Rect,
    pub cost_cap_input: Rect,
    pub allow_input: Rect,
    pub allow_add_button: Rect,
    pub allow_entries: Vec<(usize, Rect, Rect)>,
    pub deny_box: Rect,
    pub clear_history_button: Rect,
    pub notice_rect: Option<Rect>,
}

impl Default for AiSectionLayout {
    fn default() -> Self {
        let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
        Self {
            default_provider_buttons: Vec::new(),
            cli_section: zero,
            api_rows: Vec::new(),
            ollama_input: zero,
            cost_cap_input: zero,
            allow_input: zero,
            allow_add_button: zero,
            allow_entries: Vec::new(),
            deny_box: zero,
            clear_history_button: zero,
            notice_rect: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AiApiRowLayout {
    pub index: usize,
    pub row: Rect,
    pub set_key_button: Rect,
    pub clear_key_button: Option<Rect>,
    pub key_input: Option<Rect>,
    pub key_save_button: Option<Rect>,
}

/// Map the AI settings to the agent engine's configuration (AGENT.md §1.6).
///
/// `detected_clis` is the result of [`detect_cli_agents_on_path`]; `has_key` tells whether the
/// keyring holds a key for an API provider account. Auto (no default provider) prefers an
/// installed coding-agent CLI, then the first API provider with a key, then local Ollama.
pub fn engine_config(
    config: &TwittyConfig,
    detected_clis: &[String],
    has_key: impl Fn(&str) -> bool,
) -> twitty_ai::engine::EngineConfig {
    use twitty_ai::engine::model::ProviderKind;
    let cli_program = config
        .ai_agent
        .clone()
        .or_else(|| config.agent_command.clone())
        .filter(|p| !p.trim().is_empty())
        .or_else(|| detected_clis.first().cloned());
    let ollama = || ProviderKind::Ollama {
        base_url: config.ai_ollama_url.clone(),
    };
    let api = |id: &str| match id {
        "anthropic" => Some(ProviderKind::Anthropic),
        "openai" => Some(ProviderKind::OpenAi),
        "gemini" => Some(ProviderKind::Gemini),
        _ => None,
    };
    let provider = match config.ai_default_provider.as_deref().unwrap_or("") {
        "cli" => ProviderKind::Cli {
            program: cli_program.unwrap_or_else(|| "claude".into()),
        },
        "ollama" => ollama(),
        id if api(id).is_some() => api(id).unwrap_or_else(ollama),
        _ => match cli_program {
            Some(program) => ProviderKind::Cli { program },
            None => API_PROVIDERS
                .iter()
                .find(|spec| has_key(spec.keyring_account))
                .and_then(|spec| api(spec.id))
                .unwrap_or_else(ollama),
        },
    };
    let allow_lists = config
        .ai_project_allow_list
        .iter()
        .map(|(root, prefixes)| {
            let root = std::path::PathBuf::from(root);
            let root = std::fs::canonicalize(&root).unwrap_or(root);
            let argv = prefixes
                .iter()
                .map(|p| p.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
                .filter(|argv| !argv.is_empty())
                .collect();
            (root, argv)
        })
        .collect();
    twitty_ai::engine::EngineConfig {
        provider,
        timeout: std::time::Duration::from_secs(config.ai_timeout_secs),
        // One request can never spend more than the whole monthly cap.
        cost_cap_micros_usd: config
            .ai_monthly_cost_cap_usd
            .map(|usd| (usd * 1_000_000.0).round() as u64),
        allow_lists,
        ..twitty_ai::engine::EngineConfig::default()
    }
}
