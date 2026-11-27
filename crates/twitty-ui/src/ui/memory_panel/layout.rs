//! Physical-pixel layout geometry and hit testing for the Project Memory screen.

use twitty_store::{MemoryEntry, MemoryKind, RulesFileInfo, RulesFileState};

use super::{MemoryHit, MemoryInputTarget, MemoryPanelState, MemoryTab};
use crate::layout::Rect;
use crate::ui::chrome::UiLanguage;

// ---- Physical-pixel geometry shared by hit testing and rendering ----

#[derive(Clone, Debug)]
pub struct RulesFileRowLayout {
    pub index: usize,
    pub rect: Rect,
    pub title_rect: Rect,
    pub hash_rect: Rect,
    pub badge_rect: Rect,
    pub approve_btn: Option<Rect>,
    pub revoke_btn: Option<Rect>,
}

#[derive(Clone, Debug)]
pub struct MemoryEntryRowLayout {
    pub id: i64,
    pub list_index: usize,
    pub rect: Rect,
    pub text_rect: Rect,
    pub meta_rect: Rect,
    pub badge_rect: Option<Rect>,
    pub approve_btn: Option<Rect>,
    pub edit_btn: Option<Rect>,
    pub delete_btn: Option<Rect>,
    pub edit_input: Option<Rect>,
    pub save_btn: Option<Rect>,
    pub cancel_btn: Option<Rect>,
}

#[derive(Clone, Debug)]
pub struct ExclusionRowLayout {
    pub index: usize,
    pub rect: Rect,
    pub text_rect: Rect,
    pub remove_btn: Rect,
}

#[derive(Clone, Debug)]
pub struct MemoryLayout {
    pub mirrored: bool,
    pub panel: Rect,
    pub header: Rect,
    pub header_title: Rect,
    pub header_subtitle: Rect,
    pub excluded_badge: Option<Rect>,
    pub forget_button: Option<Rect>,
    pub close_button: Rect,
    pub sidebar: Rect,
    pub divider: Rect,
    pub tabs: Vec<(MemoryTab, Rect)>,
    pub viewport: Rect,
    pub add_input: Option<Rect>,
    pub add_submit: Option<Rect>,
    pub refresh_button: Option<Rect>,
    pub exclude_current_btn: Option<Rect>,
    pub excluded_banner: Option<Rect>,
    pub rules_heading: Option<Rect>,
    pub rules_rows: Vec<RulesFileRowLayout>,
    pub user_rules_heading: Option<Rect>,
    pub entry_rows: Vec<MemoryEntryRowLayout>,
    pub exclusion_rows: Vec<ExclusionRowLayout>,
    pub max_scroll: f32,
    pub scrollbar: Option<Rect>,
}

impl MemoryLayout {
    pub fn build(surface: (f32, f32), scale: f32, state: &MemoryPanelState) -> Self {
        let px = |v: f32| (v * scale).round();
        let mirrored = state.language == UiLanguage::Arabic;
        let (sw, sh) = surface;

        let margin = px(24.0);
        let max_w = (sw - 2.0 * margin).max(0.0);
        let max_h = (sh - 2.0 * margin).max(0.0);
        let panel_w = max_w.min(px(840.0)).max(max_w.min(px(360.0)));
        let panel_h = max_h.min(px(600.0)).max(max_h.min(px(300.0)));
        let panel_x = ((sw - panel_w) / 2.0).round().max(0.0);
        let panel_y = ((sh - panel_h) / 2.0).round().max(0.0);
        let panel = Rect::new(panel_x, panel_y, panel_w, panel_h);

        let header_h = px(56.0).min(panel_h);
        let header = Rect::new(panel.x, panel.y, panel.w, header_h);
        let pad_x = px(16.0);
        let close_side = px(28.0);
        let btn_h = px(28.0);
        let close_y = header.y + ((header.h - close_side) / 2.0).round();
        let forget_w = if state.project_root.is_some() {
            px(148.0)
        } else {
            0.0
        };
        let gap = px(8.0);

        let (close_button, forget_button, title_x, title_w) = if mirrored {
            let cb = Rect::new(header.x + pad_x, close_y, close_side, close_side);
            let fb =
                (forget_w > 0.0).then(|| Rect::new(cb.right() + gap, close_y, forget_w, btn_h));
            let left_bound = fb.map_or(cb.right(), |r| r.right()) + px(12.0);
            let w = (header.right() - pad_x - left_bound).max(0.0);
            (cb, fb, left_bound, w)
        } else {
            let cb = Rect::new(
                header.right() - pad_x - close_side,
                close_y,
                close_side,
                close_side,
            );
            let fb = (forget_w > 0.0)
                .then(|| Rect::new(cb.x - gap - forget_w, close_y, forget_w, btn_h));
            let right_bound = fb.map_or(cb.x, |r| r.x) - px(12.0);
            let tx = header.x + pad_x;
            let w = (right_bound - tx).max(0.0);
            (cb, fb, tx, w)
        };

        let header_title = Rect::new(title_x, header.y + px(8.0), title_w, px(22.0));
        let badge_w = px(74.0);
        let (header_subtitle, excluded_badge) = if state.excluded && title_w > badge_w + px(40.0) {
            let sy = header_title.bottom() + px(2.0);
            let sh_row = px(18.0);
            if mirrored {
                let badge = Rect::new(title_x, sy, badge_w, sh_row);
                let sub = Rect::new(
                    badge.right() + px(8.0),
                    sy,
                    (title_w - badge_w - px(8.0)).max(0.0),
                    sh_row,
                );
                (sub, Some(badge))
            } else {
                let badge = Rect::new(title_x + title_w - badge_w, sy, badge_w, sh_row);
                let sub = Rect::new(title_x, sy, (title_w - badge_w - px(8.0)).max(0.0), sh_row);
                (sub, Some(badge))
            }
        } else {
            (
                Rect::new(title_x, header_title.bottom() + px(2.0), title_w, px(18.0)),
                None,
            )
        };

        let body_y = header.bottom();
        let body_h = (panel.bottom() - body_y).max(0.0);
        let sidebar_w = px(184.0).min((panel.w * 0.30).round());
        let div_w = px(1.0).max(1.0);

        let (sidebar, divider, content_area) = if mirrored {
            let sb = Rect::new(panel.right() - sidebar_w, body_y, sidebar_w, body_h);
            let dv = Rect::new(sb.x - div_w, body_y, div_w, body_h);
            let ca = Rect::new(panel.x, body_y, (dv.x - panel.x).max(0.0), body_h);
            (sb, dv, ca)
        } else {
            let sb = Rect::new(panel.x, body_y, sidebar_w, body_h);
            let dv = Rect::new(sb.right(), body_y, div_w, body_h);
            let ca = Rect::new(
                dv.right(),
                body_y,
                (panel.right() - dv.right()).max(0.0),
                body_h,
            );
            (sb, dv, ca)
        };

        // Sidebar tabs.
        let mut tabs = Vec::with_capacity(MemoryTab::ALL.len());
        let tab_pad_x = px(10.0);
        let tab_h = px(36.0);
        let tab_gap = px(4.0);
        let mut ty = sidebar.y + px(12.0);
        for tab in MemoryTab::ALL {
            let r = Rect::new(
                sidebar.x + tab_pad_x,
                ty,
                (sidebar.w - 2.0 * tab_pad_x).max(0.0),
                tab_h,
            );
            tabs.push((tab, r));
            ty += tab_h + tab_gap;
        }

        // Content top bar (Add input / Refresh / Excluded banner) + scrollable viewport.
        let inner_pad_x = px(16.0);
        let inner_pad_y = px(12.0);
        let scrollbar_reserve = px(10.0);
        let content_x = content_area.x + inner_pad_x;
        let content_w = (content_area.w - 2.0 * inner_pad_x - scrollbar_reserve).max(0.0);
        let mut top_cursor = content_area.y + inner_pad_y;

        let mut add_input = None;
        let mut add_submit = None;
        let mut refresh_button = None;
        let mut exclude_current_btn = None;
        let mut excluded_banner = None;

        if state.tab == MemoryTab::Exclusions {
            let bar_h = px(34.0);
            let add_w = px(78.0);
            let (inp, btn) = split_bar(
                content_x, top_cursor, content_w, bar_h, add_w, gap, mirrored,
            );
            add_input = Some(inp);
            add_submit = Some(btn);
            top_cursor += bar_h + px(8.0);

            if state.project_root.is_some() && !state.excluded {
                let ex_h = px(28.0);
                let ex_w = px(176.0).min(content_w);
                let ex_x = if mirrored {
                    content_x + content_w - ex_w
                } else {
                    content_x
                };
                exclude_current_btn = Some(Rect::new(ex_x, top_cursor, ex_w, ex_h));
                top_cursor += ex_h + px(10.0);
            }
        } else if state.excluded {
            let banner_h = px(40.0);
            excluded_banner = Some(Rect::new(content_x, top_cursor, content_w, banner_h));
            top_cursor += banner_h + px(10.0);
        } else if state.project_root.is_some() {
            let bar_h = px(34.0);
            let add_w = px(78.0);
            let ref_w = if state.tab == MemoryTab::Env {
                px(86.0)
            } else {
                0.0
            };
            if ref_w > 0.0 {
                let total_btns = add_w + gap + ref_w;
                let (inp, btns_rect) = split_bar(
                    content_x, top_cursor, content_w, bar_h, total_btns, gap, mirrored,
                );
                let (a_btn, r_btn) = if mirrored {
                    let r = Rect::new(btns_rect.x, top_cursor, ref_w, bar_h);
                    let a = Rect::new(r.right() + gap, top_cursor, add_w, bar_h);
                    (a, r)
                } else {
                    let a = Rect::new(btns_rect.x, top_cursor, add_w, bar_h);
                    let r = Rect::new(a.right() + gap, top_cursor, ref_w, bar_h);
                    (a, r)
                };
                add_input = Some(inp);
                add_submit = Some(a_btn);
                refresh_button = Some(r_btn);
            } else {
                let (inp, btn) = split_bar(
                    content_x, top_cursor, content_w, bar_h, add_w, gap, mirrored,
                );
                add_input = Some(inp);
                add_submit = Some(btn);
            }
            top_cursor += bar_h + px(10.0);
        }

        let viewport = Rect::new(
            content_x,
            top_cursor,
            content_w,
            (content_area.bottom() - inner_pad_y - top_cursor).max(0.0),
        );

        let scroll_px = (state.scroll_y * scale).round();
        let mut cursor_y = viewport.y - scroll_px;
        let row_gap = px(8.0);
        let heading_h = px(20.0);

        let mut rules_heading = None;
        let mut rules_rows = Vec::new();
        let mut user_rules_heading = None;
        let mut entry_rows = Vec::new();
        let mut exclusion_rows = Vec::new();

        match state.tab {
            MemoryTab::Rules => {
                rules_heading = Some(Rect::new(viewport.x, cursor_y, viewport.w, heading_h));
                cursor_y += heading_h + px(6.0);

                for (idx, file) in state.rules_files.iter().enumerate() {
                    let h = px(56.0);
                    let rect = Rect::new(viewport.x, cursor_y, viewport.w, h);
                    rules_rows.push(layout_rules_file_row(idx, rect, file, scale, mirrored));
                    cursor_y += h + row_gap;
                }
                if state.rules_files.is_empty() {
                    cursor_y += px(24.0) + row_gap;
                }

                cursor_y += px(4.0);
                user_rules_heading = Some(Rect::new(viewport.x, cursor_y, viewport.w, heading_h));
                cursor_y += heading_h + px(6.0);

                let offset = state.rules_files.len();
                for (i, entry) in state.entries_for_tab(MemoryTab::Rules).iter().enumerate() {
                    let editing =
                        state.input_target == Some(MemoryInputTarget::EditEntry(entry.id));
                    let h = px(if editing { 64.0 } else { 58.0 });
                    let rect = Rect::new(viewport.x, cursor_y, viewport.w, h);
                    entry_rows.push(layout_entry_row(
                        entry,
                        offset + i,
                        rect,
                        editing,
                        false,
                        scale,
                        mirrored,
                    ));
                    cursor_y += h + row_gap;
                }
            }
            MemoryTab::Facts | MemoryTab::Commands | MemoryTab::Env => {
                for (i, entry) in state.entries_for_tab(state.tab).iter().enumerate() {
                    let editing =
                        state.input_target == Some(MemoryInputTarget::EditEntry(entry.id));
                    let show_approve = state.tab == MemoryTab::Facts && !entry.approved;
                    let h = px(if editing { 64.0 } else { 58.0 });
                    let rect = Rect::new(viewport.x, cursor_y, viewport.w, h);
                    entry_rows.push(layout_entry_row(
                        entry,
                        i,
                        rect,
                        editing,
                        show_approve,
                        scale,
                        mirrored,
                    ));
                    cursor_y += h + row_gap;
                }
            }
            MemoryTab::Exclusions => {
                for (index, _) in state.exclusions.iter().enumerate() {
                    let h = px(44.0);
                    let rect = Rect::new(viewport.x, cursor_y, viewport.w, h);
                    let pad = px(12.0);
                    let rm_w = px(74.0);
                    let rm_h = px(26.0);
                    let rm_y = rect.y + ((rect.h - rm_h) / 2.0).round();
                    let (text_rect, remove_btn) = if mirrored {
                        let rb = Rect::new(rect.x + pad, rm_y, rm_w, rm_h);
                        let tr = Rect::new(
                            rb.right() + px(10.0),
                            rect.y,
                            (rect.right() - pad - (rb.right() + px(10.0))).max(0.0),
                            rect.h,
                        );
                        (tr, rb)
                    } else {
                        let rb = Rect::new(rect.right() - pad - rm_w, rm_y, rm_w, rm_h);
                        let tr = Rect::new(
                            rect.x + pad,
                            rect.y,
                            (rb.x - px(10.0) - (rect.x + pad)).max(0.0),
                            rect.h,
                        );
                        (tr, rb)
                    };
                    exclusion_rows.push(ExclusionRowLayout {
                        index,
                        rect,
                        text_rect,
                        remove_btn,
                    });
                    cursor_y += h + row_gap;
                }
            }
        }

        let content_total_h = (cursor_y + scroll_px - viewport.y).max(0.0);
        let max_scroll_px = (content_total_h - viewport.h).max(0.0);
        let max_scroll = if scale > 0.0 {
            max_scroll_px / scale
        } else {
            0.0
        };

        let scrollbar = (max_scroll_px > 1.0 && viewport.h > px(24.0)).then(|| {
            let sb_w = px(4.0).max(2.0);
            let sb_x = content_area.right() - px(8.0) - sb_w;
            let thumb_h = ((viewport.h / content_total_h) * viewport.h)
                .max(px(24.0))
                .min(viewport.h);
            let ratio = (scroll_px / max_scroll_px).clamp(0.0, 1.0);
            let thumb_y = viewport.y + ratio * (viewport.h - thumb_h);
            Rect::new(sb_x, thumb_y, sb_w, thumb_h)
        });

        Self {
            mirrored,
            panel,
            header,
            header_title,
            header_subtitle,
            excluded_badge,
            forget_button,
            close_button,
            sidebar,
            divider,
            tabs,
            viewport,
            add_input,
            add_submit,
            refresh_button,
            exclude_current_btn,
            excluded_banner,
            rules_heading,
            rules_rows,
            user_rules_heading,
            entry_rows,
            exclusion_rows,
            max_scroll,
            scrollbar,
        }
    }

    pub fn selected_rect(&self, selected: usize) -> Option<Rect> {
        if let Some(row) = self.rules_rows.iter().find(|r| r.index == selected) {
            return Some(row.rect);
        }
        if let Some(row) = self.entry_rows.iter().find(|r| r.list_index == selected) {
            return Some(row.rect);
        }
        self.exclusion_rows
            .iter()
            .find(|r| r.index == selected)
            .map(|r| r.rect)
    }

    /// Hit-test physical pixel `(x, y)` against all interactive elements.
    pub fn hit(&self, x: f64, y: f64) -> MemoryHit {
        if !self.panel.contains(x, y) {
            return MemoryHit::Outside;
        }
        if self.close_button.contains(x, y) {
            return MemoryHit::CloseButton;
        }
        if let Some(fb) = self.forget_button {
            if fb.contains(x, y) {
                return MemoryHit::ForgetProject;
            }
        }
        for &(tab, rect) in &self.tabs {
            if rect.contains(x, y) {
                return MemoryHit::Tab(tab);
            }
        }
        if let Some(inp) = self.add_input {
            if inp.contains(x, y) {
                return MemoryHit::AddInput;
            }
        }
        if let Some(btn) = self.add_submit {
            if btn.contains(x, y) {
                return MemoryHit::AddSubmit;
            }
        }
        if let Some(btn) = self.refresh_button {
            if btn.contains(x, y) {
                return MemoryHit::RefreshEnv;
            }
        }
        if let Some(btn) = self.exclude_current_btn {
            if btn.contains(x, y) {
                return MemoryHit::ExcludeCurrentProject;
            }
        }

        if self.viewport.contains(x, y) {
            for row in &self.rules_rows {
                if !row.rect.contains(x, y) {
                    continue;
                }
                if let Some(btn) = row.approve_btn {
                    if btn.contains(x, y) {
                        return MemoryHit::RulesApprove(row.index);
                    }
                }
                if let Some(btn) = row.revoke_btn {
                    if btn.contains(x, y) {
                        return MemoryHit::RulesRevoke(row.index);
                    }
                }
            }
            for row in &self.entry_rows {
                if !row.rect.contains(x, y) {
                    continue;
                }
                if let Some(btn) = row.save_btn {
                    if btn.contains(x, y) {
                        return MemoryHit::EditSave(row.id);
                    }
                }
                if let Some(btn) = row.cancel_btn {
                    if btn.contains(x, y) {
                        return MemoryHit::EditCancel;
                    }
                }
                if let Some(inp) = row.edit_input {
                    if inp.contains(x, y) {
                        return MemoryHit::EditInput(row.id);
                    }
                }
                if let Some(btn) = row.approve_btn {
                    if btn.contains(x, y) {
                        return MemoryHit::EntryApprove(row.id);
                    }
                }
                if let Some(btn) = row.edit_btn {
                    if btn.contains(x, y) {
                        return MemoryHit::EntryEdit(row.id);
                    }
                }
                if let Some(btn) = row.delete_btn {
                    if btn.contains(x, y) {
                        return MemoryHit::EntryDelete(row.id);
                    }
                }
                return MemoryHit::EntryRow(row.id);
            }
            for row in &self.exclusion_rows {
                if row.remove_btn.contains(x, y) {
                    return MemoryHit::ExclusionRemove(row.index);
                }
            }
        }

        MemoryHit::Panel
    }
}

fn split_bar(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    trailing_w: f32,
    gap: f32,
    mirrored: bool,
) -> (Rect, Rect) {
    let tw = trailing_w.min(w);
    let inp_w = (w - tw - gap).max(0.0);
    if mirrored {
        (Rect::new(x + tw + gap, y, inp_w, h), Rect::new(x, y, tw, h))
    } else {
        (Rect::new(x, y, inp_w, h), Rect::new(x + w - tw, y, tw, h))
    }
}

fn layout_rules_file_row(
    index: usize,
    rect: Rect,
    file: &RulesFileInfo,
    scale: f32,
    mirrored: bool,
) -> RulesFileRowLayout {
    let px = |v: f32| (v * scale).round();
    let pad = px(12.0);
    let gap = px(6.0);
    let btn_h = px(26.0);
    let btn_y = rect.y + ((rect.h - btn_h) / 2.0).round();
    let badge_w = px(88.0);
    let badge_h = px(22.0);
    let badge_y = rect.y + ((rect.h - badge_h) / 2.0).round();

    let (approve_w, revoke_w) = match file.state {
        RulesFileState::Approved => (0.0, px(70.0)),
        RulesFileState::Unapproved => (px(78.0), 0.0),
        RulesFileState::Changed => (px(94.0), px(68.0)),
    };

    let (title_x, title_w, badge_rect, approve_btn, revoke_btn) = if mirrored {
        let mut cursor = rect.x + pad;
        let rev = (revoke_w > 0.0).then(|| {
            let r = Rect::new(cursor, btn_y, revoke_w, btn_h);
            cursor = r.right() + gap;
            r
        });
        let app = (approve_w > 0.0).then(|| {
            let r = Rect::new(cursor, btn_y, approve_w, btn_h);
            cursor = r.right() + gap;
            r
        });
        let badge = Rect::new(cursor, badge_y, badge_w, badge_h);
        cursor = badge.right() + px(10.0);
        let tw = (rect.right() - pad - cursor).max(0.0);
        (cursor, tw, badge, app, rev)
    } else {
        let mut cursor = rect.right() - pad;
        let rev = (revoke_w > 0.0).then(|| {
            cursor -= revoke_w;
            let r = Rect::new(cursor, btn_y, revoke_w, btn_h);
            cursor -= gap;
            r
        });
        let app = (approve_w > 0.0).then(|| {
            cursor -= approve_w;
            let r = Rect::new(cursor, btn_y, approve_w, btn_h);
            cursor -= gap;
            r
        });
        cursor -= badge_w;
        let badge = Rect::new(cursor, badge_y, badge_w, badge_h);
        let tx = rect.x + pad;
        let tw = (badge.x - px(10.0) - tx).max(0.0);
        (tx, tw, badge, app, rev)
    };

    let title_rect = Rect::new(title_x, rect.y + px(8.0), title_w, px(20.0));
    let hash_rect = Rect::new(title_x, title_rect.bottom() + px(2.0), title_w, px(16.0));

    RulesFileRowLayout {
        index,
        rect,
        title_rect,
        hash_rect,
        badge_rect,
        approve_btn,
        revoke_btn,
    }
}

fn layout_entry_row(
    entry: &MemoryEntry,
    list_index: usize,
    rect: Rect,
    editing: bool,
    show_approve: bool,
    scale: f32,
    mirrored: bool,
) -> MemoryEntryRowLayout {
    let px = |v: f32| (v * scale).round();
    let pad = px(12.0);
    let gap = px(6.0);
    let btn_h = px(26.0);
    let btn_y = rect.y + ((rect.h - btn_h) / 2.0).round();

    if editing {
        let save_w = px(66.0);
        let cancel_w = px(68.0);
        let inp_h = px(32.0);
        let inp_y = rect.y + ((rect.h - inp_h) / 2.0).round();
        let (edit_input, save_btn, cancel_btn) = if mirrored {
            let cb = Rect::new(rect.x + pad, btn_y, cancel_w, btn_h);
            let sb = Rect::new(cb.right() + gap, btn_y, save_w, btn_h);
            let ix = sb.right() + px(8.0);
            let iw = (rect.right() - pad - ix).max(0.0);
            (Rect::new(ix, inp_y, iw, inp_h), sb, cb)
        } else {
            let cb = Rect::new(rect.right() - pad - cancel_w, btn_y, cancel_w, btn_h);
            let sb = Rect::new(cb.x - gap - save_w, btn_y, save_w, btn_h);
            let ix = rect.x + pad;
            let iw = (sb.x - px(8.0) - ix).max(0.0);
            (Rect::new(ix, inp_y, iw, inp_h), sb, cb)
        };
        return MemoryEntryRowLayout {
            id: entry.id,
            list_index,
            rect,
            text_rect: edit_input,
            meta_rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            badge_rect: None,
            approve_btn: None,
            edit_btn: None,
            delete_btn: None,
            edit_input: Some(edit_input),
            save_btn: Some(save_btn),
            cancel_btn: Some(cancel_btn),
        };
    }

    let del_w = px(60.0);
    let edit_w = px(56.0);
    let app_w = if show_approve { px(74.0) } else { 0.0 };
    let badge_w = if show_approve || entry.kind == MemoryKind::Command {
        px(78.0)
    } else {
        0.0
    };
    let badge_h = px(20.0);
    let badge_y = rect.y + ((rect.h - badge_h) / 2.0).round();

    let (text_x, text_w, badge_rect, approve_btn, edit_btn, delete_btn) = if mirrored {
        let mut cursor = rect.x + pad;
        let del = Rect::new(cursor, btn_y, del_w, btn_h);
        cursor = del.right() + gap;
        let ed = Rect::new(cursor, btn_y, edit_w, btn_h);
        cursor = ed.right() + gap;
        let app = (app_w > 0.0).then(|| {
            let r = Rect::new(cursor, btn_y, app_w, btn_h);
            cursor = r.right() + gap;
            r
        });
        let badge = (badge_w > 0.0).then(|| {
            let r = Rect::new(cursor, badge_y, badge_w, badge_h);
            cursor = r.right() + px(8.0);
            r
        });
        let tx = cursor + px(4.0);
        let tw = (rect.right() - pad - tx).max(0.0);
        (tx, tw, badge, app, Some(ed), Some(del))
    } else {
        let mut cursor = rect.right() - pad;
        cursor -= del_w;
        let del = Rect::new(cursor, btn_y, del_w, btn_h);
        cursor -= gap + edit_w;
        let ed = Rect::new(cursor, btn_y, edit_w, btn_h);
        let app = (app_w > 0.0).then(|| {
            cursor -= gap + app_w;
            Rect::new(cursor, btn_y, app_w, btn_h)
        });
        let badge = (badge_w > 0.0).then(|| {
            cursor -= gap + badge_w;
            Rect::new(cursor, badge_y, badge_w, badge_h)
        });
        let tx = rect.x + pad;
        let tw = (cursor - px(10.0) - tx).max(0.0);
        (tx, tw, badge, app, Some(ed), Some(del))
    };

    let text_rect = Rect::new(text_x, rect.y + px(8.0), text_w, px(22.0));
    let meta_rect = Rect::new(text_x, text_rect.bottom() + px(2.0), text_w, px(16.0));

    MemoryEntryRowLayout {
        id: entry.id,
        list_index,
        rect,
        text_rect,
        meta_rect,
        badge_rect,
        approve_btn,
        edit_btn,
        delete_btn,
        edit_input: None,
        save_btn: None,
        cancel_btn: None,
    }
}
