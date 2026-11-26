//! Short UI transitions sampled from monotonic time. Nothing here owns a timer or a window.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

mod chrome;
pub(crate) use chrome::{ChromeAnimations, ChromeMotion};

pub const HOVER: Duration = Duration::from_millis(120);
pub const PANEL: Duration = Duration::from_millis(160);
pub const FRAME_STEP: Duration = Duration::from_millis(16);

pub fn ease_out(progress: f32) -> f32 {
    1.0 - (1.0 - progress.clamp(0.0, 1.0)).powi(3)
}

pub fn progress(elapsed: Duration, duration: Duration) -> f32 {
    if elapsed >= duration {
        1.0
    } else {
        elapsed.as_secs_f32() / duration.as_secs_f32()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Tween {
    from: f32,
    target: f32,
    timing: Option<(Instant, Duration)>,
}

impl Tween {
    pub fn fixed(value: f32) -> Self {
        Self {
            from: value,
            target: value,
            timing: None,
        }
    }

    pub fn value(self, now: Instant) -> f32 {
        self.timing.map_or(self.target, |(start, duration)| {
            let fraction = ease_out(progress(now.saturating_duration_since(start), duration));
            self.from + (self.target - self.from) * fraction
        })
    }

    pub fn set(&mut self, target: f32, now: Instant, duration: Duration) {
        if duration.is_zero() {
            *self = Self::fixed(target);
        } else if target != self.target {
            let from = self.value(now);
            *self = Self {
                from,
                target,
                timing: (from != target).then_some((now, duration)),
            };
        }
    }

    /// Includes the endpoint, so the last frame always paints the exact target.
    pub fn next_frame(self, now: Instant) -> Option<Instant> {
        let (start, duration) = self.timing?;
        let end = start + duration;
        (now < end).then(|| (now + FRAME_STEP).min(end))
    }
}

/// Cross-fades keyed highlights, including rapid changes across more than two items.
pub struct Highlights<K> {
    target: Option<K>,
    tracks: HashMap<K, Tween>,
}

impl<K: Copy + Eq + Hash> Default for Highlights<K> {
    fn default() -> Self {
        Self {
            target: None,
            tracks: HashMap::new(),
        }
    }
}

impl<K: Copy + Eq + Hash> Highlights<K> {
    pub fn set(&mut self, target: Option<K>, now: Instant, duration: Duration) {
        if duration.is_zero() {
            self.tracks.clear();
        } else if target != self.target {
            if let Some(previous) = self.target {
                self.tracks
                    .entry(previous)
                    .or_insert_with(|| Tween::fixed(1.0))
                    .set(0.0, now, duration);
            }
            if let Some(current) = target {
                self.tracks
                    .entry(current)
                    .or_default()
                    .set(1.0, now, duration);
            }
        }
        self.target = target;
        self.tracks
            .retain(|key, track| Some(*key) == target || track.next_frame(now).is_some());
    }

    pub fn value(&self, key: K, now: Instant) -> f32 {
        self.tracks
            .get(&key)
            .map_or(f32::from(self.target == Some(key)), |t| t.value(now))
    }

    pub fn next_frame(&self, now: Instant) -> Option<Instant> {
        self.tracks
            .values()
            .filter_map(|track| track.next_frame(now))
            .min()
    }
}

/// A moving emphasis across the three working-status dots, with a continuous loop seam.
pub fn working_dot(phase: f32, index: usize) -> f32 {
    let distance = (phase.rem_euclid(3.0) - index as f32).abs();
    let distance = distance.min(3.0 - distance).min(1.0);
    0.5 + 0.5 * (std::f32::consts::PI * distance).cos()
}

/// Keep the last visible view through its exit. Input still uses only the live view.
pub(crate) struct AnimatedView<T> {
    view: Option<T>,
    open: bool,
    visibility: Tween,
}

impl<T> Default for AnimatedView<T> {
    fn default() -> Self {
        Self {
            view: None,
            open: false,
            visibility: Tween::default(),
        }
    }
}

impl<T> AnimatedView<T> {
    pub(crate) fn set(&mut self, view: Option<T>, now: Instant, reduced: bool) {
        self.open = view.is_some();
        self.visibility.set(
            f32::from(self.open),
            now,
            if reduced { Duration::ZERO } else { PANEL },
        );
        if view.is_some() || self.visibility.next_frame(now).is_none() {
            self.view = view;
        }
    }

    pub(crate) fn live(&self) -> Option<&T> {
        self.view.as_ref().filter(|_| self.open)
    }

    pub(crate) fn sample(&mut self, now: Instant) -> Option<(&T, f32, bool)> {
        let opacity = self.visibility.value(now);
        if !self.open && self.visibility.next_frame(now).is_none() {
            self.view = None;
        }
        Some((self.view.as_ref()?, opacity, self.open))
    }

    pub(crate) fn next_frame(&self, now: Instant) -> Option<Instant> {
        self.visibility.next_frame(now)
    }
}

/// Vertical offsets preserve the last painted positions when workspace order changes.
#[derive(Default)]
pub struct Reorder {
    order: Vec<u64>,
    positions: HashMap<u64, f32>,
    offsets: HashMap<u64, Tween>,
}

impl Reorder {
    pub fn update(
        &mut self,
        order: &[u64],
        positions: HashMap<u64, f32>,
        now: Instant,
        reduced: bool,
    ) {
        if reduced {
            self.offsets.clear();
        } else if self.order != order {
            self.offsets = if self.order.len() == order.len()
                && order.iter().all(|id| self.positions.contains_key(id))
            {
                positions
                    .iter()
                    .filter_map(|(&id, &y)| {
                        let previous = self.positions.get(&id)? + self.offset(id, now);
                        if previous == y {
                            return None;
                        }
                        let mut tween = Tween::fixed(previous - y);
                        tween.set(0.0, now, PANEL);
                        Some((id, tween))
                    })
                    .collect()
            } else {
                HashMap::new()
            };
        }
        self.offsets
            .retain(|_, tween| tween.next_frame(now).is_some());
        self.order.clear();
        self.order.extend_from_slice(order);
        self.positions = positions;
    }

    pub fn offset(&self, id: u64, now: Instant) -> f32 {
        self.offsets.get(&id).map_or(0.0, |track| track.value(now))
    }

    pub fn next_frame(&self, now: Instant) -> Option<Instant> {
        self.offsets
            .values()
            .filter_map(|track| track.next_frame(now))
            .min()
    }
}
