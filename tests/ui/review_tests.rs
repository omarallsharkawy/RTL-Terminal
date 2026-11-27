//! The code review panel, window-free: diff parsing and its bounds, rows and scrolling, the
//! panel's keys and clicks, comment formatting and the agent pane choice, layout and hit
//! testing, git loading on a scratch repository, and the panel drawn headless.

use std::path::PathBuf;

use cosmic_text::FontSystem;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, SmolStr};

use twitty::color::Palette;
use twitty::commands;
use twitty::input::{handle_key_mode_raw, InputAction};
use twitty::layout::{PaneId, Rect};
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::review::{build, ReviewCache};
use twitty::render::text::ui::UiFont;
use twitty::render::ui_style::UiStyle;
use twitty::render::CellMetrics;
use twitty::review::comment::{
    choose_agent_pane, clean_comment, format_comment, runs_agent, CommentTarget, PaneInfo,
    MAX_COMMENT_CHARS,
};
use twitty::review::diff::{parse, Limits, OneLine, Untracked, MAX_DIFF_BYTES};
use twitty::review::layout::{split_content, PanelHit, PanelLayout};
use twitty::review::panel::{display_text, Load, RowStyle};
use twitty::review::rows::{
    build_rows, clamp_top, nearest, next_file, reveal, row_at, step, LineRef, Note, Row,
};
use twitty::review::{Diff, FileStatus, LineKind, LoadError, PanelCmd, ReviewPanel, Snapshot};
use twitty::ui::chrome::UiLanguage;
use twitty::workflows::encode_insert;

// ---- fixtures ----

const TWO_FILES: &str = "\
diff --git a/src/main.rs b/src/main.rs
index 1111111..2222222 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,4 +1,5 @@ fn main() {
 use std::io;
-let a = 1;
+let a = 2;
+let b = 3;

 fn x() {}
@@ -20,3 +21,2 @@ impl Foo
 one
-two
 three
diff --git a/README.md b/README.md
index 3333333..4444444 100644
--- a/README.md
+++ b/README.md
@@ -1 +1 @@
-old
+new
";

fn parsed(text: &str) -> Diff {
    parse(text.as_bytes(), false, &Limits::default())
}

#[test]
fn parses_files_hunks_and_line_numbers() {
    let diff = parsed(TWO_FILES);
    assert_eq!(diff.files.len(), 2);
    let main = &diff.files[0];
    assert_eq!(main.path(), "src/main.rs");
    assert_eq!(main.status, FileStatus::Modified);
    assert_eq!((main.added, main.removed), (2, 2));
    assert_eq!(main.hunks.len(), 2);
    let h = &main.hunks[0];
    assert_eq!(
        (h.old_start, h.old_count, h.new_start, h.new_count),
        (1, 4, 1, 5)
    );
    assert_eq!(h.section, "fn main() {");
    let numbered: Vec<_> = h.lines.iter().map(|l| (l.kind, l.old, l.new)).collect();
    assert_eq!(
        numbered,
        vec![
            (LineKind::Context, Some(1), Some(1)),
            (LineKind::Del, Some(2), None),
            (LineKind::Add, None, Some(2)),
            (LineKind::Add, None, Some(3)),
            (LineKind::Context, Some(3), Some(4)),
            (LineKind::Context, Some(4), Some(5)),
        ]
    );
    // The empty context line (a lone space) keeps an empty text.
    assert_eq!(h.lines[4].text, "");
    let h2 = &main.hunks[1];
    assert_eq!(h2.lines[1].old, Some(21));
    assert_eq!(h2.lines[2].new, Some(22));
    assert_eq!(h2.header(), "@@ -20,3 +21,2 @@ impl Foo");
    assert_eq!(diff.totals(), (3, 3));
    assert_eq!(diff.files[1].path(), "README.md");
    assert_eq!(diff.files[1].hunks[0].old_count, 1);
    assert!(!diff.truncated && !diff.lines_capped);
}

#[test]
fn renames_copies_new_and_deleted_files() {
    let text = "\
diff --git a/old name.txt b/new name.txt
similarity index 100%
rename from old name.txt
rename to new name.txt
diff --git a/a.rs b/b.rs
similarity index 90%
rename from a.rs
rename to b.rs
index 1..2 100644
--- a/a.rs
+++ b/b.rs
@@ -1 +1 @@
-x
+y
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index 5..0
--- a/gone.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-1
-2
diff --git a/fresh.txt b/fresh.txt
new file mode 100644
index 0..7
--- /dev/null
+++ b/fresh.txt
@@ -0,0 +1 @@
+hello
";
    let diff = parsed(text);
    assert_eq!(diff.files.len(), 4);
    let pure = &diff.files[0];
    assert_eq!(pure.status, FileStatus::Renamed);
    assert_eq!(pure.old_path.as_deref(), Some("old name.txt"));
    assert_eq!(pure.new_path.as_deref(), Some("new name.txt"));
    assert_eq!(pure.similarity, Some(100));
    assert_eq!(pure.display_path(), "old name.txt → new name.txt");
    assert!(pure.hunks.is_empty());
    let edited = &diff.files[1];
    assert_eq!(edited.status, FileStatus::Renamed);
    assert_eq!(edited.path(), "b.rs");
    assert_eq!(edited.hunks[0].lines.len(), 2);
    let gone = &diff.files[2];
    assert_eq!(gone.status, FileStatus::Deleted);
    assert_eq!(gone.new_path, None);
    assert_eq!(gone.path(), "gone.txt");
    assert_eq!(gone.removed, 2);
    let fresh = &diff.files[3];
    assert_eq!(fresh.status, FileStatus::Added);
    assert_eq!(fresh.old_path, None);
    assert_eq!(fresh.hunks[0].lines[0].new, Some(1));
}

#[test]
fn binary_and_mode_changes_are_one_line_entries() {
    let text = "\
diff --git a/logo.png b/logo.png
index 1..2 100644
Binary files a/logo.png and b/logo.png differ
diff --git a/run.sh b/run.sh
old mode 100644
new mode 100755
";
    let diff = parsed(text);
    assert_eq!(diff.files[0].one_line, Some(OneLine::Binary));
    assert_eq!(diff.files[0].path(), "logo.png");
    assert_eq!(diff.files[1].path(), "run.sh");
    assert_eq!(
        diff.files[1].mode_change().as_deref(),
        Some("100644 → 100755")
    );
    let rows = build_rows(&diff, |_| false, None);
    assert_eq!(
        rows,
        vec![
            Row::File(0),
            Row::Note(0, Note::OneLine(OneLine::Binary)),
            Row::File(1),
            Row::Note(1, Note::Mode),
        ]
    );
}

#[test]
fn no_newline_markers_attach_to_their_line() {
    let text = "\
diff --git a/f b/f
--- a/f
+++ b/f
@@ -1 +1 @@
-old
\\ No newline at end of file
+new
\\ No newline at end of file
";
    let diff = parsed(text);
    let lines = &diff.files[0].hunks[0].lines;
    assert_eq!(lines.len(), 2);
    assert!(lines[0].no_newline && lines[1].no_newline);
    assert_eq!(lines[1].text, "new");
    let rows = build_rows(&diff, |_| false, None);
    let r1 = LineRef {
        file: 0,
        hunk: 0,
        line: 1,
    };
    assert_eq!(rows[rows.len() - 2..], [Row::Line(r1), Row::NoNewline(r1)]);
}

#[test]
fn crlf_lines_lose_their_carriage_return() {
    let text = "diff --git a/w.txt b/w.txt\r\n--- a/w.txt\r\n+++ b/w.txt\r\n@@ -1,2 +1,2 @@\r\n same\r\n-old\r\n+new\r\n";
    let diff = parsed(text);
    let file = &diff.files[0];
    assert_eq!(file.path(), "w.txt");
    let texts: Vec<_> = file.hunks[0]
        .lines
        .iter()
        .map(|l| l.text.as_str())
        .collect();
    assert_eq!(texts, ["same", "old", "new"]);
}

#[test]
fn arabic_lines_and_paths_stay_intact() {
    let text = "\
diff --git a/مستندات/ملاحظات.md b/مستندات/ملاحظات.md
--- a/مستندات/ملاحظات.md
+++ b/مستندات/ملاحظات.md
@@ -1 +1 @@
-السلام عليكم
+مرحبا بالعالم 42 ok
";
    let diff = parsed(text);
    let file = &diff.files[0];
    assert_eq!(file.path(), "مستندات/ملاحظات.md");
    assert_eq!(file.hunks[0].lines[1].text, "مرحبا بالعالم 42 ok");
    assert_eq!(file.hunks[0].lines[0].text, "السلام عليكم");
}

#[test]
fn quoted_paths_are_unquoted() {
    let text = "\
diff --git \"a/tab\\there\" \"b/tab\\there\"
--- \"a/tab\\there\"
+++ \"b/tab\\there\"
@@ -1 +1 @@
-a
+b
diff --git \"a/caf\\303\\251.bin\" \"b/caf\\303\\251.bin\"
Binary files \"a/caf\\303\\251.bin\" and \"b/caf\\303\\251.bin\" differ
";
    let diff = parsed(text);
    assert_eq!(diff.files[0].path(), "tab\there");
    // Binary: no ---/+++ lines, so the name comes from the `diff --git` header.
    assert_eq!(diff.files[1].path(), "café.bin");
    // Names with a space get a tab after them from git.
    let spaced =
        "diff --git a/my file b/my file\n--- a/my file\t\n+++ b/my file\t\n@@ -1 +1 @@\n-a\n+b\n";
    assert_eq!(parsed(spaced).files[0].path(), "my file");
}

#[test]
fn a_cut_input_drops_its_partial_line_and_says_so() {
    let mut text = TWO_FILES.as_bytes().to_vec();
    text.truncate(TWO_FILES.find("+let b").unwrap() + 4);
    let diff = parse(&text, true, &Limits::default());
    assert!(diff.truncated);
    let lines = &diff.files[0].hunks[0].lines;
    assert_eq!(lines.last().unwrap().text, "let a = 2;");
    assert_eq!(
        build_rows(&diff, |_| false, None).last(),
        Some(&Row::Truncated)
    );
}

#[test]
fn bounds_on_files_lines_and_line_length() {
    let mut text = String::new();
    for i in 0..5 {
        text.push_str(&format!(
            "diff --git a/f{i} b/f{i}\n--- a/f{i}\n+++ b/f{i}\n@@ -1,3 +1,3 @@\n-a\n+b\n c\n d\n"
        ));
    }
    let limits = Limits {
        max_files: 3,
        max_lines: 7,
        max_file_lines: 100,
        max_line_chars: 10,
    };
    let diff = parse(text.as_bytes(), false, &limits);
    assert_eq!(diff.files.len(), 3);
    assert_eq!(diff.omitted_files, 2);
    assert_eq!(diff.file_count(), 5);
    // Four lines each: the second file still fits (8 > 7 would not), the third does not.
    assert!(diff.files[0].one_line.is_none());
    assert_eq!(diff.files[1].one_line, Some(OneLine::TooLarge));
    assert!(diff.files[1].hunks.is_empty());
    assert_eq!((diff.files[1].added, diff.files[1].removed), (1, 1));
    assert!(diff.lines_capped);
    assert!(diff.stored_lines <= 7);
    let rows = build_rows(&diff, |_| false, None);
    assert!(rows.contains(&Row::Omitted) && rows.contains(&Row::Truncated));

    // One file over the per-file bound is a one-line entry; others stay.
    let mut big = String::from("diff --git a/big b/big\n--- a/big\n+++ b/big\n@@ -0,0 +1,50 @@\n");
    for i in 0..50 {
        big.push_str(&format!("+line {i}\n"));
    }
    big.push_str("diff --git a/small b/small\n--- a/small\n+++ b/small\n@@ -1 +1 @@\n-x\n+y\n");
    let limits = Limits {
        max_file_lines: 20,
        ..Limits::default()
    };
    let diff = parse(big.as_bytes(), false, &limits);
    assert_eq!(diff.files[0].one_line, Some(OneLine::TooLarge));
    assert_eq!(diff.files[0].added, 50);
    assert_eq!(diff.files[1].hunks[0].lines.len(), 2);
    assert!(!diff.lines_capped);

    // Long lines are cut with an ellipsis, by characters (never inside one).
    let long = format!(
        "diff --git a/l b/l\n--- a/l\n+++ b/l\n@@ -0,0 +1 @@\n+{}\n",
        "ب".repeat(30)
    );
    let diff = parse(long.as_bytes(), false, &limits_chars(10));
    assert_eq!(
        diff.files[0].hunks[0].lines[0].text,
        format!("{}…", "ب".repeat(10))
    );
}

fn limits_chars(n: usize) -> Limits {
    Limits {
        max_line_chars: n,
        ..Limits::default()
    }
}

#[test]
fn huge_input_parses_within_the_default_bounds() {
    let mut text = String::new();
    let mut i = 0;
    while text.len() < MAX_DIFF_BYTES {
        text.push_str(&format!(
            "diff --git a/f{i} b/f{i}\n--- a/f{i}\n+++ b/f{i}\n@@ -1,40 +1,40 @@\n"
        ));
        for j in 0..40 {
            text.push_str(&format!("-old {j}\n+new {j}\n"));
        }
        i += 1;
    }
    let cut = &text.as_bytes()[..MAX_DIFF_BYTES];
    let diff = parse(cut, true, &Limits::default());
    assert!(diff.truncated);
    assert!(diff.files.len() <= Limits::default().max_files);
    assert!(diff.stored_lines <= Limits::default().max_lines);
    assert!(diff.omitted_files > 0);
}

#[test]
fn untracked_files_become_added_files() {
    let limits = Limits::default();
    let mut diff = Diff::default();
    diff.push_untracked("notes.txt", Untracked::Text(b"one\ntwo".to_vec()), &limits);
    diff.push_untracked("img.bin", Untracked::Text(b"PNG\0\x01".to_vec()), &limits);
    diff.push_untracked("empty", Untracked::Text(Vec::new()), &limits);
    diff.push_untracked("huge.log", Untracked::TooBig(9_000_000), &limits);
    diff.push_untracked("link", Untracked::Special, &limits);
    let notes = &diff.files[0];
    assert!(notes.untracked);
    assert_eq!(notes.status, FileStatus::Added);
    assert_eq!(notes.added, 2);
    let lines = &notes.hunks[0].lines;
    assert_eq!(lines[1].new, Some(2));
    assert!(lines[1].no_newline && !lines[0].no_newline);
    assert_eq!(diff.files[1].one_line, Some(OneLine::Binary));
    assert!(diff.files[2].hunks.is_empty());
    assert_eq!(diff.files[3].one_line, Some(OneLine::TooBig(9_000_000)));
    assert_eq!(diff.files[4].one_line, Some(OneLine::Special));
    let rows = build_rows(&diff, |_| false, None);
    assert!(rows.contains(&Row::Note(2, Note::Empty)));
}

// ---- rows and scrolling ----

#[test]
fn rows_follow_collapse_and_the_comment_box() {
    let diff = parsed(TWO_FILES);
    let all = build_rows(&diff, |_| false, None);
    assert_eq!(all[0], Row::File(0));
    assert_eq!(all[1], Row::Hunk(0, 0));
    assert_eq!(all.iter().filter(|r| matches!(r, Row::File(_))).count(), 2);
    let collapsed = build_rows(&diff, |f| f == 0, None);
    assert_eq!(collapsed[..2], [Row::File(0), Row::File(1)]);
    let anchor = LineRef {
        file: 0,
        hunk: 0,
        line: 1,
    };
    let with_box = build_rows(&diff, |_| false, Some(anchor));
    let at = with_box
        .iter()
        .position(|r| *r == Row::Line(anchor))
        .unwrap();
    assert_eq!(
        with_box[at + 1..at + 3],
        [Row::CommentInput, Row::CommentHint]
    );
    assert_eq!(with_box.len(), all.len() + 2);
}

#[test]
fn scroll_and_selection_math() {
    let diff = parsed(TWO_FILES);
    let rows = build_rows(&diff, |_| false, None);
    // Selection skips hunk headers.
    assert_eq!(step(&rows, None, 1), Some(0));
    assert_eq!(step(&rows, Some(0), 1), Some(2));
    assert_eq!(step(&rows, Some(2), -1), Some(0));
    assert_eq!(step(&rows, Some(0), -5), Some(0));
    let last = rows.len() - 1;
    assert_eq!(step(&rows, Some(last), 3), Some(last));
    assert_eq!(nearest(&rows, 1), Some(2));
    // Files.
    let second = rows.iter().position(|r| *r == Row::File(1)).unwrap();
    assert_eq!(next_file(&rows, Some(3), true), Some(second));
    assert_eq!(next_file(&rows, Some(second), true), Some(second));
    assert_eq!(next_file(&rows, Some(second + 2), false), Some(second));
    assert_eq!(next_file(&rows, Some(second), false), Some(0));
    // Keeping a row in view.
    assert_eq!(reveal(0, 3, 10), 0);
    assert_eq!(reveal(5, 3, 10), 3);
    assert_eq!(reveal(0, 12, 10), 3);
    assert_eq!(clamp_top(50, 20, 10), 10);
    assert_eq!(clamp_top(3, 5, 10), 0);
    // Pointer rows.
    assert_eq!(row_at(4, 20, 100.0, 20.0, 99.0), None);
    assert_eq!(row_at(4, 20, 100.0, 20.0, 100.0), Some(4));
    assert_eq!(row_at(4, 20, 100.0, 20.0, 145.0), Some(6));
    assert_eq!(row_at(4, 6, 100.0, 20.0, 145.0), None);
}

// ---- the panel ----

fn snapshot(text: &str) -> Snapshot {
    Snapshot {
        root: PathBuf::from("/repo"),
        name: "repo".into(),
        branch: Some("main".into()),
        diff: parsed(text),
    }
}

fn loaded(text: &str) -> ReviewPanel {
    let mut panel = ReviewPanel::default();
    panel.open();
    let generation = panel.begin_refresh().unwrap();
    assert!(!panel.finish_refresh(generation, Ok(snapshot(text))));
    panel
}

const NONE: ModifiersState = ModifiersState::empty();

fn named(panel: &mut ReviewPanel, key: NamedKey) -> PanelCmd {
    panel.key(&Key::Named(key), None, None, NONE, 5)
}

fn letter(panel: &mut ReviewPanel, c: &str, physical: Option<KeyCode>) -> PanelCmd {
    panel.key(&Key::Character(SmolStr::new(c)), physical, Some(c), NONE, 5)
}

#[test]
fn keys_move_the_selection_and_files() {
    let mut panel = loaded(TWO_FILES);
    assert_eq!(named(&mut panel, NamedKey::ArrowDown), PanelCmd::Redraw);
    assert_eq!(panel.selected(), Some(0));
    named(&mut panel, NamedKey::ArrowDown);
    assert_eq!(
        panel.rows()[panel.selected().unwrap()],
        Row::Line(LineRef {
            file: 0,
            hunk: 0,
            line: 0
        })
    );
    letter(&mut panel, "n", Some(KeyCode::KeyN));
    assert_eq!(panel.rows()[panel.selected().unwrap()], Row::File(1));
    // An Arabic layout types `ح` on the P key: the physical key still means "previous file".
    letter(&mut panel, "ح", Some(KeyCode::KeyP));
    assert_eq!(panel.rows()[panel.selected().unwrap()], Row::File(0));
    // Collapse with Enter on a file row, expand with Right.
    named(&mut panel, NamedKey::Enter);
    assert_eq!(panel.rows()[1], Row::File(1));
    named(&mut panel, NamedKey::ArrowRight);
    assert_eq!(panel.rows()[1], Row::Hunk(0, 0));
    // Page down scrolls and keeps the selection on a selectable row in view.
    named(&mut panel, NamedKey::PageDown);
    let sel = panel.selected().unwrap();
    assert!(panel.rows()[sel].selectable());
    assert!(sel >= panel.top() && sel < panel.top() + 5);
    named(&mut panel, NamedKey::End);
    assert_eq!(panel.selected(), Some(panel.rows().len() - 1));
    named(&mut panel, NamedKey::Home);
    assert_eq!((panel.selected(), panel.top()), (Some(0), 0));
    assert_eq!(
        letter(&mut panel, "r", Some(KeyCode::KeyR)),
        PanelCmd::Refresh
    );
    assert_eq!(named(&mut panel, NamedKey::Escape), PanelCmd::Unfocus);
}

#[test]
fn a_comment_is_typed_submitted_and_cancelled() {
    let mut panel = loaded(TWO_FILES);
    // Select the added `let b = 3;` line (new line 3).
    for _ in 0..5 {
        named(&mut panel, NamedKey::ArrowDown);
    }
    let sel = panel.selected().unwrap();
    assert_eq!(
        panel.rows()[sel],
        Row::Line(LineRef {
            file: 0,
            hunk: 0,
            line: 3
        })
    );
    assert_eq!(
        letter(&mut panel, "c", Some(KeyCode::KeyC)),
        PanelCmd::Redraw
    );
    assert!(panel.is_editing());
    assert_eq!(panel.rows()[sel + 1], Row::CommentInput);
    // Letters now type; Enter with nothing typed does nothing.
    assert_eq!(named(&mut panel, NamedKey::Enter), PanelCmd::None);
    letter(&mut panel, "n", Some(KeyCode::KeyN));
    panel.text_input("ame it\u{202e} بشكل أوضح\n");
    named(&mut panel, NamedKey::Backspace);
    assert_eq!(panel.comment_text(), Some("name it بشكل أوضح"));
    let PanelCmd::Submit(target) = named(&mut panel, NamedKey::Enter) else {
        panic!("expected a submit");
    };
    assert_eq!(target.path, "src/main.rs");
    assert_eq!((target.line, target.kind), (3, LineKind::Add));
    assert_eq!(target.code, "let b = 3;");
    assert_eq!(target.comment, "name it بشكل أوضح");
    panel.comment_sent(Some("claude"));
    assert!(!panel.is_editing());
    assert!(panel.notice().unwrap().text.contains("claude"));

    // No agent: the box stays and Enter then asks to start one.
    letter(&mut panel, "c", Some(KeyCode::KeyC));
    panel.text_input("why?");
    assert!(matches!(
        named(&mut panel, NamedKey::Enter),
        PanelCmd::Submit(_)
    ));
    panel.offer_agent();
    assert!(matches!(
        named(&mut panel, NamedKey::Enter),
        PanelCmd::StartAgent(_)
    ));
    // Esc cancels.
    assert_eq!(named(&mut panel, NamedKey::Escape), PanelCmd::Redraw);
    assert!(!panel.is_editing());
    assert!(!panel.rows().contains(&Row::CommentInput));
}

#[test]
fn deleted_lines_comment_on_their_old_number() {
    let mut panel = loaded(TWO_FILES);
    let del = panel
        .rows()
        .iter()
        .position(|r| {
            *r == Row::Line(LineRef {
                file: 0,
                hunk: 1,
                line: 1,
            })
        })
        .unwrap();
    // A click on the gutter opens the box.
    assert_eq!(
        panel.click(
            PanelHit::Row {
                index: del,
                gutter: true
            },
            5
        ),
        PanelCmd::Redraw
    );
    panel.text_input("keep this");
    let PanelCmd::Submit(target) = named(&mut panel, NamedKey::Enter) else {
        panic!("expected a submit");
    };
    assert_eq!((target.line, target.kind), (21, LineKind::Del));
    assert_eq!(
        format_comment(&target),
        "src/main.rs:21: keep this (removed line: \"two\") "
    );
}

#[test]
fn comment_length_is_bounded() {
    let mut panel = loaded(TWO_FILES);
    named(&mut panel, NamedKey::ArrowDown);
    named(&mut panel, NamedKey::ArrowDown);
    letter(&mut panel, "c", Some(KeyCode::KeyC));
    panel.text_input(&"x".repeat(MAX_COMMENT_CHARS + 50));
    assert_eq!(
        panel.comment_text().unwrap().chars().count(),
        MAX_COMMENT_CHARS
    );
    assert_eq!(panel.text_input("more"), PanelCmd::None);
}

#[test]
fn a_refresh_waits_for_the_comment_and_keeps_the_selection() {
    let mut panel = loaded(TWO_FILES);
    for _ in 0..3 {
        named(&mut panel, NamedKey::ArrowDown);
    }
    let before = panel.rows()[panel.selected().unwrap()];
    letter(&mut panel, "c", Some(KeyCode::KeyC));
    let g = panel.begin_refresh().unwrap();
    // A second refresh while one runs is merged into a rerun.
    assert_eq!(panel.begin_refresh(), None);
    let changed = TWO_FILES
        .replace("-old\n+new\n", "-old\n+newer\n+again\n")
        .replace("@@ -1 +1 @@", "@@ -1 +1,2 @@");
    assert!(panel.finish_refresh(g, Ok(snapshot(&changed))));
    // Not applied while typing.
    assert_eq!(panel.snapshot().unwrap().diff.files[1].added, 1);
    named(&mut panel, NamedKey::Escape);
    assert_eq!(panel.snapshot().unwrap().diff.files[1].added, 2);
    assert_eq!(panel.rows()[panel.selected().unwrap()], before);
    // A stale result is ignored.
    let g2 = panel.begin_refresh().unwrap();
    assert!(!panel.finish_refresh(g2 - 1, Err(LoadError::NotRepo)));
    assert!(!panel.finish_refresh(g2, Err(LoadError::NotRepo)));
    assert_eq!(panel.load_state(), Load::NotRepo);
    assert!(panel.rows().is_empty());
}

#[test]
fn clicks_and_wheel() {
    let mut panel = loaded(TWO_FILES);
    assert_eq!(panel.click(PanelHit::Close, 5), PanelCmd::Close);
    assert_eq!(panel.click(PanelHit::Commit, 5), PanelCmd::Commit);
    assert_eq!(panel.click(PanelHit::Refresh, 5), PanelCmd::Refresh);
    // Clicking outside gives the keyboard back.
    assert_eq!(panel.click(PanelHit::Outside, 5), PanelCmd::Redraw);
    assert!(!panel.is_focused());
    assert_eq!(
        panel.key(&Key::Named(NamedKey::ArrowDown), None, None, NONE, 5),
        PanelCmd::None
    );
    panel.click(PanelHit::Inside, 5);
    assert!(panel.is_focused());
    // A file row click collapses it.
    panel.click(
        PanelHit::Row {
            index: 0,
            gutter: false,
        },
        5,
    );
    assert_eq!(panel.rows()[1], Row::File(1));
    panel.click(
        PanelHit::Row {
            index: 0,
            gutter: false,
        },
        5,
    );
    // The wheel scrolls by whole rows, summing small deltas.
    assert!(!panel.wheel(-0.5, 3));
    assert!(panel.wheel(-0.6, 3));
    assert_eq!(panel.top(), 1);
    assert!(panel.wheel(-100.0, 3));
    assert_eq!(panel.top(), panel.rows().len() - 3);
    assert!(panel.wheel(100.0, 3));
    assert_eq!(panel.top(), 0);
}

#[test]
fn the_view_shows_the_visible_rows() {
    let mut panel = loaded(TWO_FILES);
    named(&mut panel, NamedKey::ArrowDown);
    let view = panel.view(Rect::new(0.0, 0.0, 400.0, 300.0), 4);
    assert_eq!(view.title, "repo");
    assert_eq!(view.branch.as_deref(), Some("main"));
    assert_eq!(view.summary, "2 files / 2 ملفات");
    assert_eq!(view.totals, Some((3, 3)));
    assert_eq!(view.gutter_cols, 7);
    assert_eq!(view.rows.len(), 4);
    assert_eq!(
        view.rows[0].style,
        RowStyle::File {
            collapsed: false,
            status: FileStatus::Modified
        }
    );
    assert_eq!(view.rows[0].text, "src/main.rs");
    assert_eq!(view.rows[0].counts, Some((2, 2)));
    assert!(view.rows[0].selected);
    assert_eq!(view.rows[2].gutter, "  1   1");
    assert_eq!(view.rows[3].style, RowStyle::Del);
    assert_eq!(view.rows[3].text, "-let a = 1;");
    assert_eq!(view.rows[3].gutter, "  2    ");

    let mut empty = ReviewPanel::default();
    empty.open();
    let view = empty.view(Rect::new(0.0, 0.0, 400.0, 300.0), 4);
    assert_eq!(view.rows[0].style, RowStyle::Message);
    let clean = loaded("");
    let view = clean.view(Rect::new(0.0, 0.0, 400.0, 300.0), 4);
    assert!(view.rows[0].text.starts_with("No uncommitted changes"));
}

#[test]
fn display_text_expands_tabs_and_shows_hidden_controls() {
    assert_eq!(display_text("\tx", 100), "    x");
    assert_eq!(display_text("ab\tc", 100), "ab  c");
    assert_eq!(display_text("a\u{202e}b\x1bc", 100), "a\u{fffd}b\u{fffd}c");
    assert_eq!(display_text("abcdef", 3), "abc");
    assert_eq!(display_text("中文字", 4), "中文");
}

// ---- comments and agents ----

fn target(comment: &str, code: &str) -> CommentTarget {
    CommentTarget {
        path: "src/lib.rs".into(),
        line: 7,
        kind: LineKind::Add,
        code: code.into(),
        comment: comment.into(),
    }
}

#[test]
fn comments_are_one_clean_line() {
    assert_eq!(
        format_comment(&target("use a match here", "if x {\tfoo()")),
        "src/lib.rs:7: use a match here (line: \"if x { foo()\") "
    );
    // Control characters, escape sequences, bidi overrides and line breaks are gone.
    let nasty = target(
        "run\x1b[201~\r\nrm -rf /\u{202e}\u{2066}x\u{2028}y",
        "code\u{202e}\n",
    );
    let text = format_comment(&nasty);
    assert!(!text.contains(['\n', '\r', '\x1b', '\u{202e}', '\u{2066}', '\u{2028}']));
    assert!(
        text.starts_with("src/lib.rs:7: run[201~  rm -rf /xy"),
        "{text}"
    );
    // Bounded.
    let long = format_comment(&target(&"ب".repeat(10_000), &"x".repeat(10_000)));
    assert!(long.chars().count() < MAX_COMMENT_CHARS + 300);
    assert_eq!(clean_comment("  hi\tthere \n"), "hi there");
    assert_eq!(format_comment(&target("   ", "x")), "");
    assert_eq!(format_comment(&target("ok", "")), "src/lib.rs:7: ok ");
}

#[test]
fn inserted_text_never_contains_a_newline() {
    for comment in ["a\nb", "x\r\ny", "\u{85}z", "ok"] {
        let text = format_comment(&target(comment, "line\n"));
        for bracketed in [true, false] {
            let bytes = encode_insert(&text, bracketed);
            assert!(
                !bytes.contains(&b'\n') && !bytes.contains(&b'\r'),
                "{bytes:?}"
            );
        }
        let bytes = encode_insert(&text, true);
        assert!(bytes.starts_with(b"\x1b[200~") && bytes.ends_with(b"\x1b[201~"));
        // The only escapes are the paste brackets.
        assert_eq!(bytes.iter().filter(|&&b| b == 0x1b).count(), 2);
    }
}

fn info(id: u64) -> PaneInfo<'static> {
    PaneInfo {
        id: Some(PaneId(id)),
        ..PaneInfo::default()
    }
}

#[test]
fn the_agent_pane_is_chosen_from_the_tab() {
    let shell = PaneInfo {
        program: Some("bash"),
        ..info(1)
    };
    let claude = PaneInfo {
        program: Some("claude"),
        ..info(2)
    };
    let codex_in_shell = PaneInfo {
        program: Some("zsh"),
        running_command: Some("FOO=1 npx -y codex --model x"),
        ..info(3)
    };
    let hooked = PaneInfo {
        agent_status: true,
        ..info(4)
    };
    assert_eq!(choose_agent_pane(&[shell], &[]), None);
    assert_eq!(choose_agent_pane(&[shell, claude], &[]), Some(PaneId(2)));
    // The focused pane wins when it is an agent.
    let focused_codex = PaneInfo {
        focused: true,
        ..codex_in_shell
    };
    assert_eq!(
        choose_agent_pane(&[shell, claude, focused_codex], &[]),
        Some(PaneId(3))
    );
    // A focused shell does not.
    let focused_shell = PaneInfo {
        focused: true,
        ..shell
    };
    assert_eq!(
        choose_agent_pane(&[focused_shell, hooked], &[]),
        Some(PaneId(4))
    );
    // The configured agent counts too; unrelated commands do not.
    let custom = PaneInfo {
        program: Some("/opt/bin/my-agent"),
        ..info(5)
    };
    assert_eq!(choose_agent_pane(&[custom], &[]), None);
    assert_eq!(choose_agent_pane(&[custom], &["my-agent"]), Some(PaneId(5)));
    assert!(runs_agent("/usr/local/bin/claude --resume", &[]));
    assert!(runs_agent("env gemini", &[]));
    assert!(!runs_agent("vim claude.md", &[]));
    assert!(!runs_agent("", &[]));
}

// ---- layout ----

#[test]
fn the_panel_takes_the_right_of_the_panes() {
    let content = Rect::new(0.0, 30.0, 1200.0, 800.0);
    let (panes, panel) = split_content(content, 1200.0, 1.0);
    let panel = panel.unwrap();
    assert_eq!(panel.w, 540.0);
    assert_eq!(panel.right(), 1200.0);
    assert_eq!(panes.w + panel.w, 1200.0);
    assert_eq!((panel.y, panel.h), (30.0, 800.0));
    // At least 360 px…
    let (_, panel) = split_content(Rect::new(0.0, 0.0, 700.0, 500.0), 700.0, 1.0);
    assert_eq!(panel.unwrap().w, 360.0);
    // …scaled, while leaving the panes 160 px.
    let (panes, panel) = split_content(Rect::new(0.0, 0.0, 1000.0, 500.0), 1000.0, 2.0);
    assert_eq!(panel.unwrap().w, 680.0);
    assert_eq!(panes.w, 320.0);
    // No room: no panel.
    let content = Rect::new(0.0, 0.0, 300.0, 500.0);
    assert_eq!(split_content(content, 300.0, 1.0), (content, None));
}

#[test]
fn panel_hits() {
    let layout = PanelLayout::new(
        Rect::new(500.0, 0.0, 400.0, 400.0),
        20.0,
        10.0,
        1.0,
        false,
        14,
    );
    assert_eq!(layout.page_rows(), 16);
    let center = |r: Rect| ((r.x + r.w / 2.0) as f64, (r.y + r.h / 2.0) as f64);
    let (x, y) = center(layout.close);
    assert_eq!(layout.hit(x, y, 0, 100, 7), PanelHit::Close);
    let (x, y) = center(layout.refresh);
    assert_eq!(layout.hit(x, y, 0, 100, 7), PanelHit::Refresh);
    let (x, y) = center(layout.commit);
    assert_eq!(layout.hit(x, y, 0, 100, 7), PanelHit::Commit);
    assert!(layout.close.right() <= layout.panel.right());
    assert!(layout.title.right() <= layout.refresh.x);
    assert_eq!(layout.hit(100.0, 100.0, 0, 100, 7), PanelHit::Outside);
    let row_y = (layout.body.y + 2.5 * 20.0) as f64;
    assert_eq!(
        layout.hit((layout.gutter_left() + 5.0) as f64, row_y, 10, 100, 7),
        PanelHit::Row {
            index: 12,
            gutter: true
        }
    );
    assert_eq!(
        layout.hit((layout.text_left(7) + 5.0) as f64, row_y, 10, 100, 7),
        PanelHit::Row {
            index: 12,
            gutter: false
        }
    );
    // Past the last row.
    assert_eq!(
        layout.hit((layout.text_left(7) + 5.0) as f64, row_y, 0, 2, 7),
        PanelHit::Inside
    );
    // A footer takes room from the body.
    let with_footer = PanelLayout::new(layout.panel, 20.0, 10.0, 1.0, true, 14);
    assert!(with_footer.body.h < layout.body.h);
    assert!(with_footer.footer.unwrap().bottom() <= layout.panel.bottom() + 0.01);
}

#[test]
fn review_header_and_counts_do_not_overlap_at_360_logical_pixels() {
    for scale in [1.0, 1.25, 2.0] {
        let rect = Rect::new(0.0, 0.0, 360.0 * scale, 300.0 * scale);
        let layout = PanelLayout::new(rect, 20.0 * scale, 10.0 * scale, scale, false, 14);
        let (repo, branch) = layout.repository_and_branch(300.0 * scale, 500.0 * scale);
        assert!(repo.right() < branch.x && branch.right() < layout.refresh.x);
        assert!(layout.refresh.right() < layout.close.x);
        assert!(layout.summary.right() < layout.commit.x);
        assert!(layout.close.bottom() <= layout.summary.y);
        assert!(layout.commit.right() < rect.right());
        assert!(layout.commit.bottom() < layout.body.y);
    }
}

// ---- shortcut and command ----

#[test]
fn review_toggle_is_a_command_with_a_shortcut() {
    assert_eq!(
        commands::action("review.toggle"),
        Some(InputAction::ToggleReview)
    );
    let command = commands::all()
        .into_iter()
        .find(|c| c.name == "review.toggle")
        .unwrap();
    assert_eq!(command.title_en, "Review changes");
    assert_eq!(command.title_ar, "مراجعة التغييرات");
    assert_eq!(command.shortcut, Some("Ctrl+Shift+R"));
    let pressed = handle_key_mode_raw(
        Some(KeyCode::KeyR),
        &Key::Dead(None),
        None,
        None,
        winit::event::ElementState::Pressed,
        false,
        ModifiersState::CONTROL | ModifiersState::SHIFT,
        alacritty_terminal::term::TermMode::empty(),
    );
    assert_eq!(pressed, Some(InputAction::ToggleReview));
}

// ---- git (Unix: the scratch repositories use /dev/null for config and hooks) ----

#[cfg(unix)]
mod git {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use twitty::review::diff::Untracked;
    use twitty::review::git::{load, read_untracked};
    use twitty::review::{FileStatus, LineKind, LoadError};

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "twitty-review-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn run_git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(status.status.success(), "git {args:?}: {status:?}");
    }

    #[test]
    fn loads_a_repository_with_changes_renames_and_untracked_files() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new("load");
        let repo = &tmp.0;
        run_git(repo, &["init", "-q"]);
        std::fs::write(repo.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(repo.join("move.txt"), "a\nb\nc\nd\ne\nf\n").unwrap();
        std::fs::write(repo.join(".gitignore"), "ignored.log\n").unwrap();
        run_git(repo, &["add", "."]);
        run_git(repo, &["commit", "-q", "-m", "first"]);
        std::fs::write(repo.join("a.txt"), "one\nاتنين\nthree\n").unwrap();
        run_git(repo, &["mv", "move.txt", "moved.txt"]);
        std::fs::write(repo.join("new.md"), "مرحبا\n").unwrap();
        std::fs::write(repo.join("ignored.log"), "x\n").unwrap();
        std::fs::create_dir(repo.join("sub")).unwrap();

        let snap = load(&repo.join("sub")).unwrap_or_else(|e| panic!("load failed: {e:?}"));
        assert_eq!(
            snap.root.canonicalize().unwrap(),
            repo.canonicalize().unwrap()
        );
        assert_eq!(snap.branch.as_deref(), Some("main"));
        let paths: Vec<_> = snap
            .diff
            .files
            .iter()
            .map(|f| f.path().to_owned())
            .collect();
        assert!(paths.contains(&"a.txt".to_owned()), "{paths:?}");
        assert!(paths.contains(&"moved.txt".to_owned()), "{paths:?}");
        assert!(paths.contains(&"new.md".to_owned()), "{paths:?}");
        assert!(!paths.iter().any(|p| p == "ignored.log"));
        let a = snap
            .diff
            .files
            .iter()
            .find(|f| f.path() == "a.txt")
            .unwrap();
        let added = a.hunks[0]
            .lines
            .iter()
            .find(|l| l.kind == LineKind::Add)
            .unwrap();
        assert_eq!((added.text.as_str(), added.new), ("اتنين", Some(2)));
        let moved = snap
            .diff
            .files
            .iter()
            .find(|f| f.path() == "moved.txt")
            .unwrap();
        assert_eq!(moved.status, FileStatus::Renamed);
        assert_eq!(moved.old_path.as_deref(), Some("move.txt"));
        let new = snap
            .diff
            .files
            .iter()
            .find(|f| f.path() == "new.md")
            .unwrap();
        assert!(new.untracked);
        assert_eq!(new.hunks[0].lines[0].text, "مرحبا");
    }

    #[test]
    fn a_repository_without_commits_and_a_plain_folder() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new("fresh");
        run_git(&tmp.0, &["init", "-q"]);
        std::fs::write(tmp.0.join("first.txt"), "hello\n").unwrap();
        let snap = load(&tmp.0).unwrap();
        assert_eq!(snap.diff.files.len(), 1);
        assert!(snap.diff.files[0].untracked);

        let plain = TempDir::new("plain");
        // A folder inside no repository (temp dirs normally are not).
        if load(&plain.0).is_err() {
            assert_eq!(load(&plain.0), Err(LoadError::NotRepo));
        }
    }

    #[test]
    fn untracked_reads_never_follow_links_or_open_fifos() {
        let tmp = TempDir::new("untracked");
        let file = tmp.0.join("f.txt");
        std::fs::write(&file, "data\n").unwrap();
        assert_eq!(
            read_untracked(&file, 1 << 20),
            Untracked::Text(b"data\n".to_vec())
        );
        assert_eq!(read_untracked(&file, 2), Untracked::TooBig(5));
        std::os::unix::fs::symlink(&file, tmp.0.join("link")).unwrap();
        assert_eq!(
            read_untracked(&tmp.0.join("link"), 1 << 20),
            Untracked::Special
        );
        let fifo = tmp.0.join("fifo");
        if Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .is_ok_and(|s| s.success())
        {
            assert_eq!(read_untracked(&fifo, 1 << 20), Untracked::Special);
        }
        assert_eq!(
            read_untracked(&tmp.0.join("missing"), 10),
            Untracked::Unreadable
        );
    }
}

// ---- drawing ----

fn metrics() -> CellMetrics {
    CellMetrics {
        font_size: 15.0,
        scale: 1.0,
        char_width: 9.0,
        line_height: 20.0,
        padding_left: 8.0,
        padding_top: 6.0,
    }
}

#[test]
fn the_panel_draws_inside_its_rect_with_arabic_shaped_like_the_terminal() {
    let text = "\
diff --git a/ar.txt b/ar.txt
--- a/ar.txt
+++ b/ar.txt
@@ -1 +1,2 @@
-old
+مرحبا بالعالم
+┌──┐ box
";
    let mut panel = loaded(text);
    named(&mut panel, NamedKey::ArrowDown);
    panel.warn("careful / خلي بالك");
    let rect = Rect::new(400.0, 0.0, 400.0, 300.0);
    let view = panel.view(rect, 10);
    for palette in [Palette::default(), light_palette()] {
        let mut fonts = FontSystem::new();
        let mut ctx = DrawContext {
            font_system: &mut fonts,
            family: None,
            palette: &palette,
            metrics: metrics(),
            is_srgb: false,
        };
        let mut cache = ReviewCache::default();
        let font = UiFont::default();
        let style = UiStyle::new(&palette, None, 1.0);
        let scene = build(
            &mut ctx,
            &view,
            &mut cache,
            style,
            &font,
            UiLanguage::English,
        );
        assert!(!scene.quads.is_empty());
        for &(x, y, w, h, _) in &scene.quads {
            assert!(x >= rect.x - 0.01 && y >= rect.y - 0.01, "{x},{y}");
            assert!(x + w <= rect.right() + 0.01 && y + h <= rect.bottom() + 0.01);
        }
        for t in &scene.texts {
            assert!(t.bounds.left >= rect.x as i32 && t.bounds.right <= rect.right() as i32);
            assert!(t.bounds.top >= rect.y as i32 && t.bounds.bottom <= rect.bottom() as i32);
        }
        // The Arabic line went through the row shaper as right-to-left text.
        assert!(scene
            .texts
            .iter()
            .any(|t| t.row.segments.iter().any(|s| s.has_rtl)));
        let cached = cache.len();
        assert!(cached > 0);
        // A second frame reuses every label.
        let again = build(
            &mut ctx,
            &view,
            &mut cache,
            style,
            &font,
            UiLanguage::English,
        );
        assert_eq!(cache.len(), cached);
        assert_eq!(again.texts.len(), scene.texts.len());
    }
}

fn light_palette() -> Palette {
    let mut p = Palette::default();
    std::mem::swap(&mut p.background, &mut p.foreground);
    p
}

#[test]
fn review_text_is_clipped_under_the_command_palette() {
    use twitty::render::scene::frame::{build_frame, FrameOptions};
    use twitty::render::scene::palette::{PaletteItem, PaletteView};
    use twitty::render::scene::preedit::PreeditState;
    use twitty::render::RowCache;

    let panel = loaded(TWO_FILES);
    let rect = Rect::new(300.0, 0.0, 500.0, 600.0);
    let view = panel.view(rect, 20);
    let palette = Palette::default();
    let mut fonts = FontSystem::new();
    let mut ctx = DrawContext {
        font_system: &mut fonts,
        family: None,
        palette: &palette,
        metrics: metrics(),
        is_srgb: false,
    };
    let opts = FrameOptions {
        window_focused: true,
        cursor_style: "block",
        cursor_blink_on: true,
        surface: (800.0, 600.0),
    };
    let mut rows = RowCache::default();
    let mut ime = PreeditState::default();
    let mut frame = build_frame(&mut ctx, &opts, &mut rows, &mut ime, &[], &[], &[]);
    let mut cache = ReviewCache::default();
    frame.add_review(
        &mut ctx,
        &view,
        &mut cache,
        UiStyle::new(&palette, None, 1.0),
        &UiFont::default(),
        UiLanguage::English,
    );
    let without = frame.text_runs(&[], &rows, &ime).len();
    assert!(without > 0);
    let style = twitty::render::ui_style::UiStyle::new(&palette, None, ctx.metrics.scale);
    let palette_view = PaletteView {
        query: String::new(),
        items: vec![PaletteItem {
            title_en: "Review changes".into(),
            title_ar: "راجع التغييرات".into(),
            shortcut: Some("Ctrl+Shift+R".into()),
            dimmed: false,
        }],
        selected: 0,
        placeholder: None,
    };
    frame.add_palette(
        &mut ctx,
        style,
        &twitty::render::text::ui::UiFont::default(),
        &palette_view,
    );
    let panel_rect = twitty::render::scene::palette::PaletteLayout::new(
        ctx.metrics.scale,
        (800.0, 600.0),
        palette_view.items.len(),
    )
    .panel;
    let runs = frame.text_runs(&[], &rows, &ime);
    // Review text never draws inside the palette (only the palette's own labels do).
    let inside = |b: &glyphon::TextBounds| {
        b.left < panel_rect.right() as i32
            && b.right > panel_rect.x as i32
            && b.top < panel_rect.bottom() as i32
            && b.bottom > panel_rect.y as i32
    };
    let review_inside = runs
        .iter()
        // The palette's own labels come last: the query row plus five per item.
        .take(runs.len() - (1 + palette_view.items.len() * 5))
        .filter(|r| inside(&r.bounds))
        .count();
    assert_eq!(review_inside, 0);
}

#[test]
fn review_panel_stage_and_unstage_keys_work_on_latin_and_arabic_layouts() {
    let mut panel = ReviewPanel::default();
    panel.open();
    let gen = panel.begin_refresh().unwrap();
    let diff = parse(
        b"diff --git a/src/app.rs b/src/app.rs\n--- a/src/app.rs\n+++ b/src/app.rs\n@@ -1,1 +1,2 @@\n fn main() {}\n+// added\n",
        false,
        &Limits::default(),
    );
    panel.finish_refresh(
        gen,
        Ok(Snapshot {
            root: PathBuf::from("/tmp/repo"),
            name: "repo".into(),
            branch: Some("main".into()),
            diff,
        }),
    );

    // Select the first row (file header), then test Latin 's' and Arabic physical KeyS ('س').
    assert_eq!(
        panel.key(
            &Key::Named(NamedKey::ArrowDown),
            None,
            None,
            ModifiersState::empty(),
            20,
        ),
        PanelCmd::Redraw
    );
    let cmd = panel.key(
        &Key::Character("s".into()),
        Some(KeyCode::KeyS),
        Some("s"),
        ModifiersState::empty(),
        20,
    );
    assert_eq!(cmd, PanelCmd::StageFile("src/app.rs".into()));

    let cmd_ar = panel.key(
        &Key::Character("س".into()),
        Some(KeyCode::KeyS),
        Some("س"),
        ModifiersState::empty(),
        20,
    );
    assert_eq!(cmd_ar, PanelCmd::StageFile("src/app.rs".into()));

    // Latin 'u' and Arabic physical KeyU ('ع') unstage selected file.
    let cmd_u = panel.key(
        &Key::Character("ع".into()),
        Some(KeyCode::KeyU),
        Some("ع"),
        ModifiersState::empty(),
        20,
    );
    assert_eq!(cmd_u, PanelCmd::UnstageFile("src/app.rs".into()));

    // Path traversal / absolute paths are rejected by stage_file and unstage_file.
    assert!(twitty::review::git::stage_file(std::path::Path::new("/tmp"), "../escape").is_err());
    assert!(
        twitty::review::git::unstage_file(std::path::Path::new("/tmp"), "/etc/passwd").is_err()
    );
    assert!(twitty::review::git::stage_file(std::path::Path::new("/tmp"), "").is_err());

    // Pressing 'a' (Arabic KeyA 'ش') hands off a draft prompt to the built-in Agent panel.
    let cmd_agent = panel.key(
        &Key::Character("ش".into()),
        Some(KeyCode::KeyA),
        Some("ش"),
        ModifiersState::empty(),
        20,
    );
    assert_eq!(
        cmd_agent,
        PanelCmd::SendToAgentPanel("Review changes in src/app.rs: ".into())
    );

    // Move to the first diff line and test Shift+S / Shift+U hunk-level patch staging.
    assert_eq!(
        panel.key(
            &Key::Named(NamedKey::ArrowDown),
            None,
            None,
            ModifiersState::empty(),
            20,
        ),
        PanelCmd::Redraw
    );
    let hunk_stage = panel.key(
        &Key::Character("S".into()),
        Some(KeyCode::KeyS),
        Some("S"),
        ModifiersState::SHIFT,
        20,
    );
    let PanelCmd::StageHunk(patch) = hunk_stage else {
        panic!("expected PanelCmd::StageHunk, got {hunk_stage:?}");
    };
    assert!(patch.contains("--- a/src/app.rs\n+++ b/src/app.rs\n"));
    assert!(patch.contains("@@ -1,1 +1,2 @@\n"));
    assert!(patch.contains("+// added\n"));

    let hunk_unstage = panel.key(
        &Key::Character("U".into()),
        Some(KeyCode::KeyU),
        Some("U"),
        ModifiersState::SHIFT,
        20,
    );
    assert_eq!(hunk_unstage, PanelCmd::UnstageHunk(patch));

    // Verify ssh.reconnect command registration and RemotePane manual reconnect reset.
    assert_eq!(
        commands::action("ssh.reconnect"),
        Some(InputAction::ReconnectSsh)
    );
    let target = twitty::remote::Target::new("user@example.com", "dev").unwrap();
    let mut remote = twitty::remote::RemotePane::new(target);
    let _ = remote.disconnected(false);
    assert!(remote.waiting_for_enter);
    remote.reset_for_manual_reconnect();
    assert!(!remote.waiting_for_enter);
    assert_eq!(remote.state, twitty::remote::ConnectionState::Disconnected);
    assert!(remote.next_retry_deadline().is_none());
}

#[test]
fn stage_and_unstage_single_hunk_in_real_git_repo() {
    use std::process::Command;
    use twitty::review::diff::format_hunk_patch;
    use twitty::review::git::{load, stage_hunk, unstage_hunk};

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("twitty-hunk-stage-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());

    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(&dir)
            .output()
            .expect("git must be available");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    git(&["init", "-q"]);
    git(&["config", "user.email", "test@twitty.local"]);
    git(&["config", "user.name", "Twitty Test"]);

    let initial: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    std::fs::write(dir.join("notes.txt"), &initial).unwrap();
    git(&["add", "notes.txt"]);
    git(&["commit", "-q", "-m", "initial"]);

    let modified: String = (1..=20)
        .map(|i| match i {
            2 => "line 2 edited\n".to_string(),
            19 => "line 19 edited\n".to_string(),
            _ => format!("line {i}\n"),
        })
        .collect();
    std::fs::write(dir.join("notes.txt"), &modified).unwrap();

    let snapshot = load(&dir).expect("load diff from repo");
    let files = &snapshot.diff.files;
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].hunks.len(), 2);

    let patch_hunk1 = format_hunk_patch(&files[0], &files[0].hunks[0]).expect("hunk 1 patch");
    stage_hunk(&dir, &patch_hunk1).expect("stage hunk 1");

    let staged = git(&["diff", "--cached"]);
    assert!(staged.contains("+line 2 edited"), "hunk 1 must be staged");
    assert!(
        !staged.contains("+line 19 edited"),
        "hunk 2 must stay unstaged"
    );

    let unstaged = git(&["diff"]);
    assert!(
        !unstaged.contains("+line 2 edited"),
        "hunk 1 must no longer appear in unstaged diff"
    );
    assert!(
        unstaged.contains("+line 19 edited"),
        "hunk 2 must still be unstaged"
    );

    unstage_hunk(&dir, &patch_hunk1).expect("unstage hunk 1");
    let staged_after = git(&["diff", "--cached"]);
    assert!(
        staged_after.trim().is_empty(),
        "index must be clean after unstaging hunk 1, got: {staged_after}"
    );
}
