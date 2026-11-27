//! Settings → Commands: add, edit and delete the user's saved commands (`config.commands`)
//! without editing `config.json`. Window-free state, layout and input handling; drawing lives in
//! `render::scene::settings`.
//!
//! Entries are validated by the same rules as `config.json` ([`crate::workflows::parse_entry`]):
//! a name and a command are required, text is cleaned and bounded, names are unique.

use serde_json::json;
use twitty_core::en;
use twitty_core::language::fill;
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::config::TwittyConfig;
use crate::layout::Rect;
use crate::ui::chrome::UiLanguage;
use crate::workflows::{parse_entry, CommandMode, WorkflowSpec, MAX_COMMANDS_PER_SOURCE};

/// A text field of the add/edit form, in Tab order (then the mode switch).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormField {
    Name,
    NameAr,
    Command,
    Mode,
}

impl FormField {
    pub const ALL: [FormField; 4] = [
        FormField::Name,
        FormField::NameAr,
        FormField::Command,
        FormField::Mode,
    ];

    pub fn label(self, lang: UiLanguage) -> &'static str {
        match self {
            FormField::Name => lang.t("Name"),
            FormField::NameAr => lang.t("Arabic name (optional)"),
            FormField::Command => lang.t("Command"),
            FormField::Mode => lang.t("When picked"),
        }
    }
}

/// The add/edit form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandForm {
    /// Index in `config.commands` being edited; `None` adds a new command.
    pub editing: Option<usize>,
    pub name: String,
    pub name_ar: String,
    pub command: String,
    pub mode: CommandMode,
    pub field: FormField,
    /// Why the last Save was refused, `(English, Arabic)`.
    pub error: Option<(String, String)>,
}

impl CommandForm {
    fn text_mut(&mut self) -> Option<&mut String> {
        match self.field {
            FormField::Name => Some(&mut self.name),
            FormField::NameAr => Some(&mut self.name_ar),
            FormField::Command => Some(&mut self.command),
            FormField::Mode => None,
        }
    }

    pub fn text(&self, field: FormField) -> &str {
        match field {
            FormField::Name => &self.name,
            FormField::NameAr => &self.name_ar,
            FormField::Command => &self.command,
            FormField::Mode => "",
        }
    }
}

/// What a key or click did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommandsOutcome {
    pub handled: bool,
    pub redraw: bool,
    /// `config.commands` changed: save and reload commands.
    pub changed: bool,
}

impl CommandsOutcome {
    fn redraw() -> Self {
        Self {
            handled: true,
            redraw: true,
            changed: false,
        }
    }

    fn changed() -> Self {
        Self {
            handled: true,
            redraw: true,
            changed: true,
        }
    }
}

/// Editor state kept by the Settings panel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandsEditorState {
    /// Highlighted row in the list.
    pub selected: usize,
    /// First visible row.
    pub scroll: usize,
    pub form: Option<CommandForm>,
}

/// Text limit for one form field while typing (the saved value is cleaned again).
const MAX_FIELD_CHARS: usize = 4096;

impl CommandsEditorState {
    pub fn start_add(&mut self) {
        self.form = Some(CommandForm {
            editing: None,
            name: String::new(),
            name_ar: String::new(),
            command: String::new(),
            mode: CommandMode::Insert,
            field: FormField::Name,
            error: None,
        });
    }

    pub fn start_edit(&mut self, index: usize, config: &TwittyConfig) {
        let Some(spec) = config.commands.get(index) else {
            return;
        };
        self.selected = index;
        self.form = Some(CommandForm {
            editing: Some(index),
            name: spec.name.clone(),
            name_ar: spec.name_ar.clone(),
            command: spec.command.clone(),
            mode: spec.mode,
            field: FormField::Name,
            error: None,
        });
    }

    pub fn delete(&mut self, index: usize, config: &mut TwittyConfig) -> bool {
        if index >= config.commands.len() {
            return false;
        }
        config.commands.remove(index);
        self.selected = self.selected.min(config.commands.len().saturating_sub(1));
        true
    }

    /// Validate the form and write it into `config.commands`.
    pub fn save(&mut self, config: &mut TwittyConfig) -> bool {
        let Some(form) = self.form.as_mut() else {
            return false;
        };
        let value = json!({
            "name": form.name,
            "name_ar": form.name_ar,
            "command": form.command,
            "mode": form.mode.as_str(),
        });
        let mut spec = match parse_entry(&value) {
            Ok(spec) => spec,
            Err(reason) => {
                form.error = Some(match reason.as_str() {
                    r if r.contains("\"name\"") => both(en!("Give the command a name"), &[]),
                    r if r.contains("\"command\"") => both(en!("Write the command to run"), &[]),
                    _ => (reason.clone(), reason),
                });
                return false;
            }
        };
        let duplicate = config.commands.iter().enumerate().any(|(i, c)| {
            Some(i) != form.editing && c.name.to_lowercase() == spec.name.to_lowercase()
        });
        if duplicate {
            form.error = Some(both(
                en!("A command named \"{}\" already exists"),
                &[&spec.name],
            ));
            return false;
        }
        match form.editing {
            Some(index) if index < config.commands.len() => {
                // Keep fields the form does not show (working folder, description).
                let old = &config.commands[index];
                spec.cwd = old.cwd.clone();
                spec.description = old.description.clone();
                config.commands[index] = spec;
                self.selected = index;
            }
            _ => {
                if config.commands.len() >= MAX_COMMANDS_PER_SOURCE {
                    form.error = Some(both(
                        en!("At most {} commands"),
                        &[&MAX_COMMANDS_PER_SOURCE],
                    ));
                    return false;
                }
                config.commands.push(spec);
                self.selected = config.commands.len() - 1;
            }
        }
        self.form = None;
        true
    }

    /// Typed or IME text goes into the focused form field.
    pub fn on_text(&mut self, text: &str) -> CommandsOutcome {
        let Some(form) = self.form.as_mut() else {
            return CommandsOutcome::default();
        };
        let Some(target) = form.text_mut() else {
            return CommandsOutcome {
                handled: true,
                ..Default::default()
            };
        };
        let before = target.len();
        for c in text.chars().filter(|c| !c.is_control()) {
            if target.chars().count() >= MAX_FIELD_CHARS {
                break;
            }
            target.push(c);
        }
        if target.len() != before {
            form.error = None;
            CommandsOutcome::redraw()
        } else {
            CommandsOutcome {
                handled: true,
                ..Default::default()
            }
        }
    }

    fn move_field(&mut self, back: bool) {
        if let Some(form) = self.form.as_mut() {
            let n = FormField::ALL.len();
            let i = FormField::ALL
                .iter()
                .position(|f| *f == form.field)
                .unwrap_or(0);
            form.field = FormField::ALL[if back { (i + n - 1) % n } else { (i + 1) % n }];
        }
    }

    /// Keys while the form is open: it owns Esc, Tab, Enter and editing keys.
    pub fn on_form_key(
        &mut self,
        key: &Key,
        text: Option<&str>,
        modifiers: ModifiersState,
        config: &mut TwittyConfig,
    ) -> CommandsOutcome {
        let Some(form) = self.form.as_mut() else {
            return CommandsOutcome::default();
        };
        match key {
            Key::Named(NamedKey::Escape) => {
                self.form = None;
                CommandsOutcome::redraw()
            }
            Key::Named(NamedKey::Tab) => {
                self.move_field(modifiers.shift_key());
                CommandsOutcome::redraw()
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.move_field(false);
                CommandsOutcome::redraw()
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.move_field(true);
                CommandsOutcome::redraw()
            }
            Key::Named(NamedKey::Enter) => {
                if self.save(config) {
                    CommandsOutcome::changed()
                } else {
                    CommandsOutcome::redraw()
                }
            }
            Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::Space)
                if form.field == FormField::Mode =>
            {
                form.mode = match form.mode {
                    CommandMode::Insert => CommandMode::Run,
                    CommandMode::Run => CommandMode::Insert,
                };
                CommandsOutcome::redraw()
            }
            Key::Named(NamedKey::Backspace) => {
                if let Some(target) = form.text_mut() {
                    let changed = if modifiers.control_key() {
                        let trimmed = target.trim_end().len();
                        let cut = target[..trimmed]
                            .rfind(char::is_whitespace)
                            .map_or(0, |i| i + 1);
                        let changed = cut < target.len();
                        target.truncate(cut);
                        changed
                    } else {
                        target.pop().is_some()
                    };
                    if changed {
                        form.error = None;
                        return CommandsOutcome::redraw();
                    }
                }
                CommandsOutcome {
                    handled: true,
                    ..Default::default()
                }
            }
            Key::Named(NamedKey::Space) => self.on_text(" "),
            _ => match text.filter(|_| {
                !modifiers.control_key() && !modifiers.alt_key() && !modifiers.super_key()
            }) {
                Some(t) => self.on_text(t),
                None => CommandsOutcome {
                    handled: true,
                    ..Default::default()
                },
            },
        }
    }

    /// Keys on the list: arrows move, Enter edits, Delete removes, Ctrl+N or Insert adds.
    pub fn on_list_key(
        &mut self,
        key: &Key,
        modifiers: ModifiersState,
        config: &mut TwittyConfig,
        visible_rows: usize,
    ) -> CommandsOutcome {
        let count = config.commands.len();
        let add = matches!(key, Key::Named(NamedKey::Insert))
            || (modifiers.control_key()
                && matches!(key, Key::Character(c) if c.eq_ignore_ascii_case("n")));
        if add || (count == 0 && matches!(key, Key::Named(NamedKey::Enter))) {
            self.start_add();
            return CommandsOutcome::redraw();
        }
        if count == 0 {
            return CommandsOutcome::default();
        }
        match key {
            Key::Named(NamedKey::ArrowDown) => {
                self.selected = (self.selected.min(count - 1) + 1) % count;
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.selected = (self.selected.min(count - 1) + count - 1) % count;
            }
            Key::Named(NamedKey::Enter) => {
                self.start_edit(self.selected.min(count - 1), config);
            }
            Key::Named(NamedKey::Delete) => {
                if self.delete(self.selected.min(count - 1), config) {
                    self.keep_visible(visible_rows);
                    return CommandsOutcome::changed();
                }
                return CommandsOutcome::default();
            }
            _ => return CommandsOutcome::default(),
        }
        self.keep_visible(visible_rows);
        CommandsOutcome::redraw()
    }

    fn keep_visible(&mut self, visible: usize) {
        let visible = visible.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible {
            self.scroll = self.selected + 1 - visible;
        }
    }
}

/// One command row in the list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CommandRowLayout {
    pub index: usize,
    pub rect: Rect,
    pub edit: Rect,
    pub delete: Rect,
}

/// Geometry of the Commands section.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandsLayout {
    pub add_button: Option<Rect>,
    /// One-line explanation above the list (or the empty-state text).
    pub intro: Option<Rect>,
    pub rows: Vec<CommandRowLayout>,
    pub visible_rows: usize,
    /// Form: `(field, label rect, input rect)`.
    pub fields: Vec<(FormField, Rect, Rect)>,
    pub mode_buttons: Vec<(CommandMode, Rect)>,
    pub save: Option<Rect>,
    pub cancel: Option<Rect>,
    pub error: Option<Rect>,
}

impl CommandsLayout {
    pub fn build(
        viewport: Rect,
        scale: f32,
        mirrored: bool,
        state: &CommandsEditorState,
        config: &TwittyConfig,
    ) -> Self {
        let px = |l: f32| (l * scale).round();
        let vp = viewport;
        let mut out = CommandsLayout::default();
        // Place a `w`-wide box at the reading-start or reading-end of `[x, x + width]`.
        let at_start = |x: f32, width: f32, w: f32| if mirrored { x + width - w } else { x };
        let at_end = |x: f32, width: f32, w: f32| if mirrored { x } else { x + width - w };

        if let Some(form) = &state.form {
            let mut y = vp.y;
            let label_h = px(18.0);
            let input_h = px(32.0);
            for field in FormField::ALL {
                let label = Rect::new(vp.x, y, vp.w, label_h);
                y += label_h + px(4.0);
                if field == FormField::Mode {
                    let w = px(120.0);
                    let gap = px(6.0);
                    let x0 = at_start(vp.x, vp.w, 2.0 * w + gap);
                    let first = Rect::new(x0, y, w, input_h);
                    let second = Rect::new(x0 + w + gap, y, w, input_h);
                    let (insert, run) = if mirrored {
                        (second, first)
                    } else {
                        (first, second)
                    };
                    out.mode_buttons = vec![(CommandMode::Insert, insert), (CommandMode::Run, run)];
                    out.fields
                        .push((field, label, Rect::new(x0, y, 2.0 * w + gap, input_h)));
                } else {
                    out.fields
                        .push((field, label, Rect::new(vp.x, y, vp.w, input_h)));
                }
                y += input_h + px(12.0);
            }
            if form.error.is_some() {
                out.error = Some(Rect::new(vp.x, y, vp.w, px(20.0)));
                y += px(26.0);
            }
            let bw = px(96.0);
            let gap = px(8.0);
            let save = Rect::new(at_end(vp.x, vp.w, bw), y, bw, px(32.0));
            let cancel_x = if mirrored {
                save.right() + gap
            } else {
                save.x - gap - bw
            };
            out.save = Some(save);
            out.cancel = Some(Rect::new(cancel_x, y, bw, px(32.0)));
            return out;
        }

        let add_w = px(150.0).min(vp.w * 0.4);
        let head_h = px(32.0);
        out.add_button = Some(Rect::new(at_end(vp.x, vp.w, add_w), vp.y, add_w, head_h));
        out.intro = Some(Rect::new(
            at_start(vp.x, vp.w, vp.w - add_w - px(12.0)),
            vp.y,
            (vp.w - add_w - px(12.0)).max(0.0),
            head_h,
        ));
        let list_y = vp.y + head_h + px(10.0);
        let row_h = px(48.0).max(1.0);
        let visible = (((vp.bottom() - list_y).max(0.0)) / row_h).floor() as usize;
        out.visible_rows = visible;
        let scroll = state
            .scroll
            .min(config.commands.len().saturating_sub(visible));
        let btn_h = px(26.0);
        let btn_w = px(64.0);
        let gap = px(6.0);
        let pad = px(8.0);
        for (slot, index) in (scroll..config.commands.len()).take(visible).enumerate() {
            let rect = Rect::new(vp.x, list_y + slot as f32 * row_h, vp.w, row_h - px(4.0));
            let by = rect.y + (rect.h - btn_h) / 2.0;
            let (edit, delete) = if mirrored {
                let delete = Rect::new(rect.x + pad, by, btn_w, btn_h);
                (Rect::new(delete.right() + gap, by, btn_w, btn_h), delete)
            } else {
                let delete = Rect::new(rect.right() - pad - btn_w, by, btn_w, btn_h);
                (Rect::new(delete.x - gap - btn_w, by, btn_w, btn_h), delete)
            };
            out.rows.push(CommandRowLayout {
                index,
                rect,
                edit,
                delete,
            });
        }
        out
    }
}

/// Display name of a command in the UI language.
pub fn display_name(spec: &WorkflowSpec, lang: UiLanguage) -> &str {
    if lang == UiLanguage::Arabic && !spec.name_ar.is_empty() {
        &spec.name_ar
    } else {
        &spec.name
    }
}

pub fn mode_label(mode: CommandMode, lang: UiLanguage) -> &'static str {
    match mode {
        CommandMode::Insert => lang.t("Insert for review"),
        CommandMode::Run => lang.t("Run in a split"),
    }
}

/// `template` filled in English and in Arabic, for a message shown in either language.
fn both(template: &str, args: &[&dyn std::fmt::Display]) -> (String, String) {
    (
        fill(template, args),
        fill(twitty_core::language::UiLanguage::Arabic.t(template), args),
    )
}
