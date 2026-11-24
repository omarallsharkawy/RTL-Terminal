//! Command blocks from OSC 133 marks: building them, keeping them on their text while it scrolls
//! (also with a full scrollback, clears and resizes), copying their output, navigating.
use std::time::{Duration, Instant};
use twitty::blocks::{BlockList, BlockNav, BlockStatus};
use twitty::terminal::Terminal;

const A: &str = "\x1b]133;A\x07";
const B: &str = "\x1b]133;B\x07";
const C: &str = "\x1b]133;C\x07";

fn d(code: i32) -> String {
    format!("\x1b]133;D;{code}\x07")
}

/// A prompt, the typed command and its output, the way an integrated shell sends them.
fn command(cmd: &str, output: &str, code: i32) -> String {
    format!("{A}$ {B}{cmd}\r\n{C}{output}{}", d(code))
}

struct Pane {
    term: Terminal,
    blocks: BlockList,
    clock: Instant,
}

impl Pane {
    fn new(cols: usize, rows: usize, scrollback: usize) -> Self {
        Self {
            term: Terminal::with_scrollback(cols, rows, scrollback),
            blocks: BlockList::new(),
            clock: Instant::now(),
        }
    }

    fn feed(&mut self, text: &str) {
        self.term.process_bytes(text.as_bytes());
        for event in self.term.take_shell_events() {
            self.blocks.on_event(&event, &self.term, self.clock);
        }
        self.blocks.refresh(&self.term);
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        self.term.resize(cols, rows);
        self.blocks.refresh(&self.term);
    }

    /// Text of viewport row `row`.
    fn row_text(&self, row: usize) -> String {
        let (lines, _) = self.term.snapshot();
        lines[row]
            .cells
            .iter()
            .map(|c| c.c)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    /// The prompt line of every block, as text, read back through the absolute lines.
    fn prompt_texts(&self) -> Vec<String> {
        self.blocks
            .blocks()
            .map(|b| {
                self.term
                    .text_between((b.prompt.line, 0), (b.prompt.line + 1, 0))
                    .unwrap_or_else(|| "<gone>".into())
            })
            .collect()
    }

    fn outputs(&self) -> Vec<String> {
        self.blocks
            .blocks()
            .map(|b| {
                self.blocks
                    .text(b, &self.term)
                    .map(|t| t.output)
                    .unwrap_or_default()
            })
            .collect()
    }
}

#[test]
fn marks_make_blocks() {
    let mut p = Pane::new(40, 10, 100);
    p.feed(&command("ls", "a\r\nb\r\n", 0));
    p.feed(&command("false", "", 1));
    p.feed(&format!("{A}$ "));
    let blocks: Vec<_> = p.blocks.blocks().cloned().collect();
    assert_eq!(blocks.len(), 2);
    assert_eq!(
        blocks[0].command.as_deref(),
        Some("ls"),
        "read from the grid"
    );
    assert_eq!(blocks[0].status(), BlockStatus::Succeeded);
    assert_eq!(blocks[1].status(), BlockStatus::Failed(1));
    assert_eq!(blocks[0].prompt.column, 0);
    assert_eq!(blocks[0].command_start.unwrap().column, 2);
    assert_eq!(p.outputs(), vec!["a\nb".to_string(), String::new()]);
    // Lines: 0 "$ ls", 1 "a", 2 "b", 3 "$ false", 4 "$ ".
    let decor = p.blocks.decor(&p.term);
    let rows: Vec<_> = decor
        .iter()
        .map(|d| (d.first_row, d.last_row, d.status))
        .collect();
    assert_eq!(
        rows,
        vec![
            (0, 2, BlockStatus::Succeeded),
            (3, 3, BlockStatus::Failed(1)),
            (4, 4, BlockStatus::Prompt),
        ]
    );
    assert_eq!(decor[1].badge.as_deref(), Some("✗ 1"));
}

#[test]
fn command_line_from_osc_633_wins_and_running_blocks() {
    let mut p = Pane::new(40, 10, 100);
    p.feed(&format!(
        "{A}$ {B}sl\r\n\x1b]633;E;ls\\x3b echo مرحبا\x07{C}out\r\n"
    ));
    let block = p.blocks.blocks().next().unwrap().clone();
    assert_eq!(block.command.as_deref(), Some("ls; echo مرحبا"));
    assert_eq!(block.status(), BlockStatus::Running);
    assert_eq!(block.badge(), None);
    let decor = p.blocks.decor(&p.term);
    assert_eq!(
        (decor[0].first_row, decor[0].last_row),
        (0, 2),
        "to the cursor"
    );
    // The running block is not what "copy output" takes; there is no finished one yet.
    assert!(p.blocks.target().is_none());
    // Asking about "the selected or last block" does take it.
    let text = p.blocks.selected_or_last_text(&p.term).unwrap();
    assert_eq!(text.command.as_deref(), Some("ls; echo مرحبا"));
    assert_eq!(text.output, "out");
    assert!(text.running);
    p.feed(&d(0));
    assert_eq!(p.blocks.target_text(&p.term).unwrap().output, "out");
}

#[test]
fn prompts_without_commands_and_duplicate_marks() {
    let mut p = Pane::new(40, 10, 100);
    // Ctrl+C at the prompt: D without C, then a new prompt.
    p.feed(&format!("{A}$ {B}^C\r\n{}", d(130)));
    p.feed(&format!("{A}$ {B}"));
    assert!(p.blocks.is_empty());
    // Two integrations at once (fish 4 marks prompts itself): C and D twice.
    p.feed(&format!("ls\r\n{C}{C}x\r\n{}{}", d(0), d(2)));
    assert_eq!(p.blocks.len(), 1);
    assert_eq!(p.blocks.blocks().next().unwrap().exit, Some(0));
}

#[test]
fn duration_badge() {
    let mut p = Pane::new(40, 10, 100);
    p.feed(&format!("{A}$ {B}sleep 3\r\n{C}"));
    p.clock += Duration::from_millis(3400);
    p.feed(&d(2));
    let block = p.blocks.blocks().next().unwrap();
    assert_eq!(block.badge().as_deref(), Some("✗ 2 · 3.4s"));
    assert_eq!(
        twitty::blocks::format_duration(Duration::from_secs(125)),
        "2m 5s"
    );
    assert_eq!(
        twitty::blocks::format_duration(Duration::from_secs(3720)),
        "1h 2m"
    );
}

#[test]
fn blocks_follow_text_into_the_scrollback() {
    let mut p = Pane::new(20, 5, 1000);
    for i in 0..30 {
        p.feed(&command(&format!("echo {i}"), &format!("{i}\r\n"), 0));
    }
    p.feed(&format!("{A}$ "));
    assert_eq!(p.blocks.len(), 30);
    let prompts = p.prompt_texts();
    for (i, text) in prompts.iter().enumerate() {
        assert_eq!(text, &format!("$ echo {i}"));
    }
    assert_eq!(p.outputs()[7], "7");
}

#[test]
fn full_scrollback_drops_blocks_that_left_it() {
    // 5 rows + 12 lines of scrollback: only the last few commands are kept.
    let mut p = Pane::new(20, 5, 12);
    for i in 0..40 {
        p.feed(&command(&format!("echo {i}"), &format!("{i}\r\n"), 0));
        // Every block still kept must still sit on its own prompt.
        for (b, text) in p.blocks.blocks().zip(p.prompt_texts()) {
            let n = text
                .strip_prefix("$ echo ")
                .unwrap_or_else(|| panic!("{text:?}"));
            let out = p.blocks.text(b, &p.term).map(|t| t.output);
            assert_eq!(out.as_deref(), Some(n), "block of `echo {n}`");
        }
    }
    let n = p.blocks.len();
    assert!((7..=9).contains(&n), "{n} blocks kept");
    assert_eq!(p.prompt_texts().last().unwrap(), "$ echo 39");
}

#[test]
fn one_flood_bigger_than_the_scrollback() {
    let mut p = Pane::new(20, 5, 50);
    p.feed(&command("echo a", "a\r\n", 0));
    let flood: String = (0..500).map(|i| format!("line {i}\r\n")).collect();
    p.feed(&format!("{A}$ {B}cat big\r\n{C}{flood}{}", d(0)));
    // The flood pushed both blocks' prompts out of the scrollback.
    assert!(p.blocks.is_empty(), "{:?}", p.prompt_texts());
    p.feed(&command("echo b", "b\r\n", 0));
    assert_eq!(p.prompt_texts(), vec!["$ echo b".to_string()]);
}

#[test]
fn many_small_batches_with_a_full_scrollback() {
    let mut p = Pane::new(20, 5, 30);
    p.feed(&format!("{A}$ {B}seq\r\n{C}"));
    for i in 0..200 {
        p.feed(&format!("{i}\r\n"));
    }
    p.feed(&d(0));
    // The block's prompt left the scrollback long ago.
    assert!(p.blocks.is_empty());
    p.feed(&command("echo x", "x\r\n", 0));
    p.feed(&command("echo y", "y\r\n", 0));
    assert_eq!(p.prompt_texts(), vec!["$ echo x", "$ echo y"]);
}

#[test]
fn clear_and_reset() {
    let mut p = Pane::new(20, 5, 100);
    for i in 0..10 {
        p.feed(&command(&format!("echo {i}"), &format!("{i}\r\n"), 0));
    }
    // `clear`: ED 2 pushes the screen into the scrollback, ED 3 clears it.
    p.feed(&format!("{A}$ {B}clear\r\n{C}\x1b[H\x1b[2J\x1b[3J{}", d(0)));
    p.feed(&format!("{A}$ "));
    assert!(p.blocks.is_empty());
    p.feed(&format!("{B}echo z\r\n{C}z\r\n{}", d(0)));
    assert_eq!(p.prompt_texts(), vec!["$ echo z"]);

    // Ctrl+L: the screen goes into the scrollback, blocks stay with it.
    p.feed(&command("echo w", "w\r\n", 0));
    p.feed(&format!("{A}$ \x1b[H\x1b[2J{A}$ "));
    assert_eq!(p.prompt_texts(), vec!["$ echo z", "$ echo w"]);

    // `reset` (RIS) clears everything.
    p.feed("\x1bc");
    p.feed(&format!("{A}$ "));
    assert!(p.blocks.is_empty());
}

#[test]
fn clear_on_a_fresh_terminal() {
    // No scrollback yet when the screen is cleared: the new prompt lands on old blocks' lines.
    let mut p = Pane::new(20, 8, 100);
    p.feed(&command("echo a", "a\r\n", 0));
    p.feed(&command("echo b", "b\r\n", 0));
    p.feed(&format!(
        "{A}$ {B}clear\r\n{C}\x1b[H\x1b[2J\x1b[3J{}{A}$ ",
        d(0)
    ));
    assert!(p.blocks.is_empty(), "{:?}", p.prompt_texts());
}

#[test]
fn alt_screen_programs_make_no_blocks() {
    let mut p = Pane::new(20, 5, 100);
    p.feed(&command("echo a", "a\r\n", 0));
    p.feed(&format!("{A}$ {B}vim\r\n{C}\x1b[?1049h"));
    // A TUI that sends marks of its own, and scrolls its screen.
    p.feed(&format!("{A}{B}{C}{}", d(0)));
    for i in 0..20 {
        p.feed(&format!("{i}\r\n"));
    }
    assert!(
        p.blocks.decor(&p.term).is_empty(),
        "nothing drawn on the alt screen"
    );
    assert!(!p.blocks.navigable(&p.term));
    p.feed(&format!("\x1b[?1049l{}", d(0)));
    p.feed(&format!("{A}$ "));
    assert_eq!(p.prompt_texts(), vec!["$ echo a", "$ vim"]);
    assert!(p.blocks.navigable(&p.term));
}

#[test]
fn resize_rows_keeps_blocks_on_their_lines() {
    let mut p = Pane::new(20, 6, 100);
    for i in 0..8 {
        p.feed(&command(&format!("echo {i}"), &format!("{i}\r\n"), 0));
    }
    p.feed(&format!("{A}$ "));
    let expected: Vec<_> = (0..8).map(|i| format!("$ echo {i}")).collect();
    for rows in [3, 10, 4, 30, 6] {
        p.resize(20, rows);
        assert_eq!(p.prompt_texts(), expected, "{rows} rows");
    }
}

#[test]
fn reflow_keeps_blocks_on_their_lines() {
    let mut p = Pane::new(20, 6, 100);
    for i in 0..6 {
        let long = format!("{i}").repeat(30);
        p.feed(&command(&format!("echo {i}"), &format!("{long}\r\n"), 0));
    }
    p.feed(&format!("{A}$ "));
    for cols in [13, 40, 7, 25, 20] {
        p.resize(cols, 6);
        let prompts = p.prompt_texts();
        assert_eq!(prompts.len(), 6, "{cols} cols");
        for (i, text) in prompts.iter().enumerate() {
            assert!(
                format!("$ echo {i}").starts_with(text.as_str()) && text.len() >= 2,
                "{cols} cols: {text:?}"
            );
        }
        let outputs = p.outputs();
        assert_eq!(outputs[3], "3".repeat(30), "{cols} cols");
    }
}

#[test]
fn reflow_on_the_alt_screen_drops_blocks() {
    let mut p = Pane::new(20, 5, 100);
    p.feed(&command("echo a", "a\r\n", 0));
    p.feed(&format!("{A}$ {B}vim\r\n{C}\x1b[?1049h"));
    p.resize(30, 5);
    p.feed(&format!("\x1b[?1049l{}{A}$ ", d(0)));
    assert!(p.blocks.is_empty());
    p.feed(&format!("{B}ls\r\n{C}{}", d(0)));
    assert_eq!(p.prompt_texts(), vec!["$ ls"]);
}

#[test]
fn copy_output_with_arabic() {
    let mut p = Pane::new(20, 6, 100);
    // Wrapped Arabic output, a wide character, trailing blank lines.
    let arabic = "مرحبا بالعالم يا صديقي";
    p.feed(&command(
        "cat",
        &format!("{arabic}\r\n漢字 ok\r\n\r\n\r\n"),
        0,
    ));
    p.feed(&format!("{A}$ "));
    let text = p.blocks.target_text(&p.term).unwrap();
    assert_eq!(text.output, format!("{arabic}\n漢字 ok"));
    assert_eq!(text.command.as_deref(), Some("cat"));
    assert_eq!(text.exit, Some(0));
    assert!(!text.running);
    // Output without a final newline ends at the D mark.
    p.feed(&format!("{B}printf x\r\n{C}x{}", d(0)));
    p.feed(&format!("\r\n{A}$ "));
    assert_eq!(p.blocks.target_text(&p.term).unwrap().output, "x");
}

#[test]
fn navigation_order_and_selection() {
    let mut p = Pane::new(20, 5, 100);
    for i in 0..6 {
        p.feed(&command(&format!("echo {i}"), &format!("{i}\r\n"), 0));
    }
    p.feed(&format!("{A}$ "));
    let prompts: Vec<_> = p.blocks.blocks().map(|b| b.prompt.line).collect();
    assert_eq!(p.blocks.select_next(), BlockNav::None, "nothing selected");
    assert_eq!(p.blocks.select_prev(), BlockNav::ScrollTo(prompts[5]));
    assert_eq!(p.blocks.select_prev(), BlockNav::ScrollTo(prompts[4]));
    let BlockNav::ScrollTo(line) = p.blocks.select_prev() else {
        panic!()
    };
    assert_eq!(line, prompts[3]);
    p.term.scroll_to_line(line);
    assert_eq!(p.row_text(0), "$ echo 3");
    assert_eq!(p.blocks.target_text(&p.term).unwrap().output, "3");
    let decor = p.blocks.decor(&p.term);
    assert!(decor.iter().any(|d| d.selected && d.first_row == 0));
    for _ in 0..5 {
        p.blocks.select_prev();
    }
    assert_eq!(p.blocks.selected().unwrap().prompt.line, prompts[0]);
    p.term.scroll_to_line(prompts[0]);
    assert_eq!(p.row_text(0), "$ echo 0");
    for &line in &prompts[1..] {
        assert_eq!(p.blocks.select_next(), BlockNav::ScrollTo(line));
    }
    assert_eq!(p.blocks.select_next(), BlockNav::Bottom);
    assert!(p.blocks.selected().is_none());
    // A click in the gutter: row 1 shows the output of `echo 4`.
    p.term.scroll_to_bottom();
    assert_eq!(p.row_text(1), "4");
    assert!(p.blocks.select_at_row(&p.term, 1));
    assert_eq!(p.blocks.target_text(&p.term).unwrap().output, "4");
    assert!(p.blocks.clear_selection());
    assert!(!p.blocks.clear_selection());
    // The prompt still waiting for a command is not a block.
    assert!(!p.blocks.select_at_row(&p.term, 4));
}

#[test]
fn pane_without_marks_has_no_blocks() {
    let mut p = Pane::new(20, 5, 100);
    p.feed("hello\r\nworld\r\n");
    assert!(p.term.line_view().is_none());
    assert!(p.blocks.decor(&p.term).is_empty());
    assert!(!p.blocks.navigable(&p.term));
    p.feed(&format!("{A}$ "));
    assert!(
        p.blocks.decor(&p.term).is_empty(),
        "a prompt alone draws nothing"
    );
}

#[test]
fn block_list_is_bounded() {
    let mut p = Pane::new(20, 5, 100_000);
    for _ in 0..(twitty::blocks::MAX_BLOCKS + 10) {
        p.feed(&format!("{A}$ {B}x\r\n{C}{}", d(0)));
    }
    assert_eq!(p.blocks.len(), twitty::blocks::MAX_BLOCKS);
}

#[test]
fn resizes_with_a_full_scrollback() {
    let mut p = Pane::new(20, 6, 20);
    for i in 0..12u8 {
        let n = char::from(b'a' + i);
        let long = n.to_string().repeat(15);
        p.feed(&command(&format!("echo {n}"), &format!("{long}\r\n"), 0));
    }
    p.feed(&format!("{A}$ "));
    for (cols, rows) in [(8, 6), (8, 3), (30, 9), (6, 4), (20, 6), (20, 12)] {
        p.resize(cols, rows);
        let kept: Vec<_> = p.blocks.blocks().cloned().collect();
        assert!(!kept.is_empty(), "{cols}x{rows}");
        for (b, text) in kept.iter().zip(p.prompt_texts()) {
            let out = p.blocks.text(b, &p.term).unwrap().output;
            let n = out.chars().next().unwrap();
            assert_eq!(out, n.to_string().repeat(15), "{cols}x{rows}");
            assert!(
                format!("$ echo {n}").starts_with(&text) && text.len() >= 2,
                "{cols}x{rows}: {text:?} for output {out:?}"
            );
        }
        // The newest block is always kept.
        assert_eq!(
            p.blocks.blocks().last().unwrap().command.as_deref(),
            Some("echo l")
        );
    }
}
