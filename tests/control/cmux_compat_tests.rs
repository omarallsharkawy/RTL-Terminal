//! Window-free tests for cmux v2/v1 control protocol compatibility, headless workspace method
//! mapping, `CMUX_*` environment handling, `claude` wrapper pass-through/injection, `claude-hook`
//! JSON parsing, and per-pane sidebar agent status rendering (including Arabic text).

// Some imports are only used by the Unix-only socket tests below.
#![cfg_attr(not(unix), allow(unused_imports))]

use std::io::Cursor;
use std::path::{Path, PathBuf};

use alacritty_terminal::term::TermMode;
use cosmic_text::FontSystem;
use serde_json::{json, Value};
use twitty::color::Palette;
use twitty::config::TwittyConfig;
use twitty::control::claude_hook::{plan_hook_calls, read_bounded_stdin, MAX_HOOK_STDIN_BYTES};
use twitty::control::cli::{help_text, parse_args, CliAction};
use twitty::control::cmux::{encode_key_name, parse_v1_line, HeadlessHost, SUPPORTED_METHODS};
use twitty::control::protocol::{parse_wire_line, Request, Response};
use twitty::layout::{Rect, SidebarPosition};
use twitty::pty::{is_stripped_env_var, INHERITED_TERMINAL_VARS};
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::sidebar::{build, LabelCache};
use twitty::render::text::ui::UiFont;
use twitty::render::ui_style::UiStyle;
use twitty::render::CellMetrics;
use twitty::session::shell_setup::{install_claude_wrapper, prepend_path};
use twitty::session::spawn::cmux_env_vars;
use twitty::sidebar::cards::ScrollState;
use twitty::sidebar::{AgentState, AgentStatus};
use twitty::ui::chrome::UiLanguage;

fn s(items: &[&str]) -> Vec<String> {
    items.iter().map(|v| (*v).to_owned()).collect()
}

// ---------------------------------------------------------------------------
// 1. v2 / v1 parsing, error shapes, and unknown methods
// ---------------------------------------------------------------------------

#[test]
fn v2_and_v1_wire_parsing_and_error_shapes() {
    // Good cmux v2 requests (string id, int id, omitted params, window_id ignored).
    let req = parse_wire_line(
        r#"{"id":"req-1","method":"workspace.list","params":{},"window_id":"ignored"}"#,
    )
    .unwrap();
    assert_eq!(req.id, json!("req-1"));
    assert_eq!(req.method, "workspace.list");
    assert_eq!(req.params, json!({}));

    let req = parse_wire_line(r#"{"id":7,"method":"system.identify"}"#).unwrap();
    assert_eq!(req.id, json!(7));
    assert_eq!(req.method, "system.identify");

    // Malformed / non-object JSON errors.
    assert!(!parse_wire_line("").unwrap_err().ok);
    assert!(!parse_wire_line("{bad json").unwrap_err().ok);
    assert!(!parse_wire_line("[1,2,3]").unwrap_err().ok);
    assert!(!parse_wire_line(r#"{"id":1}"#).unwrap_err().ok);
    assert!(
        !parse_wire_line(r#"{"id":1,"method":"workspace.list","params":[]}"#)
            .unwrap_err()
            .ok
    );

    // Minimal v1 text commands for the Claude hook path.
    let v1_ping = parse_v1_line("ping").unwrap();
    assert_eq!(v1_ping.method, "system.ping");

    let v1_set =
        parse_v1_line(r#"set_status claude working "شغال على الإصلاح" --panel=3 --tab=1"#).unwrap();
    assert_eq!(v1_set.method, "workspace.set_status");
    assert_eq!(v1_set.params["key"], json!("claude"));
    assert_eq!(v1_set.params["value"], json!("working"));
    assert_eq!(v1_set.params["label"], json!("شغال على الإصلاح"));
    assert_eq!(v1_set.params["surface"], json!("3"));
    assert_eq!(v1_set.params["workspace"], json!("1"));

    let v1_clear = parse_v1_line("clear_status claude --surface=3").unwrap();
    assert_eq!(v1_clear.method, "workspace.clear_status");
    assert_eq!(v1_clear.params["key"], json!("claude"));
    assert_eq!(v1_clear.params["surface"], json!("3"));

    let v1_notify = parse_v1_line(r#"notify "Claude" "يحتاج إدخال" --panel=2"#).unwrap();
    assert_eq!(v1_notify.method, "notification.create");
    assert_eq!(v1_notify.params["title"], json!("Claude"));
    assert_eq!(v1_notify.params["body"], json!("يحتاج إدخال"));
    assert_eq!(v1_notify.params["surface"], json!("2"));

    let v1_notify_surface = parse_v1_line(r#"notify_surface 4 "Claude" "تم""#).unwrap();
    assert_eq!(v1_notify_surface.method, "notification.create");
    assert_eq!(v1_notify_surface.params["surface"], json!("4"));

    let v1_notify_target = parse_v1_line(r#"notify_target 1 4 "Claude" "جاهز""#).unwrap();
    assert_eq!(v1_notify_target.method, "notification.create");
    assert_eq!(v1_notify_target.params["workspace"], json!("1"));
    assert_eq!(v1_notify_target.params["surface"], json!("4"));

    // Unknown v1 command returns cmux error shape.
    let v1_unknown = parse_v1_line("open_browser https://example.com").unwrap_err();
    assert!(!v1_unknown.ok);
    assert_eq!(v1_unknown.error_code.as_deref(), Some("unknown_method"));

    // Wire serialization: native Twitty error stays a string; cmux error is {code, message}.
    let native_json: Value =
        serde_json::to_value(Response::err(json!(1), "native failure")).unwrap();
    assert_eq!(native_json["error"], json!("native failure"));

    let cmux_resp = Response::cmux_err(json!("2"), "unknown_method", "Unknown method: foo");
    let cmux_json: Value = serde_json::to_value(&cmux_resp).unwrap();
    assert_eq!(
        cmux_json["error"],
        json!({"code": "unknown_method", "message": "Unknown method: foo"})
    );
    // Both shapes deserialize cleanly into Response.
    let decoded: Response = serde_json::from_value(cmux_json).unwrap();
    assert_eq!(decoded, cmux_resp);
}

#[test]
fn unsupported_methods_return_cmux_method_not_found() {
    let mut host = HeadlessHost::new();
    for method in [
        "browser.open",
        "browser.navigate",
        "markdown.open",
        "settings.open",
        "settings.reload",
        "no.such.method",
    ] {
        let (resp, exit) = host.dispatch(Request {
            id: json!("u1"),
            method: method.into(),
            params: json!({}),
        });
        assert!(!exit);
        assert!(!resp.ok, "{method} should fail");
        assert_eq!(resp.error_code.as_deref(), Some("unknown_method"));
        let wire: Value = serde_json::to_value(&resp).unwrap();
        assert_eq!(wire["error"]["code"], json!("unknown_method"));
        assert!(wire["error"]["message"].as_str().unwrap().contains(method));
    }
}

// ---------------------------------------------------------------------------
// 2. Every supported cmux v2 method on HeadlessHost
// ---------------------------------------------------------------------------

#[test]
fn headless_workspace_maps_all_cmux_v2_methods() {
    let mut host = HeadlessHost::new();

    // system.ping, system.identify, system.capabilities
    let (ping, _) = host.dispatch_line(r#"{"id":"1","method":"system.ping","params":{}}"#);
    assert!(ping.ok);
    let ping_res = ping.result.unwrap();
    assert_eq!(ping_res["pong"], json!(true));
    assert!(ping_res["version"].is_string());
    assert!(ping_res["pid"].is_u64());

    let (ident, _) = host.dispatch_line(r#"{"id":"2","method":"system.identify","params":{}}"#);
    assert!(ident.ok);
    let ident_res = ident.result.unwrap();
    assert_eq!(ident_res["app"], json!("twitty"));
    assert_eq!(ident_res["protocol"], json!("cmux-v2"));
    assert_eq!(ident_res["platform"], json!(std::env::consts::OS));

    let (caps, _) = host.dispatch_line(r#"{"id":"3","method":"system.capabilities","params":{}}"#);
    assert!(caps.ok);
    let methods = caps.result.unwrap()["methods"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for expected in SUPPORTED_METHODS {
        assert!(
            methods.contains(&(*expected).to_owned()),
            "missing {expected}"
        );
    }
    assert!(!methods.iter().any(|m| m.starts_with("browser.")));
    assert!(!methods.iter().any(|m| m.starts_with("markdown.")));
    assert!(!methods.iter().any(|m| m.starts_with("settings.")));

    // workspace.list & workspace.current
    let (list, _) = host.dispatch_line(r#"{"id":4,"method":"workspace.list","params":{}}"#);
    assert!(list.ok);
    let workspaces = list.result.unwrap()["workspaces"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0]["id"], json!("1"));
    assert_eq!(workspaces[0]["selected"], json!(true));
    assert_eq!(workspaces[0]["is_selected"], json!(true));

    let (cur, _) = host.dispatch_line(r#"{"id":5,"method":"workspace.current","params":{}}"#);
    assert!(cur.ok);
    assert_eq!(cur.result.unwrap()["id"], json!("1"));

    // workspace.create preserves selection; workspace.new selects the new workspace.
    let (created, _) = host.dispatch(Request {
        id: json!(6),
        method: "workspace.create".into(),
        params: json!({"title": "عمل\u{202E} خلفي"}),
    });
    assert!(created.ok);
    let ws2_id = created.result.unwrap()["workspace_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(ws2_id, "2");
    assert_eq!(host.active_tab(), 0, "workspace.create keeps current tab");
    assert_eq!(host.tab_count(), 2);

    let (ws_new, _) = host.dispatch(Request {
        id: json!(7),
        method: "workspace.new".into(),
        params: json!({"title": "Active New"}),
    });
    assert!(ws_new.ok);
    assert_eq!(host.active_tab(), 2, "workspace.new selects new tab");

    // workspace.select by index and by stable workspace_id.
    let (sel_idx, _) = host.dispatch(Request {
        id: json!(8),
        method: "workspace.select".into(),
        params: json!({"index": 0}),
    });
    assert!(sel_idx.ok);
    assert_eq!(host.active_tab(), 0);

    let (sel_id, _) = host.dispatch(Request {
        id: json!(9),
        method: "workspace.select".into(),
        params: json!({"workspace_id": ws2_id}),
    });
    assert!(sel_id.ok);
    assert_eq!(host.active_tab(), 1);

    let (sel_bad, _) = host.dispatch(Request {
        id: json!(10),
        method: "workspace.select".into(),
        params: json!({"index": 99}),
    });
    assert!(!sel_bad.ok);
    assert_eq!(sel_bad.error_code.as_deref(), Some("not_found"));

    // workspace.rename (Arabic title, bidi override stripped).
    let (ren, _) = host.dispatch(Request {
        id: json!(11),
        method: "workspace.rename".into(),
        params: json!({"workspace": "2", "title": "المشروع\u{202E} الرئيسي"}),
    });
    assert!(ren.ok);
    assert_eq!(ren.result.unwrap()["title"], json!("المشروع الرئيسي"));

    // pane.split (cmux orientation) / pane.new / surface.split / pane.list / surface.list / surface.current
    let (split_cmux, _) = host.dispatch(Request {
        id: json!(12),
        method: "pane.split".into(),
        params: json!({"orientation": "horizontal"}),
    });
    assert!(split_cmux.ok);
    let new_surface = split_cmux.result.unwrap()["surface_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let (plist_cmux, _) = host.dispatch(Request {
        id: json!(13),
        method: "pane.list".into(),
        params: json!({"workspace": "2"}),
    });
    assert!(plist_cmux.ok);
    assert_eq!(
        plist_cmux.result.unwrap()["panels"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let (slist, _) = host.dispatch(Request {
        id: json!(14),
        method: "surface.list".into(),
        params: json!({"workspace": "2"}),
    });
    assert!(slist.ok);
    let slist_res = slist.result.unwrap();
    assert_eq!(slist_res["panels"].as_array().unwrap().len(), 2);
    assert_eq!(slist_res["surfaces"].as_array().unwrap().len(), 2);

    let (scur, _) = host.dispatch(Request {
        id: json!(15),
        method: "surface.current".into(),
        params: json!({}),
    });
    assert!(scur.ok);
    assert_eq!(scur.result.unwrap()["id"], json!(new_surface));

    // pane.focus (cmux panel/surface param) and surface.focus
    let (pfocus, _) = host.dispatch(Request {
        id: json!(16),
        method: "pane.focus".into(),
        params: json!({"panel": "1"}),
    });
    assert!(pfocus.ok);
    assert_eq!(host.focused_pane(), Some(1));

    let (sfocus, _) = host.dispatch(Request {
        id: json!(17),
        method: "surface.focus".into(),
        params: json!({"surface": new_surface}),
    });
    assert!(sfocus.ok);
    assert_eq!(host.focused_pane(), new_surface.parse::<u64>().ok());

    // surface.send_text, surface.send_input, surface.send_key, surface.read_text
    let target_num = new_surface.parse::<u64>().unwrap();
    let (stext, _) = host.dispatch(Request {
        id: json!(18),
        method: "surface.send_text".into(),
        params: json!({"surface": new_surface, "text": "echo مرحبا\n"}),
    });
    assert!(stext.ok);
    let (sinput, _) = host.dispatch(Request {
        id: json!(19),
        method: "surface.send_input".into(),
        params: json!({"surface": new_surface, "input": "ls\n"}),
    });
    assert!(sinput.ok);
    let (skey, _) = host.dispatch(Request {
        id: json!(20),
        method: "surface.send_key".into(),
        params: json!({"surface": new_surface, "key": "c", "mods": ["ctrl"]}),
    });
    assert!(skey.ok);
    assert_eq!(
        host.written_inputs(),
        &[
            (
                target_num,
                b"echo \xd9\x85\xd8\xb1\xd8\xad\xd8\xa8\xd8\xa7\n".to_vec()
            ),
            (target_num, b"ls\n".to_vec()),
            (target_num, vec![0x03]),
        ]
    );

    host.feed_pane_output(target_num, "سطر أول\r\nسطر ثاني".as_bytes());
    let (sread, _) = host.dispatch(Request {
        id: json!(21),
        method: "surface.read_text".into(),
        params: json!({"surface": new_surface, "lines": 10}),
    });
    assert!(sread.ok);
    assert_eq!(sread.result.unwrap()["text"], json!("سطر أول\nسطر ثاني"));

    // notification.create, notification.list, notification.clear
    let (ncreate, _) = host.dispatch(Request {
        id: json!(22),
        method: "notification.create".into(),
        params: json!({
            "surface": new_surface,
            "title": "Claude\u{202E}",
            "body": "انتهت المهمة بنجاح",
            "send_desktop": false
        }),
    });
    assert!(ncreate.ok);
    assert!(
        host.desktop_notifications().is_empty(),
        "send_desktop: false should suppress desktop delivery"
    );

    let (ncreate2, _) = host.dispatch(Request {
        id: json!(23),
        method: "notification.create_for_surface".into(),
        params: json!({
            "surface": "1",
            "title": "Agent",
            "body": "بحاجة لإدخال"
        }),
    });
    assert!(ncreate2.ok);
    assert_eq!(host.desktop_notifications().len(), 1);

    let (nlist, _) = host.dispatch(Request {
        id: json!(24),
        method: "notification.list".into(),
        params: json!({"unread": true}),
    });
    assert!(nlist.ok);
    let nlist_res = nlist.result.unwrap();
    assert_eq!(nlist_res["count"], json!(2));

    let (nclear, _) = host.dispatch(Request {
        id: json!(25),
        method: "notification.clear".into(),
        params: json!({}),
    });
    assert!(nclear.ok);
    let (nlist_after, _) = host.dispatch(Request {
        id: json!(26),
        method: "notification.list".into(),
        params: json!({}),
    });
    assert_eq!(nlist_after.result.unwrap()["count"], json!(0));

    // sidebar.hide / show / toggle / status
    let (shide, _) = host.dispatch_line(r#"{"id":27,"method":"sidebar.hide","params":{}}"#);
    assert!(shide.ok && !host.sidebar_visible());
    let (sshow, _) = host.dispatch_line(r#"{"id":28,"method":"sidebar.show","params":{}}"#);
    assert!(sshow.ok && host.sidebar_visible());
    let (stog, _) = host.dispatch_line(r#"{"id":29,"method":"sidebar.toggle","params":{}}"#);
    assert!(stog.ok && !host.sidebar_visible());
    let (sstat, _) = host.dispatch_line(r#"{"id":30,"method":"sidebar.status","params":{}}"#);
    assert!(sstat.ok);
    let sstat_res = sstat.result.unwrap();
    assert_eq!(sstat_res["visible"], json!(false));
    assert_eq!(sstat_res["workspace_count"], json!(3));

    // pane.close & workspace.close (non-last vs last exits)
    let (pclose, exit) = host.dispatch(Request {
        id: json!(31),
        method: "pane.close".into(),
        params: json!({"panel": new_surface}),
    });
    assert!(pclose.ok && !exit);

    let (wclose3, exit) = host.dispatch(Request {
        id: json!(32),
        method: "workspace.close".into(),
        params: json!({"workspace": "3"}),
    });
    assert!(wclose3.ok && !exit);

    let (wclose2, exit) = host.dispatch(Request {
        id: json!(33),
        method: "workspace.close".into(),
        params: json!({"workspace": "2"}),
    });
    assert!(wclose2.ok && !exit);
    assert_eq!(host.tab_count(), 1);

    let (wclose1, exit) = host.dispatch(Request {
        id: json!(34),
        method: "workspace.close".into(),
        params: json!({}),
    });
    assert!(wclose1.ok);
    assert!(exit, "closing the last workspace signals exit");
}

#[test]
fn existing_twitty_ctl_methods_stay_unchanged() {
    let mut host = HeadlessHost::new();

    // tab.list returns a JSON array
    let (tabs, _) = host.dispatch(Request {
        id: json!(1),
        method: "tab.list".into(),
        params: json!({}),
    });
    assert!(tabs.ok);
    assert!(tabs.result.unwrap().is_array());

    // pane.list with {} returns a JSON array (existing twitty ctl contract)
    let (panes, _) = host.dispatch(Request {
        id: json!(2),
        method: "pane.list".into(),
        params: json!({}),
    });
    assert!(panes.ok);
    let arr = panes.result.unwrap();
    assert!(arr.is_array());
    assert_eq!(arr[0]["id"], json!(1));
    assert_eq!(arr[0]["tab"], json!(0));

    // tab.new & pane.split
    let (tnew, _) = host.dispatch(Request {
        id: json!(3),
        method: "tab.new".into(),
        params: json!({}),
    });
    assert!(tnew.ok);
    let p2 = tnew.result.unwrap()["pane"].as_u64().unwrap();

    let (psplit, _) = host.dispatch(Request {
        id: json!(4),
        method: "pane.split".into(),
        params: json!({"pane": p2, "direction": "right"}),
    });
    assert!(psplit.ok);
    let p3 = psplit.result.unwrap()["pane"].as_u64().unwrap();

    // pane.focus with integer "pane"
    let (pf, _) = host.dispatch(Request {
        id: json!(5),
        method: "pane.focus".into(),
        params: json!({"pane": 1}),
    });
    assert!(pf.ok);
    assert_eq!(pf.result.unwrap(), json!({"pane": 1, "tab": 0}));

    // pane.send-text & pane.read
    let (send, _) = host.dispatch(Request {
        id: json!(6),
        method: "pane.send-text".into(),
        params: json!({"pane": p3, "text": "pwd\n"}),
    });
    assert!(send.ok);
    assert_eq!(send.result.unwrap(), json!({"pane": p3, "bytes": 4}));

    host.feed_pane_output(p3, b"hello from pane 3");
    let (read, _) = host.dispatch(Request {
        id: json!(7),
        method: "pane.read".into(),
        params: json!({"pane": p3, "lines": 5}),
    });
    assert!(read.ok);
    assert_eq!(
        read.result.unwrap(),
        json!({"pane": p3, "text": "hello from pane 3"})
    );

    // notify
    let (notified, _) = host.dispatch(Request {
        id: json!(8),
        method: "notify".into(),
        params: json!({"pane": p3, "title": "Build", "body": "done"}),
    });
    assert!(notified.ok);
    assert_eq!(notified.result.unwrap(), json!({"pane": p3}));

    // command.list & command.run
    let (clist, _) = host.dispatch(Request {
        id: json!(9),
        method: "command.list".into(),
        params: json!({}),
    });
    assert!(clist.ok && clist.result.unwrap().is_array());

    let (crun, _) = host.dispatch(Request {
        id: json!(10),
        method: "command.run".into(),
        params: json!({"name": "split.right", "pane": 1}),
    });
    assert!(crun.ok);
    assert_eq!(
        crun.result.unwrap(),
        json!({"command": "split.right", "pane": 1})
    );
}

#[test]
fn surface_send_key_encodes_named_and_modified_keys() {
    assert_eq!(
        encode_key_name("Return", &[], TermMode::NONE).unwrap(),
        b"\r"
    );
    assert_eq!(
        encode_key_name("Escape", &[], TermMode::NONE).unwrap(),
        b"\x1b"
    );
    assert_eq!(
        encode_key_name("c", &s(&["ctrl"]), TermMode::NONE).unwrap(),
        vec![0x03]
    );
    assert_eq!(
        encode_key_name("ctrl+d", &[], TermMode::NONE).unwrap(),
        vec![0x04]
    );
    assert_eq!(
        encode_key_name("Up", &[], TermMode::NONE).unwrap(),
        b"\x1b[A"
    );
    assert_eq!(
        encode_key_name("Up", &[], TermMode::APP_CURSOR).unwrap(),
        b"\x1bOA"
    );
    assert_eq!(
        encode_key_name("Tab", &s(&["shift"]), TermMode::NONE).unwrap(),
        b"\x1b[Z"
    );
    assert!(encode_key_name("\x03", &[], TermMode::NONE).is_err());
    assert!(encode_key_name("NonExistentKey", &[], TermMode::NONE).is_err());
    assert!(encode_key_name("", &[], TermMode::NONE).is_err());
}

// ---------------------------------------------------------------------------
// 3. Config defaults, CMUX_* env set/stripped, and private wrapper install
// ---------------------------------------------------------------------------

#[test]
fn config_defaults_and_cmux_env_vars_set_and_stripped() {
    let defaults = TwittyConfig::default();
    assert!(defaults.cmux_compat);
    assert!(defaults.claude_hooks);

    // Existing configs without the new keys default both to true.
    let parsed: TwittyConfig = serde_json::from_str(r#"{"font_size": 15.0}"#).unwrap();
    assert!(parsed.cmux_compat);
    assert!(parsed.claude_hooks);

    // Explicit false is honored.
    let disabled: TwittyConfig =
        serde_json::from_str(r#"{"cmux_compat": false, "claude_hooks": false}"#).unwrap();
    assert!(!disabled.cmux_compat);
    assert!(!disabled.claude_hooks);

    // CMUX_* env mapping.
    let env = cmux_env_vars("/run/user/1000/twitty/twitty-42.sock", 3, 9);
    assert_eq!(
        env,
        [
            (
                "CMUX_SOCKET_PATH".into(),
                "/run/user/1000/twitty/twitty-42.sock".into()
            ),
            ("CMUX_WORKSPACE_ID".into(), "3".into()),
            ("CMUX_SURFACE_ID".into(), "9".into()),
        ]
    );

    // Inherited CMUX_* vars are stripped alongside TWITTY_* vars.
    for key in [
        "CMUX_SOCKET_PATH",
        "CMUX_WORKSPACE_ID",
        "CMUX_SURFACE_ID",
        "CMUX_SOCKET_PASSWORD",
        "CMUX_CLAUDE_PID",
        "CMUX_CLAUDE_HOOKS_DISABLED",
        "TWITTY_SOCKET",
        "TWITTY_PANE_ID",
        "TWITTY_BIN",
    ] {
        assert!(
            INHERITED_TERMINAL_VARS.contains(&key),
            "{key} missing from INHERITED_TERMINAL_VARS"
        );
        assert!(is_stripped_env_var(key));
    }
    assert!(is_stripped_env_var("CMUX_ANY_FUTURE_VAR"));
    assert!(!is_stripped_env_var("HOME"));
    assert!(!is_stripped_env_var("PATH"));

    // PATH prepending deduplicates.
    let bin = Path::new("/run/user/1000/twitty/bin");
    assert_eq!(
        prepend_path(bin, Some("/usr/bin:/run/user/1000/twitty/bin:/bin")),
        "/run/user/1000/twitty/bin:/usr/bin:/bin"
    );
    assert_eq!(prepend_path(bin, None), "/run/user/1000/twitty/bin");
}

// ---------------------------------------------------------------------------
// 4. claude-hook CLI & JSON parsing (bounded, Arabic, unknown ignored)
// ---------------------------------------------------------------------------

#[test]
fn claude_hook_cli_parsing_and_bounded_arabic_payloads() {
    assert!(help_text().contains("claude-hook"));
    assert_eq!(
        parse_args(&s(&["claude-hook", "stop"]), Some("5")).unwrap(),
        CliAction::ClaudeHook {
            event: "stop".into(),
            pane: Some(5),
        }
    );
    assert_eq!(
        parse_args(
            &s(&["claude-hook", "notification", "--pane", "9"]),
            Some("5")
        )
        .unwrap(),
        CliAction::ClaudeHook {
            event: "notification".into(),
            pane: Some(9),
        }
    );
    assert!(parse_args(&s(&["claude-hook"]), None).is_err());

    // Bounded stdin reading caps huge Arabic input without panicking on UTF-8 boundaries.
    let huge = "م".repeat(MAX_HOOK_STDIN_BYTES); // 2 bytes each -> 128 KiB
    let truncated = read_bounded_stdin(Cursor::new(huge.as_bytes()), MAX_HOOK_STDIN_BYTES).unwrap();
    assert!(truncated.len() <= MAX_HOOK_STDIN_BYTES + 3);

    // session-start -> working
    let calls = plan_hook_calls("session-start", "{}", Some(2));
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].method, "workspace.set_status");
    assert_eq!(calls[0].params["value"], json!("working"));
    assert_eq!(calls[0].params["surface"], json!("2"));

    // notification with Arabic message + bidi override + control char -> needs_input + notify
    let hook_json = r#"{"title":"كلود\u202E","message":"يحتاج\u0007 إلى موافقة لتشغيل الأمر"}"#;
    let calls = plan_hook_calls("notification", hook_json, Some(2));
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].method, "workspace.set_status");
    assert_eq!(calls[0].params["value"], json!("needs_input"));
    assert_eq!(
        calls[0].params["label"],
        json!("يحتاج إلى موافقة لتشغيل الأمر")
    );
    assert_eq!(calls[1].method, "notification.create");
    assert_eq!(calls[1].params["title"], json!("كلود"));
    assert_eq!(
        calls[1].params["body"],
        json!("يحتاج إلى موافقة لتشغيل الأمر")
    );

    // stop with Arabic last_assistant_message -> done + notify
    let stop_json = r#"{"last_assistant_message":"تم تعديل الملفات بنجاح"}"#;
    let calls = plan_hook_calls("stop", stop_json, Some(2));
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].params["value"], json!("done"));
    assert_eq!(calls[0].params["label"], json!("تم تعديل الملفات بنجاح"));
    assert_eq!(calls[1].method, "notification.create");
    assert_eq!(calls[1].params["body"], json!("تم تعديل الملفات بنجاح"));

    // malformed / empty JSON falls back cleanly without failing
    let fallback = plan_hook_calls("stop", "{truncated", Some(2));
    assert_eq!(fallback.len(), 2);
    assert_eq!(fallback[0].params["label"], json!("Done"));

    // Unknown events are ignored cleanly.
    assert!(plan_hook_calls("unknown-future-hook", "{}", Some(2)).is_empty());
    assert!(plan_hook_calls("", "{}", Some(2)).is_empty());
}

// ---------------------------------------------------------------------------
// 5. Per-pane agent status lifecycle & sidebar Arabic rendering
// ---------------------------------------------------------------------------

#[test]
fn per_pane_agent_status_lifecycle_and_sidebar_rendering() {
    let mut host = HeadlessHost::new();
    // Split tab 0 into two panes: pane 1 and pane 2.
    let (split, _) = host.dispatch(Request {
        id: json!(1),
        method: "pane.split".into(),
        params: json!({"direction": "right"}),
    });
    assert!(split.ok);
    let p2 = split.result.unwrap()["pane"].as_u64().unwrap();
    assert_eq!(p2, 2);

    // Pane 1 starts working; pane 2 requests input in Arabic.
    for call in plan_hook_calls("session-start", "{}", Some(1)) {
        let (r, _) = host.dispatch(Request {
            id: json!(10),
            method: call.method,
            params: call.params,
        });
        assert!(r.ok);
    }
    for call in plan_hook_calls(
        "notification",
        r#"{"title":"Claude","message":"هل أتابع النشر؟"}"#,
        Some(2),
    ) {
        let (r, _) = host.dispatch(Request {
            id: json!(11),
            method: call.method,
            params: call.params,
        });
        assert!(r.ok);
    }

    // Per-pane status is tracked independently on each pane.
    assert_eq!(
        host.pane_agent_status(1),
        Some(AgentStatus::new(AgentState::Working, Some("Working")))
    );
    assert_eq!(
        host.pane_agent_status(2),
        Some(AgentStatus::new(
            AgentState::NeedsInput,
            Some("هل أتابع النشر؟")
        ))
    );
    // Notification history and desktop notification (unfocused window) both received the event.
    assert_eq!(host.desktop_notifications().len(), 1);
    assert_eq!(host.desktop_notifications()[0].2, "هل أتابع النشر؟");

    // Sidebar entry surfaces the higher-priority NeedsInput state and Arabic notification.
    let entries = host.sidebar_entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].agent_status.as_ref().map(|s| s.state),
        Some(AgentState::NeedsInput)
    );
    assert!(entries[0]
        .display_notice()
        .unwrap()
        .contains("هل أتابع النشر؟"));

    // Render sidebar scene without GPU to verify Arabic + badges shape cleanly.
    let mut font_system = FontSystem::new();
    let palette = Palette::default();
    let metrics = CellMetrics {
        font_size: 14.0,
        scale: 1.0,
        char_width: 9.0,
        line_height: 22.0,
        padding_left: 8.0,
        padding_top: 6.0,
    };
    let area = Rect::new(0.0, 0.0, 240.0, 400.0);
    let mut cache = LabelCache::default();
    let mut ctx = DrawContext {
        font_system: &mut font_system,
        family: None,
        palette: &palette,
        metrics,
        is_srgb: false,
    };
    let font = UiFont::default();
    let cards = cache.cards(&font, ctx.font_system, entries, area, metrics.scale);
    let view = ScrollState::default().view(
        &cards,
        0,
        area,
        metrics.scale,
        SidebarPosition::Right,
        UiLanguage::English,
    );
    let scene = build(
        &mut ctx,
        &view,
        &mut cache,
        UiStyle::new(&palette, None, metrics.scale),
        &font,
    );
    assert!(
        scene.labels.len() >= 4,
        "expected title, Arabic notice, cwd, and agent badge labels"
    );

    // Pane 2 receives pre-tool-use -> back to Working; then stop -> Done + desktop notify.
    for call in plan_hook_calls("pre-tool-use", "{}", Some(2)) {
        let (r, _) = host.dispatch(Request {
            id: json!(12),
            method: call.method,
            params: call.params,
        });
        assert!(r.ok);
    }
    assert_eq!(
        host.pane_agent_status(2).map(|s| s.state),
        Some(AgentState::Working)
    );

    for call in plan_hook_calls(
        "stop",
        r#"{"last_assistant_message":"اكتمل البناء"}"#,
        Some(2),
    ) {
        let (r, _) = host.dispatch(Request {
            id: json!(13),
            method: call.method,
            params: call.params,
        });
        assert!(r.ok);
    }
    assert_eq!(
        host.pane_agent_status(2),
        Some(AgentStatus::new(AgentState::Done, Some("اكتمل البناء")))
    );
    assert_eq!(host.desktop_notifications().len(), 2);
    assert_eq!(host.desktop_notifications()[1].2, "اكتمل البناء");
    // Pane 1 is still Working (unaffected by pane 2).
    assert_eq!(
        host.pane_agent_status(1).map(|s| s.state),
        Some(AgentState::Working)
    );

    // session-end on pane 2 clears only pane 2's status.
    for call in plan_hook_calls("session-end", "{}", Some(2)) {
        let (r, _) = host.dispatch(Request {
            id: json!(14),
            method: call.method,
            params: call.params,
        });
        assert!(r.ok);
    }
    assert_eq!(host.pane_agent_status(2), None);
    assert_eq!(
        host.pane_agent_status(1).map(|s| s.state),
        Some(AgentState::Working)
    );
}

// ---------------------------------------------------------------------------
// 6. Unix socket v1/v2 round-trip and bash wrapper script tests
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod unix_tests {
    use super::*;
    use std::fs::{self, Permissions};
    use std::io::{BufReader, Write};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use twitty::control::protocol::{read_bounded_line, ReadLine};
    use twitty::control::server::{ControlServer, ServerConfig};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "twitty-cmux-test-{}-{}-{n}",
                std::process::id(),
                label
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn find_in_path(program: &str) -> Option<PathBuf> {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|dir| dir.join(program))
            .find(|p| p.is_file())
    }

    #[test]
    fn socket_handles_v1_v2_and_oversized_lines() {
        let tmp = TempDir::new("socket-v1-v2");
        let dir = tmp.0.join("run");
        let host = Arc::new(Mutex::new(HeadlessHost::new()));
        let host_bg = Arc::clone(&host);

        let handler =
            Arc::new(move |req: Request| -> Response { host_bg.lock().unwrap().dispatch(req).0 });
        let cfg = ServerConfig {
            max_line_bytes: 512,
            ..ServerConfig::default()
        };
        let server = ControlServer::start_in_dir(&dir, handler, cfg).unwrap();
        let sock = server.path().unwrap().to_path_buf();

        let mut stream = UnixStream::connect(&sock).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());

        // v1 ping + set_status + notify over the real socket.
        for line in [
            "ping\n",
            "set_status claude working \"جاري العمل\"\n",
            "notify \"Claude\" \"جاهز\"\n",
            "{\"id\":\"m1\",\"method\":\"browser.open\",\"params\":{}}\n",
        ] {
            stream.write_all(line.as_bytes()).unwrap();
            stream.flush().unwrap();
            let ReadLine::Line(reply) = read_bounded_line(&mut reader, 4096).unwrap() else {
                panic!("expected response line");
            };
            let v: Value = serde_json::from_str(&reply).unwrap();
            if line.contains("browser.open") {
                assert_eq!(v["ok"], json!(false));
                assert_eq!(v["error"]["code"], json!("unknown_method"));
            } else {
                assert_eq!(v["ok"], json!(true), "line {line:?} failed: {v}");
            }
        }

        // Verify host state updated via v1 commands.
        let guard = host.lock().unwrap();
        assert_eq!(
            guard.pane_agent_status(1),
            Some(AgentStatus::new(AgentState::Working, Some("جاري العمل")))
        );
        assert_eq!(guard.desktop_notifications().len(), 1);
        drop(guard);

        // Oversized cmux v2 line rejected and connection closed.
        let mut big_stream = UnixStream::connect(&sock).unwrap();
        big_stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let big = format!(
            "{{\"id\":1,\"method\":\"surface.send_text\",\"params\":{{\"text\":\"{}\"}}}}\n",
            "a".repeat(600)
        );
        big_stream.write_all(big.as_bytes()).unwrap();
        let mut big_reader = BufReader::new(big_stream);
        let ReadLine::Line(err_line) = read_bounded_line(&mut big_reader, 4096).unwrap() else {
            panic!("expected oversized error");
        };
        let err_val: Value = serde_json::from_str(&err_line).unwrap();
        assert_eq!(err_val["ok"], json!(false));
        assert_eq!(
            read_bounded_line(&mut big_reader, 4096).unwrap(),
            ReadLine::Eof
        );
    }

    #[test]
    fn claude_wrapper_install_and_passthrough_or_injection() {
        let Some(bash) = find_in_path("bash") else {
            eprintln!("bash not installed, skipped");
            return;
        };

        let tmp = TempDir::new("claude-wrapper");
        let wrapper_dir = tmp.0.join("twitty-bin");
        let wrapper = install_claude_wrapper(&wrapper_dir).unwrap();
        assert_eq!(
            fs::symlink_metadata(&wrapper_dir).unwrap().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::symlink_metadata(&wrapper).unwrap().mode() & 0o777,
            0o700
        );

        // Real fake `claude` binary in a separate PATH directory that dumps args + CLAUDECODE.
        let real_bin = tmp.0.join("real-bin");
        fs::create_dir_all(&real_bin).unwrap();
        let real_claude = real_bin.join("claude");
        let out_file = tmp.0.join("invoked.txt");
        fs::write(
            &real_claude,
            format!(
                "#!/usr/bin/env bash\n\
                 printf 'CLAUDECODE=%s\\n' \"${{CLAUDECODE-UNSET}}\" > {:?}\n\
                 for a in \"$@\"; do printf 'ARG:%s\\n' \"$a\" >> {:?}; done\n",
                out_file, out_file
            ),
        )
        .unwrap();
        fs::set_permissions(&real_claude, Permissions::from_mode(0o700)).unwrap();

        let home = tmp.0.join("home");
        fs::create_dir_all(&home).unwrap();
        let twitty_exe = PathBuf::from(env!("CARGO_BIN_EXE_twitty"));
        let sys_path = std::env::var("PATH").unwrap_or_default();
        let test_path = format!(
            "{}:{}:{}",
            wrapper_dir.display(),
            real_bin.display(),
            sys_path
        );

        let run_wrapper = |envs: &[(&str, String)], args: &[&str]| -> String {
            let _ = fs::remove_file(&out_file);
            let mut cmd = Command::new(&bash);
            cmd.arg(&wrapper)
                .args(args)
                .env_clear()
                .env("PATH", &test_path)
                .env("HOME", &home)
                .env("TWITTY_BIN", &twitty_exe);
            for (k, v) in envs {
                cmd.env(k, v);
            }
            let status = cmd.status().unwrap();
            assert!(status.success(), "wrapper failed: {status}");
            fs::read_to_string(&out_file).unwrap()
        };

        // 1. Outside Twitty -> passes through unchanged (keeps CLAUDECODE, no --settings).
        let out = run_wrapper(&[("CLAUDECODE", "1".into())], &["--model", "sonnet"]);
        assert!(out.contains("CLAUDECODE=1"), "{out}");
        assert!(!out.contains("--settings"), "{out}");
        assert!(out.contains("ARG:--model\nARG:sonnet\n"), "{out}");

        // 2. Inside Twitty with live socket -> injects --settings calling claude-hook, unsets CLAUDECODE.
        let run_dir = tmp.0.join("run");
        let host = Arc::new(Mutex::new(HeadlessHost::new()));
        let host_bg = Arc::clone(&host);
        let server = ControlServer::start_in_dir(
            &run_dir,
            Arc::new(move |req| host_bg.lock().unwrap().dispatch(req).0),
            ServerConfig::default(),
        )
        .unwrap();
        let live_sock = server.path().unwrap().to_string_lossy().into_owned();

        let out = run_wrapper(
            &[
                ("TWITTY_PANE_ID", "1".into()),
                ("TWITTY_SOCKET", live_sock.clone()),
                ("CLAUDECODE", "1".into()),
            ],
            &["-p", "hi"],
        );
        assert!(out.contains("CLAUDECODE=UNSET"), "{out}");
        assert!(out.contains("ARG:--settings\n"), "{out}");
        assert!(out.contains("claude-hook session-start"), "{out}");
        assert!(out.contains("claude-hook notification"), "{out}");
        assert!(out.contains("claude-hook stop"), "{out}");
        assert!(out.contains("ARG:-p\nARG:hi\n"), "{out}");
        assert!(
            !home.join(".claude").exists(),
            "wrapper must never create or overwrite user Claude settings"
        );

        // 3. Subcommand pass-through even with live socket.
        let out = run_wrapper(
            &[
                ("TWITTY_PANE_ID", "1".into()),
                ("TWITTY_SOCKET", live_sock.clone()),
            ],
            &["mcp", "list"],
        );
        assert!(!out.contains("--settings"), "{out}");
        assert!(out.contains("ARG:mcp\nARG:list\n"), "{out}");

        // 4. Disabled via TWITTY_CLAUDE_HOOKS_DISABLED=1 or CMUX_CLAUDE_HOOKS_DISABLED=1 -> pass-through.
        for flag in ["TWITTY_CLAUDE_HOOKS_DISABLED", "CMUX_CLAUDE_HOOKS_DISABLED"] {
            let out = run_wrapper(
                &[
                    ("TWITTY_PANE_ID", "1".into()),
                    ("TWITTY_SOCKET", live_sock.clone()),
                    (flag, "1".into()),
                ],
                &["--continue"],
            );
            assert!(!out.contains("--settings"), "{flag}: {out}");
            assert!(out.contains("ARG:--continue\n"), "{flag}: {out}");
        }

        // 5. Dead/missing socket -> pass-through.
        drop(server);
        let out = run_wrapper(
            &[("TWITTY_PANE_ID", "1".into()), ("TWITTY_SOCKET", live_sock)],
            &["--continue"],
        );
        assert!(!out.contains("--settings"), "{out}");

        // 6. Hung (unresponsive) socket times out within ~0.75 s and passes through.
        let hung_dir = tmp.0.join("hung");
        fs::create_dir_all(&hung_dir).unwrap();
        fs::set_permissions(&hung_dir, Permissions::from_mode(0o700)).unwrap();
        let hung_sock = hung_dir.join("hung.sock");
        let _listener = UnixListener::bind(&hung_sock).unwrap();
        fs::set_permissions(&hung_sock, Permissions::from_mode(0o600)).unwrap();
        let started = Instant::now();
        let out = run_wrapper(
            &[
                ("TWITTY_PANE_ID", "1".into()),
                ("TWITTY_SOCKET", hung_sock.to_string_lossy().into_owned()),
            ],
            &["hello"],
        );
        let elapsed = started.elapsed();
        assert!(!out.contains("--settings"), "{out}");
        assert!(out.contains("ARG:hello\n"), "{out}");
        assert!(
            elapsed < Duration::from_secs(4),
            "hung socket check took too long: {elapsed:?}"
        );
    }
}

#[test]
fn ssh_tab_method_reaches_the_native_dispatcher() {
    let mut host = HeadlessHost::new();
    let (resp, exit) = host.dispatch(Request {
        id: json!(1),
        method: "tab.new-ssh".into(),
        params: json!({ "destination": "dev@example.org", "name": "t1" }),
    });
    assert!(!exit);
    assert!(!resp.ok);
    // Known to the control socket (not "unknown method"); a headless host just can't open it.
    assert_ne!(resp.error_code.as_deref(), Some("unknown_method"));
    let wire: Value = serde_json::to_value(&resp).unwrap();
    assert!(wire["error"].as_str().unwrap().contains("SSH tabs"));
    let (bad, _) = host.dispatch(Request {
        id: json!(2),
        method: "tab.new-ssh".into(),
        params: json!({ "destination": "-oProxyCommand=x" }),
    });
    assert!(!bad.ok);
}

#[test]
fn cmux_workspace_set_color_and_tab_color_update_and_clear() {
    let mut host = HeadlessHost::new();

    // Set color via `workspace.set_color`.
    let (resp, _) = host.dispatch(Request {
        id: json!(1),
        method: "workspace.set_color".into(),
        params: json!({ "color": "amber" }),
    });
    assert!(resp.ok, "{resp:?}");
    assert_eq!(resp.result.as_ref().unwrap()["color"], "amber");

    // Reflected in `workspace.list` and `workspace.current`.
    let (list, _) = host.dispatch(Request {
        id: json!(2),
        method: "workspace.list".into(),
        params: json!({}),
    });
    assert_eq!(list.result.unwrap()["workspaces"][0]["color"], "amber");

    // Alias `tab.color` clears when given "none" or null.
    let (cleared, _) = host.dispatch(Request {
        id: json!(3),
        method: "tab.color".into(),
        params: json!({ "color": "none" }),
    });
    assert!(cleared.ok, "{cleared:?}");
    assert!(cleared.result.unwrap()["color"].is_null());

    // Unknown color returns invalid_params.
    let (bad, _) = host.dispatch(Request {
        id: json!(4),
        method: "workspace.set_color".into(),
        params: json!({ "color": "ultraviolet" }),
    });
    assert!(!bad.ok);
    assert_eq!(bad.error_code.as_deref(), Some("invalid_params"));
}
