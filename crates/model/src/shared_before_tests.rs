// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The shared mailboxes an account opened before (Q4), as a mailbox file has them: a file from
//! before there were any opens with none, and a hostile list is capped and cleaned on reading.
//! Every address is invented.

use serde_json::json;

use crate::{Account, AccountKind, MAX_SHARED, Mailbox};

fn read(account: serde_json::Value) -> Mailbox {
    let mut mb: Mailbox = serde_json::from_value(json!({"accounts": [account]})).unwrap();
    mb.repair();
    mb
}

#[test]
fn a_file_from_before_opens_with_none() {
    let mb = read(json!({"id": 1, "email": "owner@example.org", "kind": "microsoft"}));
    assert!(mb.accounts[0].shared_before.is_empty());
    let back = serde_json::to_string(&mb).unwrap();
    assert!(!back.contains("sharedBefore"), "nothing written for none");
}

#[test]
fn what_is_not_a_list_is_none() {
    for junk in [json!("support@example.org"), json!(7), json!({"address": "support@example.org"}), json!(null)] {
        let mb = read(json!({"id": 1, "email": "owner@example.org", "kind": "microsoft", "sharedBefore": junk}));
        assert!(mb.accounts[0].shared_before.is_empty(), "{junk}");
    }
}

#[test]
fn a_hostile_shared_before_in_a_file_is_capped_and_cleaned() {
    let mut list = vec![
        json!({"address": "Support@Example.org", "name": "Sup\u{202e}port\u{0}\r\nTeam"}),
        json!({"address": "SUPPORT@example.org", "name": "twice"}),
        json!({"address": "not an address", "name": "x"}),
        json!({"address": "owner@example.org", "name": "its own"}),
        json!({"address": 7, "name": []}),
        json!(42),
        json!({"address": "long@example.org", "name": "n".repeat(10_000)}),
        json!({"address": format!("{}@example.org", "a".repeat(400))}),
        json!({"address": "\u{0}evil@example.org"}),
    ];
    list.extend((0..50).map(|i| json!({"address": format!("team{i:02}@example.org")})));
    let mb = read(json!({"id": 1, "email": "owner@example.org", "kind": "microsoft", "sharedBefore": list}));
    let kept = &mb.accounts[0].shared_before;
    assert_eq!(kept.len(), MAX_SHARED, "at most {MAX_SHARED}");
    assert!(kept[0].address.eq_ignore_ascii_case("support@example.org"), "{:?}", kept[0]);
    assert_eq!(kept.iter().filter(|e| e.address.eq_ignore_ascii_case("support@example.org")).count(), 1, "once");
    assert!(kept.iter().all(|e| crate::mailbox_address(&e.address).as_deref() == Some(e.address.as_str())), "each an address as an address is read");
    assert!(!kept.iter().any(|e| e.address.eq_ignore_ascii_case("owner@example.org")), "never the account's own");
    for e in kept {
        assert!(e.name.chars().count() <= crate::MAX_NAME, "{}", e.name.len());
        assert!(!e.name.chars().any(|c| c.is_control() || ('\u{202a}'..='\u{202e}').contains(&c)), "{:?}", e.name);
    }
    assert_eq!(kept[0].name, "Support Team");
    assert_eq!(kept[1].address, "long@example.org");
}

#[test]
fn only_an_account_that_can_open_shared_mailboxes_keeps_any() {
    let entry = json!([{"address": "support@example.org"}]);
    let local = read(json!({"id": 1, "email": "ada@example.org", "sharedBefore": entry}));
    assert!(local.accounts[0].shared_before.is_empty(), "a local account");
    let mut mb: Mailbox = serde_json::from_value(json!({"accounts": [
        {"id": 1, "email": "owner@example.org", "kind": "microsoft"},
        {"id": 2, "email": "support@example.org", "kind": "microsoft", "sharedOf": 1, "sharedBefore": [{"address": "sales@example.org"}]},
    ]}))
    .unwrap();
    mb.repair();
    assert!(mb.accounts.iter().all(|a| a.shared_before.is_empty()), "a shared mailbox opens none");
}

#[test]
fn one_open_now_is_not_among_those_opened_before() {
    let mut mb: Mailbox = serde_json::from_value(json!({"accounts": [
        {"id": 1, "email": "owner@example.org", "kind": "microsoft", "sharedBefore": [{"address": "support@example.org"}, {"address": "sales@example.org"}]},
        {"id": 2, "email": "SUPPORT@example.org", "kind": "microsoft", "sharedOf": 1},
    ]}))
    .unwrap();
    mb.repair();
    let owner = mb.accounts.iter().find(|a| a.id == 1).unwrap();
    let kept: Vec<&str> = owner.shared_before.iter().map(|e| e.address.as_str()).collect();
    assert_eq!(kept, ["sales@example.org"]);
}

#[test]
fn remembering_cleans_dedupes_and_caps() {
    let mut a = Account { id: 1, email: "owner@example.org".into(), kind: AccountKind::Microsoft, ..Default::default() };
    a.remember_shared("not an address", "x");
    assert!(a.shared_before.is_empty());
    for i in 0..30 {
        a.remember_shared(&format!("team{i:02}@example.org"), "\u{202e}Team\u{0}");
    }
    a.remember_shared("TEAM29@example.org", "Again");
    assert_eq!(a.shared_before.len(), MAX_SHARED);
    assert_eq!(a.shared_before.iter().filter(|e| e.address.eq_ignore_ascii_case("team29@example.org")).count(), 1);
    assert_eq!(a.shared_before[0].name, "Again");
    assert_eq!(a.shared_before[1].name, "Team");
    assert_eq!(a.forget_shared(Some("team28@EXAMPLE.org")), 1);
    assert_eq!(a.forget_shared(Some("team28@example.org")), 0);
    assert_eq!(a.forget_shared(None), MAX_SHARED - 1);
}
