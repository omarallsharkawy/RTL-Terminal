use crate::terminal::TerminalGrid;

#[derive(Debug, Default)]
pub struct AnsiParser {
    state: State,
    csi: String,
    osc: String,
}

#[derive(Debug, Default)]
enum State {
    #[default]
    Ground,
    Escape,
    Csi,
    Osc,
    OscEsc,
}

impl AnsiParser {
    pub fn push(&mut self, input: &str, grid: &mut TerminalGrid) {
        for ch in input.chars() {
            match self.state {
                State::Ground => match ch {
                    '\u{1b}' => self.state = State::Escape,
                    _ => grid.put_char(ch),
                },
                State::Escape => match ch {
                    '[' => { self.csi.clear(); self.state = State::Csi; }
                    ']' => { self.osc.clear(); self.state = State::Osc; }
                    'c' => { grid.clear_screen(); self.state = State::Ground; }
                    _ => self.state = State::Ground,
                },
                State::Csi => {
                    if ch.is_ascii_alphabetic() || matches!(ch, '~') {
                        self.dispatch_csi(ch, grid);
                        self.state = State::Ground;
                    } else {
                        self.csi.push(ch);
                    }
                }
                State::Osc => match ch {
                    '\u{07}' => { self.dispatch_osc(grid); self.state = State::Ground; }
                    '\u{1b}' => self.state = State::OscEsc,
                    _ => self.osc.push(ch),
                },
                State::OscEsc => match ch {
                    '\\' => { self.dispatch_osc(grid); self.state = State::Ground; }
                    _ => { self.osc.push('\u{1b}'); self.osc.push(ch); self.state = State::Osc; }
                },
            }
        }
    }

    fn dispatch_csi(&self, command: char, grid: &mut TerminalGrid) {
        let params: Vec<u16> = self.csi
            .split(';')
            .filter_map(|part| part.trim_matches('?').parse().ok())
            .collect();
        match command {
            'm' => grid.sgr(&params),
            'H' | 'f' => grid.move_cursor(params.first().copied().unwrap_or(1) as usize, params.get(1).copied().unwrap_or(1) as usize),
            'J' => grid.clear_screen(),
            'K' => grid.clear_line_from_cursor(),
            _ => {}
        }
    }

    fn dispatch_osc(&self, grid: &mut TerminalGrid) {
        if let Some(title) = self.osc.strip_prefix("0;").or_else(|| self.osc.strip_prefix("2;")) {
            grid.set_title(title.to_string());
        }
    }
}
