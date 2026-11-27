//! App-level hooks for built-in non-interactive AI features:
//! - Natural language to command in the composer (`#`, Ctrl+Enter, Generate button)
//! - Failed command block explanation and fix suggestions (block toolbar button & palette commands)
//! - One-time bilingual privacy consent persisted in the state directory
//! - Off-thread execution with timeout, cancellation on Esc, and animated spinner ticks

use std::time::{Duration, Instant};

use winit::event::{KeyEvent, MouseButton};
use winit::keyboard::{Key, NamedKey};
use winit::window::CursorIcon;

use super::{App, AppEvent};
use crate::ai::{
    agent_display_name, current_os_name, detect_shell_name, explain_block_prompt,
    nl_to_command_prompt, non_interactive_argv, parse_command_reply, parse_explain_reply,
    resolve_ai_agent, run_command_blocking, AiCancel, AiConsentStore, AiError, AiResolveError,
    AiResult, BlockAiCardState, BlockAiCardStatus, BlockAiMode, ComposerAiState, ConsentDecision,
    PendingAiAction, PendingConsent, MAX_AI_OUTPUT_BYTES,
};
use crate::blocks::cards::layout_cards;
use crate::composer::ComposerMode;
use crate::layout::PaneId;
use crate::render::scene::ai::{BlockAiHit, BlockAiView, ConsentHit, ConsentOverlayView};
use crate::render::scene::composer::{ComposerAiBanner, ComposerAiView};
use crate::ui::chrome::UiLanguage;

/// Spinner dot step interval while waiting on a local coding agent.
const SPINNER_INTERVAL: Duration = Duration::from_millis(240);

/// Window-wide built-in AI state owned by `App`.
pub(super) struct AiHost {
    consent: AiConsentStore,
    pending_consent: Option<PendingConsent>,
    pub composer_ai: ComposerAiState,
    block_card: Option<BlockAiCardState>,
    active: Option<(u64, AiCancel)>,
    next_request_id: u64,
    spinner_tick: Option<Instant>,
    spinner_phase: usize,
}

impl AiHost {
    pub(super) fn load() -> Self {
        Self {
            consent: AiConsentStore::load_default(),
            pending_consent: None,
            composer_ai: ComposerAiState::Idle,
            block_card: None,
            active: None,
            next_request_id: 0,
            spinner_tick: None,
            spinner_phase: 0,
        }
    }

    fn take_request_id(&mut self) -> u64 {
        self.next_request_id += 1;
        self.next_request_id
    }

    pub(super) fn is_animating(&self) -> bool {
        self.composer_ai.is_loading()
            || self
                .block_card
                .as_ref()
                .is_some_and(BlockAiCardState::is_loading)
    }

    pub(super) fn cancel_active(&mut self) {
        if let Some((_, cancel)) = self.active.take() {
            cancel.cancel();
        }
        if !self.is_animating() {
            self.spinner_tick = None;
            self.spinner_phase = 0;
        }
    }
}

impl App {
    pub(super) fn ai_consent_open(&self) -> bool {
        self.ai.pending_consent.is_some()
    }

    pub(super) fn block_ai_open(&self) -> bool {
        self.ai.block_card.is_some()
    }

    /// Cancel any in-flight AI request (used on Esc, new requests, and window exit).
    pub(super) fn cancel_ai_request(&mut self) {
        self.ai.cancel_active();
        if self.ai.composer_ai.is_loading() {
            self.ai.composer_ai = ComposerAiState::Idle;
            self.composer.state.ai_loading = false;
        }
        if self
            .ai
            .block_card
            .as_ref()
            .is_some_and(BlockAiCardState::is_loading)
        {
            self.ai.block_card = None;
        }
        self.sync_ai_spinner();
        self.window.request_redraw();
    }

    /// Clear composer AI review/error banners (e.g. after discard or submit).
    pub(super) fn clear_composer_ai_state(&mut self) {
        if let Some(req_id) = self.ai.composer_ai.loading_request_id() {
            if self.ai.active.as_ref().is_some_and(|(id, _)| *id == req_id) {
                self.ai.cancel_active();
            }
        }
        self.ai.composer_ai = ComposerAiState::Idle;
        self.composer.state.ai_loading = false;
        self.sync_ai_spinner();
    }

    /// Advance the thinking spinner while an AI request is in flight.
    pub(super) fn poll_ai_animation(&mut self, now: Instant) -> Option<Instant> {
        if !self.ai.is_animating() {
            self.ai.spinner_tick = None;
            return None;
        }
        let next = self
            .ai
            .spinner_tick
            .get_or_insert_with(|| now + SPINNER_INTERVAL);
        if now >= *next {
            self.ai.spinner_phase = (self.ai.spinner_phase + 1) % 3;
            *next = now + SPINNER_INTERVAL;
            self.window.request_redraw();
        }
        Some(*next)
    }

    fn sync_ai_spinner(&mut self) {
        if self.ai.is_animating() {
            self.ai
                .spinner_tick
                .get_or_insert_with(|| Instant::now() + SPINNER_INTERVAL);
        } else {
            self.ai.spinner_tick = None;
            self.ai.spinner_phase = 0;
        }
    }

    fn resolve_configured_ai_agent(&self) -> Result<Vec<String>, AiError> {
        let path = std::env::var_os("PATH");
        resolve_ai_agent(
            self.config.ai,
            self.config.ai_agent.as_deref(),
            self.config.agent_command.as_deref(),
            path.as_deref(),
        )
        .map_err(|e| match e {
            AiResolveError::Disabled => AiError::Disabled,
            AiResolveError::NotFound => AiError::NoAgent,
        })
    }

    // ---- Natural language to command (composer) ----

    /// Trigger natural-language-to-command generation from the composer (`#`, Ctrl+Enter, or button).
    pub(super) fn trigger_composer_ai(&mut self, request: String) {
        let request = request.trim().to_owned();
        if request.is_empty() {
            return;
        }
        let agent = match self.resolve_configured_ai_agent() {
            Ok(a) => a,
            Err(err) => {
                self.ai.composer_ai = ComposerAiState::Error {
                    message: err.message_localized(),
                };
                self.composer.state.ai_loading = false;
                self.window.request_redraw();
                return;
            }
        };
        if !self.ai.consent.is_accepted() {
            self.ai.pending_consent = Some(PendingConsent {
                agent_name: agent_display_name(&agent),
                action: PendingAiAction::ComposerGenerate { request },
            });
            self.window.request_redraw();
            return;
        }
        self.start_composer_ai_request(agent, request);
    }

    fn start_composer_ai_request(&mut self, agent: Vec<String>, request: String) {
        self.ai.cancel_active();
        let request_id = self.ai.take_request_id();
        let cancel = AiCancel::new();
        self.ai.active = Some((request_id, cancel.clone()));
        self.ai.composer_ai = ComposerAiState::Loading {
            request_id,
            prompt: request.clone(),
        };
        self.composer.state.ai_loading = true;
        self.composer.state.popup = None;
        self.sync_ai_spinner();

        let focused = self.focused_id();
        let cwd = focused.and_then(|id| self.pane_cwd(id));
        let pane_program = focused
            .and_then(|id| self.ws.pane(id))
            .and_then(|p| p.pty.as_ref()?.program_name())
            .map(str::to_owned);
        let shell = detect_shell_name(pane_program.as_deref());
        let os = current_os_name();
        let lang = self.window.ui_language();
        let prompt = nl_to_command_prompt(&request, cwd.as_deref(), os, &shell, lang);
        let argv = non_interactive_argv(&agent, &prompt);
        let timeout = Duration::from_secs(self.config.ai_timeout_secs.max(1));
        let proxy = self.proxy.clone();

        let spawn = std::thread::Builder::new()
            .name("twitty-ai-composer".into())
            .spawn(move || {
                let res = run_command_blocking(&argv, timeout, MAX_AI_OUTPUT_BYTES, &cancel)
                    .and_then(|raw| parse_command_reply(&raw).ok_or(AiError::UnparseableReply));
                let _ = proxy.send_event(AppEvent::AiComplete(
                    request_id,
                    AiResult::ComposerCommand(res),
                ));
            });

        if let Err(err) = spawn {
            self.ai.active = None;
            self.composer.state.ai_loading = false;
            self.ai.composer_ai = ComposerAiState::Error {
                message: AiError::SpawnFailed(err.to_string()).message_localized(),
            };
            self.sync_ai_spinner();
        }
        self.window.request_redraw();
    }

    // ---- Failed command block Explain / Suggest Fix ----

    /// Trigger Explain Error or Suggest a Fix on the selected or newest failed command block of `id`.
    pub(super) fn analyze_focused_failed_block(&mut self, id: PaneId, mode: BlockAiMode) {
        let Some(pane) = self.ws.pane(id) else {
            return;
        };
        let block_id = pane
            .blocks
            .selected()
            .filter(|b| b.exit.is_some_and(|c| c != 0))
            .or_else(|| {
                pane.blocks
                    .blocks()
                    .rev()
                    .find(|b| b.exit.is_some_and(|c| c != 0))
            })
            .map(|b| b.id());
        if let Some(bid) = block_id {
            self.analyze_failed_block(id, bid, mode);
        } else {
            self.ai.block_card = Some(BlockAiCardState {
                pane_id: id,
                block_id: 0,
                mode,
                command: None,
                exit_code: 0,
                status: BlockAiCardStatus::Error(
                    crate::ui::chrome::t("No failed command in this pane.").into(),
                ),
                scroll_lines: 0,
            });
            self.window.request_redraw();
        }
    }

    /// Trigger Explain Error or Suggest a Fix for a specific failed block `block_id` in pane `id`.
    pub(super) fn analyze_failed_block(&mut self, id: PaneId, block_id: u64, mode: BlockAiMode) {
        let Some(pane) = self.ws.pane(id) else {
            return;
        };
        let Some(block_text) = pane.blocks.block_text_by_id(block_id, &pane.terminal) else {
            return;
        };
        let exit_code = block_text.exit.unwrap_or(1);
        let agent = match self.resolve_configured_ai_agent() {
            Ok(a) => a,
            Err(err) => {
                self.ai.block_card = Some(BlockAiCardState {
                    pane_id: id,
                    block_id,
                    mode,
                    command: block_text.command,
                    exit_code,
                    status: BlockAiCardStatus::Error(err.message_localized()),
                    scroll_lines: 0,
                });
                self.window.request_redraw();
                return;
            }
        };
        if !self.ai.consent.is_accepted() {
            self.ai.pending_consent = Some(PendingConsent {
                agent_name: agent_display_name(&agent),
                action: PendingAiAction::BlockAnalyze {
                    pane_id: id,
                    block_id,
                    mode,
                },
            });
            self.window.request_redraw();
            return;
        }
        self.start_block_ai_request(agent, id, block_id, mode);
    }

    fn start_block_ai_request(
        &mut self,
        agent: Vec<String>,
        id: PaneId,
        block_id: u64,
        mode: BlockAiMode,
    ) {
        let Some(pane) = self.ws.pane(id) else {
            return;
        };
        let Some(block_text) = pane.blocks.block_text_by_id(block_id, &pane.terminal) else {
            return;
        };
        let exit_code = block_text.exit.unwrap_or(1);
        let cwd = self.pane_cwd(id);
        let pane_program = pane
            .pty
            .as_ref()
            .and_then(|p| p.program_name())
            .map(str::to_owned);
        let shell = detect_shell_name(pane_program.as_deref());
        let os = current_os_name();
        let lang = self.window.ui_language();

        self.ai.cancel_active();
        let request_id = self.ai.take_request_id();
        let cancel = AiCancel::new();
        self.ai.active = Some((request_id, cancel.clone()));
        self.ai.block_card = Some(BlockAiCardState::new_loading(
            id,
            block_id,
            mode,
            block_text.command.clone(),
            exit_code,
            request_id,
        ));
        self.sync_ai_spinner();

        let prompt = explain_block_prompt(
            mode,
            block_text.command.as_deref(),
            exit_code,
            &block_text.output,
            cwd.as_deref(),
            os,
            &shell,
            lang,
        );
        let argv = non_interactive_argv(&agent, &prompt);
        let timeout = Duration::from_secs(self.config.ai_timeout_secs.max(1));
        let proxy = self.proxy.clone();

        let spawn = std::thread::Builder::new()
            .name("twitty-ai-block".into())
            .spawn(move || {
                let res = run_command_blocking(&argv, timeout, MAX_AI_OUTPUT_BYTES, &cancel)
                    .and_then(|raw| parse_explain_reply(&raw).ok_or(AiError::EmptyReply));
                let _ = proxy.send_event(AppEvent::AiComplete(
                    request_id,
                    AiResult::BlockAnalysis(res),
                ));
            });

        if let Err(err) = spawn {
            self.ai.active = None;
            if let Some(card) = self.ai.block_card.as_mut() {
                card.status = BlockAiCardStatus::Error(
                    AiError::SpawnFailed(err.to_string()).message_localized(),
                );
            }
            self.sync_ai_spinner();
        }
        self.window.request_redraw();
    }

    /// Receive the result of an off-thread AI invocation on the UI thread.
    pub(super) fn on_ai_complete(&mut self, request_id: u64, result: AiResult) {
        if self
            .ai
            .active
            .as_ref()
            .is_some_and(|(id, _)| *id == request_id)
        {
            self.ai.active = None;
        }
        match result {
            AiResult::ComposerCommand(res) => {
                if self.ai.composer_ai.loading_request_id() != Some(request_id) {
                    return;
                }
                let original_prompt = match &self.ai.composer_ai {
                    ComposerAiState::Loading { prompt, .. } => prompt.clone(),
                    _ => String::new(),
                };
                self.composer.state.ai_loading = false;
                match res {
                    Ok(generated) => {
                        // NEVER run the generated command automatically: populate editor for review.
                        let explanation = generated.explanation.unwrap_or_else(|| {
                            match self.window.ui_language() {
                                UiLanguage::Arabic => "أمر مقترح — راجعه قبل التشغيل".to_owned(),
                                UiLanguage::English => {
                                    "Suggested command — review before running".to_owned()
                                }
                            }
                        });
                        self.composer.state.apply_ai_command(&generated.command);
                        self.ai.composer_ai = ComposerAiState::Ready {
                            explanation,
                            original_prompt,
                        };
                    }
                    Err(AiError::Cancelled) => {
                        self.ai.composer_ai = ComposerAiState::Idle;
                    }
                    Err(err) => {
                        self.ai.composer_ai = ComposerAiState::Error {
                            message: err.message_localized(),
                        };
                    }
                }
            }
            AiResult::BlockAnalysis(res) => {
                let matches_card = self.ai.block_card.as_ref().is_some_and(|c| {
                    matches!(c.status, BlockAiCardStatus::Loading { request_id: rid } if rid == request_id)
                });
                if !matches_card {
                    return;
                }
                match res {
                    Ok(reply) => {
                        if let Some(card) = self.ai.block_card.as_mut() {
                            card.status = BlockAiCardStatus::Ready(reply);
                            card.scroll_lines = 0;
                        }
                    }
                    Err(AiError::Cancelled) => {
                        self.ai.block_card = None;
                    }
                    Err(err) => {
                        if let Some(card) = self.ai.block_card.as_mut() {
                            card.status = BlockAiCardStatus::Error(err.message_localized());
                        }
                    }
                }
            }
        }
        self.sync_ai_spinner();
        self.window.request_redraw();
    }

    /// Insert the suggested fix command from the block AI popover into the composer for review
    /// (NEVER runs it automatically).
    pub(super) fn insert_block_fix_into_composer(&mut self) {
        let Some(cmd) = self
            .ai
            .block_card
            .as_ref()
            .and_then(BlockAiCardState::suggested_command)
            .map(str::to_owned)
        else {
            return;
        };
        let pane_id = self.ai.block_card.as_ref().map(|c| c.pane_id);
        self.ai.block_card = None;

        if self.config.composer_mode() == ComposerMode::Off {
            self.clipboard.set_text(cmd);
            self.window.request_redraw();
            return;
        }
        if let Some(pid) = pane_id {
            self.focus_pane(pid);
        }
        self.open_composer_with_ai_fix(&cmd);
    }

    // ---- Privacy consent modal input ----

    pub(super) fn handle_ai_consent_key(&mut self, event: &KeyEvent) -> bool {
        let Some(pending) = self.ai.pending_consent.as_ref() else {
            return false;
        };
        if !event.state.is_pressed() {
            return true;
        }
        match pending.key(&event.logical_key, self.modifiers) {
            ConsentDecision::Accept => self.accept_ai_consent(),
            ConsentDecision::Cancel => self.cancel_ai_consent(),
            ConsentDecision::None => {}
        }
        true
    }

    pub(super) fn ai_consent_cursor(&self, x: f64, y: f64) -> Option<CursorIcon> {
        if !self.ai_consent_open() {
            return None;
        }
        let icon = match self.window.ai_consent_hit(x, y) {
            ConsentHit::Accept | ConsentHit::Cancel => CursorIcon::Pointer,
            ConsentHit::Inside | ConsentHit::Outside => CursorIcon::Default,
        };
        Some(icon)
    }

    pub(super) fn ai_consent_mouse_press(&mut self, button: MouseButton) -> bool {
        if !self.ai_consent_open() {
            return false;
        }
        if button == MouseButton::Left {
            let (x, y) = self.pointer.pos;
            match self.window.ai_consent_hit(x, y) {
                ConsentHit::Accept => self.accept_ai_consent(),
                ConsentHit::Cancel | ConsentHit::Outside => self.cancel_ai_consent(),
                ConsentHit::Inside => {}
            }
        }
        true
    }

    fn accept_ai_consent(&mut self) {
        let Some(pending) = self.ai.pending_consent.take() else {
            return;
        };
        self.ai.consent.accept(&pending.agent_name);
        if let Err(err) = self.ai.consent.save_default() {
            crate::config::warn(format!("twitty: could not save AI consent ({err})"));
        }
        let agent = match self.resolve_configured_ai_agent() {
            Ok(a) => a,
            Err(err) => {
                match pending.action {
                    PendingAiAction::ComposerGenerate { .. } => {
                        self.ai.composer_ai = ComposerAiState::Error {
                            message: err.message_localized(),
                        };
                    }
                    PendingAiAction::BlockAnalyze {
                        pane_id,
                        block_id,
                        mode,
                    } => {
                        self.ai.block_card = Some(BlockAiCardState {
                            pane_id,
                            block_id,
                            mode,
                            command: None,
                            exit_code: 1,
                            status: BlockAiCardStatus::Error(err.message_localized()),
                            scroll_lines: 0,
                        });
                    }
                }
                self.window.request_redraw();
                return;
            }
        };
        match pending.action {
            PendingAiAction::ComposerGenerate { request } => {
                self.start_composer_ai_request(agent, request);
            }
            PendingAiAction::BlockAnalyze {
                pane_id,
                block_id,
                mode,
            } => {
                self.start_block_ai_request(agent, pane_id, block_id, mode);
            }
        }
    }

    fn cancel_ai_consent(&mut self) {
        self.ai.pending_consent = None;
        self.window.request_redraw();
    }

    // ---- Block AI card input ----

    pub(super) fn handle_block_ai_key(&mut self, event: &KeyEvent) -> bool {
        if !self.block_ai_open() {
            return false;
        }
        if !event.state.is_pressed() {
            return false;
        }
        if !self.modifiers.is_empty() {
            return false;
        }
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => {
                if self
                    .ai
                    .block_card
                    .as_ref()
                    .is_some_and(BlockAiCardState::is_loading)
                {
                    self.ai.cancel_active();
                }
                self.ai.block_card = None;
                self.sync_ai_spinner();
                self.window.request_redraw();
                true
            }
            Key::Named(NamedKey::ArrowUp) => {
                if let Some(card) = self.ai.block_card.as_mut() {
                    card.scroll_by(-1, 200);
                    self.window.request_redraw();
                }
                true
            }
            Key::Named(NamedKey::ArrowDown) => {
                if let Some(card) = self.ai.block_card.as_mut() {
                    card.scroll_by(1, 200);
                    self.window.request_redraw();
                }
                true
            }
            _ => false,
        }
    }

    pub(super) fn block_ai_cursor(&self, x: f64, y: f64) -> Option<CursorIcon> {
        if !self.block_ai_open() {
            return None;
        }
        match self.window.block_ai_hit(x, y) {
            BlockAiHit::Outside => None,
            BlockAiHit::Close | BlockAiHit::InsertFix | BlockAiHit::SuggestFix => {
                Some(CursorIcon::Pointer)
            }
            BlockAiHit::Inside => Some(CursorIcon::Default),
        }
    }

    pub(super) fn block_ai_mouse_press(&mut self, button: MouseButton) -> bool {
        if !self.block_ai_open() {
            return false;
        }
        let (x, y) = self.pointer.pos;
        match self.window.block_ai_hit(x, y) {
            BlockAiHit::Outside => false,
            BlockAiHit::Close if button == MouseButton::Left => {
                if self
                    .ai
                    .block_card
                    .as_ref()
                    .is_some_and(BlockAiCardState::is_loading)
                {
                    self.ai.cancel_active();
                }
                self.ai.block_card = None;
                self.sync_ai_spinner();
                self.window.request_redraw();
                true
            }
            BlockAiHit::InsertFix if button == MouseButton::Left => {
                self.insert_block_fix_into_composer();
                true
            }
            BlockAiHit::SuggestFix if button == MouseButton::Left => {
                if let Some((pane_id, block_id)) =
                    self.ai.block_card.as_ref().map(|c| (c.pane_id, c.block_id))
                {
                    if block_id != 0 {
                        self.analyze_failed_block(pane_id, block_id, BlockAiMode::SuggestFix);
                    }
                }
                true
            }
            BlockAiHit::Inside
            | BlockAiHit::Close
            | BlockAiHit::InsertFix
            | BlockAiHit::SuggestFix => true,
        }
    }

    pub(super) fn block_ai_wheel(&mut self, lines: f64) -> bool {
        if !self.block_ai_open() {
            return false;
        }
        let (x, y) = self.pointer.pos;
        if self.window.block_ai_hit(x, y) == BlockAiHit::Outside {
            return false;
        }
        if let Some(card) = self.ai.block_card.as_mut() {
            let delta = if lines > 0.0 { -2 } else { 2 };
            card.scroll_by(delta, 200);
            self.window.request_redraw();
        }
        true
    }

    // ---- View builders ----

    pub(super) fn composer_ai_view(&self) -> ComposerAiView {
        let banner = match &self.ai.composer_ai {
            ComposerAiState::Idle => ComposerAiBanner::None,
            ComposerAiState::Loading { .. } => ComposerAiBanner::Loading {
                phase: self.ai.spinner_phase,
            },
            ComposerAiState::Ready { explanation, .. } => ComposerAiBanner::Ready {
                explanation: explanation.clone(),
            },
            ComposerAiState::Error { message } => ComposerAiBanner::Error {
                message: message.clone(),
            },
        };
        ComposerAiView {
            enabled: self.config.ai,
            banner,
        }
    }

    pub(super) fn block_ai_view(&self) -> Option<BlockAiView> {
        let card = self.ai.block_card.as_ref()?;
        let pane_rect = self.pane_rect(card.pane_id)?;
        let anchor = self
            .block_card_rect(card.pane_id, card.block_id)
            .unwrap_or(pane_rect);
        Some(BlockAiView {
            anchor,
            mode: card.mode,
            command: card.command.clone(),
            exit_code: card.exit_code,
            status: card.status.clone(),
            spinner_phase: self.ai.spinner_phase,
            scroll_lines: card.scroll_lines,
            language: self.window.ui_language(),
        })
    }

    pub(super) fn ai_consent_view(&self) -> Option<ConsentOverlayView> {
        let pending = self.ai.pending_consent.as_ref()?;
        Some(ConsentOverlayView {
            agent_name: pending.agent_name.clone(),
            language: self.window.ui_language(),
        })
    }

    fn block_card_rect(&self, pane_id: PaneId, block_id: u64) -> Option<crate::layout::Rect> {
        if block_id == 0 {
            return None;
        }
        let pane = self.ws.pane(pane_id)?;
        let pane_rect = self.pane_rect(pane_id)?;
        let metrics = self.window.metrics()?;
        let decor = pane.blocks.decor(&pane.terminal);
        let cards = layout_cards(pane_rect, metrics, pane.size.1, &decor, None, None, |_| 0.0);
        cards.into_iter().find(|c| c.id == block_id).map(|c| c.card)
    }
}
