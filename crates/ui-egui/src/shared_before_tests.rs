// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Add Shared Mailbox's "Opened before" (Q4), drawn:
//! - nothing when nothing is remembered;
//! - Add again goes the way Add does (checked, kept only once its Inbox reads), with the
//!   remembered name;
//! - a re-add whose check fails stays remembered, and a typed one that fails isn't remembered;
//! - Forget and Forget all are the owner's own clicks;
//! - twenty fit a short window.
//!
//! Hosts are fakes; every address is invented.

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use mailcraft_engine::agent::Class;
use mailcraft_model::Id;
use serde_json::json;

use crate::control::ControlRequest;
use crate::dialogs::Dialog;
use crate::{GraphRequest, GraphResult, MailApp, Services};

type H = egui_kittest::Harness<'static, MailApp>;
const OWNER: &str = "noor@example.org";

/// A window with a work or school account, `remembered` shared mailboxes opened under it and
/// removed since (the first removed last, so newest first), mailbox runs held, and Add Shared
/// Mailbox open. No directory host: the dialog as the browser build has it.
fn window(remembered: &[(&str, &str)]) -> (MailApp, Id, Arc<Mutex<Vec<Sender<GraphResult>>>>) {
    let mut app = MailApp::new(Session::empty(), Services::default());
    app.autosave = false;
    let owner =
        app.execute("account.add", json!({"email": OWNER, "kind": "microsoft", "auth": "oauth2", "directory": "organisation"})).unwrap()["id"]
            .as_u64()
            .unwrap();
    // (Signed in this session.)
    let signed_in =
        mailcraft_model::sync::Tokens { access: "a".into(), refresh: "r".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
    app.tokens.insert(owner, signed_in);
    for (address, name) in remembered.iter().rev() {
        let id = app.execute("account.addSharedMailbox", json!({"account": owner, "address": address, "displayName": name})).unwrap()["id"]
            .as_u64()
            .unwrap();
        app.execute("account.removeSharedMailbox", json!({"id": id})).unwrap();
    }
    let held: Arc<Mutex<Vec<Sender<GraphResult>>>> = Default::default();
    let keep = held.clone();
    app.session.driver.set_auto_sync(false);
    app.services.graph = Some(Box::new(move |req: GraphRequest| {
        let (tx, rx) = mpsc::channel();
        if req.mail.is_none() {
            let _ = tx.send(GraphResult::default());
        } else {
            keep.lock().unwrap().push(tx);
        }
        rx
    }));
    app.services.sync = Some(Box::new(|req: mailcraft_model::sync::SyncRequest| {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(mailcraft_model::sync::SyncResult { account: req.account.id, ..Default::default() });
        rx
    }));
    app.execute("ui.dialog", json!({"name": "sharedMailbox", "account": owner})).unwrap();
    (app, owner, held)
}

fn harness_at(app: MailApp, width: f32, height: f32) -> H {
    egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(width, height)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    )
}

fn click(h: &mut H, at: egui::Pos2) {
    h.event(egui::Event::PointerMoved(at));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
}

fn remembered(app: &MailApp, owner: Id) -> Vec<String> {
    app.session.mailbox.account(owner).map(|a| a.shared_before.iter().map(|e| e.address.clone()).collect()).unwrap_or_default()
}

fn open_shared(app: &MailApp) -> Vec<(Id, String, String)> {
    app.session.mailbox.accounts.iter().filter(|a| a.is_shared()).map(|a| (a.id, a.email.clone(), a.display_name.clone())).collect()
}

fn checking(app: &MailApp) -> Option<Id> {
    match &app.dialog {
        Some(Dialog::SharedMailbox { checking, .. }) => *checking,
        _ => None,
    }
}

/// An agent's control request with every grant, its frames fed in as the window's own.
fn agent_click(h: &mut H, kind: &str, id: u64) {
    let (req, _rx) = ControlRequest::new("ui.click", json!({"kind": kind, "id": id}));
    let req = req.from_agent(Class::ALL.to_vec());
    let _ = crate::control::handle(h.state_mut(), &egui::Context::default(), &req);
    for _ in 0..6 {
        let mut raw = egui::RawInput::default();
        h.state_mut().raw_input_hook(&mut raw);
        h.input_mut().events.extend(raw.events);
        h.step();
    }
}

#[test]
fn nothing_is_shown_when_nothing_was_opened_before() {
    let (app, _, _) = window(&[]);
    let mut h = harness_at(app, 1280.0, 800.0);
    h.run_steps(3);
    assert!(h.query_by_label(tl!("Opened before")).is_none());
    assert!(h.state().hit.rect("sharedBeforeAdd", 0).is_none());
}

#[test]
fn one_opened_before_is_shown_and_added_again_as_add_does_with_its_name() {
    let (app, owner, _held) = window(&[("support@example.org", "Support Team")]);
    let mut h = harness_at(app, 1280.0, 800.0);
    h.run_steps(3);
    assert!(h.query_by_label(tl!("Opened before")).is_some());
    assert!(h.query_by_label_contains("support@example.org").is_some(), "its address is shown");
    let again = h.state().hit.rect("sharedBeforeAdd", 0).expect("Add again");
    click(&mut h, again.center());
    let open = open_shared(h.state());
    assert_eq!(open.iter().map(|(_, a, n)| (a.as_str(), n.as_str())).collect::<Vec<_>>(), [("support@example.org", "Support Team")]);
    assert_eq!(checking(h.state()), Some(open[0].0), "checked as a typed address is");
    assert!(remembered(h.state(), owner).is_empty(), "off the list while it is open");
}

#[test]
fn a_re_add_whose_check_fails_stays_remembered_and_a_typed_one_that_fails_is_not() {
    let (app, owner, _held) = window(&[("support@example.org", "Support Team")]);
    let mut h = harness_at(app, 1280.0, 800.0);
    h.run_steps(3);
    let again = h.state().hit.rect("sharedBeforeAdd", 0).expect("Add again");
    click(&mut h, again.center());
    let id = checking(h.state()).expect("being checked");
    h.state_mut().finish_shared_check(id, Err("no access".into()));
    h.run_steps(2);
    assert_eq!(remembered(h.state(), owner), ["support@example.org"], "remembered again");
    assert!(open_shared(h.state()).is_empty());
    // A typed address whose check fails is never remembered.
    if let Some(Dialog::SharedMailbox { address, .. }) = &mut h.state_mut().dialog {
        *address = "other@example.org".into();
    }
    h.run_steps(1);
    h.query_all_by_label(tl!("Add")).last().expect("Add").click();
    h.run_steps(3);
    let id = checking(h.state()).expect("being checked");
    h.state_mut().finish_shared_check(id, Err("no access".into()));
    h.run_steps(2);
    assert_eq!(remembered(h.state(), owner), ["support@example.org"]);
}

#[test]
fn forget_and_forget_all_are_the_owners_clicks() {
    let (app, owner, _held) = window(&[("support@example.org", ""), ("sales@example.org", ""), ("billing@example.org", "")]);
    let mut h = harness_at(app, 1280.0, 800.0);
    h.run_steps(3);
    agent_click(&mut h, "sharedBeforeForget", 0);
    agent_click(&mut h, "sharedBeforeForgetAll", 0);
    assert_eq!(remembered(h.state(), owner).len(), 3, "an agent's clicks forget nothing");
    let forget = h.state().hit.rect("sharedBeforeForget", 1).expect("Forget");
    click(&mut h, forget.center());
    assert_eq!(remembered(h.state(), owner), ["support@example.org", "billing@example.org"]);
    let all = h.state().hit.rect("sharedBeforeForgetAll", 0).expect("Forget all");
    click(&mut h, all.center());
    assert!(remembered(h.state(), owner).is_empty());
    h.run_steps(2);
    assert!(h.query_by_label(tl!("Opened before")).is_none(), "and the section goes");
}

#[test]
fn twenty_opened_before_fit_a_short_window_and_only_those_in_view_are_targets() {
    let names: Vec<String> = (0..20).map(|i| format!("team{i:02}@example.org")).collect();
    let list: Vec<(&str, &str)> = names.iter().map(|a| (a.as_str(), "")).collect();
    let (app, owner, _) = window(&list);
    assert_eq!(remembered(&app, owner).len(), 20);
    let mut h = harness_at(app, 760.0, 480.0);
    h.run_steps(5);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(760.0, 480.0));
    for label in [tl!("Add Shared Mailbox"), tl!("Opened before"), tl!("Add"), tl!("Cancel")] {
        let rects: Vec<egui::Rect> = h.query_all_by_label(label).map(|n| n.rect()).collect();
        assert!(!rects.is_empty() && rects.iter().all(|r| screen.contains_rect(*r)), "{label} on screen: {rects:?}");
    }
    assert!(h.state().hit.rect("sharedBeforeAdd", 0).is_some(), "the first row is a target");
    assert!(h.state().hit.rect("sharedBeforeAdd", 19).is_none(), "the last, out of view, is none");
}

#[test]
fn a_long_address_is_cut_to_its_row_and_its_buttons_stay_in_the_dialog() {
    let long = format!("{}@{}.example.org", "a".repeat(60), "b".repeat(60));
    let (app, _, _) = window(&[(long.as_str(), "A rather long name for a shared mailbox of the finance department")]);
    let mut h = harness_at(app, 760.0, 480.0);
    h.run_steps(4);
    let dialog = h.get_by_label(tl!("Add Shared Mailbox")).rect();
    let add = h.state().hit.rect("sharedBeforeAdd", 0).expect("Add again");
    let forget = h.state().hit.rect("sharedBeforeForget", 0).expect("Forget");
    assert!(add.right() <= 760.0 && forget.right() <= 760.0 && forget.left() > dialog.left(), "{add:?} {forget:?}");
}
