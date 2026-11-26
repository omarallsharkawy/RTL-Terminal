//! Settings → AI mapped onto the agent engine's configuration (AGENT.md §1.6).
use twitty::config::TwittyConfig;
use twitty::ui::settings_ai::{engine_config, API_PROVIDERS};
use twitty_ai::engine::model::ProviderKind;
use twitty_ai::engine::provider::secret_account;

#[test]
fn settings_store_api_keys_under_the_accounts_the_engine_reads() {
    for (spec, kind) in API_PROVIDERS.iter().zip([
        ProviderKind::Anthropic,
        ProviderKind::OpenAi,
        ProviderKind::Gemini,
    ]) {
        assert_eq!(
            Some(spec.keyring_account),
            secret_account(&kind),
            "{}",
            spec.id
        );
    }
}

#[test]
fn auto_prefers_an_installed_cli_then_a_keyed_api_then_ollama() {
    let config = TwittyConfig::default();
    let cli = engine_config(&config, &["codex".into()], |_| true);
    assert_eq!(
        cli.provider,
        ProviderKind::Cli {
            program: "codex".into()
        }
    );

    let api = engine_config(&config, &[], |account| account == "openai-api-key");
    assert_eq!(api.provider, ProviderKind::OpenAi);

    let local = engine_config(&config, &[], |_| false);
    assert_eq!(
        local.provider,
        ProviderKind::Ollama {
            base_url: config.ai_ollama_url.clone()
        }
    );
}

#[test]
fn explicit_provider_agent_override_caps_and_allow_lists_carry_over() {
    let mut config = TwittyConfig::default();
    config.ai_default_provider = Some("cli".into());
    config.ai_agent = Some("gemini".into());
    config.ai_timeout_secs = 90;
    config.ai_monthly_cost_cap_usd = Some(2.5);
    let root = std::env::temp_dir();
    config.ai_project_allow_list.insert(
        root.to_string_lossy().into_owned(),
        vec!["cargo test".into(), "  ".into(), "git status".into()],
    );
    let engine = engine_config(&config, &["claude".into()], |_| false);
    assert_eq!(
        engine.provider,
        ProviderKind::Cli {
            program: "gemini".into()
        }
    );
    assert_eq!(engine.timeout.as_secs(), 90);
    assert_eq!(engine.cost_cap_micros_usd, Some(2_500_000));
    let canonical = std::fs::canonicalize(&root).unwrap();
    assert_eq!(
        engine.allow_lists.get(&canonical),
        Some(&vec![
            vec!["cargo".to_owned(), "test".to_owned()],
            vec!["git".to_owned(), "status".to_owned()],
        ])
    );

    config.ai_default_provider = Some("anthropic".into());
    assert_eq!(
        engine_config(&config, &[], |_| false).provider,
        ProviderKind::Anthropic
    );
}
