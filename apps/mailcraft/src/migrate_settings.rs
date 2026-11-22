// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The one-time copy of MailCraft's settings folder into OuroMail's, at OuroMail's first start.
//!
//! OuroMail keeps its settings, mailbox, logs and pictures in a folder of its own
//! (`mailcraft_control_key::settings_dir`: `~/.config/ouromail`, `%APPDATA%\OuroMail`,
//! `~/Library/Application Support/OuroMail`), so that it never writes into MailCraft's. The first
//! time it starts and that folder isn't there, what MailCraft kept
//! (`mailcraft_control_key::legacy_mailcraft_settings_dir`) is copied into it, once:
//!
//! - **MailCraft's folder is only read.** Nothing in it is written, renamed or removed.
//! - **Only when OuroMail's folder is missing**, by its own name (anything there, even an empty
//!   folder or a link, means it was set up already), and MailCraft's is a real folder, not a link.
//! - **All or nothing.** The copy is made in a new private folder beside the destination
//!   (`.ouromail-migrate-<pid>-<n>`, mode 0700, made with `create_dir`, so that it can neither be
//!   one somebody left there nor overwrite anything), and renamed into place when it is whole. On
//!   any failure that folder is removed and the start goes on without the copy.
//! - **Never through a link, and never what only lives while a program runs:** links, sockets,
//!   pipes and devices are skipped, and so are the control channel's key files
//!   (`control-key.*`), lock files (`*.lock`) and sockets (`*.sock`). A file is opened only after
//!   checking it is still the regular file that was looked at.
//! - **Bounded:** at most [`MAX_DEPTH`] folders deep and [`MAX_ENTRIES`] entries; past either the
//!   copy is abandoned as a whole. Files are streamed, a bounded buffer at a time, never read
//!   into memory.
//! - Folders are made 0700 and files 0600 on Unix: the settings are the owner's alone.
//!
//! Nothing here panics; what goes wrong is logged by its kind and the start continues.

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

/// Folders deeper than this under MailCraft's are not copied: the copy is abandoned instead (a
/// mailbox's content folder is two deep).
pub(crate) const MAX_DEPTH: usize = 8;
/// Entries the copy looks at before it gives up on the whole (a mailbox with many attachments
/// keeps one file for each in its content folder).
pub(crate) const MAX_ENTRIES: usize = 2_000_000;
/// How many names for the staging folder are tried before giving up.
const STAGING_TRIES: u32 = 16;
/// Maximum `ui.json` size inspected to rewrite MailCraft's default `"blue"` accent to `"ouro"`
/// (matches `main.rs`'s 4 MiB preferences read cap; larger files are left as copied).
const MAX_UI_JSON_BYTES: u64 = 4 * 1024 * 1024;

/// What became of the copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// MailCraft's folder was copied: the files and folders copied, and the entries skipped.
    Copied { files: usize, folders: usize, skipped: usize },
    /// Nothing was to be done, and nothing was done.
    NotNeeded(Why),
}

/// Why nothing was copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Why {
    /// OuroMail's folder is there already (whatever it is: a folder, even empty, a file, a link).
    DestinationExists,
    /// MailCraft's folder isn't there.
    NoSource,
    /// MailCraft's folder is a link, a file or anything else but a real folder.
    SourceNotAFolder,
    /// The two are the same folder, or one is inside the other.
    Overlapping,
}

/// At start: MailCraft's settings copied into OuroMail's folder when OuroMail has none yet. Never
/// a reason not to start: what goes wrong is logged.
pub(crate) fn at_start() {
    let (Some(dst), Some(src)) = (mailcraft_control_key::settings_dir(), mailcraft_control_key::legacy_mailcraft_settings_dir()) else {
        return;
    };
    match migrate(&src, &dst) {
        Ok(Outcome::Copied { files, folders, skipped }) => {
            log::info!("copied MailCraft's settings into OuroMail's folder: {files} files, {folders} folders ({skipped} skipped)");
        }
        Ok(Outcome::NotNeeded(_)) => {}
        Err(e) => log::warn!("MailCraft's settings were not copied into OuroMail's folder: {} ({e})", e.kind()),
    }
}

/// Copy the folder `src` to `dst`, which must not exist, all or nothing (see the module's
/// comment). `src` is only ever read.
pub(crate) fn migrate(src: &Path, dst: &Path) -> io::Result<Outcome> {
    if src == dst || src.starts_with(dst) || dst.starts_with(src) {
        return Ok(Outcome::NotNeeded(Why::Overlapping));
    }
    match fs::symlink_metadata(dst) {
        Ok(_) => return Ok(Outcome::NotNeeded(Why::DestinationExists)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    match fs::symlink_metadata(src) {
        Ok(m) if m.file_type().is_dir() => {}
        Ok(_) => return Ok(Outcome::NotNeeded(Why::SourceNotAFolder)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Outcome::NotNeeded(Why::NoSource)),
        Err(e) => return Err(e),
    }
    let parent = dst.parent().filter(|p| !p.as_os_str().is_empty()).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent folder"))?;
    fs::create_dir_all(parent)?;
    let staging = make_staging(parent)?;
    let mut count = Count::default();
    let copied = copy_folder(src, &staging, 0, &mut count).and_then(|()| {
        rewrite_default_accent(&staging);
        // Made by somebody else meanwhile (another start): theirs stays, the copy goes.
        if fs::symlink_metadata(dst).is_ok() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "the destination appeared during the copy"));
        }
        fs::rename(&staging, dst)
    });
    match copied {
        Ok(()) => {
            sync_folder(parent);
            Ok(Outcome::Copied { files: count.files, folders: count.folders, skipped: count.skipped })
        }
        Err(e) => {
            // Only the staging folder, which this function made and wrote alone (it holds no
            // links: none are ever made), is removed.
            if let Err(cleanup) = fs::remove_dir_all(&staging) {
                log::warn!("the unfinished copy of MailCraft's settings couldn't be removed: {}", cleanup.kind());
            }
            Err(e)
        }
    }
}

/// What the copy has looked at and done so far.
#[derive(Default)]
struct Count {
    entries: usize,
    files: usize,
    folders: usize,
    skipped: usize,
}

/// A new, private, empty folder in `parent` to copy into: made with `create_dir`, so a name
/// already taken (by anything, a link included) is never used.
fn make_staging(parent: &Path) -> io::Result<PathBuf> {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let mut last = io::Error::from(io::ErrorKind::AlreadyExists);
    for n in 0..STAGING_TRIES {
        let path = parent.join(format!(".ouromail-migrate-{pid}-{}", nanos.wrapping_add(n)));
        match new_folder(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = e,
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

/// Make the folder `path`, which must not exist: 0700 on Unix.
fn new_folder(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Whether an entry is something that lives only while a program runs, and is never copied: a
/// control channel key file, a lock file or a socket.
pub(crate) fn transient(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name.starts_with("control-key.") || name.ends_with(".lock") || name.ends_with(".sock")
}

/// Copy what is in the folder `from` into the existing folder `to`, `depth` folders down.
fn copy_folder(from: &Path, to: &Path, depth: usize, count: &mut Count) -> io::Result<()> {
    if depth > MAX_DEPTH {
        return Err(io::Error::other("folders nested too deep"));
    }
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        count.entries = count.entries.saturating_add(1);
        if count.entries > MAX_ENTRIES {
            return Err(io::Error::other("too many entries"));
        }
        let name = entry.file_name();
        if transient(&name) {
            count.skipped = count.skipped.saturating_add(1);
            continue;
        }
        let (source, target) = (from.join(&name), to.join(&name));
        // Never followed: a link is itself looked at, and skipped.
        let meta = fs::symlink_metadata(&source)?;
        let kind = meta.file_type();
        if kind.is_dir() {
            new_folder(&target)?;
            count.folders = count.folders.saturating_add(1);
            copy_folder(&source, &target, depth.saturating_add(1), count)?;
        } else if kind.is_file() {
            copy_file(&source, &target, &meta)?;
            count.files = count.files.saturating_add(1);
        } else {
            // A link, a socket, a pipe or a device.
            count.skipped = count.skipped.saturating_add(1);
        }
    }
    Ok(())
}

/// Copy the regular file `from` (looked at as `seen`) to the new file `to` (0600 on Unix),
/// streamed and flushed to the disk.
fn copy_file(from: &Path, to: &Path, seen: &fs::Metadata) -> io::Result<()> {
    let mut source = fs::File::open(from)?;
    // Still the file that was looked at: not swapped for a link (which `open` follows) since.
    let opened = source.metadata()?;
    if !opened.file_type().is_file() || !same_file(seen, &opened) {
        return Err(io::Error::other("a file changed while it was copied"));
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut target = options.open(to)?;
    io::copy(&mut source, &mut target)?;
    target.sync_all()
}

/// Whether two looks at a path saw the same file.
#[cfg(unix)]
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    a.dev() == b.dev() && a.ino() == b.ino()
}

/// Whether two looks at a path saw the same file: where the system gives no identity of a file,
/// its size and time of change stand in.
#[cfg(not(unix))]
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.len() == b.len() && a.modified().ok() == b.modified().ok()
}

/// In the copied top-level `ui.json` (never the source), rewrite MailCraft's default `"blue"`
/// accent to `"ouro"` so OuroMail opens in its own palette. Any other accent (a colour the owner
/// picked), a missing field, malformed JSON, or an oversized file is left byte-for-byte as copied.
/// Best effort: a failure here never aborts the migration.
fn rewrite_default_accent(staging: &Path) {
    let path = staging.join("ui.json");
    let Ok(meta) = fs::symlink_metadata(&path) else { return };
    if !meta.file_type().is_file() || meta.len() > MAX_UI_JSON_BYTES {
        return;
    }
    let Ok(file) = fs::File::open(&path) else { return };
    let mut bytes = Vec::new();
    if file.take(MAX_UI_JSON_BYTES.saturating_add(1)).read_to_end(&mut bytes).is_err() {
        return;
    }
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_UI_JSON_BYTES {
        return;
    }
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return };
    let Some(obj) = value.as_object_mut() else { return };
    let is_blue = obj.get("accent").and_then(serde_json::Value::as_str).is_some_and(|s| s.trim().eq_ignore_ascii_case("blue"));
    if !is_blue {
        return;
    }
    obj.insert("accent".to_string(), serde_json::Value::String("ouro".to_string()));
    let Ok(updated) = serde_json::to_vec_pretty(&value) else { return };
    if u64::try_from(updated.len()).unwrap_or(u64::MAX) > MAX_UI_JSON_BYTES {
        return;
    }
    // A temporary file of a fresh name beside it (0600 on Unix), flushed, then renamed over, and
    // removed when anything fails: no file the source folder happens to hold is in the way.
    let _ = mailcraft_engine::store::write_atomic(&path, &updated);
}

/// Flush the folder `dir`, so that the rename in it reaches the disk (where folders can be
/// flushed); best effort.
fn sync_folder(dir: &Path) {
    #[cfg(unix)]
    {
        if let Ok(dir) = fs::File::open(dir) {
            let _ = dir.sync_all();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
}

#[cfg(test)]
#[path = "migrate_settings_tests.rs"]
mod migrate_settings_tests;
