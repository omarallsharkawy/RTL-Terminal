//! Hostile program output and links: oversized OSC strings, bidi controls that would reorder a
//! line, and `file://` links that would run a program when clicked.
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::Event;
use twitty::links::{display_target, file_link_target};
use twitty::terminal::{Terminal, MAX_OSC_LEN};

fn clipboard_stores(chunks: &[&[u8]]) -> (Vec<String>, Terminal) {
    let stored = Arc::new(Mutex::new(Vec::new()));
    let sink = stored.clone();
    let mut term = Terminal::with_events(40, 4, move |e| {
        if let Event::ClipboardStore(_, text) = e {
            sink.lock().unwrap().push(text);
        }
    });
    for chunk in chunks {
        term.process_bytes(chunk);
    }
    let stored = stored.lock().unwrap().clone();
    (stored, term)
}

fn row_text(term: &Terminal, row: usize) -> String {
    let (lines, _) = term.snapshot();
    let mut s = String::new();
    for cell in lines[row].cells.iter() {
        cell.push_text(&mut s);
    }
    s.trim_end().to_string()
}

#[test]
fn a_normal_osc52_copy_still_works() {
    // "aGk=" is base64 for "hi".
    let (stored, _) = clipboard_stores(&[b"\x1b]52;c;aGk=\x07"]);
    assert_eq!(stored, vec!["hi".to_string()]);
}

#[test]
fn an_osc_split_across_reads_still_works() {
    let (stored, _) = clipboard_stores(&[b"\x1b]5", b"2;c;aG", b"k=\x1b", b"\\"]);
    assert_eq!(stored, vec!["hi".to_string()]);
}

#[test]
fn an_oversized_osc_is_cut_and_ignored_and_output_continues() {
    let mut payload = b"\x1b]52;c;".to_vec();
    payload.extend(std::iter::repeat_n(b'A', MAX_OSC_LEN + 4096));
    let (stored, term) = clipboard_stores(&[&payload, b"\x07after"]);
    assert!(stored.is_empty(), "a cut copy must not reach the clipboard");
    assert_eq!(row_text(&term, 0), "after");
}

#[test]
fn bidi_overrides_in_output_are_not_shaped() {
    let mut term = Terminal::new(40, 4, |_| {});
    term.process_bytes("abc\u{202E}def \u{2067}ghi".as_bytes());
    assert_eq!(row_text(&term, 0), "abcdef ghi");
}

#[test]
fn file_links_to_other_hosts_are_refused() {
    assert_eq!(
        file_link_target("file://attacker.example/share/x.txt"),
        None
    );
    assert_eq!(file_link_target("file:relative"), None);
    assert!(file_link_target("file:///tmp/notes.txt").is_some());
    assert!(file_link_target("file://localhost/tmp/notes.txt").is_some());
}

#[test]
fn file_links_are_percent_decoded() {
    assert_eq!(
        file_link_target("file:///tmp/a%20b.txt?x#y"),
        Some("/tmp/a b.txt".into())
    );
    assert_eq!(file_link_target("file:///tmp/a%00b"), None);
    assert_eq!(file_link_target("file:///tmp/a%zz"), None);
}

#[test]
fn launchers_open_their_folder_instead() {
    for name in ["evil.desktop", "setup.EXE", "run.sh", "x.AppImage"] {
        assert_eq!(
            file_link_target(&format!("file:///tmp/dl/{name}")),
            Some("/tmp/dl".into()),
            "{name}"
        );
    }
}

#[cfg(unix)]
#[test]
fn executable_files_open_their_folder_instead() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("twitty-link-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("innocent.txt");
    std::fs::write(&file, "#!/bin/sh\n").unwrap();
    let url = format!("file://{}", file.display());
    assert_eq!(file_link_target(&url), Some(file.clone()));
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(file_link_target(&url), Some(dir.clone()));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn hovered_link_targets_are_shown_without_bidi_tricks() {
    assert_eq!(
        display_target("https://evil.example/\u{202E}moc.knab"),
        "https://evil.example/\u{FFFD}moc.knab"
    );
    let long = format!("https://x.example/{}", "a".repeat(500));
    assert!(display_target(&long).ends_with('…'));
}

#[test]
fn program_titles_are_capped_and_have_no_controls() {
    use twitty::app::clean_title;
    assert_eq!(clean_title("vim\x07 \x1b[31mfile"), "vim [31mfile");
    assert_eq!(clean_title(&"x".repeat(100_000)).chars().count(), 256);
}

#[test]
fn a_cell_drowning_in_combining_marks_draws_only_a_few() {
    use twitty::terminal::MAX_DRAWN_MARKS;
    let mut term = Terminal::new(40, 4, |_| {});
    let zalgo: String = std::iter::once('a')
        .chain(std::iter::repeat_n('\u{0301}', 100_000))
        .collect();
    term.process_bytes(zalgo.as_bytes());
    term.process_bytes("b".as_bytes());
    let (lines, _) = term.snapshot();
    assert_eq!(lines[0].cells[0].zerowidth.len(), MAX_DRAWN_MARKS);
    assert_eq!(lines[0].cells[1].c, 'b');
}

#[test]
fn mailto_links_cannot_attach_files() {
    use twitty::links::is_allowed_scheme;
    assert!(is_allowed_scheme("mailto:me@example.com?subject=hi"));
    assert!(!is_allowed_scheme(
        "mailto:x@example.com?attach=/home/u/.ssh/id_rsa"
    ));
    assert!(!is_allowed_scheme(
        "mailto:x@example.com?ATTACHMENT=%2Fetc%2Fpasswd"
    ));
}

#[test]
fn a_wide_character_in_the_narrowest_grid_does_not_crash() {
    for (cols, rows) in [(0, 0), (1, 1), (1, 3)] {
        let mut term = Terminal::new(cols, rows, |_| {});
        term.process_bytes("你好😀".as_bytes());
        term.resize(1, 1);
        term.process_bytes("世".as_bytes());
        let _ = term.snapshot();
    }
}
