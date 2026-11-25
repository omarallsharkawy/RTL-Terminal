//! Window-free checks for Twitty's Linux desktop packaging, AppStream metainfo,
//! hicolor icon assets, AUR PKGBUILDs/.SRCINFO, and helper scripts.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_text(rel: &str) -> String {
    let path = repo_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

fn contains_arabic(s: &str) -> bool {
    s.chars().any(|c| ('\u{0600}'..='\u{06FF}').contains(&c))
}

fn assert_no_trailing_whitespace(rel: &str, content: &str) {
    for (idx, line) in content.lines().enumerate() {
        assert!(
            !line.ends_with([' ', '\t', '\r']),
            "{rel}:{} has trailing whitespace: {:?}",
            idx + 1,
            line
        );
    }
}

fn cargo_version() -> String {
    let toml = read_text("Cargo.toml");
    let mut in_package = false;
    for line in toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package {
            if let Some(rest) = trimmed.strip_prefix("version") {
                if let Some(val) = rest.trim_start().strip_prefix('=') {
                    return val.trim().trim_matches('"').to_owned();
                }
            }
        }
    }
    panic!("version not found in Cargo.toml [package]");
}

fn parse_desktop_entry(content: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let mut in_desktop_entry = false;
    for (idx, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_desktop_entry = line == "[Desktop Entry]";
            continue;
        }
        if in_desktop_entry {
            let (k, v) = line
                .split_once('=')
                .unwrap_or_else(|| panic!("line {} is not key=value: {line:?}", idx + 1));
            assert!(
                map.insert(k.to_owned(), v.to_owned()).is_none(),
                "duplicate key {k:?} in [Desktop Entry]"
            );
        }
    }
    map
}

#[test]
fn desktop_file_is_valid_bilingual_and_matches_window_app_id() {
    let rel = "packaging/linux/twitty.desktop";
    let content = read_text(rel);
    assert_no_trailing_whitespace(rel, &content);
    assert!(
        content.lines().any(|l| l == "[Desktop Entry]"),
        "{rel} must contain [Desktop Entry]"
    );

    let entries = parse_desktop_entry(&content);
    assert_eq!(entries.get("Type").map(String::as_str), Some("Application"));
    assert_eq!(entries.get("Name").map(String::as_str), Some("Twitty"));
    assert_eq!(entries.get("Exec").map(String::as_str), Some("twitty"));
    assert_eq!(entries.get("Icon").map(String::as_str), Some("twitty"));
    assert_eq!(entries.get("Terminal").map(String::as_str), Some("false"));
    assert_eq!(
        entries.get("Categories").map(String::as_str),
        Some("System;TerminalEmulator;")
    );
    assert_eq!(
        entries.get("StartupWMClass").map(String::as_str),
        Some("twitty")
    );

    // Ensure GenericName and Comment are present in both English and Arabic.
    for key in ["GenericName", "Comment"] {
        let en = entries
            .get(key)
            .unwrap_or_else(|| panic!("missing {key} in {rel}"));
        assert!(!en.trim().is_empty(), "{key} must not be empty");
    }
    for ar_key in ["Name[ar]", "GenericName[ar]", "Comment[ar]"] {
        let ar = entries
            .get(ar_key)
            .unwrap_or_else(|| panic!("missing {ar_key} in {rel}"));
        assert!(
            contains_arabic(ar),
            "{ar_key} must contain Arabic characters, got {ar:?}"
        );
    }

    // Verify StartupWMClass matches the Linux app_id set in src/app/window.rs.
    let window_rs = read_text("src/app/window.rs");
    assert!(
        window_rs.contains("WINDOW_APP_ID: &str = \"twitty\""),
        "src/app/window.rs must set WINDOW_APP_ID to match StartupWMClass=twitty"
    );
}

/// Minimal zero-dependency well-formedness check for XML element nesting.
fn assert_xml_well_formed(rel: &str, xml: &str) {
    assert!(
        xml.starts_with("<?xml "),
        "{rel} must start with an XML declaration"
    );
    let mut stack: Vec<String> = Vec::new();
    let bytes = xml.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        // Comments <!-- ... -->
        if xml[i..].starts_with("<!--") {
            let end = xml[i + 4..]
                .find("-->")
                .unwrap_or_else(|| panic!("{rel}: unterminated XML comment"));
            i += 4 + end + 3;
            continue;
        }
        // Processing instructions <? ... ?>
        if xml[i..].starts_with("<?") {
            let end = xml[i + 2..]
                .find("?>")
                .unwrap_or_else(|| panic!("{rel}: unterminated processing instruction"));
            i += 2 + end + 2;
            continue;
        }
        let close = xml[i + 1..]
            .find('>')
            .unwrap_or_else(|| panic!("{rel}: unclosed '<' at byte {i}"));
        let tag_body = xml[i + 1..i + 1 + close].trim();
        assert!(!tag_body.is_empty(), "{rel}: empty XML tag");

        if let Some(closing) = tag_body.strip_prefix('/') {
            let name = closing.trim();
            let expected = stack
                .pop()
                .unwrap_or_else(|| panic!("{rel}: unexpected closing tag </{name}>"));
            assert_eq!(
                expected, name,
                "{rel}: mismatched XML tags: opened <{expected}>, closed </{name}>"
            );
        } else if !tag_body.ends_with('/') {
            let name = tag_body
                .split_whitespace()
                .next()
                .unwrap_or_else(|| panic!("{rel}: malformed opening tag <{tag_body}>"));
            stack.push(name.to_owned());
        }
        i += close + 2;
    }
    assert!(
        stack.is_empty(),
        "{rel}: unclosed XML elements at EOF: {stack:?}"
    );
}

#[test]
fn appstream_metainfo_is_well_formed_and_complete() {
    let rel = "packaging/linux/dev.twitty.Twitty.metainfo.xml";
    let xml = read_text(rel);
    assert_no_trailing_whitespace(rel, &xml);
    assert_xml_well_formed(rel, &xml);

    for required in [
        r#"<component type="desktop-application">"#,
        "<id>dev.twitty.Twitty</id>",
        "<metadata_license>CC0-1.0</metadata_license>",
        "<project_license>AGPL-3.0-or-later</project_license>",
        "<name>Twitty</name>",
        r#"<name xml:lang="ar">"#,
        "<summary>",
        r#"<summary xml:lang="ar">"#,
        "<description>",
        r#"<launchable type="desktop-id">twitty.desktop</launchable>"#,
        "<screenshots>",
        "https://raw.githubusercontent.com/omarallsharkawy/RTL-Terminal/main/docs/assets/screenshot.png",
        "<binary>twitty</binary>",
        "<binary>twitty-remote</binary>",
        "<category>System</category>",
        "<category>TerminalEmulator</category>",
        "<releases>",
    ] {
        assert!(
            xml.contains(required),
            "{rel} is missing required fragment: {required}"
        );
    }

    assert!(
        contains_arabic(&xml),
        "{rel} should include Arabic localized metadata"
    );
}

fn read_png_dimensions(path: &Path) -> (u32, u32) {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    assert!(
        bytes.len() >= 24,
        "{} is too short to be a valid PNG ({} bytes)",
        path.display(),
        bytes.len()
    );
    const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    assert_eq!(
        &bytes[0..8],
        &PNG_SIG,
        "{} does not have a valid PNG signature",
        path.display()
    );
    assert_eq!(
        &bytes[12..16],
        b"IHDR",
        "{} first chunk is not IHDR",
        path.display()
    );
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    (width, height)
}

#[test]
fn hicolor_icons_exist_and_match_directory_dimensions() {
    let base = repo_root().join("packaging/linux/icons/hicolor");
    for size in [16u32, 24, 32, 48, 64, 128, 256, 512] {
        let icon_path = base.join(format!("{size}x{size}/apps/twitty.png"));
        assert!(
            icon_path.is_file(),
            "missing hicolor PNG icon: {}",
            icon_path.display()
        );
        let (w, h) = read_png_dimensions(&icon_path);
        assert_eq!(
            (w, h),
            (size, size),
            "{} has dimensions {w}x{h}, expected {size}x{size}",
            icon_path.display()
        );
    }

    let scalable = base.join("scalable/apps/twitty.svg");
    let source_svg = repo_root().join("docs/assets/twitty-mark.svg");
    assert_eq!(
        fs::read(&scalable).expect("read scalable twitty.svg"),
        fs::read(&source_svg).expect("read docs/assets/twitty-mark.svg"),
        "scalable/apps/twitty.svg must stay in sync with docs/assets/twitty-mark.svg"
    );
}

fn pkgbuild_field(content: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    for line in content.lines() {
        if let Some(val) = line.strip_prefix(&prefix) {
            return Some(val.trim().trim_matches(&['\'', '"'][..]).to_owned());
        }
    }
    None
}

fn srcinfo_field(content: &str, key: &str) -> Option<String> {
    let prefix = format!("{key} = ");
    for line in content.lines() {
        if let Some(val) = line.trim().strip_prefix(&prefix) {
            return Some(val.trim().to_owned());
        }
    }
    None
}

#[test]
fn aur_pkgbuilds_and_srcinfo_match_cargo_version_and_runtime_deps() {
    let version = cargo_version();

    for (pkg, expected_pkgname) in [
        ("packaging/aur/twitty-bin", "twitty-bin"),
        ("packaging/aur/twitty-git", "twitty-git"),
    ] {
        let pkgbuild_rel = format!("{pkg}/PKGBUILD");
        let srcinfo_rel = format!("{pkg}/.SRCINFO");
        let pkgbuild = read_text(&pkgbuild_rel);
        let srcinfo = read_text(&srcinfo_rel);

        assert_no_trailing_whitespace(&pkgbuild_rel, &pkgbuild);
        assert_no_trailing_whitespace(&srcinfo_rel, &srcinfo);

        assert_eq!(
            pkgbuild_field(&pkgbuild, "pkgname").as_deref(),
            Some(expected_pkgname)
        );
        assert_eq!(
            pkgbuild_field(&pkgbuild, "pkgver").as_deref(),
            Some(version.as_str()),
            "{pkgbuild_rel} pkgver must match Cargo.toml version ({version})"
        );
        assert_eq!(
            srcinfo_field(&srcinfo, "pkgver").as_deref(),
            Some(version.as_str()),
            "{srcinfo_rel} pkgver must match Cargo.toml version ({version})"
        );
        assert!(
            pkgbuild.contains("AGPL-3.0-or-later")
                && srcinfo.contains("license = AGPL-3.0-or-later"),
            "{pkg} must declare AGPL-3.0-or-later license"
        );

        for dep in [
            "vulkan-icd-loader",
            "mesa",
            "libxkbcommon",
            "wayland",
            "libx11",
            "hicolor-icon-theme",
        ] {
            assert!(
                pkgbuild.contains(&format!("'{dep}'")),
                "{pkgbuild_rel} must depend on {dep}"
            );
            assert!(
                srcinfo.contains(&format!("depends = {dep}")),
                "{srcinfo_rel} must list depends = {dep}"
            );
        }

        for installed in [
            "usr/bin/twitty",
            "usr/bin/twitty-remote",
            "usr/share/applications/twitty.desktop",
            "usr/share/metainfo/dev.twitty.Twitty.metainfo.xml",
            "usr/share/icons/hicolor",
            "usr/share/licenses",
        ] {
            assert!(
                pkgbuild.contains(installed),
                "{pkgbuild_rel} package() must install {installed}"
            );
        }
    }

    let git_pkgbuild = read_text("packaging/aur/twitty-git/PKGBUILD");
    assert!(
        git_pkgbuild.contains("cargo build") && git_pkgbuild.contains("--locked"),
        "twitty-git PKGBUILD must build with cargo --locked"
    );
}

#[test]
fn linux_packaging_scripts_exist_and_pin_appimagetool() {
    for rel in [
        "packaging/linux/install.sh",
        "packaging/linux/build-appimage.sh",
    ] {
        let content = read_text(rel);
        assert_no_trailing_whitespace(rel, &content);
        assert!(content.starts_with("#!/"), "{rel} must have a shebang");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(repo_root().join(rel))
                .expect("stat script")
                .permissions()
                .mode();
            assert_ne!(mode & 0o111, 0, "{rel} must be executable");
        }
    }

    let appimage_script = read_text("packaging/linux/build-appimage.sh");
    assert!(appimage_script.contains("APPIMAGETOOL_VERSION="));
    assert!(appimage_script.contains("APPIMAGETOOL_SHA256="));
    assert!(appimage_script.contains("exec \"$HERE/usr/bin/twitty\""));
    assert!(appimage_script.contains("twitty-remote"));

    let install_script = read_text("packaging/linux/install.sh");
    assert!(install_script.contains("$HOME/.local"));
    assert!(install_script.contains("/usr/local"));
    assert!(install_script.contains("twitty-remote"));
}
