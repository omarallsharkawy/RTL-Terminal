//! Coordinates the chrome's keyed transitions and exposes their next frame deadline.

use std::time::{Duration, Instant};

use super::{Highlights, Reorder, FRAME_STEP, HOVER, PANEL};
use crate::sidebar::cards::{Card, View, DOT_INTERVAL};
use crate::ui::chrome::ChromeAction;

pub struct ChromeMotion<'a> {
    pub actions: &'a Highlights<ChromeAction>,
    pub tab_hover: &'a Highlights<u64>,
    pub selection: &'a Highlights<u64>,
    pub tab_ids: &'a [u64],
    pub broadcast: bool,
    pub now: Instant,
}

#[derive(Default)]
pub struct ChromeAnimations {
    reduced: bool,
    actions: Highlights<ChromeAction>,
    tab_hover: Highlights<u64>,
    close_hover: Highlights<u64>,
    selection: Highlights<u64>,
    selection_initialized: bool,
    reorder: Reorder,
    working_since: Option<Instant>,
}

impl ChromeAnimations {
    pub fn new(reduced: bool) -> Self {
        Self {
            reduced,
            ..Self::default()
        }
    }

    pub fn reduced(&self) -> bool {
        self.reduced
    }

    fn duration(&self, duration: Duration) -> Duration {
        if self.reduced {
            Duration::ZERO
        } else {
            duration
        }
    }

    pub fn begin_frame(&mut self, now: Instant, active: Option<u64>, hover: Option<ChromeAction>) {
        let selection_duration = if self.selection_initialized {
            self.duration(PANEL)
        } else {
            Duration::ZERO
        };
        self.selection.set(active, now, selection_duration);
        self.selection_initialized = true;
        self.actions.set(hover, now, self.duration(HOVER));
    }

    pub fn reorder(
        &mut self,
        view: &mut View,
        cards: &[Card],
        ids: &[u64],
        active: usize,
        now: Instant,
    ) {
        view.apply_reorder(cards, ids, active, &mut self.reorder, now, self.reduced);
    }

    pub fn apply(
        &mut self,
        sidebar: Option<&mut View>,
        hover_tab: Option<usize>,
        ids: &[u64],
        now: Instant,
        occluded: bool,
    ) {
        let hovered = sidebar.as_ref().and_then(|view| view.hover).or(hover_tab);
        let hovered_id = hovered.and_then(|index| ids.get(index)).copied();
        let duration = self.duration(HOVER);
        self.tab_hover.set(hovered_id, now, duration);
        let close_is_hot = sidebar.as_ref().is_some_and(|view| view.close_hover);
        self.close_hover
            .set(hovered_id.filter(|_| close_is_hot), now, duration);
        let Some(view) = sidebar else {
            self.reorder = Reorder::default();
            self.suspend();
            return;
        };
        view.highlights = Some(
            ids.iter()
                .map(|&id| (self.tab_hover.value(id, now), self.selection.value(id, now)))
                .collect(),
        );
        // A fading close affordance follows its card even after the pointer moves.
        view.close_mixes = ids
            .iter()
            .map(|&id| self.close_hover.value(id, now))
            .collect();
        view.new_tab_mix = Some(self.actions.value(ChromeAction::NewTab, now));
        if !occluded && !self.reduced && view.animates() {
            let started = *self.working_since.get_or_insert(now);
            let cycle = DOT_INTERVAL * 3;
            let elapsed = now.saturating_duration_since(started);
            view.dot_progress = Some(
                (elapsed.as_nanos() % cycle.as_nanos()) as f32 / DOT_INTERVAL.as_nanos() as f32,
            );
        } else {
            self.suspend();
        }
    }

    pub fn suspend(&mut self) {
        self.working_since = None;
    }

    pub fn sample<'a>(&'a self, ids: &'a [u64], broadcast: bool, now: Instant) -> ChromeMotion<'a> {
        ChromeMotion {
            actions: &self.actions,
            tab_hover: &self.tab_hover,
            selection: &self.selection,
            tab_ids: ids,
            broadcast,
            now,
        }
    }

    pub fn next_frame(&self, now: Instant) -> Option<Instant> {
        [
            self.actions.next_frame(now),
            self.tab_hover.next_frame(now),
            self.close_hover.next_frame(now),
            self.selection.next_frame(now),
            self.reorder.next_frame(now),
            self.working_since.map(|_| now + FRAME_STEP),
        ]
        .into_iter()
        .flatten()
        .min()
    }
}
