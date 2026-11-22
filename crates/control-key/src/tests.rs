// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use std::ffi::OsString;
use std::path::Path;

#[test]
fn key_file_is_named_by_port() {
    let dir = Path::new("/settings/mailcraft");
    assert_eq!(key_path(dir, 7981), dir.join("control-key.local-7981"));
    assert_eq!(key_path(dir, 7982), dir.join("control-key.local-7982"));
}

#[test]
fn flatpak_instance_does_not_change_the_name() {
    // The app and a bridge run as different Flatpak instances (different ids) and must agree.
    // Nothing is read from the environment or /.flatpak-info, so the name is a
    // function of the folder and the port alone.
    let dir = std::env::temp_dir().join("mc-1199-keys");
    let app = key_path(&dir, 7991);
    let bridge = key_path(&dir, 7991);
    assert_eq!(app, bridge);
    assert_eq!(app.file_name().and_then(|n| n.to_str()), Some("control-key.local-7991"));
}

#[test]
fn bridge_finds_the_key_the_app_wrote() {
    let dir = std::env::temp_dir().join(format!("mc-1199-find-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(key_path(&dir, 7991), "abc").unwrap();
    std::fs::write(key_path(&dir, 7992), "other").unwrap();
    assert_eq!(std::fs::read_to_string(key_path(&dir, 7991)).unwrap(), "abc");
    assert_eq!(std::fs::read_to_string(key_path(&dir, 7992)).unwrap(), "other");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn settings_dir_is_the_apps_folder() {
    if let Some(d) = settings_dir() {
        let name = d.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert!(name == "ouromail" || name == "OuroMail", "{}", d.display());
    }
    if let Some(d) = legacy_mailcraft_settings_dir() {
        let name = d.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert!(name == "mailcraft" || name == "MailCraft", "{}", d.display());
    }
}

/// An environment of its own for a test: only the variables named, never the process's.
fn env(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
    move |name| vars.iter().find(|(n, _)| *n == name).map(|(_, v)| OsString::from(v))
}

#[test]
fn the_new_and_the_old_folder_are_found_from_the_same_variables() {
    let vars: &'static [(&str, &str)] = &[("XDG_CONFIG_HOME", "/x/config"), ("HOME", "/x/home"), ("APPDATA", "/x/appdata")];
    let (new, old) = (settings_dir_with(env(vars)).unwrap(), legacy_mailcraft_settings_dir_with(env(vars)).unwrap());
    if cfg!(target_os = "macos") {
        assert_eq!(new, Path::new("/x/home/Library/Application Support/OuroMail"));
        assert_eq!(old, Path::new("/x/home/Library/Application Support/MailCraft"));
    } else if cfg!(windows) {
        assert_eq!(new, Path::new("/x/appdata").join("OuroMail"));
        assert_eq!(old, Path::new("/x/appdata").join("MailCraft"));
    } else {
        assert_eq!(new, Path::new("/x/config/ouromail"));
        assert_eq!(old, Path::new("/x/config/mailcraft"));
    }
    // Side by side, in the same folder, and never the same one.
    assert_eq!(new.parent(), old.parent());
    assert_ne!(new, old);
}

#[cfg(not(any(target_os = "macos", windows)))]
#[test]
fn without_xdg_config_home_both_are_under_home() {
    let vars: &'static [(&str, &str)] = &[("HOME", "/x/home")];
    assert_eq!(settings_dir_with(env(vars)), Some(Path::new("/x/home/.config/ouromail").to_path_buf()));
    assert_eq!(legacy_mailcraft_settings_dir_with(env(vars)), Some(Path::new("/x/home/.config/mailcraft").to_path_buf()));
}

#[test]
fn without_any_of_the_variables_there_is_no_folder() {
    assert_eq!(settings_dir_with(env(&[])), None);
    assert_eq!(legacy_mailcraft_settings_dir_with(env(&[])), None);
}
