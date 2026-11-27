//! Window-free state machine, keyboard, layout, clipping, and Settings → AI tests for the
//! Twitty Agent panel (`docs/design/AGENT.md`).

use std::path::PathBuf;
use std::time::Duration;

use cosmic_text::{fontdb, FontSystem};
use twitty::color::Palette;
use twitty::commands;
use twitty::config::TwittyConfig;
use twitty::input::keybindings::{
    default_chords_for, KeyBindings, KeybindingValue, KeybindingsConfig,
};
use twitty::input::InputAction;
use twitty::layout::{PaneId, Rect};
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::frame::{build_frame, FrameOptions};
use twitty::render::scene::preedit::PreeditState;
use twitty::render::scene::settings::SettingsView;
use twitty::render::scene::window_chrome::{ChromeView, PaneHeaderView};
use twitty::render::text::ui::UiFont;
use twitty::render::ui_style::UiStyle;
use twitty::render::{CellMetrics, PaneFrame, RowCache};
use twitty::terminal::Terminal;
use twitty::ui::agent_panel::{
    arabic_count, format_context_strip, format_usage_pill, included_context_tokens,
    paragraph_is_rtl, split_agent_content, width_from_divider_drag, AgentFocus, AgentPanelAction,
    AgentPanelHit, AgentPanelLayout, AgentPanelState, CardButton, ScriptedEngine, EXAMPLE_CHIPS,
    MAX_PANEL_PX, MIN_PANEL_PX,
};
use twitty::ui::chrome::{PaneHeaderLayout, TopBarLayout, UiLanguage, WindowLayout};
use twitty::ui::settings_ai::{AiTextField, API_PROVIDERS, DEFAULT_OLLAMA_URL, HARD_DENY_SUMMARY};
use twitty::ui::settings_panel::{
    SettingId, SettingsHit, SettingsLayout, SettingsPanelState, SettingsSection,
};
use twitty_ai::engine::model::{
    AgentCommand, AgentEvent, ContextBundle, ContextItem, ContextKind, Decision, DenyReason,
    FinishReason, PatchFile, ProviderInfo, ProviderKind, ReadTool, SessionId, Step, StepId,
    StepKind, StepStatus, Usage,
};
use twitty_store::{InMemorySecretStore, SecretStore};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey};

const NASKH: &[u8] = include_bytes!("../assets/fonts/NotoNaskhArabic-Regular.ttf");
const MONO: &[u8] = include_bytes!("golden/fonts/NotoSansMono-Subset.ttf");

fn test_font_system() -> FontSystem {
    let mut db = fontdb::Database::new();
    db.load_font_data(MONO.to_vec());
    db.load_font_data(NASKH.to_vec());
    db.set_monospace_family("Noto Sans Mono");
    db.set_sans_serif_family("Noto Sans Mono");
    db.set_serif_family("Noto Sans Mono");
    FontSystem::new_with_locale_and_db("en-US".into(), db)
}

fn sample_provider() -> ProviderInfo {
    ProviderInfo {
        kind: ProviderKind::Anthropic,
        model: "claude-sonnet".into(),
        display: "Claude · API".into(),
    }
}

fn sample_diff() -> String {
    concat!(
        "diff --git a/src/main.rs b/src/main.rs\n",
        "index 1111111..2222222 100644\n",
        "--- a/src/main.rs\n",
        "+++ b/src/main.rs\n",
        "@@ -1,3 +1,4 @@\n",
        " fn main() {\n",
        "-    println!(\"old\");\n",
        "+    println!(\"new\");\n",
        "+    println!(\"done\");\n",
        " }\n"
    )
    .to_owned()
}

fn sample_context() -> ContextBundle {
    ContextBundle {
        project_root: Some(PathBuf::from("/workspace/twitty")),
        budget_tokens: 8000,
        items: vec![
            ContextItem {
                kind: ContextKind::Rule,
                label: "AGENTS.md".into(),
                text: "Always run cargo test.".into(),
                est_tokens: 120,
                included: true,
            },
            ContextItem {
                kind: ContextKind::Memory,
                label: "fact #1".into(),
                text: "Uses llvmpipe for goldens".into(),
                est_tokens: 80,
                included: true,
            },
            ContextItem {
                kind: ContextKind::Memory,
                label: "fact #2".into(),
                text: "Arabic UI docks left".into(),
                est_tokens: 60,
                included: true,
            },
            ContextItem {
                kind: ContextKind::Memory,
                label: "fact #3".into(),
                text: "forbid(unsafe_code)".into(),
                est_tokens: 50,
                included: true,
            },
            ContextItem {
                kind: ContextKind::Block,
                label: "block: cargo test".into(),
                text: "test failed".into(),
                est_tokens: 340,
                included: true,
            },
            ContextItem {
                kind: ContextKind::Block,
                label: "block: git status".into(),
                text: "M src/lib.rs".into(),
                est_tokens: 90,
                included: true,
            },
        ],
    }
}

#[test]
fn command_table_and_keybinding_support_agent_panel_and_remapping() {
    let all = commands::all();
    let cmd = all
        .iter()
        .find(|c| c.name == "agent.panel")
        .expect("agent.panel command must be registered");
    assert_eq!(cmd.title_en, "Agent panel");
    assert_eq!(cmd.title_ar, "لوحة الوكيل");
    assert_eq!(cmd.shortcut, Some("Ctrl+Shift+."));
    assert_eq!(
        commands::action("agent.panel"),
        Some(InputAction::ToggleAgentPanel)
    );

    let defaults = default_chords_for("agent.panel");
    assert_eq!(defaults.len(), 1);
    assert_eq!(defaults[0].to_string(), "Ctrl+Shift+.");

    let mut cfg = KeybindingsConfig::default();
    cfg.set("agent.panel", KeybindingValue::Single("Ctrl+Alt+A".into()));
    let resolved = KeyBindings::resolve(&cfg);
    assert!(resolved.warnings.is_empty(), "{:?}", resolved.warnings);
    assert_eq!(
        resolved
            .bindings
            .effective_chords("agent.panel")
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec!["Ctrl+Alt+A"]
    );
}

#[test]
fn docking_geometry_respects_38_percent_min_max_50_percent_and_arabic_mirroring() {
    let tab = Rect::new(0.0, 36.0, 1200.0, 764.0);
    // 38% of 1200 = 456 (within 340..560 and <= 600).
    let (term_en, panel_en) = split_agent_content(tab, 1.0, None, UiLanguage::English);
    let panel_en = panel_en.expect("panel fits");
    assert_eq!(panel_en.w, 456.0);
    assert_eq!(panel_en.right(), tab.right());
    assert_eq!(term_en.x, tab.x);
    assert!(term_en.w >= tab.w * 0.5);

    // Arabic UI docks on the LEFT.
    let (term_ar, panel_ar) = split_agent_content(tab, 1.0, None, UiLanguage::Arabic);
    let panel_ar = panel_ar.expect("panel fits in Arabic");
    assert_eq!(panel_ar.x, tab.x);
    assert_eq!(panel_ar.w, 456.0);
    assert_eq!(term_ar.x, panel_ar.right());

    // Very wide window caps at 560 logical px.
    let wide = Rect::new(0.0, 36.0, 2000.0, 900.0);
    let (_, panel_wide) = split_agent_content(wide, 1.0, None, UiLanguage::English);
    assert_eq!(panel_wide.unwrap().w, MAX_PANEL_PX);

    // Narrow window keeps terminal >= 50% wide.
    let narrow = Rect::new(0.0, 36.0, 620.0, 600.0);
    let (term_narrow, panel_narrow) = split_agent_content(narrow, 1.0, None, UiLanguage::English);
    let panel_narrow = panel_narrow.expect("panel fits at 310px");
    assert_eq!(panel_narrow.w, 310.0);
    assert!(term_narrow.w >= narrow.w * 0.5);

    // Very narrow window: the panel still opens at its usable minimum; the terminal shrinks.
    let tight = Rect::new(0.0, 36.0, 420.0, 600.0);
    let (term_tight, panel_tight) = split_agent_content(tight, 1.0, None, UiLanguage::English);
    assert_eq!(
        panel_tight.expect("panel opens on a narrow window").w,
        240.0
    );
    assert_eq!(term_tight.w, 180.0);
    let tiny = Rect::new(0.0, 36.0, 380.0, 600.0);
    assert!(split_agent_content(tiny, 1.0, None, UiLanguage::English)
        .1
        .is_none());

    // 1.5x scale factor scales min/max cleanly.
    let hidpi = Rect::new(0.0, 54.0, 1800.0, 1000.0);
    let (_, panel_hi) = split_agent_content(hidpi, 1.5, Some(300.0), UiLanguage::English);
    assert_eq!(panel_hi.unwrap().w, (MIN_PANEL_PX * 1.5).round());

    // Divider drag clamps cleanly in both directions.
    let dragged_en = width_from_divider_drag(tab, 500.0, 1.0, UiLanguage::English);
    assert_eq!(dragged_en, MAX_PANEL_PX);
    let dragged_ar = width_from_divider_drag(tab, 420.0, 1.0, UiLanguage::Arabic);
    assert_eq!(dragged_ar, 420.0);
}

#[test]
fn scripted_engine_drives_full_approval_edit_deny_and_result_lifecycle() {
    // Scripted engine over real mpsc channels (`AgentHandle`).
    let handle = ScriptedEngine::spawn_handle(|cmd, tx| match cmd {
        AgentCommand::Start { session, .. } => {
            let _ = tx.send(AgentEvent::Started {
                session,
                provider: sample_provider(),
            });
            let _ = tx.send(AgentEvent::StepAdded {
                session,
                step: Step {
                    id: StepId(1),
                    kind: StepKind::Plan {
                        text: "1. Read Cargo.toml\n2. Run cargo test".into(),
                    },
                    status: StepStatus::Done,
                },
            });
            let _ = tx.send(AgentEvent::StepAdded {
                session,
                step: Step {
                    id: StepId(2),
                    kind: StepKind::Read {
                        tool: ReadTool::ReadFile,
                        target: "Cargo.toml".into(),
                        preview: "[package]\nname = \"twitty\"".into(),
                    },
                    status: StepStatus::Done,
                },
            });
            let _ = tx.send(AgentEvent::StepAdded {
                session,
                step: Step {
                    id: StepId(3),
                    kind: StepKind::Command {
                        command: "cargo test".into(),
                        cwd: PathBuf::from("/workspace/twitty"),
                        reason: "Verify test suite".into(),
                        after_untrusted_data: true,
                    },
                    status: StepStatus::WaitingApproval,
                },
            });
            let _ = tx.send(AgentEvent::Usage {
                session,
                usage: Usage {
                    input_tokens: 1200,
                    output_tokens: 300,
                    cost_micros_usd: Some(20_000),
                },
            });
            true
        }
        AgentCommand::Decide {
            session,
            step,
            decision,
        } => {
            match decision {
                Decision::Approve => {
                    let _ = tx.send(AgentEvent::StepUpdated {
                        session,
                        step: Step {
                            id: step,
                            kind: StepKind::Command {
                                command: "cargo test".into(),
                                cwd: PathBuf::from("/workspace/twitty"),
                                reason: "Verify test suite".into(),
                                after_untrusted_data: true,
                            },
                            status: StepStatus::Running,
                        },
                    });
                    let _ = tx.send(AgentEvent::RunInPane {
                        session,
                        step,
                        command: "cargo test".into(),
                        cwd: PathBuf::from("/workspace/twitty"),
                    });
                }
                Decision::ApproveEdited(edited) => {
                    let _ = tx.send(AgentEvent::RunInPane {
                        session,
                        step,
                        command: edited,
                        cwd: PathBuf::from("/workspace/twitty"),
                    });
                }
                Decision::Deny => {
                    let _ = tx.send(AgentEvent::StepUpdated {
                        session,
                        step: Step {
                            id: step,
                            kind: StepKind::Denied {
                                what: "cargo test".into(),
                                reason: DenyReason::User,
                            },
                            status: StepStatus::Denied,
                        },
                    });
                    let _ = tx.send(AgentEvent::Finished {
                        session,
                        reason: FinishReason::Completed,
                    });
                }
            }
            true
        }
        AgentCommand::CommandFinished {
            session,
            exit,
            duration_ms,
            output,
            ..
        } => {
            let _ = tx.send(AgentEvent::StepAdded {
                session,
                step: Step {
                    id: StepId(4),
                    kind: StepKind::CommandResult {
                        exit,
                        duration_ms,
                        tail: output,
                    },
                    status: StepStatus::Done,
                },
            });
            let _ = tx.send(AgentEvent::StepAdded {
                session,
                step: Step {
                    id: StepId(5),
                    kind: StepKind::Message {
                        text: "All tests passed.".into(),
                    },
                    status: StepStatus::Done,
                },
            });
            let _ = tx.send(AgentEvent::Finished {
                session,
                reason: FinishReason::Completed,
            });
            false
        }
        _ => false,
    });

    let mut panel = AgentPanelState::new();
    panel.open();
    panel.set_project_context(PathBuf::from("/workspace/twitty"));
    panel.set_context_bundle(sample_context());

    // Clicking empty-state chip #0 submits its prompt.
    let action = panel.on_click(AgentPanelHit::ExampleChip(0));
    let AgentPanelAction::SendCommand(cmd) = action else {
        panic!("expected Start command, got {action:?}");
    };
    assert!(matches!(
        &cmd,
        AgentCommand::Start { request, .. } if request == EXAMPLE_CHIPS[0].prompt
    ));
    handle.commands.send(cmd).unwrap();

    // Receive Started + 3 steps + Usage.
    for _ in 0..5 {
        let ev = handle
            .events
            .recv_timeout(Duration::from_secs(2))
            .expect("scripted event");
        panel.apply_event(ev);
    }

    assert!(panel.is_running());
    assert!(panel.needs_input());
    assert_eq!(panel.focus, AgentFocus::Card);
    assert_eq!(panel.selected_step, Some(StepId(3)));
    assert_eq!(
        format_usage_pill(panel.active_session().unwrap().usage, UiLanguage::English).as_deref(),
        Some("1.5k tok · $0.02")
    );

    // Tab cycles buttons: 0 (Run) -> 1 (Edit) -> 2 (Deny) -> 0 (Run).
    let none = ModifiersState::empty();
    panel.on_key(&Key::Named(NamedKey::Tab), None, None, none);
    assert_eq!(panel.focused_button, 1);
    panel.on_key(&Key::Named(NamedKey::Tab), None, None, none);
    assert_eq!(panel.focused_button, 2);
    panel.on_key(&Key::Named(NamedKey::Tab), None, None, none);
    assert_eq!(panel.focused_button, 0);

    // Press `E` to enter Command edit mode, append " --lib", press Enter to ApproveEdited.
    panel.on_key(
        &Key::Character("e".into()),
        Some(KeyCode::KeyE),
        Some("e"),
        none,
    );
    assert!(panel.is_editing_card());
    panel.on_ime_commit(" --lib");
    let edit_action = panel.on_key(&Key::Named(NamedKey::Enter), None, None, none);
    let AgentPanelAction::SendCommand(decide_cmd) = edit_action else {
        panic!("expected ApproveEdited decision, got {edit_action:?}");
    };
    assert_eq!(
        decide_cmd,
        AgentCommand::Decide {
            session: SessionId(1),
            step: StepId(3),
            decision: Decision::ApproveEdited("cargo test --lib".into()),
        }
    );
    assert!(
        panel
            .active_session()
            .unwrap()
            .step(StepId(3))
            .unwrap()
            .user_edited
    );
    handle.commands.send(decide_cmd).unwrap();

    // Receive RunInPane event.
    let run_ev = handle.events.recv_timeout(Duration::from_secs(2)).unwrap();
    let run_action = panel.apply_event(run_ev);
    assert!(matches!(
        run_action,
        AgentPanelAction::RunInPane {
            step: StepId(3),
            ref command,
            ..
        } if command == "cargo test --lib"
    ));

    // Report CommandFinished back to engine.
    handle
        .commands
        .send(AgentCommand::CommandFinished {
            session: SessionId(1),
            step: StepId(3),
            exit: Some(0),
            duration_ms: 1420,
            output: "test result: ok. 42 passed".into(),
        })
        .unwrap();

    for _ in 0..3 {
        let ev = handle.events.recv_timeout(Duration::from_secs(2)).unwrap();
        panel.apply_event(ev);
    }
    assert!(!panel.is_running());
    assert_eq!(panel.steps().len(), 5);

    // Attach block link to CommandResult and test "Open block" button.
    panel.attach_block_link(SessionId(1), StepId(4), PaneId(7), 99);
    assert_eq!(
        panel.activate_card_button(StepId(4), CardButton::OpenBlock),
        AgentPanelAction::OpenBlock {
            pane: PaneId(7),
            block_id: 99,
        }
    );

    // Copy button on Message card.
    assert_eq!(
        panel.activate_card_button(StepId(5), CardButton::Copy),
        AgentPanelAction::CopyText("All tests passed.".into())
    );
}

#[test]
fn patch_edit_mode_fact_suggestion_limit_reached_and_deny_keys_work() {
    let mut panel = AgentPanelState::new();
    panel.open();
    let sid = SessionId(10);
    let none = ModifiersState::empty();

    // Waiting Patch card: E enters inline diff editor, Enter inserts newline, Ctrl+Enter submits.
    panel.apply_event(AgentEvent::StepAdded {
        session: sid,
        step: Step {
            id: StepId(1),
            kind: StepKind::Patch {
                files: vec![PatchFile {
                    path: PathBuf::from("src/main.rs"),
                    added: 2,
                    removed: 1,
                    new_file: false,
                    deleted: false,
                }],
                diff: sample_diff(),
                reason: "Update greeting".into(),
            },
            status: StepStatus::WaitingApproval,
        },
    });
    assert!(panel.needs_input());
    panel.on_key(
        &Key::Character("e".into()),
        Some(KeyCode::KeyE),
        Some("e"),
        none,
    );
    assert!(panel.is_editing_card());
    // Plain Enter inside Patch editor inserts newline rather than submitting.
    let newline_act = panel.on_key(&Key::Named(NamedKey::Enter), None, None, none);
    assert_eq!(newline_act, AgentPanelAction::Redraw);
    assert!(panel.is_editing_card());
    // Esc cancels edit cleanly.
    panel.on_key(&Key::Named(NamedKey::Escape), None, None, none);
    assert!(!panel.is_editing_card());
    // Re-enter edit and submit with Ctrl+Enter.
    panel.begin_edit(StepId(1));
    let submit = panel.on_key(
        &Key::Named(NamedKey::Enter),
        None,
        None,
        ModifiersState::CONTROL,
    );
    assert!(matches!(
        submit,
        AgentPanelAction::SendCommand(AgentCommand::Decide {
            step: StepId(1),
            decision: Decision::ApproveEdited(_),
            ..
        })
    ));

    // PatchApplied -> Undo sends AgentCommand::Undo.
    panel.apply_event(AgentEvent::StepAdded {
        session: sid,
        step: Step {
            id: StepId(2),
            kind: StepKind::PatchApplied {
                files: vec![PathBuf::from("src/main.rs")],
                undo_available: true,
            },
            status: StepStatus::Done,
        },
    });
    assert_eq!(
        panel.activate_card_button(StepId(2), CardButton::Undo),
        AgentPanelAction::SendCommand(AgentCommand::Undo {
            session: sid,
            step: StepId(2),
        })
    );

    // FactSuggestion: D sends Deny; Enter sends Approve.
    panel.apply_event(AgentEvent::StepAdded {
        session: sid,
        step: Step {
            id: StepId(3),
            kind: StepKind::FactSuggestion {
                text: "Use cargo fmt before commit".into(),
            },
            status: StepStatus::WaitingApproval,
        },
    });
    assert_eq!(
        panel.on_key(
            &Key::Character("d".into()),
            Some(KeyCode::KeyD),
            Some("d"),
            none
        ),
        AgentPanelAction::SendCommand(AgentCommand::Decide {
            session: sid,
            step: StepId(3),
            decision: Decision::Deny,
        })
    );

    // LimitReached: Enter continues (Approve), Esc stops (Deny).
    panel.apply_event(AgentEvent::StepAdded {
        session: sid,
        step: Step {
            id: StepId(4),
            kind: StepKind::LimitReached {
                what: "Reached 25 steps limit".into(),
            },
            status: StepStatus::WaitingApproval,
        },
    });
    assert_eq!(
        panel.on_key(&Key::Named(NamedKey::Enter), None, None, none),
        AgentPanelAction::SendCommand(AgentCommand::Decide {
            session: sid,
            step: StepId(4),
            decision: Decision::Approve,
        })
    );
    assert_eq!(
        panel.on_key(&Key::Named(NamedKey::Escape), None, None, none),
        AgentPanelAction::SendCommand(AgentCommand::Decide {
            session: sid,
            step: StepId(4),
            decision: Decision::Deny,
        })
    );

    // Stop button sends AgentCommand::Stop while running.
    assert_eq!(
        panel.stop_run(),
        AgentPanelAction::SendCommand(AgentCommand::Stop { session: sid })
    );
}

#[test]
fn context_inspector_toggles_items_and_updates_strip_and_token_totals() {
    let mut panel = AgentPanelState::new();
    panel.open();
    panel.set_context_bundle(sample_context());

    assert_eq!(
        format_context_strip(&panel.context, UiLanguage::English),
        "Rules · 3 memories · 2 blocks"
    );
    assert_eq!(included_context_tokens(&panel.context), 740);

    // Open inspector sheet via click on ContextStrip.
    panel.on_click(AgentPanelHit::ContextStrip);
    assert!(panel.inspector_open);
    assert_eq!(panel.focus, AgentFocus::Inspector);

    // Space toggles item 0 (AGENTS.md, 120 tokens) off.
    panel.on_key(
        &Key::Named(NamedKey::Space),
        None,
        None,
        ModifiersState::empty(),
    );
    assert!(!panel.context.items[0].included);
    assert_eq!(included_context_tokens(&panel.context), 620);
    assert_eq!(
        format_context_strip(&panel.context, UiLanguage::English),
        "3 memories · 2 blocks"
    );

    // Esc closes inspector.
    panel.on_key(
        &Key::Named(NamedKey::Escape),
        None,
        None,
        ModifiersState::empty(),
    );
    assert!(!panel.inspector_open);
}

#[test]
fn scroll_and_jump_to_latest_and_paragraph_direction() {
    assert!(paragraph_is_rtl("مرحبا بالعالم — English inside"));
    assert!(!paragraph_is_rtl("Fix the bug in العربية"));
    assert!(!paragraph_is_rtl("12345"));

    let mut panel = AgentPanelState::new();
    panel.open();
    panel.sync_scroll(500.0);
    assert!(panel.pinned_to_bottom);
    assert!(!panel.show_jump_to_latest());
    assert_eq!(panel.scroll_y, 500.0);

    // Wheel up unpins and exposes Jump to latest pill.
    assert!(panel.on_wheel(3.0, 20.0));
    assert!(!panel.pinned_to_bottom);
    assert!(panel.show_jump_to_latest());
    assert!(panel.scroll_y < 500.0);

    // Clicking Jump to latest re-pins to bottom.
    panel.on_click(AgentPanelHit::JumpToLatest);
    assert!(panel.pinned_to_bottom);
    assert_eq!(panel.scroll_y, 500.0);
}

#[test]
fn agent_panel_clips_underlying_terminal_text_and_draws_needs_input_pane_header() {
    let mut font_system = test_font_system();
    let ui_font = UiFont::new(&mut font_system, None);
    let mut term = Terminal::new(80, 24, |_| {});
    term.process_bytes("Hello terminal text — نص عربي في الطرفية\r\n".as_bytes());
    let (lines, cursor) = term.snapshot();

    for (lang, palette_name, scale) in [
        (UiLanguage::English, "twitty-dark", 1.0_f32),
        (UiLanguage::Arabic, "twitty-light", 1.5_f32),
    ] {
        let palette = Palette::named(palette_name).unwrap();
        let metrics = CellMetrics::measure(&mut font_system, None, 14.5, scale);
        let style = UiStyle::new(&palette, None, scale);
        let surface = (1200.0 * scale, 760.0 * scale);
        let tab_area = Rect::new(0.0, 36.0 * scale, surface.0, surface.1 - 36.0 * scale);
        let (term_rect, panel_rect) = split_agent_content(tab_area, scale, None, lang);
        let panel_rect = panel_rect.unwrap();

        let mut panel = AgentPanelState::new();
        panel.open();
        panel.language = lang;
        panel.set_project_context(PathBuf::from("/workspace/twitty"));
        panel.apply_event(AgentEvent::StepAdded {
            session: SessionId(1),
            step: Step {
                id: StepId(1),
                kind: StepKind::Command {
                    command: "cargo test".into(),
                    cwd: PathBuf::from("/workspace/twitty"),
                    reason: if lang == UiLanguage::Arabic {
                        "تشغيل اختبارات المشروع".into()
                    } else {
                        "Run project tests".into()
                    },
                    after_untrusted_data: true,
                },
                status: StepStatus::WaitingApproval,
            },
        });
        let view = panel.view(panel_rect, lang, true);
        let layout = AgentPanelLayout::build(&view, metrics);
        assert!(layout.cards.len() == 1);

        // Deliberately place a terminal pane underneath the panel rect to prove clipping!
        let panes = [PaneFrame {
            id: PaneId(1),
            rect: tab_area,
            lines: lines.clone(),
            cursor: cursor.clone(),
            focused: true,
            status: None,
            blocks: Vec::new(),
        }];
        let mut cache = RowCache::default();
        let mut ime = PreeditState::default();
        let mut ctx = DrawContext {
            font_system: &mut font_system,
            family: None,
            palette: &palette,
            metrics,
            is_srgb: false,
        };
        let opts = FrameOptions {
            window_focused: true,
            cursor_style: "block",
            cursor_blink_on: true,
            surface,
        };
        let mut frame = build_frame(&mut ctx, &opts, &mut cache, &mut ime, &[], &panes, &[]);

        // Also test pane header "Needs input" badge in both languages.
        let win_layout =
            WindowLayout::new(metrics, surface, 1, Default::default(), true).with_language(lang);
        let hdr_layout = PaneHeaderLayout::new(term_rect, scale, true).with_language(lang);
        let pane_headers = [PaneHeaderView {
            id: PaneId(1),
            layout: hdr_layout,
            title: "Agent".into(),
            agent: true,
            focused: true,
            attention: false,
            needs_input: true,
            zoomed: false,
            flash: 0.0,
        }];
        let chrome = ChromeView {
            layout: &win_layout,
            top: TopBarLayout::new(win_layout.top_bar, scale, None).with_language(lang),
            diff: None,
            unread: 0,
            sidebar_visible: false,
            hover: None,
            hover_tab: None,
            language: lang,
            panes: &pane_headers,
            tabs: &[],
        };
        frame.add_chrome(&mut ctx, style, &ui_font, &chrome);
        frame.add_agent_panel(&mut ctx, &view, style, &ui_font);

        let runs = frame.text_runs(&panes, &cache, &ime);
        assert!(!runs.is_empty());

        // Verify the "Needs input" header pill label was produced.
        let expected_pill = lang.t("Needs input");
        assert!(
            runs.iter().any(|r| r
                .buffer
                .lines
                .iter()
                .any(|l| l.text().contains(expected_pill))),
            "pane header missing {expected_pill:?}"
        );

        // Verify terminal row runs (those coming from pane 1) never overlap the panel rect.
        let pane_runs: Vec<_> = runs
            .iter()
            .filter(|r| {
                r.buffer
                    .lines
                    .iter()
                    .any(|l| l.text().contains("Hello terminal text"))
            })
            .collect();
        for run in pane_runs {
            let b = run.bounds;
            let overlaps = (b.left as f32) < panel_rect.right()
                && (b.right as f32) > panel_rect.x
                && (b.top as f32) < panel_rect.bottom()
                && (b.bottom as f32) > panel_rect.y;
            assert!(
                !overlaps,
                "terminal text leaked behind agent panel: bounds={b:?}, panel={panel_rect:?}"
            );
        }
    }
}

#[test]
fn settings_ai_section_keyring_config_allow_list_and_deny_list() {
    let mut cfg = TwittyConfig::default();
    assert_eq!(cfg.ai_ollama_url, DEFAULT_OLLAMA_URL);
    assert!(cfg.ai_monthly_cost_cap_usd.is_none());
    assert!(cfg.ai_project_allow_list.is_empty());
    assert!(!HARD_DENY_SUMMARY.is_empty());

    let mut state = SettingsPanelState::new(Vec::new(), Vec::new(), None, UiLanguage::English);
    state.ai.active_project_root = Some("/workspace/twitty".into());
    state.select_section(SettingsSection::Ai, &cfg);

    let store = InMemorySecretStore::new();
    state.ai.refresh_key_flags(&store);
    assert_eq!(state.ai.saved_keys, [false, false, false]);

    // Click "Set key..." for Anthropic, type a synthetic split test secret, submit via Enter.
    let surface = (960.0, 640.0);
    let layout = SettingsLayout::build(surface, 1.0, &state, &cfg);
    let set_btn = layout.ai.as_ref().unwrap().api_rows[0].set_key_button;
    state.on_mouse_down(
        (set_btn.x + 4.0) as f64,
        (set_btn.y + 4.0) as f64,
        &mut cfg,
        surface,
        1.0,
    );
    assert_eq!(state.ai.active_field, Some(AiTextField::ApiKey(0)));

    let fake_key = concat!("sk-", "ant-", "unit-test-only-0001");
    state.on_text(fake_key, &mut cfg);
    let outcome = state.on_key(
        &Key::Named(NamedKey::Enter),
        None,
        ModifiersState::empty(),
        &mut cfg,
        surface,
        1.0,
    );
    // Secret must be scrubbed from draft and redacted in Debug output!
    assert!(state.ai.key_draft.is_empty());
    assert!(!format!("{outcome:?}").contains(fake_key));

    let (account, secret) = outcome.save_secret.expect("save_secret outcome");
    assert_eq!(account, API_PROVIDERS[0].keyring_account);
    state
        .ai
        .apply_save_secret(&store, &account, &secret, UiLanguage::English);
    assert!(state.ai.saved_keys[0]);
    assert_eq!(store.get(&account).unwrap().unwrap().expose(), fake_key);

    // Config JSON must NEVER contain the API key.
    let serialized = serde_json::to_string(&cfg).unwrap();
    assert!(!serialized.contains(fake_key));

    // Per-project allow-list: rejects shell metacharacters, accepts clean argv prefix.
    state.ai.allow_list_draft = "cargo test && rm -rf /".into();
    assert!(!state.ai.add_allow_list_entry(&mut cfg, UiLanguage::English));
    assert!(cfg.ai_project_allow_list.is_empty());

    state.ai.allow_list_draft = "cargo test".into();
    assert!(state.ai.add_allow_list_entry(&mut cfg, UiLanguage::English));
    assert_eq!(
        cfg.ai_project_allow_list.get("/workspace/twitty"),
        Some(&vec!["cargo test".to_owned()])
    );

    // Project memory entry (n21) lives in the same Settings -> AI section and opens Memory.
    assert_eq!(state.focused_setting(), Some(SettingId::ProjectMemory));
    let mem_layout = SettingsLayout::build(surface, 1.0, &state, &cfg);
    let ids: Vec<SettingId> = mem_layout.rows.iter().map(|r| r.id).collect();
    assert_eq!(
        ids,
        [
            SettingId::ProjectMemory,
            SettingId::BuiltInAi,
            SettingId::AiTimeout
        ]
    );
    let open_mem = state.on_key(
        &Key::Named(NamedKey::Enter),
        None,
        ModifiersState::empty(),
        &mut cfg,
        surface,
        1.0,
    );
    assert!(open_mem.open_memory);

    // Cost cap validation.
    state.ai.cost_cap_draft = "12.5".into();
    assert!(state.ai.commit_cost_cap(&mut cfg, UiLanguage::English));
    assert_eq!(cfg.ai_monthly_cost_cap_usd, Some(12.5));

    // Default provider click.
    let layout = SettingsLayout::build(surface, 1.0, &state, &cfg);
    let anthropic_btn = layout.ai.as_ref().unwrap().default_provider_buttons[2].1;
    let prov_out = state.on_mouse_down(
        (anthropic_btn.x + 4.0) as f64,
        (anthropic_btn.y + 4.0) as f64,
        &mut cfg,
        surface,
        1.0,
    );
    assert!(prov_out.ai_config_changed);
    assert_eq!(cfg.ai_default_provider.as_deref(), Some("anthropic"));

    // Config round-trip via temp file.
    let tmp = std::env::temp_dir().join(format!(
        "twitty-ai-settings-test-{}.json",
        std::process::id()
    ));
    cfg.save_to(&tmp).unwrap();
    let loaded = TwittyConfig::load_from(&tmp);
    let _ = std::fs::remove_file(&tmp);
    assert_eq!(loaded.ai_default_provider.as_deref(), Some("anthropic"));
    assert_eq!(loaded.ai_ollama_url, DEFAULT_OLLAMA_URL);
    assert_eq!(loaded.ai_monthly_cost_cap_usd, Some(12.5));
    assert_eq!(
        loaded.ai_project_allow_list.get("/workspace/twitty"),
        Some(&vec!["cargo test".to_owned()])
    );

    // Smoke-render Settings → AI in both English and Arabic.
    let mut font_system = test_font_system();
    let ui_font = UiFont::new(&mut font_system, None);
    let palette = Palette::named("twitty-dark").unwrap();
    let metrics = CellMetrics::measure(&mut font_system, None, 14.5, 1.0);
    let style = UiStyle::new(&palette, None, 1.0);
    for lang in [UiLanguage::English, UiLanguage::Arabic] {
        state.language = lang;
        let layout = SettingsLayout::build(surface, 1.0, &state, &cfg);
        assert!(matches!(
            layout.hit(
                (layout.ai.as_ref().unwrap().ollama_input.x + 4.0) as f64,
                (layout.ai.as_ref().unwrap().ollama_input.y + 4.0) as f64,
            ),
            SettingsHit::AiOllamaInput
        ));
        let mut cache = RowCache::default();
        let mut ime = PreeditState::default();
        let mut ctx = DrawContext {
            font_system: &mut font_system,
            family: None,
            palette: &palette,
            metrics,
            is_srgb: false,
        };
        let opts = FrameOptions {
            window_focused: true,
            cursor_style: "block",
            cursor_blink_on: true,
            surface,
        };
        let mut frame = build_frame(&mut ctx, &opts, &mut cache, &mut ime, &[], &[], &[]);
        frame.add_settings(
            &mut ctx,
            style,
            &ui_font,
            &SettingsView {
                state: state.clone(),
                config: cfg.clone(),
            },
        );
        let runs = frame.text_runs(&[], &cache, &ime);
        assert!(runs.len() > 10, "expected Settings → AI labels to render");
    }
}

#[test]
fn arabic_plural_agreement_and_context_strip_formatting() {
    assert_eq!(
        arabic_count(1, "ذكرى واحدة", "ذكريان", "ذكريات", "ذكرى"),
        "ذكرى واحدة"
    );
    assert_eq!(
        arabic_count(2, "ذكرى واحدة", "ذكريان", "ذكريات", "ذكرى"),
        "ذكريان"
    );
    assert_eq!(
        arabic_count(3, "ذكرى واحدة", "ذكريان", "ذكريات", "ذكرى"),
        "3 ذكريات"
    );
    assert_eq!(
        arabic_count(10, "ذكرى واحدة", "ذكريان", "ذكريات", "ذكرى"),
        "10 ذكريات"
    );
    assert_eq!(
        arabic_count(11, "ذكرى واحدة", "ذكريان", "ذكريات", "ذكرى"),
        "11 ذكرى"
    );
    assert_eq!(
        arabic_count(103, "ذكرى واحدة", "ذكريان", "ذكريات", "ذكرى"),
        "103 ذكريات"
    );
    assert_eq!(
        arabic_count(1, "كتلة واحدة", "كتلتان", "كتل", "كتلة"),
        "كتلة واحدة"
    );
    assert_eq!(
        arabic_count(2, "كتلة واحدة", "كتلتان", "كتل", "كتلة"),
        "كتلتان"
    );
    assert_eq!(
        arabic_count(5, "كتلة واحدة", "كتلتان", "كتل", "كتلة"),
        "5 كتل"
    );
    assert_eq!(
        arabic_count(15, "كتلة واحدة", "كتلتان", "كتل", "كتلة"),
        "15 كتلة"
    );

    // 1 rule + 1 memory + 1 block -> proper singular Arabic agreement.
    let single = ContextBundle {
        project_root: Some(PathBuf::from("/workspace/twitty")),
        budget_tokens: 4096,
        items: vec![
            ContextItem {
                kind: ContextKind::Rule,
                label: "AGENTS.md".into(),
                text: "Rules".into(),
                est_tokens: 100,
                included: true,
            },
            ContextItem {
                kind: ContextKind::Memory,
                label: "fact #1".into(),
                text: "Memory".into(),
                est_tokens: 50,
                included: true,
            },
            ContextItem {
                kind: ContextKind::Block,
                label: "block: cargo test".into(),
                text: "Block".into(),
                est_tokens: 80,
                included: true,
            },
        ],
    };
    assert_eq!(
        format_context_strip(&single, UiLanguage::Arabic),
        "القواعد · ذكرى واحدة · كتلة واحدة"
    );

    // 1 rule + 3 memories + 2 blocks -> dual + few plural forms.
    assert_eq!(
        format_context_strip(&sample_context(), UiLanguage::Arabic),
        "القواعد · 3 ذكريات · كتلتان"
    );

    // Empty bundle.
    assert_eq!(
        format_context_strip(&ContextBundle::default(), UiLanguage::Arabic),
        "السياق · لا عناصر"
    );
}

#[test]
fn context_assembly_honors_inspector_toggles_and_remember_fact_integration() {
    use twitty_ai::context::SelectedBlock;
    use twitty_store::{MemoryKind, NewMemory, Store, StoreHandle};

    let tmp_dir = std::env::temp_dir().join(format!("twitty-n20-ctx-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(tmp_dir.join(".git"));
    let resolved_root = twitty_store::resolve_project_root(&tmp_dir);
    let root_str = resolved_root.to_string_lossy().into_owned();

    let store = Store::open_in_memory().expect("in-memory store");
    store
        .create_memory(&NewMemory {
            project_root: root_str.clone(),
            kind: MemoryKind::Rule,
            text: "Keep grid logical".into(),
            source: "user".into(),
            approved: true,
        })
        .unwrap();
    store
        .create_memory(&NewMemory {
            project_root: root_str.clone(),
            kind: MemoryKind::Fact,
            text: "Golden tests need llvmpipe".into(),
            source: "agent".into(),
            approved: true,
        })
        .unwrap();

    let blocks = [SelectedBlock {
        command: Some("cargo test".into()),
        exit_code: Some(0),
        output: "test result: ok.".into(),
    }];

    let mut panel = AgentPanelState::new();
    panel.set_project_context(tmp_dir.clone());
    let first = panel.assemble_context(Some(&store), "llvmpipe", &blocks, &[], 4096);
    assert!(
        first.items.len() >= 3,
        "expected rule + fts fact + selected block, got {:?}",
        first.items
    );
    assert!(first.items.iter().all(|i| i.included));

    // Toggle item 0 off in the inspector -> returns RefreshContext and records override.
    let action = panel.toggle_context_item(0);
    assert_eq!(action, AgentPanelAction::RefreshContext);
    let toggled_label = panel.context.items[0].label.clone();

    // Re-assembling via twitty_ai::context::assemble preserves the user's untick toggle.
    let second = panel.assemble_context(Some(&store), "llvmpipe", &blocks, &[], 4096);
    let item = second
        .items
        .iter()
        .find(|i| i.label == toggled_label)
        .expect("toggled item still listed in inspector");
    assert!(
        !item.included,
        "unticked item must remain excluded after re-assembly"
    );

    // Verify twitty::remember_fact works through StoreHandle.
    let handle = StoreHandle::open_in_memory();
    assert!(twitty::remember_fact(
        &handle,
        Some(resolved_root.as_path()),
        "Always run cargo clippy with -D warnings"
    ));
    let listed = handle.list_memory(&root_str, true, 10).unwrap();
    assert!(listed
        .iter()
        .any(|m| m.text.contains("cargo clippy") && m.approved));

    let _ = std::fs::remove_dir_all(&tmp_dir);
}
