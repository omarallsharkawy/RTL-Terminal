//! Coalesce PTY wakes and parse a bounded amount of output between input and redraw events.

use std::time::Instant;
use winit::event_loop::ActiveEventLoop;

use super::App;
use crate::layout::PaneId;
use crate::pty::schedule::{OutputSchedule, CHUNK_BYTES};

#[derive(Default)]
pub(super) struct OutputWork {
    pub(super) schedule: OutputSchedule,
    scratch: Vec<u8>,
}

impl App {
    pub(super) fn on_pty_exit(
        &mut self,
        event_loop: &ActiveEventLoop,
        id: PaneId,
        status: Option<u32>,
    ) {
        if !self.output.schedule.defer_exit(id, status) {
            self.finish_pty_exit(event_loop, id, status);
        }
    }

    fn finish_pty_exit(&mut self, event_loop: &ActiveEventLoop, id: PaneId, status: Option<u32>) {
        if !self.on_ssh_exit(id, status) && self.close_pane(id) {
            event_loop.exit();
        }
    }

    pub(super) fn poll_pty_output(&mut self, event_loop: &ActiveEventLoop) {
        while let Some(id) = self.output.schedule.next_pane(Instant::now) {
            let more = self.parse_pty_chunk(id);
            if let Some(status) = self.output.schedule.chunk_drained(id, more) {
                self.finish_pty_exit(event_loop, id, status);
            }
        }
    }

    fn parse_pty_chunk(&mut self, id: PaneId) -> bool {
        let Some(pane) = self.ws.pane_mut(id) else {
            return false;
        };
        let Some(pty) = &pane.pty else {
            return false;
        };
        let more = pty.take_output(&mut self.output.scratch, CHUNK_BYTES);
        let bytes = &self.output.scratch;
        if bytes.is_empty() {
            return more;
        }
        let term = &mut pane.terminal;
        let before = term.cursor_position();
        term.process_bytes(bytes);
        let moved = term.cursor_position() != before;
        let events = term.take_shell_events();
        pane.blocks.refresh(term);
        if moved && self.focused_id() == Some(id) {
            self.cursor_blink.reset(Instant::now());
        }
        if self.ws.is_visible(id) {
            self.window.request_redraw();
        }
        if self.modifiers.control_key()
            && !self.palette_open()
            && !self.settings_open()
            && !self.welcome_open()
        {
            self.update_link_hover(id);
        }
        for event in events {
            self.on_shell_event(id, event);
        }
        more
    }
}
