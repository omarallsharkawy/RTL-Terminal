//! Edge cases in the terminal core: degenerate sizes and literal search.
use alacritty_terminal::index::Direction;
use twitty::terminal::Terminal;

#[test]
fn zero_sized_terminal_does_not_panic() {
    let mut term = Terminal::new(0, 0, |_| {});
    term.process_bytes(b"hello\r\nworld");
    let _ = term.snapshot();
    term.resize(0, 0);
    term.process_bytes("مرحبا".as_bytes());
    let _ = term.snapshot();
    term.resize(10, 3);
    let (lines, _) = term.snapshot();
    assert_eq!(lines.len(), 3);
}

#[test]
fn search_treats_regex_characters_literally() {
    for query in [
        "a-b", "#1", "~/x", "a&b", "[x]", "c++", "1.5", "a b", "x/y", "\"q\"", "'s", "%d", "@me",
        "a,b", "a=b", "<t>", "!x",
    ] {
        let mut term = Terminal::new(40, 3, |_| {});
        term.process_bytes(format!("zz {query} zz").as_bytes());
        assert!(
            term.search(query, Direction::Left),
            "no match for {query:?}"
        );
    }
}

mod config {
    use std::fs;
    use twitty::config::TwittyConfig;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("twitty-cfg-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn broken_config_is_never_overwritten() {
        let path = dir("broken").join("config.json");
        let broken = "{ \"font_size\": 16, \"theme\": \"dracula\", }";
        fs::write(&path, broken).unwrap();
        let mut config = TwittyConfig::load_from(&path);
        config.font_size = 20.0;
        config.save_to(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), broken);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let path = dir("range").join("config.json");
        fs::write(&path, r#"{"font_size": 0.5, "background_opacity": 7}"#).unwrap();
        let config = TwittyConfig::load_from(&path);
        assert_eq!(config.font_size, 8.0);
        assert_eq!(config.background_opacity, 1.0);
    }

    #[test]
    fn save_round_trips_and_keeps_unknown_keys() {
        let path = dir("round").join("nested").join("config.json");
        let mut config = TwittyConfig::default();
        config.extra.insert("future_key".into(), 42.into());
        config.font_size = 18.0;
        config.save_to(&path).unwrap();
        let back = TwittyConfig::load_from(&path);
        assert_eq!(back.font_size, 18.0);
        assert_eq!(back.extra.get("future_key"), Some(&42.into()));
        assert!(!path.with_extension("json.tmp").exists());
    }
}

#[test]
fn shell_does_not_inherit_another_terminals_identity() {
    use twitty::pty::INHERITED_TERMINAL_VARS as VARS;
    for key in [
        "KITTY_WINDOW_ID",
        "TMUX",
        "VTE_VERSION",
        "WEZTERM_PANE",
        "TERMINFO",
    ] {
        assert!(VARS.contains(&key), "{key}");
    }
    assert!(!VARS.contains(&"TERM_PROGRAM"));
}

#[cfg(target_os = "linux")]
#[test]
fn shell_starts_in_the_requested_directory_and_reports_it() {
    use std::time::{Duration, Instant};
    use twitty::pty::Pty;
    let dir = std::env::temp_dir().canonicalize().unwrap();
    std::env::set_var("TWITTY_SHELL", "sleep 5");
    let pty = Pty::spawn_in(Some(&dir), 80, 24, |_| {}, || {}).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while pty.cwd().as_deref() != Some(dir.as_path()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(pty.cwd(), Some(dir));
}
