mod support;

use std::fs;

use support::Temp;
use twitty_ai::engine::patch::{Patch, PatchError, MAX_PATCH_BYTES};

#[test]
fn edit_create_delete_and_restore_exact_contents() {
    let temp = Temp::new();
    let root = &temp.0;
    fs::write(root.join("modify"), "one\r\ntwo").unwrap();
    fs::write(root.join("delete"), "old\n").unwrap();
    let diff = concat!(
        "diff --git a/modify b/modify\n",
        "index 39a7..7dd9 100644\n",
        "--- a/modify\n+++ b/modify\n@@ -1,2 +1,2 @@\n",
        " one\r\n-two\n\\ No newline at end of file\n+اثنان\n\\ No newline at end of file\n",
        "--- /dev/null\n+++ b/new\n@@ -0,0 +1 @@\n+first\n",
        "--- a/delete\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n"
    );
    let patch = Patch::validate(root, diff).unwrap();
    let summaries = patch.files();
    assert_eq!((summaries[0].added, summaries[0].removed), (1, 1));
    assert!(summaries[1].new_file);
    assert!(summaries[2].deleted);
    patch.apply().unwrap();
    assert_eq!(
        fs::read_to_string(root.join("modify")).unwrap(),
        "one\r\nاثنان"
    );
    assert_eq!(fs::read_to_string(root.join("new")).unwrap(), "first\n");
    assert!(!root.join("delete").exists());
    patch.undo().unwrap();
    assert_eq!(
        fs::read_to_string(root.join("modify")).unwrap(),
        "one\r\ntwo"
    );
    assert_eq!(fs::read_to_string(root.join("delete")).unwrap(), "old\n");
    assert!(!root.join("new").exists());
}

#[test]
fn validate_all_files_before_apply_or_undo() {
    let temp = Temp::new();
    let root = &temp.0;
    fs::write(root.join("a"), "old\n").unwrap();
    fs::write(root.join("b"), "old\n").unwrap();
    let diff = concat!(
        "--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n",
        "--- a/b\n+++ b/b\n@@ -1 +1 @@\n-old\n+new\n"
    );
    let patch = Patch::validate(root, diff).unwrap();
    fs::write(root.join("b"), "changed\n").unwrap();
    assert_eq!(patch.apply(), Err(PatchError::Conflict));
    assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "old\n");
    fs::write(root.join("b"), "old\n").unwrap();
    patch.apply().unwrap();
    fs::write(root.join("b"), "changed\n").unwrap();
    assert_eq!(patch.undo(), Err(PatchError::Conflict));
    assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "new\n");
}

#[test]
fn malformed_protected_binary_and_oversized_patches_are_rejected() {
    let temp = Temp::new();
    let root = &temp.0;
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join("binary"), b"text\0other").unwrap();
    fs::write(root.join("text"), "old\n").unwrap();
    let cases = [
        "--- /dev/null\n+++ b/../outside\n@@ -0,0 +1 @@\n+new\n",
        "--- /dev/null\n+++ b/.git/config\n@@ -0,0 +1 @@\n+new\n",
        "--- /dev/null\n+++ /etc/twitty\n@@ -0,0 +1 @@\n+new\n",
        "--- a/binary\n+++ b/binary\n@@ -1 +1 @@\n-old\n+new\n",
        "--- a/text\n+++ b/text\n@@ -1,2 +1 @@\n-old\n+new\n",
        "--- a/text\n+++ b/text\n@@ -1 +1 @@\n-wrong\n+new\n",
        "--- a/text\n+++ b/new\n@@ -1 +1 @@\n-old\n+new\n",
        "--- a/text\n+++ b/text\n@@ -1 +1 @@\n-old\n+new\n--- a/text\n+++ b/text\n@@ -1 +1 @@\n-old\n+new\n",
        "GIT binary patch\n",
    ];
    for diff in cases {
        assert!(Patch::validate(root, diff).is_err(), "{diff}");
    }
    assert!(Patch::validate(root, &"x".repeat(MAX_PATCH_BYTES + 1)).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(temp.0.parent().unwrap(), root.join("link")).unwrap();
        let diff = "--- /dev/null\n+++ b/link/twitty-patch-outside\n@@ -0,0 +1 @@\n+new\n";
        assert!(Patch::validate(root, diff).is_err());
    }
}
