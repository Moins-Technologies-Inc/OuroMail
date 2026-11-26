// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window's side of the signature guard (ouromail-signature-guard): an agent's requests
//! come through the control channel as the MCP bridge sends them (`control::handle` with the
//! request marked as an agent's, so through `agent_gate`), and frames run between them as the
//! window runs them. An agent may open File › Signatures (the page opens the first signature by
//! itself, as for the owner), but nothing it does there is kept: its edits are refused or undone,
//! the owner is told in one line, and leaving the page or quitting saves only what the owner
//! wrote. An agent's typing meant for its own message while the owner is on the page is refused.
//!
//! The text is invented; it is data.

use std::sync::mpsc::Receiver;

use mailcraft_engine::agent::Class;
use serde_json::{Value, json};

use crate::control::{ControlRequest, Outcome};
use crate::people_view::testing::{self, Window};

const CANARY: &str = "CANARY-7Q\u{200b}";
const SAID: &str = "An agent tried to change the signature you are editing; it was left as it was.";

/// `method` sent by an agent the owner allowed `allow` in advance: its answer, when it has one.
fn agent(w: &mut Window, method: &str, params: Value, allow: Vec<Class>) -> (Option<Value>, Receiver<Value>) {
    let (req, rx) = ControlRequest::new(method, params);
    let out = crate::control::handle(w.state_mut(), &egui::Context::default(), &req.from_agent(allow));
    (if let Outcome::Done(v) = out { Some(v) } else { None }, rx)
}

/// An agent's `engine.execute` of `command`.
fn agent_runs(w: &mut Window, command: &str, params: Value, allow: Vec<Class>) -> Value {
    agent(w, "engine.execute", json!({"command": command, "params": params}), allow).0.unwrap_or_else(|| json!({"pending": true}))
}

/// The sample window with `Standard` holding the owner's text, and the page closed.
fn window() -> Window {
    let mut w = testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.state_mut().execute("signature.set", json!({"name": "Standard", "text": "Avery Quinn"})).unwrap();
    w.run_steps(2);
    w
}

fn kept(w: &Window) -> Vec<String> {
    w.state().session.mailbox.signatures.iter().map(|s| format!("{}={}", s.name, s.plain())).collect()
}

fn said(w: &Window) -> (Option<String>, Option<String>) {
    let a = w.state();
    (a.status_msg.as_ref().map(|(m, _)| m.clone()), a.signature_note.as_ref().filter(|n| n.error).map(|n| n.text.clone()))
}

/// Route 6 of the probe: an agent with nothing allowed opens File › Signatures itself, tries to
/// write in the signature the page opened, and leaves the page (which saves a changed one).
#[test]
fn an_agent_that_opens_the_signatures_page_gets_nothing_saved() {
    for allow in [Vec::new(), Class::ALL.iter().copied().filter(|c| *c != Class::Input).collect()] {
        let mut w = window();
        let before = kept(&w);
        let opened = agent(&mut w, "ui.backstage", json!({"value": true, "page": "signatures"}), allow.clone()).0;
        assert_eq!(opened.as_ref().map(|v| v["ok"].clone()), Some(json!(true)), "the page may open: {opened:?}");
        w.run_steps(3);
        assert!(w.state().session.signature_edit.is_some(), "the page opened a signature, as for the owner");
        let typed = agent_runs(&mut w, "compose.type", json!({"text": CANARY}), allow.clone());
        assert_eq!(typed["ok"], false, "{typed}");
        assert!(typed["error"].as_str().is_some_and(|e| e.contains("signature.set")), "{typed}");
        assert_eq!(said(&w), (Some(SAID.to_string()), Some(SAID.to_string())), "the owner is told, below the editor and in the status bar");
        // A command the gate let through (as if the list had forgotten it) is undone by the run,
        // and `agent_gate::run` reports the S3 status notice in both status bar and below editor.
        w.state_mut().status_msg = None;
        w.state_mut().signature_note = None;
        let forgotten = crate::agent_gate::run_for_test(w.state_mut(), "compose.type", json!({"text": CANARY}), &allow);
        assert!(matches!(forgotten, Outcome::Done(ref v) if v["ok"] == false), "backstop undo must fail");
        assert_eq!(said(&w), (Some(SAID.to_string()), Some(SAID.to_string())), "backstop undo must show S3 notice");
        w.run_steps(2);
        assert!(w.state().session.signature_edit.as_ref().is_some_and(|e| !e.dirty && !e.body.plain().contains("CANARY")));
        // SEC-B1: an agent cannot leave File › Signatures while a signature is open.
        let left = agent(&mut w, "ui.backstage", json!({"value": false}), allow.clone()).0;
        assert_eq!(left.as_ref().map(|v| v["ok"].clone()), Some(json!(false)), "agent must not leave open signature: {left:?}");
        w.run_steps(3);
        assert!(w.state().ui.backstage && w.state().ui.backstage_page == "signatures");
        // The owner leaving closes the page and saves nothing of the agent's.
        w.state_mut().execute("ui.backstage", json!({"value": false})).unwrap();
        w.run_steps(3);
        assert!(!w.state().ui.backstage);
        assert_eq!(kept(&w), before, "leaving the page saved nothing of the agent's");
    }
}

/// Leaving the page, and quitting, after an agent's attempt keep what the owner wrote and
/// nothing of the agent's.
#[test]
fn leaving_or_quitting_after_an_agents_attempt_keeps_only_the_owners_change() {
    for quit in [false, true] {
        let mut w = window();
        w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
        w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
        w.run_steps(3);
        w.state_mut().execute("compose.type", json!({"text": "Dr "})).unwrap();
        w.run_steps(2);
        for (id, params) in [
            ("compose.type", json!({"text": CANARY})),
            ("edit.paste", json!({"html": "<a href=\"https://attacker.example/\">x</a>"})),
            ("compose.link", json!({"url": "https://attacker.example/", "from": 0, "to": 2})),
            ("format.color", json!({"value": "#ffffff", "from": 0, "to": 2})),
            ("compose.undo", json!({})),
        ] {
            let r = agent_runs(&mut w, id, params, Class::ALL.to_vec());
            assert_eq!(r["ok"], false, "{id}: {r}");
        }
        w.run_steps(2);
        if quit {
            assert!(w.state_mut().close_requested(), "nothing to ask: it closes");
        } else {
            w.state_mut().execute("ui.backstage", json!({"value": false})).unwrap();
            w.run_steps(3);
        }
        let standard = w.state().session.mailbox.signature("Standard").map(|s| s.plain()).unwrap_or_default();
        assert_eq!(standard, "Dr Avery Quinn", "quit={quit}: the owner's change, and only it");
    }
}

/// An agent typing in its own message while the owner is on File › Signatures is refused: its
/// words never go into the signature, and the message stays as it was.
#[test]
fn an_agents_typing_for_its_message_while_the_owner_is_on_signatures_is_refused() {
    let mut w = window();
    let started = agent_runs(&mut w, "mail.new", json!({}), Vec::new());
    assert_eq!(started["ok"], true, "{started}");
    let compose = w.state().session.compose_id().unwrap();
    let typed = agent_runs(&mut w, "compose.type", json!({"text": "Hello", "compose": compose}), Vec::new());
    assert_eq!(typed["ok"], true, "no signature open: it writes in its message, {typed}");
    w.run_steps(2);
    // The owner goes to File › Signatures.
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
    w.run_steps(3);
    let body = |w: &Window| w.state().session.all_composing().find(|c| c.id == compose).map(|c| c.body.plain());
    let before = (kept(&w), body(&w), w.state().session.signature_edit.as_ref().map(|e| e.body.plain()));
    for params in [json!({"text": " there", "compose": compose}), json!({"text": " there"})] {
        let r = agent_runs(&mut w, "compose.type", params.clone(), Vec::new());
        assert_eq!(r["ok"], false, "{params}: {r}");
        assert!(r["error"].as_str().is_some_and(|e| e.contains("wait until the owner has left File › Signatures") && e.contains("mail.new")), "{r}");
        w.run_steps(1);
        assert_eq!((kept(&w), body(&w), w.state().session.signature_edit.as_ref().map(|e| e.body.plain())), before, "{params}");
    }
    assert_eq!(said(&w).1.as_deref(), Some(SAID));
    // Once the owner has left the page, an agent writes in a message of its own again (the one
    // open meanwhile may have become the owner's, as before: the refusal says to start one).
    w.state_mut().execute("ui.backstage", json!({"value": false})).unwrap();
    w.run_steps(3);
    let started = agent_runs(&mut w, "mail.new", json!({}), Vec::new());
    assert_eq!(started["ok"], true, "{started}");
    // (It opens beside the one there, which stays current: it is named by the id it answers.)
    let own = started["result"]["compose"].as_u64().unwrap();
    let text = |w: &Window| w.state().session.all_composing().find(|c| c.id == own).map(|c| c.body.plain()).unwrap_or_default();
    // (A new message starts with the account's signature below the caret.)
    let fresh = text(&w);
    let r = agent_runs(&mut w, "compose.type", json!({"text": "Hi", "compose": own}), Vec::new());
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(text(&w), format!("Hi{fresh}"), "typed in its own message, at the caret");
    assert_eq!(kept(&w), before.0, "and nothing went into a signature");
}

/// A whole signature an agent proposes is asked about, in a question that shows its text; the
/// owner's yes keeps it, and nothing is said about a refusal.
#[test]
fn signature_set_from_an_agent_on_the_page_is_asked_and_kept_on_a_yes() {
    let mut w = window();
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
    w.run_steps(3);
    let (now, rx) = agent(
        &mut w,
        "engine.execute",
        json!({"command": "signature.set", "params": {"name": "Standard", "text": "Avery Quinn, Studio Example"}}),
        Vec::new(),
    );
    assert!(now.is_none(), "asked: {now:?}");
    let what = w.state().asking.front().map(|a| a.what.clone()).unwrap_or_default();
    assert!(what.contains("Avery Quinn, Studio Example"), "{what}");
    crate::agent_gate::answer(w.state_mut(), true);
    let r = rx.try_recv().unwrap();
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(w.state().session.mailbox.signature("Standard").map(|s| s.plain()).as_deref(), Some("Avery Quinn, Studio Example"));
    assert_ne!(said(&w).1.as_deref(), Some(SAID));
    // With a character that draws nothing, it isn't asked at all.
    let (now, _rx) = agent(
        &mut w,
        "engine.execute",
        json!({"command": "signature.set", "params": {"name": "Standard", "text": "Avery\u{200b} Quinn"}}),
        Vec::new(),
    );
    let now = now.unwrap();
    assert_eq!(now["ok"], false);
    assert!(now["error"].as_str().is_some_and(|e| e.contains("U+200B")), "{now}");
    assert!(w.state().asking.is_empty());
}

/// SEC-B1: agent navigation commands leaving File › Signatures are refused while a signature is
/// open (or being renamed), under both empty grants and `Class::ALL`, leaving `mailbox.signatures`,
/// `signature_edit`, `signature_rename` untouched and showing the S3 status notice.
#[test]
fn sec_b1_agent_navigation_cannot_leave_open_signature() {
    let nav_calls: [(&str, Value); 5] = [
        ("ui.backstage", json!({"value": false})),
        ("ui.backstage", json!({"value": true, "page": "info"})),
        ("ui.tab", json!({"tab": "Home"})),
        ("ui.focusSearch", json!({})),
        ("file.account", json!({})),
    ];
    for allow in [Vec::new(), Class::ALL.to_vec()] {
        for (method, params) in &nav_calls {
            let mut w = window();
            let before_kept = kept(&w);
            w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
            w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
            w.run_steps(3);
            w.state_mut().execute("compose.type", json!({"text": "UNFINISHED "})).unwrap();
            w.run_steps(2);
            w.state_mut().status_msg = None;
            w.state_mut().signature_note = None;

            let r = agent(&mut w, method, params.clone(), allow.clone()).0.expect("sync refusal");
            assert_eq!(r["ok"], false, "{method} {params} under {allow:?} must be refused: {r}");
            w.run_steps(3);
            assert!(w.state().ui.backstage && w.state().ui.backstage_page == "signatures", "{method} left File › Signatures");
            assert_eq!(kept(&w), before_kept, "{method} saved unfinished owner edits");
            assert!(
                w.state().session.signature_edit.as_ref().is_some_and(|e| e.dirty && e.body.plain() == "UNFINISHED Avery Quinn"),
                "{method} altered signature_edit"
            );
            assert_eq!(said(&w), (Some(SAID.to_string()), Some(SAID.to_string())), "{method}: S3 status notice");
        }
    }

    // Active `signature_rename` is also protected against leaving File › Signatures.
    let mut w = window();
    let before_kept = kept(&w);
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
    w.run_steps(3);
    // Open rename field by setting `signature_rename` as the Rename button does.
    w.state_mut().signature_rename = Some(crate::signatures::Rename::for_test("Standard", "NewName"));
    w.state_mut().status_msg = None;
    w.state_mut().signature_note = None;
    let r = agent(&mut w, "ui.backstage", json!({"value": false}), Class::ALL.to_vec()).0.expect("sync refusal");
    assert_eq!(r["ok"], false, "rename active must refuse ui.backstage: {r}");
    w.run_steps(3);
    assert!(w.state().signature_rename.is_some(), "signature_rename must stay intact");
    assert_eq!(kept(&w), before_kept, "rename must not be committed by agent navigation");
    assert_eq!(said(&w), (Some(SAID.to_string()), Some(SAID.to_string())));

    // Chaining `ui.backstage {"value": false}` + `signature.html` under `--allow settings` is blocked.
    let mut w = window();
    let before_kept = kept(&w);
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
    w.run_steps(3);
    w.state_mut().execute("compose.type", json!({"text": "UNFINISHED "})).unwrap();
    w.run_steps(2);
    let r1 = agent(&mut w, "ui.backstage", json!({"value": false}), vec![Class::Settings]).0.expect("sync refusal");
    assert_eq!(r1["ok"], false, "chained leave must be refused: {r1}");
    w.run_steps(3);
    let r2 = agent_runs(&mut w, "signature.html", json!({"name": "Standard", "html": "<p>BYPASSED</p>"}), vec![Class::Settings]);
    assert_eq!(r2["ok"], false, "chained signature.html must be refused: {r2}");
    w.run_steps(2);
    assert_eq!(kept(&w), before_kept);
    assert!(w.state().session.signature_edit.as_ref().is_some_and(|e| e.dirty && e.body.plain() == "UNFINISHED Avery Quinn"));

    // Agent `compose.popOut` on a reading-pane compose while owner is on File › Signatures must not drop backstage and trigger `leave_signature`.
    let mut w = window();
    let before_kept = kept(&w);
    w.state_mut().execute("mail.new", json!({})).unwrap();
    let pane_id = w.state().session.compose_id().unwrap();
    w.run_steps(2);
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
    w.run_steps(3);
    w.state_mut().execute("compose.type", json!({"text": "UNFINISHED "})).unwrap();
    w.run_steps(2);
    let popped = agent_runs(&mut w, "compose.popOut", json!({"compose": pane_id}), Vec::new());
    assert_eq!(popped["ok"], true, "{popped}");
    w.run_steps(3);
    assert!(w.state().ui.backstage && w.state().ui.backstage_page == "signatures", "compose.popOut must not leave File › Signatures");
    assert_eq!(kept(&w), before_kept, "compose.popOut must not save unfinished signature");
    assert!(w.state().session.signature_edit.as_ref().is_some_and(|e| e.dirty && e.body.plain() == "UNFINISHED Avery Quinn"));

    // CORR-R2-m1: owner `compose.popOut` on a reading-pane compose still closes backstage.
    let mut w = window();
    w.state_mut().execute("mail.new", json!({})).unwrap();
    let owner_pane_id = w.state().session.compose_id().unwrap();
    w.run_steps(2);
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
    w.run_steps(3);
    assert!(w.state().ui.backstage && w.state().ui.backstage_page == "signatures");
    w.state_mut().execute("compose.popOut", json!({"compose": owner_pane_id})).unwrap();
    assert!(!w.state().ui.backstage, "owner compose.popOut must still close backstage");
}

/// SEC-R2-B1: `file.account` is refused for agents while a signature is open or being renamed
/// (for both empty grants and `Class::ALL`, and when chained with `signature.html` under
/// `--allow settings`), `send_receive::after` also refuses to switch `backstage_page` under an
/// active agent as defense-in-depth, and owner `file.account` still switches to `about`.
#[test]
fn sec_r2_b1_file_account_cannot_leave_open_signature_or_rename() {
    // 1. Active `signature_rename` refuses `file.account {}` under empty grants and `Class::ALL`.
    for allow in [Vec::new(), Class::ALL.to_vec()] {
        let mut w = window();
        let before_kept = kept(&w);
        w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
        w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
        w.run_steps(3);
        w.state_mut().signature_rename = Some(crate::signatures::Rename::for_test("Standard", "PrematureRename"));
        w.state_mut().status_msg = None;
        w.state_mut().signature_note = None;

        let r = agent(&mut w, "file.account", json!({}), allow.clone()).0.expect("sync refusal");
        assert_eq!(r["ok"], false, "file.account under {allow:?} with rename active must be refused: {r}");
        w.run_steps(3);
        assert!(w.state().ui.backstage && w.state().ui.backstage_page == "signatures", "file.account left File › Signatures");
        assert!(w.state().signature_rename.is_some(), "signature_rename must stay intact");
        assert_eq!(kept(&w), before_kept, "file.account must not commit rename");
        assert_eq!(said(&w), (Some(SAID.to_string()), Some(SAID.to_string())), "S3 notice on file.account refusal");
    }

    // 2. Chaining `file.account {}` + `signature.html` under `--allow settings` is blocked.
    let mut w = window();
    let before_kept = kept(&w);
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
    w.run_steps(3);
    w.state_mut().execute("compose.type", json!({"text": "UNFINISHED "})).unwrap();
    w.run_steps(2);
    let r1 = agent(&mut w, "file.account", json!({}), vec![Class::Settings]).0.expect("sync refusal");
    assert_eq!(r1["ok"], false, "chained file.account must be refused: {r1}");
    w.run_steps(3);
    let r2 = agent_runs(&mut w, "signature.html", json!({"name": "Standard", "html": "<p>BYPASSED_VIA_FILE_ACCOUNT</p>"}), vec![Class::Settings]);
    assert_eq!(r2["ok"], false, "chained signature.html after file.account must be refused: {r2}");
    w.run_steps(2);
    assert!(w.state().ui.backstage && w.state().ui.backstage_page == "signatures");
    assert_eq!(kept(&w), before_kept, "signature must stay untouched");
    assert!(w.state().session.signature_edit.as_ref().is_some_and(|e| e.dirty && e.body.plain() == "UNFINISHED Avery Quinn"));

    // 3. Defense-in-depth in `send_receive::after`: bypassing `leaves_open_signature` via
    // `agent_gate::run_for_test` must still not switch `backstage_page` to `about`.
    let mut w = window();
    let before_kept = kept(&w);
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
    w.run_steps(3);
    w.state_mut().execute("compose.type", json!({"text": "UNFINISHED "})).unwrap();
    w.run_steps(2);
    let direct = crate::agent_gate::run_for_test(w.state_mut(), "file.account", json!({}), &[]);
    assert!(matches!(direct, Outcome::Done(ref v) if v["ok"] == true), "engine file.account itself succeeds");
    assert!(
        w.state().ui.backstage && w.state().ui.backstage_page == "signatures",
        "send_receive::after must not switch backstage_page under an agent"
    );
    w.run_steps(3);
    assert_eq!(kept(&w), before_kept, "bypassed file.account must not trigger leave_signature");
    assert!(w.state().session.signature_edit.as_ref().is_some_and(|e| e.dirty && e.body.plain() == "UNFINISHED Avery Quinn"));

    // 4. Owner `file.account {}` still works normally and switches to File › Account (`about`).
    let mut w = window();
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
    w.run_steps(3);
    let owner_r = w.state_mut().execute("file.account", json!({})).expect("owner file.account succeeds");
    assert!(owner_r["accounts"].is_array(), "owner file.account returns accounts: {owner_r}");
    assert!(w.state().ui.backstage && w.state().ui.backstage_page == "about", "owner file.account switches to about page");
}

/// SEC-R2-m2: `leaves_open_signature` still refuses navigation when `signature_rename = Some(...)`
/// and `signature_edit = None` (kills mutant `SMR2_02`).
#[test]
fn sec_r2_m2_signature_rename_only_refuses_navigation() {
    let nav_calls: [(&str, Value); 4] =
        [("ui.backstage", json!({"value": false})), ("ui.tab", json!({"tab": "Home"})), ("ui.focusSearch", json!({})), ("file.account", json!({}))];
    for allow in [Vec::new(), Class::ALL.to_vec()] {
        for (method, params) in &nav_calls {
            let mut w = window();
            let before_kept = kept(&w);
            w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
            w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
            w.run_steps(3);
            // Close the open signature editor while keeping an active `signature_rename`.
            w.state_mut().signature_rename = Some(crate::signatures::Rename::for_test("Standard", "RenamedOnly"));
            w.state_mut().execute("signature.close", json!({})).unwrap();
            assert!(w.state().session.signature_edit.is_none(), "signature_edit must be None");
            assert!(w.state().signature_rename.is_some(), "signature_rename must be Some");
            w.state_mut().status_msg = None;
            w.state_mut().signature_note = None;

            let r = agent(&mut w, method, params.clone(), allow.clone()).0.expect("sync refusal");
            assert_eq!(r["ok"], false, "rename-only state must refuse {method} under {allow:?}: {r}");
            assert!(w.state().ui.backstage && w.state().ui.backstage_page == "signatures", "{method} left File › Signatures");
            assert!(w.state().signature_rename.is_some(), "{method} dropped signature_rename");
            assert_eq!(said(&w), (Some(SAID.to_string()), Some(SAID.to_string())), "{method}: S3 status notice");
            w.run_steps(3);
            assert_eq!(kept(&w), before_kept, "{method} committed rename");
        }
    }
}

/// SEC-m1: a queued `signature.html` approved after File › Signatures opened meanwhile is refused
/// and shows the S3 status notice in both status bar and below the editor.
#[test]
fn sec_m1_queued_signature_html_toctou_shows_status_notice() {
    let mut w = window();
    let before = kept(&w);
    let (now, rx) =
        agent(&mut w, "engine.execute", json!({"command": "signature.html", "params": {"name": "Standard", "html": "<p>Queued</p>"}}), Vec::new());
    assert!(now.is_none(), "queued while signature closed: {now:?}");
    // Meanwhile, the owner opens File › Signatures.
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
    w.run_steps(3);
    w.state_mut().status_msg = None;
    w.state_mut().signature_note = None;

    crate::agent_gate::answer(w.state_mut(), true);
    let reply = rx.try_recv().unwrap();
    assert_eq!(reply["ok"], false, "must be refused once signature is open: {reply}");
    assert_eq!(kept(&w), before);
    assert_eq!(said(&w), (Some(SAID.to_string()), Some(SAID.to_string())), "TOCTOU refusal must show S3 notice");
}

/// CORR-01: `signature_note` clears on owner edit (rich & HTML), save, signature switch, and leaving the page.
#[test]
fn corr_01_signature_note_clears_on_owner_actions() {
    let mut w = window();
    w.state_mut().execute("signature.set", json!({"name": "Work", "text": "Work Sig"})).unwrap();
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
    w.run_steps(3);

    let trigger_refusal = |w: &mut Window| {
        let r = agent_runs(w, "compose.type", json!({"text": CANARY}), Vec::new());
        assert_eq!(r["ok"], false);
        assert_eq!(said(w).1.as_deref(), Some(SAID));
    };

    // 1. Owner rich edit clears signature_note.
    trigger_refusal(&mut w);
    w.state_mut().execute("compose.type", json!({"text": "Dr "})).unwrap();
    assert!(w.state().signature_note.is_none(), "owner compose.type must clear signature_note");

    // 2. Owner save clears signature_note.
    trigger_refusal(&mut w);
    w.state_mut().execute("signature.save", json!({})).unwrap();
    assert!(w.state().signature_note.is_none(), "owner signature.save must clear signature_note");

    // 3. Owner signature switch clears signature_note.
    trigger_refusal(&mut w);
    w.state_mut().execute("signature.open", json!({"name": "Work"})).unwrap();
    assert!(w.state().signature_note.is_none(), "owner signature.open switch must clear signature_note");

    // 4. Owner HTML edit clears signature_note.
    trigger_refusal(&mut w);
    w.state_mut().execute("signature.html", json!({"html": "<p>Work HTML</p>"})).unwrap();
    assert!(w.state().signature_note.is_none(), "owner signature.html edit must clear signature_note");
    w.state_mut().execute("signature.save", json!({})).unwrap();

    // 5. Leaving the page clears signature_note.
    trigger_refusal(&mut w);
    w.state_mut().execute("ui.backstage", json!({"value": false})).unwrap();
    w.run_steps(3);
    assert!(w.state().signature_note.is_none(), "leaving File › Signatures must clear signature_note");
}
