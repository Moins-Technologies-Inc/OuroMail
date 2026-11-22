// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The control channel's key file, one rule for the app (which writes it) and the MCP bridge
//! (which reads it): `<settings>/control-key.local-<port>`. See `docs/control-protocol.md`, Keys.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub mod proof;
#[cfg(test)]
mod proof_tests;
#[cfg(test)]
mod tests;

/// The settings folder's name: on macOS and Windows, and on the other systems.
const NAMES: (&str, &str) = ("OuroMail", "ouromail");
/// The name the settings folder had before OuroMail, MailCraft's own: what
/// [`legacy_mailcraft_settings_dir`] finds, for the one-time copy at the first start.
const LEGACY_NAMES: (&str, &str) = ("MailCraft", "mailcraft");

/// The app's settings folder (preferences, logs, key files): macOS
/// `~/Library/Application Support/OuroMail`, Windows `%APPDATA%\OuroMail`, otherwise
/// `$XDG_CONFIG_HOME/ouromail` or `~/.config/ouromail`.
pub fn settings_dir() -> Option<PathBuf> {
    settings_dir_with(|name| std::env::var_os(name))
}

/// The folder MailCraft kept its settings in (`MailCraft` on macOS and Windows, `mailcraft`
/// otherwise), found from the same variables as [`settings_dir`]. Only ever read: OuroMail copies
/// it once into its own folder when that doesn't exist yet, and never writes into it.
pub fn legacy_mailcraft_settings_dir() -> Option<PathBuf> {
    legacy_mailcraft_settings_dir_with(|name| std::env::var_os(name))
}

/// [`settings_dir`] with the environment read through `env`.
pub(crate) fn settings_dir_with(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    folder(&env, NAMES)
}

/// [`legacy_mailcraft_settings_dir`] with the environment read through `env`.
pub(crate) fn legacy_mailcraft_settings_dir_with(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    folder(&env, LEGACY_NAMES)
}

/// The folder named `names` (the macOS and Windows name, the name elsewhere) where this system
/// keeps an application's settings.
fn folder(env: &impl Fn(&str) -> Option<OsString>, (titled, lower): (&str, &str)) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        env("HOME").map(|h| PathBuf::from(h).join("Library/Application Support").join(titled))
    } else if cfg!(windows) {
        env("APPDATA").map(|a| PathBuf::from(a).join(titled))
    } else {
        env("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".config"))).map(|c| c.join(lower))
    }
}

/// `<settings>/control-key.local-<port>`: named by the port the app listens on, so a bridge that
/// connects to a port finds the key of the app on that port. (Inside Flatpak the name once held
/// the instance id, which differs for every `flatpak run`, so a bridge run as an instance of its
/// own never found the app's key, #1199.) Only one app can hold a port, so one file per port is
/// enough, and a stale file from an earlier run is replaced when the app starts.
pub fn key_path(settings: &Path, port: u16) -> PathBuf {
    settings.join(format!("control-key.local-{port}"))
}
