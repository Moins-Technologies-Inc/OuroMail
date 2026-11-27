// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The directory search's text and answers (Add Shared Mailbox): what is sent of what was typed,
//! and what of a hostile answer is kept.

use proptest::prelude::*;
use serde_json::{Value, json};

use super::*;

fn page(value: Value) -> Option<DirectoryPage> {
    read_page(value.to_string().as_bytes())
}

#[test]
fn what_is_typed_is_sent_trimmed_on_one_line_without_ampersands_or_what_shows_nothing() {
    assert_eq!(search_text("  Help   Desk \n"), Some("Help Desk".to_string()));
    assert_eq!(search_text("Q&A\tteam"), Some("QA team".to_string()));
    assert_eq!(search_text("he\u{7}lp\u{202e}\u{200b}"), Some("help".to_string()), "control and format characters go");
    assert_eq!(search_text("\"quoted\" \\ back"), Some("\"quoted\" \\ back".to_string()), "escaped when the request is written");
    // Fewer than two characters left: nothing is asked.
    for short in ["", " ", "a", " a ", "&&&a&", "\u{200b}b\u{200b}"] {
        assert_eq!(search_text(short), None, "{short:?}");
    }
    assert_eq!(search_text("ab"), Some("ab".to_string()));
    assert_eq!(MIN_QUERY, 2);
    // At most 64 characters, counted in characters, and never ending in a space.
    let long = "é".repeat(200);
    assert_eq!(search_text(&long).map(|t| t.chars().count()), Some(MAX_QUERY));
    let spaced = format!("{} b", "a".repeat(63));
    assert_eq!(search_text(&spaced), Some("a".repeat(63)));
    // However much was pasted, only the start is looked at.
    let pasted = format!("{}{}", " ".repeat(10_000), "late");
    assert_eq!(search_text(&pasted), None);
    let query = DirectoryQuery { account: 3, text: "  Su ".into(), ticket: 9 };
    assert_eq!(query.search_text(), Some("Su".to_string()));
}

#[test]
fn an_entry_is_kept_only_at_a_mailboxs_own_address_with_its_name_cleaned() {
    assert_eq!(
        DirectoryEntry::found(" Help\u{202e} Desk\n", " helpdesk@example.org "),
        Some(DirectoryEntry { name: "Help Desk".into(), address: "helpdesk@example.org".into() })
    );
    assert_eq!(DirectoryEntry::found("", "a@example.org").map(|e| e.name), Some(String::new()));
    for address in ["", "nobody", "a@b", "x/../y@example.org", "a b@example.org", "ü@example.org", "a@example.org\r\nBcc: x@example.org"] {
        assert_eq!(DirectoryEntry::found("Somebody", address), None, "{address:?}");
    }
    let name = "N".repeat(10_000);
    assert_eq!(DirectoryEntry::found(&name, "n@example.org").map(|e| e.name.chars().count()), Some(MAX_NAME));
}

#[test]
fn a_page_keeps_fifteen_usable_entries_once_each_and_says_when_there_are_more() {
    let users: Vec<Value> = (0..20).map(|i| json!({"displayName": format!("Team {i:02}"), "mail": format!("team{i:02}@example.org")})).collect();
    let read = page(json!({"value": users})).unwrap();
    assert_eq!(read.entries.len(), MAX_RESULTS);
    assert_eq!(read.entries.first().map(|e| e.address.as_str()), Some("team00@example.org"));
    assert!(read.more, "more usable entries than are kept");
    // Fifteen and no next page: no more.
    let users: Vec<Value> = (0..15).map(|i| json!({"mail": format!("t{i}@example.org")})).collect();
    assert!(!page(json!({"value": users})).unwrap().more);
    // A next page only says that there is more.
    let read = page(json!({"value": [{"mail": "a@example.org"}], "@odata.nextLink": "https://graph.microsoft.com/v1.0/users?$skiptoken=x"})).unwrap();
    assert_eq!((read.entries.len(), read.more), (1, true));
    // Without a usable mail, the same address twice, fields that aren't text: left out.
    let read = page(json!({"value": [
        {"displayName": "No Mail", "userPrincipalName": "nomail@example.org"},
        {"displayName": "Null Mail", "mail": null},
        {"displayName": "Number", "mail": 7},
        {"displayName": ["x"], "mail": "array.name@example.org"},
        {"displayName": "Help Desk", "mail": "helpdesk@example.org"},
        {"displayName": "Help Desk again", "mail": "HelpDesk@Example.org"},
        "not an object",
        42
    ]}))
    .unwrap();
    let addresses: Vec<&str> = read.entries.iter().map(|e| e.address.as_str()).collect();
    assert_eq!(addresses, ["array.name@example.org", "helpdesk@example.org"]);
    assert_eq!(read.entries.first().map(|e| e.name.as_str()), Some(""), "a name that isn't text is none");
    assert!(!read.more);
    // Not a page: nothing.
    for body in [&b""[..], b"null", b"[]", b"{}", b"{\"value\": {}}", b"{\"value\": \"x\"}", b"\xff\xfe", b"{\"value\": ["] {
        assert_eq!(read_page(body), None, "{:?}", String::from_utf8_lossy(body));
    }
    assert_eq!(page(json!({"value": []})), Some(DirectoryPage::default()));
}

/// An entry with a usable address whose name is missing or isn't text is listed by its address,
/// with an empty name; one whose `mail` isn't text is left out, whatever its name.
#[test]
fn a_name_that_isnt_text_leaves_the_entry_with_an_empty_name() {
    let names = [
        None,
        Some(Value::Null),
        Some(json!(7)),
        Some(json!(1.5)),
        Some(json!(true)),
        Some(json!(["Help Desk"])),
        Some(json!({"name": "Help Desk"})),
    ];
    for name in names {
        let mut user = json!({"mail": "helpdesk@example.org"});
        if let Some(name) = name.clone() {
            user["displayName"] = name;
        }
        let read = page(json!({"value": [user]})).unwrap();
        assert_eq!(read.entries, [DirectoryEntry { name: String::new(), address: "helpdesk@example.org".into() }], "{name:?}");
    }
    for mail in [Value::Null, json!(7), json!(true), json!(["helpdesk@example.org"]), json!({"address": "helpdesk@example.org"})] {
        let read = page(json!({"value": [{"displayName": "Help Desk", "mail": mail}]})).unwrap();
        assert!(read.entries.is_empty(), "{mail:?}");
    }
}

#[test]
fn a_page_of_more_entries_than_are_looked_at_stops_there() {
    // Many entries without a usable address before one that has one: not looked at past the cap.
    let mut users: Vec<Value> = (0..MAX_SCANNED).map(|_| json!({"mail": "nobody"})).collect();
    users.push(json!({"mail": "late@example.org"}));
    let read = page(json!({"value": users})).unwrap();
    assert!(read.entries.is_empty() && read.more);
}

/// A JSON value of any shape, a few levels deep.
fn any_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| json!(n)),
        ".{0,40}".prop_map(Value::String),
        "[a-z]{1,8}@example\\.org".prop_map(Value::String),
    ];
    leaf.prop_recursive(4, 64, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
            prop::collection::vec((prop_oneof![Just("mail".to_string()), Just("displayName".to_string()), ".{0,6}"], inner), 0..6)
                .prop_map(|pairs| Value::Object(pairs.into_iter().collect())),
        ]
    })
}

/// What is kept of any answer holds the caps, whatever the answer.
fn holds_the_caps(read: &DirectoryPage) {
    assert!(read.entries.len() <= MAX_RESULTS);
    for (i, entry) in read.entries.iter().enumerate() {
        assert_eq!(crate::mailbox_address(&entry.address).as_deref(), Some(entry.address.as_str()));
        assert!(entry.name.chars().count() <= MAX_NAME && !entry.name.chars().any(|c| c.is_control() || mailcraft_names::is_format(c)));
        assert!(read.entries.iter().skip(i + 1).all(|other| !other.address.eq_ignore_ascii_case(&entry.address)));
    }
}

proptest! {
    #[test]
    fn hostile_answers_never_panic(body in prop::collection::vec(any::<u8>(), 0..2048)) {
        if let Some(read) = read_page(&body) {
            holds_the_caps(&read);
        }
    }

    #[test]
    fn hostile_pages_never_panic(users in prop::collection::vec(any_json(), 0..40), next in any_json()) {
        let body = json!({"value": users, "@odata.nextLink": next}).to_string();
        let read = read_page(body.as_bytes());
        prop_assert!(read.is_some());
        if let Some(read) = read {
            holds_the_caps(&read);
        }
    }

    #[test]
    fn any_typed_text_is_sent_within_its_caps(typed in ".{0,200}") {
        if let Some(text) = search_text(&typed) {
            prop_assert!((MIN_QUERY..=MAX_QUERY).contains(&text.chars().count()));
            prop_assert!(!text.contains('&') && !text.contains("  ") && text.trim() == text);
            prop_assert!(!text.chars().any(|c| c.is_control() || mailcraft_names::is_format(c)));
        }
    }
}
