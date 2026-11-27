// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared mailboxes opened before (Q4): a removed one is remembered under the account it was
//! opened through, newest first and at most 20, for one-click re-add. Its address and name are
//! kept in the mailbox file only. Adding it again, forgetting it and removing the account take it
//! off; an agent's forgetting waits for the owner. Every address is invented.

use mailcraft_model::{Id, MAX_SHARED};
use serde_json::{Value, json};

use crate::Session;
use crate::agent::{Access, Class, Verdict, access, check};

const OWNER: &str = "owner@example.org";

fn session() -> (Session, Id) {
    let mut s = Session::empty();
    let owner = s.run("account.add", &json!({"email": OWNER, "kind": "microsoft", "auth": "oauth2", "directory": "organisation"})).unwrap()["id"]
        .as_u64()
        .unwrap();
    (s, owner)
}

fn open(s: &mut Session, owner: Id, address: &str) -> Id {
    s.run("account.addSharedMailbox", &json!({"account": owner, "address": address})).unwrap()["id"].as_u64().unwrap()
}

fn remove(s: &mut Session, shared: Id) {
    s.run("account.removeSharedMailbox", &json!({"id": shared})).unwrap();
}

fn before(s: &Session, owner: Id) -> Vec<(String, String)> {
    s.mailbox.account(owner).unwrap().shared_before.iter().map(|e| (e.address.clone(), e.name.clone())).collect()
}

fn addresses(s: &Session, owner: Id) -> Vec<String> {
    before(s, owner).into_iter().map(|(a, _)| a).collect()
}

#[test]
fn a_removed_shared_mailbox_is_remembered_under_its_owner_newest_first_at_most_20() {
    let (mut s, owner) = session();
    let support = open(&mut s, owner, "support@example.org");
    s.mailbox.account_mut(support).unwrap().display_name = "Support Team".into();
    remove(&mut s, support);
    assert_eq!(before(&s, owner), [("support@example.org".to_string(), "Support Team".to_string())]);
    // Twenty-one more, removed one after the other: the newest first, the oldest let go of.
    for i in 0..21 {
        let id = open(&mut s, owner, &format!("team{i:02}@example.org"));
        remove(&mut s, id);
    }
    let kept = addresses(&s, owner);
    assert_eq!(kept.len(), MAX_SHARED);
    assert_eq!(kept.first().map(String::as_str), Some("team20@example.org"), "newest first");
    assert!(!kept.contains(&"support@example.org".to_string()) && !kept.contains(&"team00@example.org".to_string()), "the oldest went");
    // Removed again: once, at the front.
    let again = open(&mut s, owner, "TEAM05@example.org");
    remove(&mut s, again);
    let kept = addresses(&s, owner);
    assert_eq!(kept.iter().filter(|a| a.eq_ignore_ascii_case("team05@example.org")).count(), 1, "{kept:?}");
    assert!(kept[0].eq_ignore_ascii_case("team05@example.org"));
}

#[test]
fn a_shared_mailbox_removed_without_remembering_it_is_not_remembered() {
    let (mut s, owner) = session();
    let id = open(&mut s, owner, "support@example.org");
    s.run("account.removeSharedMailbox", &json!({"id": id, "remember": false})).unwrap();
    assert!(before(&s, owner).is_empty());
}

#[test]
fn adding_it_again_takes_it_off_the_list() {
    let (mut s, owner) = session();
    for address in ["support@example.org", "sales@example.org"] {
        let id = open(&mut s, owner, address);
        remove(&mut s, id);
    }
    open(&mut s, owner, "SUPPORT@example.org");
    assert_eq!(addresses(&s, owner), ["sales@example.org"]);
}

#[test]
fn add_again_uses_the_remembered_name() {
    let (mut s, owner) = session();
    let id = open(&mut s, owner, "support@example.org");
    s.mailbox.account_mut(id).unwrap().display_name = "Support Team".into();
    remove(&mut s, id);
    let again = open(&mut s, owner, "support@example.org");
    assert_eq!(s.mailbox.account(again).unwrap().display_name, "Support Team", "the remembered name");
    // A name given with the address is the one taken.
    remove(&mut s, again);
    let named = s.run("account.addSharedMailbox", &json!({"account": owner, "address": "support@example.org", "displayName": "Help"})).unwrap()["id"]
        .as_u64()
        .unwrap();
    assert_eq!(s.mailbox.account(named).unwrap().display_name, "Help");
}

#[test]
fn forget_takes_one_off_and_forget_all_takes_them_all() {
    let (mut s, owner) = session();
    for address in ["support@example.org", "sales@example.org", "billing@example.org"] {
        let id = open(&mut s, owner, address);
        remove(&mut s, id);
    }
    let out = s.run("account.forgetSharedBefore", &json!({"account": owner, "address": "SALES@example.org"})).unwrap();
    assert_eq!(out["forgotten"], 1);
    assert_eq!(addresses(&s, owner), ["billing@example.org", "support@example.org"]);
    assert!(s.run("account.forgetSharedBefore", &json!({"account": owner, "address": "nobody@example.org"})).is_err(), "one that isn't remembered");
    assert!(s.run("account.forgetSharedBefore", &json!({"account": owner, "address": "not an address"})).is_err());
    let out = s.run("account.forgetSharedBefore", &json!({"account": owner})).unwrap();
    assert_eq!(out["forgotten"], 2);
    assert!(before(&s, owner).is_empty());
    assert!(s.run("account.forgetSharedBefore", &json!({"account": 9_999})).is_err(), "an account that isn't there");
    assert!(s.run("account.forgetSharedBefore", &json!({})).is_err(), "`account` is required");
}

#[test]
fn removing_the_owner_forgets_them_all() {
    let (mut s, owner) = session();
    let id = open(&mut s, owner, "support@example.org");
    remove(&mut s, id);
    s.run("account.remove", &json!({"id": owner})).unwrap();
    let file = serde_json::to_string(&s.mailbox).unwrap();
    assert!(!file.contains("support@example.org"), "nothing of it is left in the mailbox file");
}

#[test]
fn what_is_remembered_is_in_the_mailbox_file_alone() {
    let (mut s, owner) = session();
    let id = open(&mut s, owner, "support@example.org");
    remove(&mut s, id);
    let file = serde_json::to_string(&s.mailbox).unwrap();
    assert!(file.contains("sharedBefore") && file.contains("support@example.org"), "kept in the mailbox file");
    let listed = s.run("account.list", &json!({})).unwrap().to_string();
    assert!(!listed.contains("support@example.org"), "not listed for anybody");
}

#[test]
fn forgetting_one_waits_for_the_owner_and_says_what_it_forgets() {
    let (mut s, owner) = session();
    let id = open(&mut s, owner, "support@example.org");
    remove(&mut s, id);
    let what = "account.forgetSharedBefore";
    assert_eq!(access(what), Some(Access::Owner(Class::Accounts)));
    let shapes: [Value; 6] = [
        json!({"account": owner, "address": "support@example.org"}),
        json!({"account": owner}),
        json!({"account": owner, "address": 7}),
        json!({"account": "x"}),
        json!({}),
        json!(null),
    ];
    for params in &shapes {
        match check(&s, what, params, &[]) {
            Verdict::Ask(class, said) => assert!(class == Class::Accounts && !said.is_empty(), "{params}: {class:?}"),
            Verdict::Refuse(_) => {}
            Verdict::Run => panic!("ran without the owner: {params}"),
        }
        assert!(
            !matches!(check(&s, what, params, &[Class::Mailbox, Class::Settings, Class::Input]), Verdict::Run),
            "another grant isn't this one: {params}"
        );
    }
    let Verdict::Ask(_, one) = check(&s, what, &shapes[0], &[]) else { panic!("asked") };
    assert!(one.contains("support@example.org") && one.contains(OWNER), "{one}");
    let Verdict::Ask(_, all) = check(&s, what, &shapes[1], &[]) else { panic!("asked") };
    assert!(all.contains("every") && all.contains(OWNER), "{all}");
    assert!(matches!(check(&s, what, &shapes[1], &[Class::Accounts]), Verdict::Run), "with the owner's grant");
}
