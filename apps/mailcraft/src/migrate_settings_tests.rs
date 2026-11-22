// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The one-time copy of MailCraft's settings into OuroMail's folder: synthetic folders in a
//! private temporary folder only, never anybody's real settings.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::{MAX_DEPTH, Outcome, Why, migrate, transient};

/// A private scratch folder for a test, and MailCraft's and OuroMail's folders side by side in
/// it (the source made, the destination not).
fn scratch() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::Builder::new().prefix("om-mig").tempdir().unwrap();
    let src = root.path().join("mailcraft");
    let dst = root.path().join("ouromail");
    fs::create_dir(&src).unwrap();
    (root, src, dst)
}

fn put(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// Everything under `dir`, as it is: each entry's kind, bytes (a link's target), and on Unix its
/// mode and time of change, so that a test can tell the folder was left exactly as it was.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, String> {
    let mut all = BTreeMap::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(at) = todo.pop() {
        for entry in fs::read_dir(&at).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let what = if meta.file_type().is_symlink() {
                format!("link {}", fs::read_link(&path).unwrap().display())
            } else if meta.is_dir() {
                todo.push(path.clone());
                "folder".to_string()
            } else if meta.is_file() {
                format!("file {:?}", fs::read(&path).unwrap())
            } else {
                "other".to_string()
            };
            #[cfg(unix)]
            let what = {
                use std::os::unix::fs::MetadataExt as _;
                format!("{what} mode {:o} mtime {}.{}", meta.mode(), meta.mtime(), meta.mtime_nsec())
            };
            all.insert(path.strip_prefix(dir).unwrap().to_path_buf(), what);
        }
    }
    all
}

/// The staging folders left in `parent` (none, after any copy).
fn staging_left(parent: &Path) -> Vec<String> {
    fs::read_dir(parent)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".ouromail-migrate-"))
        .collect()
}

#[test]
fn nested_files_are_copied_and_the_source_is_left_exactly_as_it_was() {
    let (root, src, dst) = scratch();
    put(&src.join("mailbox.mailcraft"), b"the mailbox");
    put(&src.join("ui.json"), b"{\"theme\": \"dark\"}");
    put(&src.join("pictures/hash.png"), b"\x89PNG not really");
    put(&src.join("mailbox.mailcraft.content/ab/cd"), b"a body");
    let before = snapshot(&src);
    let outcome = migrate(&src, &dst).unwrap();
    assert_eq!(outcome, Outcome::Copied { files: 4, folders: 3, skipped: 0 });
    assert_eq!(fs::read(dst.join("mailbox.mailcraft")).unwrap(), b"the mailbox");
    assert_eq!(fs::read(dst.join("ui.json")).unwrap(), b"{\"theme\": \"dark\"}");
    assert_eq!(fs::read(dst.join("pictures/hash.png")).unwrap(), b"\x89PNG not really");
    assert_eq!(fs::read(dst.join("mailbox.mailcraft.content/ab/cd")).unwrap(), b"a body");
    // Byte for byte, mode for mode and time for time, MailCraft's folder is as it was.
    assert_eq!(snapshot(&src), before);
    assert!(staging_left(root.path()).is_empty());
}

#[cfg(unix)]
#[test]
fn the_copy_is_private_to_the_owner() {
    use std::os::unix::fs::PermissionsExt as _;
    let (_root, src, dst) = scratch();
    put(&src.join("ui.json"), b"{}");
    put(&src.join("pictures/hash.png"), b"picture");
    // Open to others in MailCraft's folder: private in the copy all the same.
    fs::set_permissions(src.join("ui.json"), fs::Permissions::from_mode(0o644)).unwrap();
    fs::set_permissions(src.join("pictures"), fs::Permissions::from_mode(0o755)).unwrap();
    migrate(&src, &dst).unwrap();
    let mode = |p: &Path| fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dst), 0o700);
    assert_eq!(mode(&dst.join("pictures")), 0o700);
    assert_eq!(mode(&dst.join("ui.json")), 0o600);
    assert_eq!(mode(&dst.join("pictures/hash.png")), 0o600);
    // And MailCraft's own are left as they were.
    assert_eq!(mode(&src.join("ui.json")), 0o644);
    assert_eq!(mode(&src.join("pictures")), 0o755);
}

#[test]
fn nothing_is_done_when_ouromails_folder_is_there_even_empty() {
    let (_root, src, dst) = scratch();
    put(&src.join("ui.json"), b"mailcraft's");
    fs::create_dir(&dst).unwrap();
    assert_eq!(migrate(&src, &dst).unwrap(), Outcome::NotNeeded(Why::DestinationExists));
    assert_eq!(fs::read_dir(&dst).unwrap().count(), 0, "the empty folder stays empty");
    // A file in its place is the same.
    fs::remove_dir(&dst).unwrap();
    put(&dst, b"a file");
    assert_eq!(migrate(&src, &dst).unwrap(), Outcome::NotNeeded(Why::DestinationExists));
    assert_eq!(fs::read(&dst).unwrap(), b"a file");
}

#[cfg(unix)]
#[test]
fn a_link_in_ouromails_place_counts_as_there() {
    let (root, src, dst) = scratch();
    put(&src.join("ui.json"), b"mailcraft's");
    // Even a link to nothing.
    std::os::unix::fs::symlink(root.path().join("nowhere"), &dst).unwrap();
    assert_eq!(migrate(&src, &dst).unwrap(), Outcome::NotNeeded(Why::DestinationExists));
    assert!(!root.path().join("nowhere").exists());
}

#[test]
fn nothing_is_done_without_mailcrafts_folder() {
    let (root, src, dst) = scratch();
    fs::remove_dir(&src).unwrap();
    assert_eq!(migrate(&src, &dst).unwrap(), Outcome::NotNeeded(Why::NoSource));
    assert!(fs::symlink_metadata(&dst).is_err());
    // A file where the folder would be is no folder.
    put(&src, b"a file");
    assert_eq!(migrate(&src, &dst).unwrap(), Outcome::NotNeeded(Why::SourceNotAFolder));
    assert!(fs::symlink_metadata(&dst).is_err());
    assert!(staging_left(root.path()).is_empty());
}

#[cfg(unix)]
#[test]
fn mailcrafts_folder_as_a_link_is_not_followed() {
    let (root, src, dst) = scratch();
    let elsewhere = root.path().join("elsewhere");
    put(&elsewhere.join("ui.json"), b"somebody else's");
    fs::remove_dir(&src).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &src).unwrap();
    assert_eq!(migrate(&src, &dst).unwrap(), Outcome::NotNeeded(Why::SourceNotAFolder));
    assert!(fs::symlink_metadata(&dst).is_err());
    assert_eq!(fs::read(elsewhere.join("ui.json")).unwrap(), b"somebody else's");
}

#[test]
fn the_same_folder_or_one_inside_the_other_is_never_copied() {
    let (_root, src, _dst) = scratch();
    put(&src.join("ui.json"), b"{}");
    assert_eq!(migrate(&src, &src).unwrap(), Outcome::NotNeeded(Why::Overlapping));
    assert_eq!(migrate(&src, &src.join("ouromail")).unwrap(), Outcome::NotNeeded(Why::Overlapping));
    assert_eq!(migrate(&src.join("inner"), &src).unwrap(), Outcome::NotNeeded(Why::Overlapping));
    assert!(fs::symlink_metadata(src.join("ouromail")).is_err());
}

#[test]
fn what_lives_only_while_a_program_runs_is_skipped() {
    let (_root, src, dst) = scratch();
    put(&src.join("ui.json"), b"{}");
    put(&src.join("control-key.local-7991"), b"a key");
    put(&src.join("mailbox.mailcraft.lock"), b"");
    put(&src.join("instance.sock"), b"");
    put(&src.join("pictures/old.lock"), b"");
    let outcome = migrate(&src, &dst).unwrap();
    assert_eq!(outcome, Outcome::Copied { files: 1, folders: 1, skipped: 4 });
    assert!(dst.join("ui.json").exists());
    for gone in ["control-key.local-7991", "mailbox.mailcraft.lock", "instance.sock", "pictures/old.lock"] {
        assert!(fs::symlink_metadata(dst.join(gone)).is_err(), "{gone}");
        assert!(src.join(gone).exists(), "{gone} stays in MailCraft's folder");
    }
    // The rule itself.
    for name in ["control-key.local-7991", "control-key.7991", "a.lock", "instance.sock"] {
        assert!(transient(std::ffi::OsStr::new(name)), "{name}");
    }
    for name in ["ui.json", "mailbox.mailcraft", "control-keys.txt", "locks", "sockets"] {
        assert!(!transient(std::ffi::OsStr::new(name)), "{name}");
    }
}

#[cfg(unix)]
#[test]
fn links_and_sockets_inside_are_skipped_and_never_followed() {
    let (root, src, dst) = scratch();
    let outside = root.path().join("outside");
    put(&outside.join("secret.txt"), b"not mailcraft's");
    put(&src.join("ui.json"), b"{}");
    std::os::unix::fs::symlink(outside.join("secret.txt"), src.join("link.txt")).unwrap();
    std::os::unix::fs::symlink(&outside, src.join("linked-folder")).unwrap();
    // A real socket, as a running program leaves one.
    let socket = std::os::unix::net::UnixListener::bind(src.join("s")).unwrap();
    let outcome = migrate(&src, &dst).unwrap();
    drop(socket);
    assert_eq!(outcome, Outcome::Copied { files: 1, folders: 0, skipped: 3 });
    for gone in ["link.txt", "linked-folder", "s"] {
        assert!(fs::symlink_metadata(dst.join(gone)).is_err(), "{gone}");
    }
    assert!(snapshot(&dst).keys().all(|p| p == Path::new("ui.json")), "{:?}", snapshot(&dst));
    assert_eq!(fs::read(outside.join("secret.txt")).unwrap(), b"not mailcraft's");
}

#[test]
fn a_failure_leaves_no_copy_and_no_staging_folder() {
    let (root, src, dst) = scratch();
    put(&src.join("ui.json"), b"{}");
    // Deeper than the copy goes: abandoned as a whole.
    let mut deep = src.join("deep");
    for n in 0..=MAX_DEPTH {
        deep = deep.join(format!("d{n}"));
    }
    put(&deep.join("file"), b"far down");
    let before = snapshot(&src);
    assert!(migrate(&src, &dst).is_err());
    assert!(fs::symlink_metadata(&dst).is_err(), "no partial copy");
    assert!(staging_left(root.path()).is_empty(), "{:?}", staging_left(root.path()));
    assert_eq!(snapshot(&src), before);
    // Once the folders are shallow enough, the next start copies them.
    fs::remove_dir_all(src.join("deep")).unwrap();
    assert_eq!(migrate(&src, &dst).unwrap(), Outcome::Copied { files: 1, folders: 0, skipped: 0 });
}

#[cfg(unix)]
#[test]
fn an_unreadable_file_leaves_no_copy_and_no_staging_folder() {
    use std::os::unix::fs::PermissionsExt as _;
    let (root, src, dst) = scratch();
    put(&src.join("ui.json"), b"{}");
    put(&src.join("pictures/locked.png"), b"picture");
    fs::set_permissions(src.join("pictures/locked.png"), fs::Permissions::from_mode(0o000)).unwrap();
    // As root everything can be read, and nothing fails.
    if fs::File::open(src.join("pictures/locked.png")).is_ok() {
        return;
    }
    assert!(migrate(&src, &dst).is_err());
    assert!(fs::symlink_metadata(&dst).is_err(), "no partial copy");
    assert!(staging_left(root.path()).is_empty());
    fs::set_permissions(src.join("pictures/locked.png"), fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn a_second_start_copies_nothing() {
    let (_root, src, dst) = scratch();
    put(&src.join("ui.json"), b"first");
    assert!(matches!(migrate(&src, &dst).unwrap(), Outcome::Copied { .. }));
    // MailCraft changes its file afterwards; OuroMail's copy is its own from now on.
    put(&src.join("ui.json"), b"changed later");
    put(&dst.join("ui.json"), b"ouromail's own");
    assert_eq!(migrate(&src, &dst).unwrap(), Outcome::NotNeeded(Why::DestinationExists));
    assert_eq!(fs::read(dst.join("ui.json")).unwrap(), b"ouromail's own");
}

#[test]
fn migrated_blue_accent_becomes_ouro_while_source_stays_unchanged() {
    let (_root, src, dst) = scratch();
    put(&src.join("ui.json"), br#"{"theme": "dark", "accent": "blue", "closeToTray": true}"#);
    let before = snapshot(&src);
    assert_eq!(migrate(&src, &dst).unwrap(), Outcome::Copied { files: 1, folders: 0, skipped: 0 });
    let got: serde_json::Value = serde_json::from_slice(&fs::read(dst.join("ui.json")).unwrap()).unwrap();
    assert_eq!(got["accent"], "ouro");
    assert_eq!(got["theme"], "dark");
    assert_eq!(got["closeToTray"], true);
    assert_eq!(snapshot(&src), before, "MailCraft's ui.json must stay byte-for-byte untouched");
}

#[test]
fn migrated_blue_accent_is_matched_case_insensitively_with_whitespace() {
    let (_root, src, dst) = scratch();
    put(&src.join("ui.json"), br#"{"accent": " BLUE "}"#);
    migrate(&src, &dst).unwrap();
    let got: serde_json::Value = serde_json::from_slice(&fs::read(dst.join("ui.json")).unwrap()).unwrap();
    assert_eq!(got["accent"], "ouro");
}

#[test]
fn non_blue_accent_and_nested_or_malformed_ui_json_are_left_verbatim() {
    let (_root, src, dst) = scratch();
    let custom = br#"{"accent": "violet", "theme": "light"}"#;
    let nested = br#"{"accent": "blue"}"#;
    put(&src.join("ui.json"), custom);
    put(&src.join("sub/ui.json"), nested);
    migrate(&src, &dst).unwrap();
    assert_eq!(fs::read(dst.join("ui.json")).unwrap(), custom);
    assert_eq!(fs::read(dst.join("sub/ui.json")).unwrap(), nested, "only top-level ui.json is rewritten");

    // Malformed top-level ui.json stays byte-for-byte as copied.
    let (_root2, src2, dst2) = scratch();
    let bad = b"{not valid json";
    put(&src2.join("ui.json"), bad);
    migrate(&src2, &dst2).unwrap();
    assert_eq!(fs::read(dst2.join("ui.json")).unwrap(), bad);
}

/// The names in `dir` itself, sorted.
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

#[test]
fn a_ui_json_tmp_already_in_the_source_does_not_stop_the_accent_rewrite() {
    let (root, src, dst) = scratch();
    put(&src.join("ui.json"), br#"{"accent": "blue"}"#);
    put(&src.join(".ui.json.tmp"), b"left there by somebody");
    let before = snapshot(&src);
    assert_eq!(migrate(&src, &dst).unwrap(), Outcome::Copied { files: 2, folders: 0, skipped: 0 });
    let got: serde_json::Value = serde_json::from_slice(&fs::read(dst.join("ui.json")).unwrap()).unwrap();
    assert_eq!(got["accent"], "ouro", "the rewrite must not depend on a fixed temporary name");
    assert_eq!(fs::read(dst.join(".ui.json.tmp")).unwrap(), b"left there by somebody", "the other file is copied as it was");
    assert_eq!(names_in(&dst), [".ui.json.tmp", "ui.json"], "no temporary file is left behind");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(fs::metadata(dst.join("ui.json")).unwrap().permissions().mode() & 0o777, 0o600, "the rewritten file is the owner's alone");
    }
    assert_eq!(snapshot(&src), before, "MailCraft's folder must stay byte-for-byte untouched");
    assert!(staging_left(root.path()).is_empty());
}

#[test]
fn a_ui_json_that_is_not_an_object_or_whose_accent_is_not_a_string_is_left_verbatim() {
    let shapes: [&[u8]; 9] = [
        b"[1, 2, 3]",
        b"\"blue\"",
        b"null",
        b"42",
        br#"{"accent": 123}"#,
        br#"{"accent": null}"#,
        br#"{"accent": true}"#,
        br#"{"accent": ["blue"]}"#,
        br#"{"accent": {"name": "blue"}}"#,
    ];
    for bytes in shapes {
        let shown = String::from_utf8_lossy(bytes);
        let (root, src, dst) = scratch();
        put(&src.join("ui.json"), bytes);
        let before = snapshot(&src);
        assert_eq!(migrate(&src, &dst).unwrap(), Outcome::Copied { files: 1, folders: 0, skipped: 0 }, "{shown}");
        assert_eq!(fs::read(dst.join("ui.json")).unwrap(), bytes, "{shown} stays as it was copied");
        assert_eq!(names_in(&dst), ["ui.json"], "{shown}: nothing else is left in the copy");
        assert_eq!(snapshot(&src), before, "{shown}: MailCraft's folder is untouched");
        assert!(staging_left(root.path()).is_empty(), "{shown}");
    }
}

#[test]
fn a_ui_json_over_the_size_cap_is_copied_verbatim_with_its_blue_accent() {
    let (root, src, dst) = scratch();
    let mut bytes = br#"{"accent": "blue", "pad": ""#.to_vec();
    bytes.resize(usize::try_from(super::MAX_UI_JSON_BYTES).unwrap() + 64, b'x');
    bytes.extend_from_slice(b"\"}");
    // Within the cap this would have been rewritten: it is JSON with the default accent.
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed["accent"], "blue");
    put(&src.join("ui.json"), &bytes);
    let before = snapshot(&src);
    assert_eq!(migrate(&src, &dst).unwrap(), Outcome::Copied { files: 1, folders: 0, skipped: 0 });
    assert!(fs::read(dst.join("ui.json")).unwrap() == bytes, "an oversized ui.json is copied as it is");
    assert_eq!(names_in(&dst), ["ui.json"]);
    assert!(snapshot(&src) == before, "MailCraft's folder is untouched");
    assert!(staging_left(root.path()).is_empty());
}

#[cfg(unix)]
#[test]
fn a_symlinked_ui_json_is_not_followed_and_its_target_is_left_alone() {
    let (root, src, dst) = scratch();
    let elsewhere = root.path().join("elsewhere.json");
    put(&elsewhere, br#"{"accent": "blue"}"#);
    std::os::unix::fs::symlink(&elsewhere, src.join("ui.json")).unwrap();
    let before = snapshot(&src);
    let target_before = snapshot(root.path()).get(Path::new("elsewhere.json")).cloned();
    assert_eq!(migrate(&src, &dst).unwrap(), Outcome::Copied { files: 0, folders: 0, skipped: 1 });
    assert!(fs::symlink_metadata(dst.join("ui.json")).is_err(), "a link is never copied, nor what it points at");
    assert_eq!(names_in(&dst), Vec::<String>::new());
    assert_eq!(snapshot(root.path()).get(Path::new("elsewhere.json")).cloned(), target_before, "the link's target is untouched");
    assert_eq!(snapshot(&src), before, "MailCraft's folder is untouched");
    assert!(staging_left(root.path()).is_empty());
}
