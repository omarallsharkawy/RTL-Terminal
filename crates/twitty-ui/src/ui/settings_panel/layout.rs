//! Settings panel geometry and pointer hit testing, shared with the renderer.

use super::*;

/// Pointer hit target inside (or outside) the settings overlay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SettingsHit {
    Outside,
    Inside,
    CloseButton,
    Section(SettingsSection),
    Toggle(SettingId),
    Segment(SettingId, usize),
    SliderMinus(SettingId),
    SliderPlus(SettingId),
    SliderTrack(SettingId, f32),
    Swatch(usize),
    TextField(SettingId),
    PickerToggle(SettingId),
    PickerSearch(SettingId),
    PickerItem(SettingId, usize),
    KeysSearch,
    KeysResetAll,
    KeyRow(usize),
    KeyChange(usize),
    KeyReset(usize),
    CopyConfigPath,
    AiDefaultProvider(usize),
    AiSetKey(usize),
    AiSaveKey(usize),
    AiClearKey(usize),
    AiKeyInput(usize),
    AiOllamaInput,
    AiCostCapInput,
    AiAllowInput,
    AiAllowAdd,
    AiAllowRemove(usize),
    AiClearHistory,
    OpenMemory,
    CommandsAdd,
    CommandRow(usize),
    CommandEdit(usize),
    CommandDelete(usize),
    CommandField(crate::ui::settings_commands::FormField),
    CommandModeButton(crate::workflows::CommandMode),
    CommandSave,
    CommandCancel,
}

impl SettingsHit {
    /// True when the pointer should show a clickable hand cursor over this target.
    pub fn is_interactive(self) -> bool {
        !matches!(self, Self::Outside | Self::Inside)
    }

    /// True when the pointer should show a text I-beam cursor over this target.
    pub fn is_text_input(self) -> bool {
        matches!(
            self,
            Self::TextField(_)
                | Self::PickerSearch(_)
                | Self::KeysSearch
                | Self::AiKeyInput(_)
                | Self::AiOllamaInput
                | Self::AiCostCapInput
                | Self::AiAllowInput
                | Self::CommandField(
                    crate::ui::settings_commands::FormField::Name
                        | crate::ui::settings_commands::FormField::NameAr
                        | crate::ui::settings_commands::FormField::Command
                )
        )
    }
}

#[derive(Clone, Debug)]
pub enum ControlGeometry {
    Toggle {
        track: Rect,
    },
    Segmented {
        container: Rect,
        segments: Vec<Rect>,
    },
    Slider {
        minus: Rect,
        track: Rect,
        plus: Rect,
        value: Rect,
    },
    Swatches {
        swatches: Vec<Rect>,
    },
    TextField {
        input: Rect,
    },
    ListPicker {
        button: Rect,
    },
    ActionButton {
        button: Rect,
    },
}

#[derive(Clone, Debug)]
pub struct ExpandedPickerLayout {
    pub rect: Rect,
    pub search: Option<Rect>,
    /// Visible rows: `(filtered_option_index, row_rect)`.
    pub items: Vec<(usize, Rect)>,
    pub total_items: usize,
    pub scroll: usize,
}

#[derive(Clone, Debug)]
pub struct ControlRowLayout {
    pub id: SettingId,
    pub rect: Rect,
    pub label: Rect,
    pub widget: Rect,
    pub geometry: ControlGeometry,
    pub picker: Option<ExpandedPickerLayout>,
}

/// Physical-pixel geometry for one visible row in the `Keys` section.
#[derive(Clone, Copy, Debug)]
pub struct KeyRowLayout {
    pub cmd_idx: usize,
    pub rect: Rect,
    pub change_button: Rect,
    pub reset_button: Option<Rect>,
}

/// Physical-pixel geometry shared by `render::scene::settings` and pointer hit testing.
#[derive(Clone, Debug)]
pub struct SettingsLayout {
    pub panel: Rect,
    pub header: Rect,
    pub header_title: Rect,
    pub close_button: Rect,
    pub sidebar: Rect,
    pub divider: Rect,
    pub content: Rect,
    pub viewport: Rect,
    pub sections: [(SettingsSection, Rect); 6],
    pub rows: Vec<ControlRowLayout>,
    pub ai: Option<crate::ui::settings_ai::AiSectionLayout>,
    pub commands: Option<crate::ui::settings_commands::CommandsLayout>,
    pub keys_search: Option<Rect>,
    pub keys_reset_all: Option<Rect>,
    pub keys_banner: Option<Rect>,
    /// Visible shortcut rows: `(filtered_command_index, row_rect)`.
    pub keys_rows: Vec<(usize, Rect)>,
    /// Detailed per-row geometry for the `Keys` section (including Change and Reset buttons).
    pub keys_row_layouts: Vec<KeyRowLayout>,
    pub keys_visible_rows: usize,
    pub about_header: Option<Rect>,
    pub about_info_rows: Vec<Rect>,
    pub copy_button: Option<Rect>,
    pub scrollbar: Option<Rect>,
    /// Maximum vertical scroll in logical pixels for Appearance/Behaviour.
    pub max_scroll: f32,
    pub mirrored: bool,
}

impl SettingsLayout {
    pub fn build(
        surface: (f32, f32),
        scale: f32,
        state: &SettingsPanelState,
        config: &TwittyConfig,
    ) -> Self {
        let px = |logical: f32| logical * scale;
        let mirrored = state.mirrored();
        let (sw, sh) = surface;
        let margin = px(16.0).min(sw / 8.0).min(sh / 8.0);
        let panel_w = px(760.0).min((sw - 2.0 * margin).max(0.0)).round();
        let panel_h = px(560.0).min((sh - 2.0 * margin).max(0.0)).round();
        let panel_x = ((sw - panel_w) / 2.0).round();
        let panel_y = ((sh - panel_h) / 2.0).round();
        let panel = Rect::new(panel_x, panel_y, panel_w, panel_h);

        let header_h = px(44.0).round().min(panel_h);
        let header = Rect::new(panel_x, panel_y, panel_w, header_h);
        let hpad = px(14.0).min(panel_w / 6.0);
        let close_side = px(26.0).min(header_h).min(panel_w / 4.0);
        let close_y = header.y + (header.h - close_side) / 2.0;
        let (close_button, header_title) = if mirrored {
            let close = Rect::new(header.x + hpad, close_y, close_side, close_side);
            let title_x = close.right() + px(8.0);
            let title = Rect::new(
                title_x,
                header.y,
                (header.right() - hpad - title_x).max(0.0),
                header.h,
            );
            (close, title)
        } else {
            let close = Rect::new(
                header.right() - hpad - close_side,
                close_y,
                close_side,
                close_side,
            );
            let title = Rect::new(
                header.x + hpad,
                header.y,
                (close.x - px(8.0) - (header.x + hpad)).max(0.0),
                header.h,
            );
            (close, title)
        };

        let body_y = header.bottom();
        let body_h = (panel.bottom() - body_y).max(0.0);
        let sidebar_w = px(164.0).min(panel_w * 0.32).round();
        let div_w = px(1.0).max(1.0);
        let (sidebar, divider, content) = if mirrored {
            let sb = Rect::new(panel.right() - sidebar_w, body_y, sidebar_w, body_h);
            let div = Rect::new(sb.x - div_w, body_y, div_w, body_h);
            let ct = Rect::new(panel.x, body_y, (div.x - panel.x).max(0.0), body_h);
            (sb, div, ct)
        } else {
            let sb = Rect::new(panel.x, body_y, sidebar_w, body_h);
            let div = Rect::new(sb.right(), body_y, div_w, body_h);
            let ct = Rect::new(
                div.right(),
                body_y,
                (panel.right() - div.right()).max(0.0),
                body_h,
            );
            (sb, div, ct)
        };

        let sb_pad = px(10.0).min(sidebar.w / 6.0);
        let sb_item_h = px(34.0).round();
        let sb_gap = px(4.0).round();
        let sections = SettingsSection::ALL.map(|sec| {
            let i = sec.index() as f32;
            let rect = Rect::new(
                sidebar.x + sb_pad,
                sidebar.y + sb_pad + i * (sb_item_h + sb_gap),
                (sidebar.w - 2.0 * sb_pad).max(0.0),
                sb_item_h,
            );
            (sec, rect)
        });

        let cpad = px(16.0).min(content.w / 8.0);
        let sb_track_w = px(6.0);
        let (vp_x, vp_w) = if mirrored {
            (
                content.x + cpad + sb_track_w,
                (content.w - 2.0 * cpad - sb_track_w).max(0.0),
            )
        } else {
            (
                content.x + cpad,
                (content.w - 2.0 * cpad - sb_track_w).max(0.0),
            )
        };
        let viewport = Rect::new(
            vp_x,
            content.y + cpad,
            vp_w,
            (content.h - 2.0 * cpad).max(0.0),
        );

        let mut layout = Self {
            panel,
            header,
            header_title,
            close_button,
            sidebar,
            divider,
            content,
            viewport,
            sections,
            rows: Vec::new(),
            ai: None,
            commands: None,
            keys_search: None,
            keys_reset_all: None,
            keys_banner: None,
            keys_rows: Vec::new(),
            keys_row_layouts: Vec::new(),
            keys_visible_rows: 0,
            about_header: None,
            about_info_rows: Vec::new(),
            copy_button: None,
            scrollbar: None,
            max_scroll: 0.0,
            mirrored,
        };

        match state.section {
            SettingsSection::Appearance | SettingsSection::Behaviour => {
                layout.build_control_rows(scale, state, config);
            }
            SettingsSection::Ai => {
                layout.build_ai_layout(scale, state, config);
            }
            SettingsSection::Commands => {
                layout.commands = Some(crate::ui::settings_commands::CommandsLayout::build(
                    layout.viewport,
                    scale,
                    mirrored,
                    &state.commands_editor,
                    config,
                ));
            }
            SettingsSection::Keys => {
                layout.build_keys_layout(scale, state, config);
            }
            SettingsSection::About => {
                layout.build_about_layout(scale);
            }
        }

        layout
    }

    /// Split a control row into its label and widget rects (mirrored in Arabic).
    fn row_rects(&self, kind: ControlKind, rect: Rect, scale: f32) -> (Rect, Rect) {
        let px = |logical: f32| logical * scale;
        let inner_pad = px(10.0);
        let inner = Rect::new(
            rect.x + inner_pad,
            rect.y,
            (rect.w - 2.0 * inner_pad).max(0.0),
            rect.h,
        );
        let widget_w = match kind {
            ControlKind::Toggle(_) => px(44.0),
            ControlKind::Segmented(_) => px(220.0).min(inner.w * 0.56),
            ControlKind::Slider(_) => px(244.0).min(inner.w * 0.58),
            ControlKind::Swatches => px(228.0).min(inner.w * 0.58),
            ControlKind::TextField { .. } => px(220.0).min(inner.w * 0.56),
            ControlKind::ListPicker { .. } => px(220.0).min(inner.w * 0.56),
            ControlKind::ActionButton => px(130.0).min(inner.w * 0.45),
        };
        let gap = px(12.0);
        if self.mirrored {
            let w_rect = Rect::new(inner.x, inner.y, widget_w, inner.h);
            let l_x = w_rect.right() + gap;
            let l_rect = Rect::new(l_x, inner.y, (inner.right() - l_x).max(0.0), inner.h);
            (l_rect, w_rect)
        } else {
            let w_rect = Rect::new(inner.right() - widget_w, inner.y, widget_w, inner.h);
            let l_rect = Rect::new(
                inner.x,
                inner.y,
                (w_rect.x - gap - inner.x).max(0.0),
                inner.h,
            );
            (l_rect, w_rect)
        }
    }

    /// Height of a plain control row (taller when it has a note line).
    fn row_height(id: SettingId, scale: f32) -> f32 {
        let logical = if id.descriptor().note.is_some() {
            48.0
        } else {
            40.0
        };
        (logical * scale).round()
    }

    fn build_control_rows(
        &mut self,
        scale: f32,
        state: &SettingsPanelState,
        _config: &TwittyConfig,
    ) {
        let px = |logical: f32| logical * scale;
        let vp = self.viewport;
        let controls = SettingsPanelState::controls_in(state.section);
        let row_gap = px(6.0).round();

        // First pass: measure logical height for scroll clamping.
        let mut total_h = 0.0_f32;
        for (i, &id) in controls.iter().enumerate() {
            let desc = id.descriptor();
            let row_h = if desc.note.is_some() {
                px(48.0).round()
            } else {
                px(40.0).round()
            };
            total_h += row_h;
            if let Some(exp) = state.expanded.as_ref().filter(|e| e.setting == id) {
                let count = state.filtered_picker_options(id, &exp.query).len();
                let visible = count.clamp(1, MAX_PICKER_ROWS);
                let searchable = matches!(desc.kind, ControlKind::ListPicker { searchable: true });
                let search_h = if searchable { px(34.0) + px(6.0) } else { 0.0 };
                let picker_h = px(12.0) + search_h + visible as f32 * px(28.0).round();
                total_h += px(4.0) + picker_h;
            }
            if i + 1 < controls.len() {
                total_h += row_gap;
            }
        }

        self.max_scroll = ((total_h - vp.h).max(0.0)) / scale;
        let scroll_px = (state.scroll_y.clamp(0.0, self.max_scroll) * scale).round();
        let mut y = vp.y - scroll_px;

        for &id in controls {
            let desc = id.descriptor();
            let row_h = if desc.note.is_some() {
                px(48.0).round()
            } else {
                px(40.0).round()
            };
            let rect = Rect::new(vp.x, y, vp.w, row_h);
            let (label, widget) = self.row_rects(desc.kind, rect, scale);

            let geometry = build_control_geometry(desc.kind, widget, scale, self.mirrored);
            y += row_h;

            let picker = if let Some(exp) = state.expanded.as_ref().filter(|e| e.setting == id) {
                let opts = state.filtered_picker_options(id, &exp.query);
                let visible = opts.len().clamp(1, MAX_PICKER_ROWS);
                let searchable = matches!(desc.kind, ControlKind::ListPicker { searchable: true });
                let pad = px(6.0);
                let search_h = px(30.0);
                let item_h = px(28.0).round();
                let picker_h = pad * 2.0
                    + if searchable { search_h + pad } else { 0.0 }
                    + visible as f32 * item_h;
                let picker_rect = Rect::new(vp.x, y + px(4.0), vp.w, picker_h);
                let mut py = picker_rect.y + pad;
                let search = if searchable {
                    let sr = Rect::new(
                        picker_rect.x + pad,
                        py,
                        (picker_rect.w - 2.0 * pad).max(0.0),
                        search_h,
                    );
                    py += search_h + pad;
                    Some(sr)
                } else {
                    None
                };
                let max_s = opts.len().saturating_sub(MAX_PICKER_ROWS);
                let scroll = exp.scroll.min(max_s);
                let mut items = Vec::with_capacity(visible);
                for (slot, opt_idx) in (scroll..opts.len()).take(MAX_PICKER_ROWS).enumerate() {
                    let ir = Rect::new(
                        picker_rect.x + pad,
                        py + slot as f32 * item_h,
                        (picker_rect.w - 2.0 * pad).max(0.0),
                        item_h,
                    );
                    items.push((opt_idx, ir));
                }
                y = picker_rect.bottom();
                Some(ExpandedPickerLayout {
                    rect: picker_rect,
                    search,
                    items,
                    total_items: opts.len(),
                    scroll,
                })
            } else {
                None
            };

            self.rows.push(ControlRowLayout {
                id,
                rect,
                label,
                widget,
                geometry,
                picker,
            });
            y += row_gap;
        }

        if self.max_scroll > 0.0 && vp.h > 0.0 {
            let total_px = total_h.max(vp.h);
            let thumb_h = (vp.h * (vp.h / total_px)).max(px(24.0)).min(vp.h);
            let frac = (state.scroll_y / self.max_scroll).clamp(0.0, 1.0);
            let thumb_y = vp.y + frac * (vp.h - thumb_h);
            let sb_w = px(4.0).max(2.0);
            let sb_x = if self.mirrored {
                self.content.x + px(4.0)
            } else {
                self.content.right() - px(4.0) - sb_w
            };
            self.scrollbar = Some(Rect::new(sb_x, thumb_y, sb_w, thumb_h));
        }
    }

    fn build_keys_layout(&mut self, scale: f32, state: &SettingsPanelState, config: &TwittyConfig) {
        let px = |logical: f32| logical * scale;
        let vp = self.viewport;
        let search_h = px(34.0).min(vp.h);
        let gap = px(8.0);
        let reset_all_w = px(106.0).min(vp.w * 0.30);
        let search_w = (vp.w - reset_all_w - gap).max(0.0);
        let (search, reset_all) = if self.mirrored {
            let ra = Rect::new(vp.x, vp.y, reset_all_w, search_h);
            let sr = Rect::new(ra.right() + gap, vp.y, search_w, search_h);
            (sr, ra)
        } else {
            let sr = Rect::new(vp.x, vp.y, search_w, search_h);
            let ra = Rect::new(vp.right() - reset_all_w, vp.y, reset_all_w, search_h);
            (sr, ra)
        };
        self.keys_search = Some(search);
        self.keys_reset_all = Some(reset_all);

        let list_y = if state.capturing_key.is_some() || state.key_capture_notice.is_some() {
            let banner_h = px(28.0).min((vp.bottom() - search.bottom()).max(0.0));
            let banner = Rect::new(vp.x, search.bottom() + px(6.0), vp.w, banner_h);
            self.keys_banner = Some(banner);
            banner.bottom() + px(6.0)
        } else {
            search.bottom() + px(10.0)
        };
        let list_h = (vp.bottom() - list_y).max(0.0);
        let row_h = px(40.0).round().max(1.0);
        let visible = (list_h / row_h).floor() as usize;
        self.keys_visible_rows = visible;

        let filtered = state.filtered_commands();
        let max_scroll = filtered.len().saturating_sub(visible);
        let scroll = state.keys_scroll.min(max_scroll);
        let pad = px(8.0);
        let btn_h = px(24.0).min((row_h - px(8.0)).max(1.0));
        let btn_gap = px(6.0);
        for (slot, cmd_idx) in (scroll..filtered.len()).take(visible).enumerate() {
            let rect = Rect::new(vp.x, list_y + slot as f32 * row_h, vp.w, row_h);
            let cmd = filtered[cmd_idx];
            let is_custom = config.keybindings.contains(cmd.name);
            let btn_y = rect.y + (rect.h - btn_h) / 2.0;
            let change_w = px(64.0).min(rect.w * 0.16);
            let reset_w = px(102.0).min(rect.w * 0.22);
            let (change_button, reset_button) = if self.mirrored {
                if is_custom {
                    let reset = Rect::new(rect.x + pad, btn_y, reset_w, btn_h);
                    let change = Rect::new(reset.right() + btn_gap, btn_y, change_w, btn_h);
                    (change, Some(reset))
                } else {
                    let change = Rect::new(rect.x + pad, btn_y, change_w, btn_h);
                    (change, None)
                }
            } else if is_custom {
                let reset = Rect::new(rect.right() - pad - reset_w, btn_y, reset_w, btn_h);
                let change = Rect::new(reset.x - btn_gap - change_w, btn_y, change_w, btn_h);
                (change, Some(reset))
            } else {
                let change = Rect::new(rect.right() - pad - change_w, btn_y, change_w, btn_h);
                (change, None)
            };
            self.keys_rows.push((cmd_idx, rect));
            self.keys_row_layouts.push(KeyRowLayout {
                cmd_idx,
                rect,
                change_button,
                reset_button,
            });
        }
        if max_scroll > 0 && list_h > 0.0 {
            let ratio = visible as f32 / filtered.len() as f32;
            let thumb_h = (list_h * ratio).max(px(24.0)).min(list_h);
            let frac = scroll as f32 / max_scroll as f32;
            let sb_w = px(4.0).max(2.0);
            let sb_x = if self.mirrored {
                self.content.x + px(4.0)
            } else {
                self.content.right() - px(4.0) - sb_w
            };
            self.scrollbar = Some(Rect::new(
                sb_x,
                list_y + frac * (list_h - thumb_h),
                sb_w,
                thumb_h,
            ));
        }
    }

    fn build_about_layout(&mut self, scale: f32) {
        let px = |logical: f32| logical * scale;
        let vp = self.viewport;
        let header_h = px(68.0);
        let about_header = Rect::new(vp.x, vp.y, vp.w, header_h);
        self.about_header = Some(about_header);

        let mut y = about_header.bottom() + px(14.0);
        let row_h = px(42.0);
        let gap = px(8.0);
        for _ in 0..3 {
            self.about_info_rows.push(Rect::new(vp.x, y, vp.w, row_h));
            y += row_h + gap;
        }
        if let Some(&config_row) = self.about_info_rows.get(2) {
            let btn_w = px(118.0).min(config_row.w * 0.4);
            let btn_h = px(28.0).min(config_row.h - px(6.0));
            let btn_y = config_row.y + (config_row.h - btn_h) / 2.0;
            let pad = px(10.0);
            let btn_x = if self.mirrored {
                config_row.x + pad
            } else {
                config_row.right() - pad - btn_w
            };
            self.copy_button = Some(Rect::new(btn_x, btn_y, btn_w, btn_h));
        }
    }

    fn build_ai_layout(&mut self, scale: f32, state: &SettingsPanelState, config: &TwittyConfig) {
        use crate::ui::settings_ai::{
            AiApiRowLayout, AiSectionLayout, AiTextField, API_PROVIDERS, DEFAULT_PROVIDER_CHOICES,
            HARD_DENY_SUMMARY,
        };
        let px = |l: f32| l * scale;
        let vp = self.viewport;
        let gap = px(8.0).round();
        let allow_count = config
            .ai_project_allow_list
            .get(state.ai.project_key())
            .map_or(0, Vec::len);
        let key_open_extra = if matches!(state.ai.active_field, Some(AiTextField::ApiKey(_))) {
            px(34.0)
        } else {
            0.0
        };
        let notice_h = if state.ai.notice.is_some() {
            px(30.0) + gap
        } else {
            0.0
        };
        let rows_h: f32 = SettingId::in_section(SettingsSection::Ai)
            .iter()
            .map(|&id| Self::row_height(id, scale) + gap)
            .sum();
        let deny_h = px(26.0) + HARD_DENY_SUMMARY.len() as f32 * px(18.0) + px(12.0);
        let total_h = notice_h
            + rows_h
            + px(56.0) // default provider
            + gap
            + px(44.0) // cli agents
            + gap
            + API_PROVIDERS.len() as f32 * (px(36.0) + px(4.0))
            + key_open_extra
            + gap
            + px(38.0) // ollama url
            + gap
            + px(38.0) // cost cap
            + gap
            + px(68.0) // allow list header + add input
            + allow_count as f32 * px(28.0)
            + gap
            + deny_h
            + gap
            + px(36.0); // clear history

        self.max_scroll = ((total_h - vp.h).max(0.0)) / scale;
        let scroll_px = (state.scroll_y.clamp(0.0, self.max_scroll) * scale).round();
        let mut y = vp.y - scroll_px;

        let mut ai = AiSectionLayout::default();
        if state.ai.notice.is_some() {
            ai.notice_rect = Some(Rect::new(vp.x, y, vp.w, px(30.0)));
            y += px(30.0) + gap;
        }

        // Plain rows first: Project memory (n21), built-in AI switch, request timeout.
        for &id in SettingId::in_section(SettingsSection::Ai) {
            let row_h = Self::row_height(id, scale);
            let rect = Rect::new(vp.x, y, vp.w, row_h);
            let kind = id.descriptor().kind;
            let (label, widget) = self.row_rects(kind, rect, scale);
            let geometry = build_control_geometry(kind, widget, scale, self.mirrored);
            self.rows.push(ControlRowLayout {
                id,
                rect,
                label,
                widget,
                geometry,
                picker: None,
            });
            y += row_h + gap;
        }

        // Default provider buttons.
        y += px(22.0);
        let n_prov = DEFAULT_PROVIDER_CHOICES.len() as f32;
        let p_gap = px(4.0);
        let p_w = ((vp.w - p_gap * (n_prov - 1.0)) / n_prov).max(px(36.0));
        for i in 0..DEFAULT_PROVIDER_CHOICES.len() {
            let vis = if self.mirrored {
                DEFAULT_PROVIDER_CHOICES.len() - 1 - i
            } else {
                i
            };
            ai.default_provider_buttons.push((
                i,
                Rect::new(vp.x + vis as f32 * (p_w + p_gap), y, p_w, px(26.0)),
            ));
        }
        y += px(34.0) + gap;

        // CLI agents box.
        ai.cli_section = Rect::new(vp.x, y, vp.w, px(44.0));
        y += px(44.0) + gap;

        // API providers with Set key… / Clear + inline masked input.
        for (i, _) in API_PROVIDERS.iter().enumerate() {
            let row = Rect::new(vp.x, y, vp.w, px(36.0));
            let btn_w = px(88.0).min(vp.w * 0.28);
            let clear_w = px(56.0);
            let by = row.y + (row.h - px(24.0)) * 0.5;
            let (set_btn, clear_btn) = if self.mirrored {
                let set = Rect::new(row.x + px(8.0), by, btn_w, px(24.0));
                let clr = state.ai.saved_keys[i]
                    .then(|| Rect::new(set.right() + px(6.0), by, clear_w, px(24.0)));
                (set, clr)
            } else {
                let set = Rect::new(row.right() - px(8.0) - btn_w, by, btn_w, px(24.0));
                let clr = state.ai.saved_keys[i]
                    .then(|| Rect::new(set.x - px(6.0) - clear_w, by, clear_w, px(24.0)));
                (set, clr)
            };
            y += px(36.0) + px(4.0);

            let (key_input, key_save_button) =
                if state.ai.active_field == Some(AiTextField::ApiKey(i)) {
                    let save_w = px(66.0);
                    let in_r = if self.mirrored {
                        let sv = Rect::new(vp.x + px(8.0), y, save_w, px(26.0));
                        let inp = Rect::new(
                            sv.right() + px(6.0),
                            y,
                            (vp.right() - px(8.0) - (sv.right() + px(6.0))).max(0.0),
                            px(26.0),
                        );
                        (Some(inp), Some(sv))
                    } else {
                        let sv = Rect::new(vp.right() - px(8.0) - save_w, y, save_w, px(26.0));
                        let inp = Rect::new(
                            vp.x + px(8.0),
                            y,
                            (sv.x - px(6.0) - (vp.x + px(8.0))).max(0.0),
                            px(26.0),
                        );
                        (Some(inp), Some(sv))
                    };
                    y += px(34.0);
                    in_r
                } else {
                    (None, None)
                };

            ai.api_rows.push(AiApiRowLayout {
                index: i,
                row,
                set_key_button: set_btn,
                clear_key_button: clear_btn,
                key_input,
                key_save_button,
            });
        }
        y += gap;

        // Ollama URL row.
        let field_w = px(240.0).min(vp.w * 0.54);
        ai.ollama_input = if self.mirrored {
            Rect::new(vp.x, y + px(5.0), field_w, px(28.0))
        } else {
            Rect::new(vp.right() - field_w, y + px(5.0), field_w, px(28.0))
        };
        y += px(38.0) + gap;

        // Monthly cost cap row.
        ai.cost_cap_input = if self.mirrored {
            Rect::new(vp.x, y + px(5.0), px(140.0).min(vp.w * 0.4), px(28.0))
        } else {
            let w = px(140.0).min(vp.w * 0.4);
            Rect::new(vp.right() - w, y + px(5.0), w, px(28.0))
        };
        y += px(38.0) + gap;

        // Per-project allow-list editor.
        y += px(24.0);
        let add_w = px(64.0);
        if self.mirrored {
            ai.allow_add_button = Rect::new(vp.x, y, add_w, px(28.0));
            ai.allow_input = Rect::new(
                ai.allow_add_button.right() + px(6.0),
                y,
                (vp.right() - (ai.allow_add_button.right() + px(6.0))).max(0.0),
                px(28.0),
            );
        } else {
            ai.allow_add_button = Rect::new(vp.right() - add_w, y, add_w, px(28.0));
            ai.allow_input = Rect::new(
                vp.x,
                y,
                (ai.allow_add_button.x - px(6.0) - vp.x).max(0.0),
                px(28.0),
            );
        }
        y += px(34.0);
        for idx in 0..allow_count {
            let row = Rect::new(vp.x, y, vp.w, px(24.0));
            let rm_w = px(24.0);
            let rm = if self.mirrored {
                Rect::new(row.x + px(4.0), row.y, rm_w, row.h)
            } else {
                Rect::new(row.right() - px(4.0) - rm_w, row.y, rm_w, row.h)
            };
            ai.allow_entries.push((idx, row, rm));
            y += px(28.0);
        }
        y += gap;

        // Read-only deny-list view.
        ai.deny_box = Rect::new(vp.x, y, vp.w, deny_h);
        y += deny_h + gap;

        // Clear agent history button.
        let clr_w = px(156.0).min(vp.w);
        ai.clear_history_button = if self.mirrored {
            Rect::new(vp.right() - clr_w, y, clr_w, px(28.0))
        } else {
            Rect::new(vp.x, y, clr_w, px(28.0))
        };

        if self.max_scroll > 0.0 && vp.h > 0.0 {
            let thumb_h = (vp.h * (vp.h / total_h)).clamp(px(24.0), vp.h);
            let ratio =
                (state.scroll_y.clamp(0.0, self.max_scroll) / self.max_scroll).clamp(0.0, 1.0);
            let thumb_y = vp.y + ratio * (vp.h - thumb_h);
            let track_x = if self.mirrored {
                self.content.x + px(4.0)
            } else {
                self.content.right() - px(10.0)
            };
            self.scrollbar = Some(Rect::new(track_x, thumb_y, px(4.0), thumb_h));
        }

        self.ai = Some(ai);
    }

    /// Hit-test physical pixel `(x, y)` against the settings overlay.
    pub fn hit(&self, x: f64, y: f64) -> SettingsHit {
        if !self.panel.contains(x, y) {
            return SettingsHit::Outside;
        }
        if self.close_button.contains(x, y) {
            return SettingsHit::CloseButton;
        }
        for &(section, rect) in &self.sections {
            if rect.contains(x, y) {
                return SettingsHit::Section(section);
            }
        }
        if let Some(reset_all) = self.keys_reset_all {
            if reset_all.contains(x, y) {
                return SettingsHit::KeysResetAll;
            }
        }
        if let Some(search) = self.keys_search {
            if search.contains(x, y) {
                return SettingsHit::KeysSearch;
            }
        }
        for row in &self.keys_row_layouts {
            if row.change_button.contains(x, y) {
                return SettingsHit::KeyChange(row.cmd_idx);
            }
            if row.reset_button.is_some_and(|r| r.contains(x, y)) {
                return SettingsHit::KeyReset(row.cmd_idx);
            }
            if row.rect.contains(x, y) {
                return SettingsHit::KeyRow(row.cmd_idx);
            }
        }
        if let Some(copy_btn) = self.copy_button {
            if copy_btn.contains(x, y) {
                return SettingsHit::CopyConfigPath;
            }
        }
        if let Some(list) = self
            .commands
            .as_ref()
            .filter(|_| self.viewport.contains(x, y))
        {
            if list.add_button.is_some_and(|r| r.contains(x, y)) {
                return SettingsHit::CommandsAdd;
            }
            for row in &list.rows {
                if row.edit.contains(x, y) {
                    return SettingsHit::CommandEdit(row.index);
                }
                if row.delete.contains(x, y) {
                    return SettingsHit::CommandDelete(row.index);
                }
                if row.rect.contains(x, y) {
                    return SettingsHit::CommandRow(row.index);
                }
            }
            for &(mode, rect) in &list.mode_buttons {
                if rect.contains(x, y) {
                    return SettingsHit::CommandModeButton(mode);
                }
            }
            for &(field, _, input) in &list.fields {
                if field != crate::ui::settings_commands::FormField::Mode && input.contains(x, y) {
                    return SettingsHit::CommandField(field);
                }
            }
            if list.save.is_some_and(|r| r.contains(x, y)) {
                return SettingsHit::CommandSave;
            }
            if list.cancel.is_some_and(|r| r.contains(x, y)) {
                return SettingsHit::CommandCancel;
            }
        }
        if let Some(ai) = &self.ai {
            if self.viewport.contains(x, y) {
                for &(idx, r) in &ai.default_provider_buttons {
                    if r.contains(x, y) {
                        return SettingsHit::AiDefaultProvider(idx);
                    }
                }
                for row in &ai.api_rows {
                    if row.set_key_button.contains(x, y) {
                        return SettingsHit::AiSetKey(row.index);
                    }
                    if row.clear_key_button.is_some_and(|r| r.contains(x, y)) {
                        return SettingsHit::AiClearKey(row.index);
                    }
                    if row.key_save_button.is_some_and(|r| r.contains(x, y)) {
                        return SettingsHit::AiSaveKey(row.index);
                    }
                    if row.key_input.is_some_and(|r| r.contains(x, y)) {
                        return SettingsHit::AiKeyInput(row.index);
                    }
                }
                if ai.ollama_input.contains(x, y) {
                    return SettingsHit::AiOllamaInput;
                }
                if ai.cost_cap_input.contains(x, y) {
                    return SettingsHit::AiCostCapInput;
                }
                if ai.allow_add_button.contains(x, y) {
                    return SettingsHit::AiAllowAdd;
                }
                if ai.allow_input.contains(x, y) {
                    return SettingsHit::AiAllowInput;
                }
                for &(idx, _, rm) in &ai.allow_entries {
                    if rm.contains(x, y) {
                        return SettingsHit::AiAllowRemove(idx);
                    }
                }
                if ai.clear_history_button.contains(x, y) {
                    return SettingsHit::AiClearHistory;
                }
            }
        }
        if self.viewport.contains(x, y) {
            for row in &self.rows {
                if let Some(picker) = &row.picker {
                    if picker.rect.contains(x, y) {
                        if let Some(search) = picker.search {
                            if search.contains(x, y) {
                                return SettingsHit::PickerSearch(row.id);
                            }
                        }
                        for &(idx, rect) in &picker.items {
                            if rect.contains(x, y) {
                                return SettingsHit::PickerItem(row.id, idx);
                            }
                        }
                        return SettingsHit::Inside;
                    }
                }
                if row.rect.contains(x, y) {
                    match &row.geometry {
                        ControlGeometry::Toggle { .. } => {
                            return SettingsHit::Toggle(row.id);
                        }
                        ControlGeometry::Segmented { segments, .. } => {
                            for (i, seg) in segments.iter().enumerate() {
                                if seg.contains(x, y) {
                                    return SettingsHit::Segment(row.id, i);
                                }
                            }
                        }
                        ControlGeometry::Slider {
                            minus, track, plus, ..
                        } => {
                            if minus.contains(x, y) {
                                return SettingsHit::SliderMinus(row.id);
                            }
                            if plus.contains(x, y) {
                                return SettingsHit::SliderPlus(row.id);
                            }
                            if track.contains(x, y) {
                                let ratio = if track.w > 0.0 {
                                    ((x as f32 - track.x) / track.w).clamp(0.0, 1.0)
                                } else {
                                    0.0
                                };
                                return SettingsHit::SliderTrack(row.id, ratio);
                            }
                        }
                        ControlGeometry::Swatches { swatches } => {
                            for (i, sw) in swatches.iter().enumerate() {
                                if sw.contains(x, y) {
                                    return SettingsHit::Swatch(i);
                                }
                            }
                        }
                        ControlGeometry::TextField { input } => {
                            if input.contains(x, y) || row.rect.contains(x, y) {
                                return SettingsHit::TextField(row.id);
                            }
                        }
                        ControlGeometry::ListPicker { .. } => {
                            return SettingsHit::PickerToggle(row.id);
                        }
                        ControlGeometry::ActionButton { button } => {
                            if button.contains(x, y) {
                                return match row.id {
                                    SettingId::ProjectMemory => SettingsHit::OpenMemory,
                                    _ => SettingsHit::CopyConfigPath,
                                };
                            }
                        }
                    }
                }
            }
        }
        SettingsHit::Inside
    }
}

fn build_control_geometry(
    kind: ControlKind,
    widget: Rect,
    scale: f32,
    mirrored: bool,
) -> ControlGeometry {
    let px = |logical: f32| logical * scale;
    match kind {
        ControlKind::Toggle(_) => {
            let w = px(38.0).min(widget.w);
            let h = px(20.0).min(widget.h);
            let x = if mirrored {
                widget.x
            } else {
                widget.right() - w
            };
            let y = widget.y + (widget.h - h) / 2.0;
            ControlGeometry::Toggle {
                track: Rect::new(x, y, w, h),
            }
        }
        ControlKind::Segmented(options) => {
            let h = px(26.0).min(widget.h - px(6.0)).max(1.0);
            let y = widget.y + (widget.h - h) / 2.0;
            let container = Rect::new(widget.x, y, widget.w, h);
            let pad = px(2.0);
            let inner_w = (container.w - 2.0 * pad).max(0.0);
            let count = options.len().max(1);
            let seg_w = inner_w / count as f32;
            let segments = (0..count)
                .map(|i| {
                    // Option 0 starts at the reading start (left in LTR, right in RTL).
                    let visual_idx = if mirrored { count - 1 - i } else { i };
                    Rect::new(
                        container.x + pad + visual_idx as f32 * seg_w,
                        container.y + pad,
                        seg_w,
                        (container.h - 2.0 * pad).max(0.0),
                    )
                })
                .collect();
            ControlGeometry::Segmented {
                container,
                segments,
            }
        }
        ControlKind::Slider(_) => {
            let btn = px(24.0).min(widget.h - px(6.0));
            let val_w = px(54.0).min(widget.w * 0.3);
            let gap = px(6.0);
            let btn_y = widget.y + (widget.h - btn) / 2.0;
            let track_h = px(20.0).min(widget.h);
            let track_y = widget.y + (widget.h - track_h) / 2.0;
            if mirrored {
                let value = Rect::new(widget.right() - val_w, widget.y, val_w, widget.h);
                let minus = Rect::new(widget.x, btn_y, btn, btn);
                let plus_x = (value.x - gap - btn).max(minus.right() + gap);
                let plus = Rect::new(plus_x, btn_y, btn, btn);
                let track_x = minus.right() + gap;
                let track = Rect::new(track_x, track_y, (plus.x - gap - track_x).max(0.0), track_h);
                ControlGeometry::Slider {
                    minus,
                    track,
                    plus,
                    value,
                }
            } else {
                let value = Rect::new(widget.x, widget.y, val_w, widget.h);
                let minus = Rect::new(value.right() + gap, btn_y, btn, btn);
                let plus = Rect::new(widget.right() - btn, btn_y, btn, btn);
                let track_x = minus.right() + gap;
                let track = Rect::new(track_x, track_y, (plus.x - gap - track_x).max(0.0), track_h);
                ControlGeometry::Slider {
                    minus,
                    track,
                    plus,
                    value,
                }
            }
        }
        ControlKind::Swatches => {
            let count = ACCENT_SWATCHES.len();
            let gap = px(6.0);
            let side = px(22.0)
                .min((widget.w - gap * (count.saturating_sub(1)) as f32) / count as f32)
                .max(8.0);
            let total_w = count as f32 * side + (count.saturating_sub(1)) as f32 * gap;
            let y = widget.y + (widget.h - side) / 2.0;
            let start_x = if mirrored {
                widget.x
            } else {
                widget.right() - total_w
            };
            let swatches = (0..count)
                .map(|i| {
                    let visual_idx = if mirrored { count - 1 - i } else { i };
                    Rect::new(start_x + visual_idx as f32 * (side + gap), y, side, side)
                })
                .collect();
            ControlGeometry::Swatches { swatches }
        }
        ControlKind::TextField { .. } => {
            let h = px(28.0).min(widget.h - px(6.0)).max(1.0);
            let y = widget.y + (widget.h - h) / 2.0;
            ControlGeometry::TextField {
                input: Rect::new(widget.x, y, widget.w, h),
            }
        }
        ControlKind::ListPicker { .. } => {
            let h = px(28.0).min(widget.h - px(6.0)).max(1.0);
            let y = widget.y + (widget.h - h) / 2.0;
            ControlGeometry::ListPicker {
                button: Rect::new(widget.x, y, widget.w, h),
            }
        }
        ControlKind::ActionButton => {
            let h = px(28.0).min(widget.h - px(6.0)).max(1.0);
            let y = widget.y + (widget.h - h) / 2.0;
            ControlGeometry::ActionButton {
                button: Rect::new(widget.x, y, widget.w, h),
            }
        }
    }
}
