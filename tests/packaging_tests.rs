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

#[test]
fn release_scripts_exist_are_executable_and_clean() {
    for rel in ["scripts/release.sh", "scripts/aur-checksums.sh"] {
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

    let release_sh = read_text("scripts/release.sh");
    assert!(release_sh.contains("--dry-run"));
    assert!(release_sh.contains("git tag -s"));
    assert!(release_sh.contains("git push"));
    assert!(release_sh.contains("dev.twitty.Twitty.metainfo.xml"));

    let aur_sh = read_text("scripts/aur-checksums.sh");
    assert!(aur_sh.contains("/releases/download/v${version}/SHA256SUMS.txt"));
    assert!(aur_sh.contains("twitty-${version}-linux-x86_64.tar.gz"));
    assert!(aur_sh.contains("makepkg --printsrcinfo"));
}

#[test]
fn release_workflow_tag_path_is_consistent() {
    let workflow = read_text(".github/workflows/build.yml");
    for required in [
        r#"tags: ["v*"]"#,
        "twitty-$version-windows-x86_64.zip",
        "twitty-$version-linux-x86_64.tar.gz",
        "twitty-${version}-linux-x86_64.AppImage",
        "twitty-${version}-linux-x86_64.tar.gz",
        "twitty-${version}-windows-x86_64.zip",
        "SHA256SUMS.txt",
        "CHANGELOG.md > release-notes.md",
        "gh release create \"$GITHUB_REF_NAME\" artifacts/*",
    ] {
        assert!(
            workflow.contains(required),
            ".github/workflows/build.yml is missing required release fragment: {required}"
        );
    }
    assert!(
        !workflow.contains("windows-x86_64.exe"),
        ".github/workflows/build.yml should attach a Windows zip archive, not a bare .exe"
    );
}

#[test]
fn changelog_readme_and_screenshots_are_release_ready() {
    let changelog = read_text("CHANGELOG.md");
    assert_no_trailing_whitespace("CHANGELOG.md", &changelog);
    let unreleased = changelog
        .split("## 3.0.0")
        .next()
        .expect("CHANGELOG.md must contain ## 3.0.0 section");
    assert!(
        unreleased.contains("## Unreleased"),
        "CHANGELOG.md must start with ## Unreleased"
    );
    for section in [
        "### Arabic text",
        "### Look",
        "### Sidebar & workspaces",
        "### Command blocks",
        "### Composer",
        "### AI",
        "### Settings",
        "### SSH",
        "### Control socket",
        "### Review panel",
        "### Packaging",
        "### Security",
    ] {
        assert!(
            unreleased.contains(section),
            "CHANGELOG.md Unreleased section is missing {section}"
        );
    }
    let lower_unreleased = unreleased.to_ascii_lowercase();
    for internal in ["lane-", "lane s", "lane n", "lane o", "round 4"] {
        assert!(
            !lower_unreleased.contains(internal),
            "CHANGELOG.md Unreleased section must not mention internal term {internal:?}"
        );
    }

    let readme = read_text("README.md");
    for required in [
        "## Screenshots",
        "docs/screenshots/",
        "yay -S twitty-bin",
        "yay -S twitty-git",
        "linux-x86_64.AppImage",
        "windows-x86_64.zip",
        "`composer`",
        "`ai`",
        "`ai_agent`",
        "`ai_timeout_secs`",
        "Ctrl+Shift+Space",
        "Ctrl+,",
    ] {
        assert!(
            readme.contains(required),
            "README.md is missing expected release content: {required}"
        );
    }
    assert!(
        repo_root().join("docs/screenshots/README.md").is_file(),
        "docs/screenshots/README.md must exist"
    );
}

#[cfg(unix)]
mod script_tests {
    use super::*;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(prefix: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "twitty-pkg-test-{prefix}-{}-{id}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create temp test dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn copy_file(src_rel: &str, dst_root: &Path) {
        let dst = dst_root.join(src_rel);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::copy(repo_root().join(src_rel), &dst)
            .unwrap_or_else(|e| panic!("failed to copy {src_rel} to {}: {e}", dst.display()));
    }

    fn run_git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn stage_temp_repo(prefix: &str) -> TempDir {
        let tmp = TempDir::new(prefix);
        for rel in [
            "Cargo.toml",
            "Cargo.lock",
            "CHANGELOG.md",
            "packaging/linux/dev.twitty.Twitty.metainfo.xml",
            "packaging/aur/twitty-bin/PKGBUILD",
            "packaging/aur/twitty-bin/.SRCINFO",
            "packaging/aur/twitty-git/PKGBUILD",
            "packaging/aur/twitty-git/.SRCINFO",
        ] {
            copy_file(rel, tmp.path());
        }
        run_git(tmp.path(), &["init", "-q"]);
        run_git(tmp.path(), &["config", "user.name", "Test User"]);
        run_git(tmp.path(), &["config", "user.email", "test@example.com"]);
        run_git(tmp.path(), &["add", "."]);
        run_git(tmp.path(), &["commit", "-q", "-m", "init"]);
        tmp
    }

    #[test]
    fn release_script_semver_comparison() {
        let script = repo_root().join("scripts/release.sh");
        let check = |old: &str, new: &str| -> bool {
            Command::new(&script)
                .args(["--check-version", old, new])
                .output()
                .expect("run release.sh --check-version")
                .status
                .success()
        };

        for (old, new) in [
            ("3.0.0", "3.0.1"),
            ("3.0.0", "3.1.0"),
            ("3.0.0", "4.0.0"),
            ("3.2.0", "3.10.0"),
            ("3.9.9", "3.10.0"),
            ("3.1.0-rc.1", "3.1.0"),
            ("3.1.0-alpha.2", "3.1.0-alpha.10"),
            ("3.1.0-alpha.9", "3.1.0-beta.1"),
            ("3.1.0-rc.1", "3.1.0-rc.1.1"),
        ] {
            assert!(check(old, new), "expected {new} > {old}");
        }

        for (old, new) in [
            ("3.0.0", "3.0.0"),
            ("3.1.0", "3.0.9"),
            ("3.10.0", "3.2.0"),
            ("3.1.0", "3.1.0-rc.1"),
            ("3.1.0-beta.1", "3.1.0-alpha.9"),
            ("3.0.0", "v3.1.0"),
            ("3.0.0", "3.1"),
            ("3.0.0", "03.1.0"),
            ("3.0.0", "3.01.0"),
            ("3.0.0", "3.1.0-01"),
            ("3.0.0", "not-a-version"),
            ("3.0.0", ""),
        ] {
            assert!(!check(old, new), "expected {new} NOT > {old}");
        }
    }

    #[test]
    fn release_script_changelog_move_and_metainfo_pure_helpers() {
        let tmp = TempDir::new("pure-helpers");
        copy_file("CHANGELOG.md", tmp.path());
        copy_file("packaging/linux/dev.twitty.Twitty.metainfo.xml", tmp.path());

        let release_sh = repo_root().join("scripts/release.sh");
        let changelog = tmp.path().join("CHANGELOG.md");
        let metainfo = tmp
            .path()
            .join("packaging/linux/dev.twitty.Twitty.metainfo.xml");

        let out = Command::new("bash")
            .arg("-c")
            .arg(
                "set -euo pipefail; source \"$1\"; move_changelog \"$2\" 3.1.0 2026-11-26; add_metainfo_release \"$3\" 3.1.0 2026-11-26",
            )
            .arg("_")
            .arg(&release_sh)
            .arg(&changelog)
            .arg(&metainfo)
            .output()
            .expect("invoke pure release helpers");
        assert!(
            out.status.success(),
            "helpers failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let updated_changelog = fs::read_to_string(&changelog).unwrap();
        assert_no_trailing_whitespace("temp CHANGELOG.md", &updated_changelog);
        assert!(updated_changelog.contains("## Unreleased\n\n## 3.1.0 - 2026-11-26\n"));
        assert!(updated_changelog.contains("## 3.0.0 — 2026-10-11"));

        // Running move_changelog again with an empty Unreleased section must fail.
        let empty_fail = Command::new("bash")
            .arg("-c")
            .arg("set -euo pipefail; source \"$1\"; move_changelog \"$2\" 3.2.0 2026-11-27")
            .arg("_")
            .arg(&release_sh)
            .arg(&changelog)
            .output()
            .expect("invoke move_changelog on empty Unreleased");
        assert!(
            !empty_fail.status.success(),
            "move_changelog should reject an empty Unreleased section"
        );

        let updated_xml = fs::read_to_string(&metainfo).unwrap();
        assert_no_trailing_whitespace("temp metainfo.xml", &updated_xml);
        assert_xml_well_formed("temp metainfo.xml", &updated_xml);
        let pos_new = updated_xml
            .find(r#"<release version="3.1.0" date="2026-11-26">"#)
            .expect("new release entry inserted");
        let pos_old = updated_xml
            .find(r#"<release version="3.0.0" date="2026-10-11">"#)
            .expect("old release entry preserved");
        assert!(
            pos_new < pos_old,
            "AppStream <release> entries must be ordered newest-first"
        );

        // Validate with appstreamcli if installed on this machine.
        if Command::new("appstreamcli")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            let val = Command::new("appstreamcli")
                .args(["validate", "--no-net"])
                .arg(&metainfo)
                .output()
                .expect("run appstreamcli");
            assert!(
                val.status.success(),
                "appstreamcli rejected updated metainfo:\n{}\n{}",
                String::from_utf8_lossy(&val.stdout),
                String::from_utf8_lossy(&val.stderr)
            );
        }

        // Duplicate release version in metainfo must fail.
        let dup_fail = Command::new("bash")
            .arg("-c")
            .arg("set -euo pipefail; source \"$1\"; add_metainfo_release \"$2\" 3.1.0 2026-11-26")
            .arg("_")
            .arg(&release_sh)
            .arg(&metainfo)
            .output()
            .expect("invoke duplicate add_metainfo_release");
        assert!(
            !dup_fail.status.success(),
            "add_metainfo_release should reject duplicate versions"
        );
    }

    #[test]
    fn release_script_dry_run_and_apply_on_temp_repo() {
        let tmp = stage_temp_repo("release-e2e");
        let script = repo_root().join("scripts/release.sh");
        let cargo_before = fs::read_to_string(tmp.path().join("Cargo.toml")).unwrap();

        // 1. Dirty working tree must be rejected.
        fs::write(tmp.path().join("dirty.txt"), "untracked\n").unwrap();
        let dirty = Command::new(&script)
            .args(["--repo-root"])
            .arg(tmp.path())
            .args(["--dry-run", "--date", "2026-11-26", "3.1.0"])
            .output()
            .expect("run release.sh on dirty repo");
        assert!(!dirty.status.success(), "dirty repo must be rejected");
        fs::remove_file(tmp.path().join("dirty.txt")).unwrap();

        // 2. --dry-run prints unified diffs without modifying any file.
        let dry = Command::new(&script)
            .args(["--repo-root"])
            .arg(tmp.path())
            .args(["--dry-run", "--no-makepkg", "--date", "2026-11-26", "3.1.0"])
            .output()
            .expect("run release.sh --dry-run");
        assert!(
            dry.status.success(),
            "--dry-run failed: {}",
            String::from_utf8_lossy(&dry.stderr)
        );
        let dry_stdout = String::from_utf8_lossy(&dry.stdout);
        for expected in [
            "a/Cargo.toml",
            "b/Cargo.toml",
            "a/Cargo.lock",
            "a/CHANGELOG.md",
            "a/packaging/linux/dev.twitty.Twitty.metainfo.xml",
            "a/packaging/aur/twitty-bin/PKGBUILD",
            "a/packaging/aur/twitty-bin/.SRCINFO",
            "a/packaging/aur/twitty-git/PKGBUILD",
            "a/packaging/aur/twitty-git/.SRCINFO",
        ] {
            assert!(
                dry_stdout.contains(expected),
                "--dry-run diff missing {expected}"
            );
        }
        assert_eq!(
            fs::read_to_string(tmp.path().join("Cargo.toml")).unwrap(),
            cargo_before,
            "--dry-run must not modify Cargo.toml"
        );

        // 3. Apply with --skip-gates updates files and prints tag/push commands without running them.
        // First bump pkgrel in twitty-bin to 3 and commit so we verify release.sh resets pkgrel=1.
        let bin_pkgbuild_path = tmp.path().join("packaging/aur/twitty-bin/PKGBUILD");
        let mutated = fs::read_to_string(&bin_pkgbuild_path)
            .unwrap()
            .replace("pkgrel=1", "pkgrel=3")
            .replace(
                "sha256sums_x86_64=('SKIP')",
                "sha256sums_x86_64=('0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef')",
            );
        fs::write(&bin_pkgbuild_path, mutated).unwrap();
        run_git(tmp.path(), &["add", "packaging/aur/twitty-bin/PKGBUILD"]);
        run_git(tmp.path(), &["commit", "-q", "-m", "simulate prev pkgrel"]);

        let apply = Command::new(&script)
            .args(["--repo-root"])
            .arg(tmp.path())
            .args([
                "--skip-gates",
                "--no-makepkg",
                "--date",
                "2026-11-26",
                "3.1.0",
            ])
            .output()
            .expect("run release.sh apply");
        assert!(
            apply.status.success(),
            "release.sh failed: {}",
            String::from_utf8_lossy(&apply.stderr)
        );
        let apply_stdout = String::from_utf8_lossy(&apply.stdout);
        assert!(apply_stdout.contains("git tag -s \"v3.1.0\""));
        assert!(apply_stdout.contains("git push origin \"v3.1.0\""));
        assert!(apply_stdout.contains("./scripts/aur-checksums.sh \"3.1.0\""));

        // Verify no git tags were actually created.
        let tags = Command::new("git")
            .current_dir(tmp.path())
            .args(["tag", "-l"])
            .output()
            .expect("git tag -l");
        assert!(
            String::from_utf8_lossy(&tags.stdout).trim().is_empty(),
            "release.sh must never create git tags"
        );

        let new_cargo = fs::read_to_string(tmp.path().join("Cargo.toml")).unwrap();
        assert!(new_cargo.contains("version = \"3.1.0\""));
        assert!(new_cargo.contains("portable-pty = \"0.9.0\""));

        let new_lock = fs::read_to_string(tmp.path().join("Cargo.lock")).unwrap();
        assert!(new_lock.contains("name = \"twitty\"\nversion = \"3.1.0\""));

        for pkg in ["packaging/aur/twitty-bin", "packaging/aur/twitty-git"] {
            let pb = fs::read_to_string(tmp.path().join(pkg).join("PKGBUILD")).unwrap();
            let si = fs::read_to_string(tmp.path().join(pkg).join(".SRCINFO")).unwrap();
            assert_no_trailing_whitespace(pkg, &pb);
            assert_no_trailing_whitespace(pkg, &si);
            assert_eq!(pkgbuild_field(&pb, "pkgver").as_deref(), Some("3.1.0"));
            assert_eq!(pkgbuild_field(&pb, "pkgrel").as_deref(), Some("1"));
            assert_eq!(srcinfo_field(&si, "pkgver").as_deref(), Some("3.1.0"));
            assert_eq!(srcinfo_field(&si, "pkgrel").as_deref(), Some("1"));
        }

        let bin_pb = fs::read_to_string(&bin_pkgbuild_path).unwrap();
        assert!(
            bin_pb.contains("sha256sums_x86_64=('SKIP')"),
            "release.sh must reset twitty-bin sha256sums_x86_64 to SKIP for the new version"
        );
        let bin_si =
            fs::read_to_string(tmp.path().join("packaging/aur/twitty-bin/.SRCINFO")).unwrap();
        assert!(bin_si.contains("/releases/download/v3.1.0/twitty-3.1.0-linux-x86_64.tar.gz"));
    }

    #[test]
    fn aur_checksums_updates_pkgbuild_and_generates_srcinfo_offline() {
        let tmp = TempDir::new("aur-sums");
        copy_file("packaging/aur/twitty-bin/PKGBUILD", tmp.path());
        copy_file("packaging/aur/twitty-bin/.SRCINFO", tmp.path());
        copy_file("packaging/aur/twitty-git/PKGBUILD", tmp.path());

        let pkg_dir = tmp.path().join("packaging/aur/twitty-bin");
        let pkgbuild_path = pkg_dir.join("PKGBUILD");
        let bumped = fs::read_to_string(&pkgbuild_path)
            .unwrap()
            .replace("pkgver=3.0.0", "pkgver=3.1.0");
        fs::write(&pkgbuild_path, bumped).unwrap();

        let expected_hash = "a1b2c3d4e5f60718293a4b5c6d7e8f90123456789abcdef0123456789abcdef0";
        let sums_path = tmp.path().join("SHA256SUMS.txt");
        fs::write(
            &sums_path,
            format!(
                "1111111111111111111111111111111111111111111111111111111111111111  twitty-3.1.0-linux-x86_64.AppImage\n\
                 {}  twitty-3.1.0-linux-x86_64.tar.gz\n\
                 2222222222222222222222222222222222222222222222222222222222222222  twitty-3.1.0-windows-x86_64.zip\n",
                expected_hash.to_ascii_uppercase()
            ),
        )
        .unwrap();

        let script = repo_root().join("scripts/aur-checksums.sh");
        let out = Command::new(&script)
            .args(["--pkg-dir"])
            .arg(&pkg_dir)
            .args(["--checksums-file"])
            .arg(&sums_path)
            .args(["--no-makepkg", "3.1.0"])
            .output()
            .expect("run aur-checksums.sh");
        assert!(
            out.status.success(),
            "aur-checksums.sh failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let updated_pb = fs::read_to_string(&pkgbuild_path).unwrap();
        assert_no_trailing_whitespace("temp twitty-bin PKGBUILD", &updated_pb);
        assert!(updated_pb.contains(&format!("sha256sums_x86_64=('{expected_hash}')")));

        let updated_si = fs::read_to_string(pkg_dir.join(".SRCINFO")).unwrap();
        assert_no_trailing_whitespace("temp twitty-bin .SRCINFO", &updated_si);
        assert_eq!(
            srcinfo_field(&updated_si, "pkgver").as_deref(),
            Some("3.1.0")
        );
        assert_eq!(
            srcinfo_field(&updated_si, "sha256sums_x86_64").as_deref(),
            Some(expected_hash)
        );
        assert!(updated_si.contains(
            "source_x86_64 = twitty-bin-3.1.0-x86_64.tar.gz::https://github.com/omarallsharkawy/RTL-Terminal/releases/download/v3.1.0/twitty-3.1.0-linux-x86_64.tar.gz"
        ));

        // Mismatched version must fail.
        let mismatch = Command::new(&script)
            .args(["--pkg-dir"])
            .arg(&pkg_dir)
            .args(["--checksums-file"])
            .arg(&sums_path)
            .args(["--no-makepkg", "3.2.0"])
            .output()
            .expect("run aur-checksums.sh with mismatched version");
        assert!(
            !mismatch.status.success(),
            "aur-checksums.sh must reject version mismatch against PKGBUILD"
        );

        // Missing asset in checksums file must fail.
        let empty_sums = tmp.path().join("EMPTY.txt");
        fs::write(&empty_sums, "deadbeef  other-file.tar.gz\n").unwrap();
        let missing = Command::new(&script)
            .args(["--pkg-dir"])
            .arg(&pkg_dir)
            .args(["--checksums-file"])
            .arg(&empty_sums)
            .args(["--no-makepkg", "3.1.0"])
            .output()
            .expect("run aur-checksums.sh with missing asset");
        assert!(
            !missing.status.success(),
            "aur-checksums.sh must fail when tarball checksum is absent"
        );

        // If makepkg is installed, verify the fallback generator matches `makepkg --printsrcinfo`.
        if Command::new("makepkg")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            for dir in [pkg_dir.clone(), tmp.path().join("packaging/aur/twitty-git")] {
                let real = Command::new("makepkg")
                    .current_dir(&dir)
                    .arg("--printsrcinfo")
                    .output()
                    .expect("run makepkg --printsrcinfo");
                assert!(real.status.success());
                let fallback = Command::new(&script)
                    .args(["--generate-srcinfo"])
                    .arg(dir.join("PKGBUILD"))
                    .arg("--no-makepkg")
                    .output()
                    .expect("run fallback srcinfo generator");
                assert!(fallback.status.success());
                assert_eq!(
                    String::from_utf8_lossy(&fallback.stdout),
                    String::from_utf8_lossy(&real.stdout),
                    "fallback .SRCINFO generator diverged from makepkg --printsrcinfo in {}",
                    dir.display()
                );
            }
        }
    }
}
