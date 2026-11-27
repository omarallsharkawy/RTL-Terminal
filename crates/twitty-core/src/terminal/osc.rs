//! OSC stream length guard and shell-signal payload capture.

use crate::shell_integration::MAX_SIGNAL_LEN;

/// Longest OSC string (title, hyperlink, clipboard copy…) passed to the parser. vte buffers an
/// OSC until its terminator with no limit, so an unterminated one (`\e]52;c;` followed by
/// endless data) would grow the terminal's memory until it is killed.
pub const MAX_OSC_LEN: usize = 8 << 20;

/// Tracks OSC strings in the byte stream and cuts the ones longer than `MAX_OSC_LEN`.
#[derive(Default)]
pub(super) struct OscGuard {
    pub(super) state: OscState,
    /// Start of the current OSC's payload, up to `MAX_SIGNAL_LEN` bytes, for shell signals.
    pub(super) payload: Vec<u8>,
    /// The payload went past `MAX_SIGNAL_LEN`: it is not a signal.
    pub(super) payload_cut: bool,
}

#[derive(Default, Clone, Copy, PartialEq)]
pub(super) enum OscState {
    #[default]
    Other,
    Escape,
    Osc(usize),
    /// Past the limit: bytes are dropped until the terminator.
    Overflow,
}

/// What to do with one byte.
#[derive(PartialEq)]
pub(super) enum OscStep {
    Keep,
    /// Keep this byte; it ends an OSC whose payload is in `OscGuard::payload`.
    End,
    Drop,
    /// The OSC just went over the limit: replace this byte with one that spoils the payload, so
    /// a cut clipboard copy (invalid base64) or link is ignored instead of applied half.
    Spoil,
}

impl OscGuard {
    pub(super) fn step(&mut self, byte: u8) -> OscStep {
        use OscState::*;
        let (next, step) = match (self.state, byte) {
            // BEL and ESC (ST is ESC \) end an OSC; CAN and SUB cancel it.
            (Osc(_), 0x07) => (Other, OscStep::End),
            (Osc(_), 0x1B) => (Escape, OscStep::End),
            (Osc(_) | Overflow, 0x18 | 0x1A) => (Other, OscStep::Keep),
            (Overflow, 0x07) => (Other, OscStep::Keep),
            (_, 0x1B) => (Escape, OscStep::Keep),
            (Escape, b']') => {
                self.payload.clear();
                self.payload_cut = false;
                (Osc(0), OscStep::Keep)
            }
            (Escape, _) => (Other, OscStep::Keep),
            (Osc(n), _) if n >= MAX_OSC_LEN => (Overflow, OscStep::Spoil),
            (Osc(n), _) => {
                if self.payload.len() < MAX_SIGNAL_LEN {
                    self.payload.push(byte);
                } else {
                    self.payload_cut = true;
                }
                (Osc(n + 1), OscStep::Keep)
            }
            (Overflow, _) => (Overflow, OscStep::Drop),
            (Other, _) => (Other, OscStep::Keep),
        };
        self.state = next;
        step
    }
}
