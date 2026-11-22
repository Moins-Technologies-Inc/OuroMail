// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Add Shared Mailbox's search of the organisation's directory: what is sent and when (two
//! characters, a pause, one search at a time, nothing for older text, a closed dialog, a personal
//! account, offline or input a script put in), the permission asked by the window first and
//! answered by the owner alone, and what never leaves the dialog. Hosts are fakes; every name and
//! address is invented.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_engine::agent::Class;
use mailcraft_model::Id;
use mailcraft_model::directory::{DirectoryAnswer, DirectoryEntry, DirectoryOutcome};
use mailcraft_model::sync::Tokens;
use serde_json::{Value, json};

use super::*;
use crate::control::{ControlRequest, Outcome};
use crate::dialogs::Dialog;
use crate::{GraphRequest, GraphResult, MailApp, Services, SignInRequest};

const OWNER: &str = "noor@example.org";
/// What lies far from any pause.
const LATER: f64 = 1.0e9;

/// What each search was asked: the text, its ticket, the token, its call-off flag, and the way to
/// answer it.
type Jobs = Arc<Mutex<Vec<(String, u64, String, Arc<AtomicBool>, Sender<DirectoryResult>)>>>;
/// What each sign-in was asked: for Graph, for the directory, for the one page, for the Favorites.
type SignIns = Arc<Mutex<Vec<(bool, bool, bool, bool, Sender<Result<Tokens, String>>)>>>;

fn tokens(access: &str, refresh: &str) -> Tokens {
    Tokens { access: access.into(), refresh: refresh.into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() }
}

/// A window with a work or school account signed in this session, hosts that note what they are
/// asked and answer when the test says, and Add Shared Mailbox open under that account.
fn window() -> (MailApp, Id, Jobs, SignIns) {
    let mut app = MailApp::new(Session::empty(), Services::default());
    app.autosave = false;
    let owner =
        app.execute("account.add", json!({"email": OWNER, "kind": "microsoft", "auth": "oauth2", "directory": "organisation"})).unwrap()["id"]
            .as_u64()
            .unwrap();
    app.tokens.insert(owner, tokens("mail-access", "ref-1"));
    let (jobs, sign_ins): (Jobs, SignIns) = Default::default();
    let log = jobs.clone();
    app.services.graph_directory = Some(Box::new(move |job: DirectoryJob| {
        let (tx, rx) = mpsc::channel();
        log.lock().unwrap().push((job.query.text.clone(), job.query.ticket, job.access.clone(), job.cancel.clone(), tx));
        rx
    }));
    let asked = sign_ins.clone();
    app.services.sign_in = Some(Box::new(move |req: SignInRequest| {
        let (tx, rx) = mpsc::channel();
        asked.lock().unwrap().push((req.graph, req.directory, req.sync, req.favorites, tx));
        rx
    }));
    app.execute("ui.dialog", json!({"name": "sharedMailbox", "account": owner})).unwrap();
    (app, owner, jobs, sign_ins)
}

/// The owner types `text` into the search field at `at`.
fn typed(app: &mut MailApp, text: &str, at: f64) {
    let mut dialog = app.dialog.take();
    if let Some(Dialog::SharedMailbox { account, search, .. }) = &mut dialog {
        search.text = text.into();
        app.directory_typed(*account, search, at);
    }
    app.dialog = dialog;
}

fn field(app: &MailApp) -> &DirectoryField {
    match &app.dialog {
        Some(Dialog::SharedMailbox { search, .. }) => search,
        _ => panic!("Add Shared Mailbox is open"),
    }
}

fn texts(jobs: &Jobs) -> Vec<String> {
    jobs.lock().unwrap().iter().map(|j| j.0.clone()).collect()
}

/// Search `n` answers `outcome` with `entries`, and perhaps tokens.
fn answer(jobs: &Jobs, n: usize, account: Id, outcome: DirectoryOutcome, entries: &[(&str, &str)], tokens: Option<Tokens>) {
    let jobs = jobs.lock().unwrap();
    let (_, ticket, _, _, tx) = &jobs[n];
    let entries = entries.iter().filter_map(|(name, address)| DirectoryEntry::found(name, address)).collect();
    let answer = DirectoryAnswer { account, ticket: *ticket, entries, outcome, more: false };
    tx.send(DirectoryResult { answer, tokens }).unwrap();
}

#[test]
fn one_character_asks_nothing() {
    let (mut app, _, jobs, _) = window();
    typed(&mut app, "h", 0.0);
    app.directory_tick_at(LATER);
    assert!(jobs.lock().unwrap().is_empty());
    assert_eq!(field(&app).area, Area::TooShort);
    // (Nor what shows nothing, or only an ampersand, beside it.)
    typed(&mut app, "h\u{200b}&", 0.0);
    app.directory_tick_at(LATER);
    assert!(jobs.lock().unwrap().is_empty());
}

#[test]
fn two_characters_ask_once_after_the_pause() {
    let (mut app, _, jobs, _) = window();
    typed(&mut app, "he", 1_000.0);
    assert_eq!(field(&app).area, Area::Searching);
    app.directory_tick_at(1_000.0 + PAUSE_MS - 1.0);
    assert!(jobs.lock().unwrap().is_empty(), "not before the pause is over");
    app.directory_tick_at(1_000.0 + PAUSE_MS);
    assert_eq!(texts(&jobs), ["he"]);
    for at in [1_400.0, 5_000.0, LATER] {
        app.directory_tick_at(at);
    }
    assert_eq!(jobs.lock().unwrap().len(), 1, "once");
}

#[test]
fn typing_on_asks_again_only_after_the_next_pause() {
    let (mut app, owner, jobs, _) = window();
    typed(&mut app, "he", 0.0);
    app.directory_tick_at(PAUSE_MS);
    answer(&jobs, 0, owner, DirectoryOutcome::Found, &[("Help Desk", "helpdesk@example.org")], None);
    app.directory_tick_at(PAUSE_MS + 1.0);
    typed(&mut app, "hel", 400.0);
    typed(&mut app, "help", 500.0);
    app.directory_tick_at(500.0 + PAUSE_MS - 1.0);
    assert_eq!(jobs.lock().unwrap().len(), 1);
    app.directory_tick_at(500.0 + PAUSE_MS);
    assert_eq!(texts(&jobs), ["he", "help"], "the text as it is after the pause, once");
}

#[test]
fn an_answer_for_an_older_text_is_dropped_and_one_search_at_a_time_is_in_flight() {
    let (mut app, owner, jobs, _) = window();
    typed(&mut app, "he", 0.0);
    app.directory_tick_at(PAUSE_MS);
    typed(&mut app, "ada", 350.0);
    // The first is still on its way: the second waits for it.
    app.directory_tick_at(350.0 + PAUSE_MS);
    assert_eq!(jobs.lock().unwrap().len(), 1, "at most one in flight");
    assert_eq!(field(&app).area, Area::Searching);
    // It comes: for the text before, so nothing of it is shown, and the newer text goes now.
    answer(&jobs, 0, owner, DirectoryOutcome::Found, &[("Help Desk", "helpdesk@example.org")], None);
    app.directory_tick_at(700.0);
    assert!(field(&app).entries.is_empty(), "no stale list under newer text");
    assert_eq!(texts(&jobs), ["he", "ada"]);
    answer(&jobs, 1, owner, DirectoryOutcome::Found, &[("Ada Lovelace", "ada@example.org")], None);
    app.directory_tick_at(701.0);
    let shown: Vec<&str> = field(&app).entries.iter().map(|e| e.address.as_str()).collect();
    assert_eq!(shown, ["ada@example.org"]);
    assert_eq!(field(&app).area, Area::Found);
}

#[test]
fn closing_the_dialog_drops_the_answer_on_its_way_and_tells_the_host_to_stop() {
    let (mut app, owner, jobs, _) = window();
    typed(&mut app, "help", 0.0);
    app.directory_tick_at(PAUSE_MS);
    app.dialog = None;
    app.directory_tick_at(PAUSE_MS + 1.0);
    assert!(jobs.lock().unwrap()[0].3.load(Ordering::Relaxed), "called off");
    answer(&jobs, 0, owner, DirectoryOutcome::Found, &[("Help Desk", "helpdesk@example.org")], None);
    app.directory_tick_at(PAUSE_MS + 2.0);
    app.execute("ui.dialog", json!({"name": "sharedMailbox", "account": owner})).unwrap();
    app.directory_tick_at(LATER);
    assert!(field(&app).entries.is_empty() && field(&app).text.is_empty());
    assert_eq!(jobs.lock().unwrap().len(), 1, "nothing asked after it closed");
}

#[test]
fn offline_or_without_the_host_hook_nothing_is_asked() {
    let (mut app, _, jobs, _) = window();
    app.execute("sync.offline", json!({"value": true})).unwrap();
    typed(&mut app, "help", 0.0);
    app.directory_tick_at(LATER);
    assert!(jobs.lock().unwrap().is_empty());
    assert_eq!(field(&app).area, Area::Offline);
    let (mut app, _, jobs, _) = window();
    app.services.graph_directory = None;
    assert!(!app.directory_offered(field(&app)), "the dialog has only the address field");
    typed(&mut app, "help", 0.0);
    app.directory_tick_at(LATER);
    assert!(jobs.lock().unwrap().is_empty());
}

#[test]
fn nothing_is_asked_for_a_personal_account_or_a_shared_mailbox() {
    let (mut app, owner, jobs, _) = window();
    let personal = app
        .execute("account.add", json!({"email": "ada@personal.example", "kind": "microsoft", "auth": "oauth2", "directory": "personal"}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    app.tokens.insert(personal, tokens("p-access", "p-ref"));
    let shared =
        app.execute("account.addSharedMailbox", json!({"account": owner, "address": "support@example.org"})).unwrap()["id"].as_u64().unwrap();
    // Neither gets the dialog at all; and one put there all the same asks nothing.
    for account in [personal, shared] {
        app.dialog = None;
        assert!(app.execute("ui.dialog", json!({"name": "sharedMailbox", "account": account})).is_err() || app.dialog.is_none());
        app.dialog = Some(Dialog::SharedMailbox { account, address: String::new(), error: String::new(), checking: None, search: Box::default() });
        typed(&mut app, "help", 0.0);
        app.directory_tick_at(LATER);
    }
    assert!(jobs.lock().unwrap().is_empty());
}

#[test]
fn text_a_script_or_an_agent_put_into_the_window_is_never_searched() {
    let (mut app, _, jobs, _) = window();
    app.driven = true;
    typed(&mut app, "help", 0.0);
    app.driven = false;
    app.directory_tick_at(LATER);
    assert!(jobs.lock().unwrap().is_empty());
    // The owner's own typing is.
    typed(&mut app, "help", 0.0);
    app.directory_tick_at(LATER);
    assert_eq!(texts(&jobs), ["help"]);
}

/// The window's answer to a control request from an agent allowed everything.
fn as_agent(app: &mut MailApp, method: &str, params: Value) -> Value {
    let (req, _rx) = ControlRequest::new(method, params);
    let req = req.from_agent(Class::ALL.to_vec());
    match crate::control::handle(app, &egui::Context::default(), &req) {
        Outcome::Done(v) => v,
        _ => json!({"ok": "asked"}),
    }
}

#[test]
fn an_agent_can_neither_search_the_directory_nor_answer_its_permission() {
    let (mut app, owner, jobs, sign_ins) = window();
    let attempts = [
        ("ui.searchDirectory", json!({"text": "help"})),
        ("engine.execute", json!({"command": "ui.searchDirectory", "params": {"text": "help"}})),
        ("engine.execute", json!({"command": "sync.directoryRequest", "params": {"account": owner, "text": "help"}})),
        ("ui.allowDirectory", json!({"account": owner, "confirmed": true})),
        ("engine.execute", json!({"command": "ui.allowDirectory", "params": {"account": owner, "confirmed": true}})),
    ];
    for (method, params) in attempts {
        let said = as_agent(&mut app, method, params.clone());
        assert_eq!(said["ok"], json!(false), "{method} {params}: {said}");
        assert!(app.asking.is_empty(), "not even put to the owner: {method}");
    }
    app.directory_tick_at(LATER);
    assert!(jobs.lock().unwrap().is_empty(), "nothing searched");
    assert!(sign_ins.lock().unwrap().is_empty(), "no page opened");
    assert!(field(&app).text.is_empty(), "nothing typed into the dialog");
}

#[test]
fn the_window_asks_first_and_only_allow_opens_the_page_for_that_permission_alone() {
    let (mut app, owner, jobs, sign_ins) = window();
    typed(&mut app, "help", 0.0);
    app.directory_tick_at(PAUSE_MS);
    answer(&jobs, 0, owner, DirectoryOutcome::ConsentNeeded, &[], None);
    app.directory_tick_at(PAUSE_MS + 1.0);
    assert_eq!(field(&app).area, Area::Offer, "the window's own question");
    assert!(sign_ins.lock().unwrap().is_empty(), "no page before the owner says Allow");
    assert_eq!(
        app.directory_line(owner).map(|l| (l.said, l.again)),
        Some((tl!("Directory search: not asked in this session"), false)),
        "File › Info: not asked yet"
    );
    // Allow (the owner's click runs this): one page, for the directory alone.
    app.run("ui.allowDirectory", json!({"account": owner, "confirmed": true})).unwrap();
    {
        let asked = sign_ins.lock().unwrap();
        assert_eq!(asked.len(), 1);
        let (graph, directory, sync, favorites, _) = &asked[0];
        assert_eq!((*graph, *directory, *sync, *favorites), (true, true, false, false));
    }
    assert_eq!(field(&app).area, Area::Asking);
    // Given: what was typed is searched at once, with the token the page brought.
    let tx = sign_ins.lock().unwrap()[0].4.clone();
    tx.send(Ok(Tokens { email: OWNER.into(), ..tokens("dir-access", "ref-2") })).unwrap();
    app.poll_sign_in();
    app.directory_tick_at(PAUSE_MS + 2.0);
    let jobs_now = jobs.lock().unwrap();
    assert_eq!(jobs_now.len(), 2);
    assert_eq!((jobs_now[1].0.as_str(), jobs_now[1].2.as_str()), ("help", "dir-access"));
    drop(jobs_now);
    assert_eq!(app.directory_line(owner).map(|l| (l.said, l.again)), Some((tl!("Directory search: allowed"), false)));
    assert_eq!(app.tokens.get(&owner).map(|t| t.refresh.as_str()), Some("ref-2"), "the newest refresh token is the account's");
}

#[test]
fn not_now_or_a_closed_question_sends_nothing_more_and_file_info_asks_again() {
    let (mut app, owner, jobs, sign_ins) = window();
    typed(&mut app, "help", 0.0);
    app.directory_tick_at(PAUSE_MS);
    answer(&jobs, 0, owner, DirectoryOutcome::ConsentNeeded, &[], None);
    app.directory_tick_at(PAUSE_MS + 1.0);
    assert_eq!(field(&app).area, Area::Offer);
    // Closed unanswered: a no for this session.
    app.dialog = None;
    app.directory_tick_at(PAUSE_MS + 2.0);
    app.execute("ui.dialog", json!({"name": "sharedMailbox", "account": owner})).unwrap();
    typed(&mut app, "helpdesk", 1_000.0);
    app.directory_tick_at(LATER);
    assert_eq!(jobs.lock().unwrap().len(), 1, "nothing more sent");
    assert_eq!(field(&app).area, Area::Declined);
    assert!(sign_ins.lock().unwrap().is_empty());
    // Adding by its address works as before.
    assert!(app.execute("account.addSharedMailbox", json!({"account": owner, "address": "support@example.org"})).is_ok());
    assert_eq!(app.directory_line(owner).map(|l| (l.said, l.again)), Some((tl!("Directory search: not now (for this session)"), true)));
    // Ask again: the dialog, with the question in it; still no page until Allow.
    app.dialog = None;
    assert!(app.directory_ask_again(owner));
    assert_eq!(field(&app).area, Area::Offer);
    assert!(sign_ins.lock().unwrap().is_empty());
}

#[test]
fn a_no_on_the_page_or_an_administrators_is_said_in_the_dialog() {
    for (said, area) in [
        (crate::favorites_sync::NEEDS_APPROVAL, Area::Approval),
        (crate::favorites_sync::REFUSED_BY_ORGANISATION, Area::NotAllowed),
        ("Microsoft's page came back without signing you in", Area::Declined),
    ] {
        let (mut app, owner, jobs, sign_ins) = window();
        typed(&mut app, "help", 0.0);
        app.run("ui.allowDirectory", json!({"account": owner, "confirmed": true})).unwrap();
        let tx = sign_ins.lock().unwrap()[0].4.clone();
        tx.send(Err(said.to_string())).unwrap();
        app.poll_sign_in();
        app.directory_tick_at(LATER);
        assert_eq!(field(&app).area, area, "{said}");
        assert!(jobs.lock().unwrap().is_empty(), "{said}: nothing searched");
        assert_eq!(app.directory_line(owner).map(|l| l.again), Some(true), "{said}: Ask again");
    }
}

#[test]
fn the_query_and_the_results_never_reach_the_log_the_status_inspect_or_the_files() {
    let lines = crate::graph_switch::tests::logged();
    let (mut app, owner, jobs, _) = window();
    typed(&mut app, "zqxcanary", 0.0);
    app.directory_tick_at(PAUSE_MS);
    answer(&jobs, 0, owner, DirectoryOutcome::Found, &[("Canary Qwertyson", "canary.qwertyson@example.org")], None);
    app.directory_tick_at(PAUSE_MS + 1.0);
    assert_eq!(field(&app).entries.len(), 1);
    let logged = lines.lock().unwrap().join("\n");
    assert!(logged.contains("directory search: 1 entries"), "the one line it logs is there: {logged}");
    let inspect = match crate::control::handle(&mut app, &egui::Context::default(), &ControlRequest::new("ui.inspect", json!({})).0) {
        Outcome::Done(v) => v.to_string(),
        _ => String::new(),
    };
    assert!(inspect.contains("sharedMailbox"), "{inspect}");
    let prefs = serde_json::to_string(&app.prefs()).unwrap();
    let mailbox = serde_json::to_string(&app.session.mailbox).unwrap();
    let status = format!("{} {:?}", app.session.sync_status, app.status_msg.as_ref().map(|s| &s.0));
    for (place, text) in [("log", &logged), ("inspect", &inspect), ("ui.json", &prefs), ("mailbox", &mailbox), ("status", &status)] {
        for canary in ["zqxcanary", "Qwertyson", "canary.qwertyson"] {
            assert!(!text.contains(canary), "{canary} reached {place}");
        }
    }
}

// ---- the dialog, drawn ----

fn harness(app: MailApp) -> egui_kittest::Harness<'static, MailApp> {
    egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    )
}

fn click(h: &mut egui_kittest::Harness<'static, MailApp>, at: egui::Pos2) {
    h.event(egui::Event::PointerMoved(at));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
}

/// A window whose Graph runs are held (the check of an added mailbox stays under way).
fn held_graph(app: &mut MailApp) -> Arc<Mutex<Vec<Sender<GraphResult>>>> {
    let held: Arc<Mutex<Vec<Sender<GraphResult>>>> = Default::default();
    let keep = held.clone();
    app.session.driver.set_auto_sync(false);
    app.services.graph = Some(Box::new(move |req: GraphRequest| {
        let (tx, rx) = mpsc::channel();
        // (The calendar's own jobs are answered at once; a mailbox's run is held.)
        if req.mail.is_none() {
            let _ = tx.send(GraphResult::default());
        } else {
            keep.lock().unwrap().push(tx);
        }
        rx
    }));
    // (A host that can open mail connections, as the desktop's: a shared mailbox never uses them.)
    app.services.sync = Some(Box::new(|req: mailcraft_model::sync::SyncRequest| {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(mailcraft_model::sync::SyncResult { account: req.account.id, ..Default::default() });
        rx
    }));
    held
}

fn found_two(h: &mut egui_kittest::Harness<'static, MailApp>) {
    let entries = json!([{"name": "Help Desk", "address": "helpdesk@example.org"}, {"name": "Sales Team", "address": "sales@example.org"}]);
    h.state_mut().execute("ui.searchDirectory", json!({"text": "he", "answer": {"outcome": "found", "entries": entries}})).unwrap();
    h.run_steps(3);
}

fn added(app: &MailApp) -> Vec<(String, String)> {
    app.session.mailbox.accounts.iter().filter(|a| a.is_shared()).map(|a| (a.email.clone(), a.display_name.clone())).collect()
}

#[test]
fn clicking_a_result_adds_it_and_checks_it_as_add_does() {
    let (mut app, _, _, _) = window();
    let _held = held_graph(&mut app);
    let mut h = harness(app);
    h.run_steps(3);
    found_two(&mut h);
    let row = h.state().hit.rect("directoryResult", 1).expect("the second row is drawn");
    click(&mut h, row.center());
    let said = match &h.state().dialog {
        Some(Dialog::SharedMailbox { error, .. }) => error.clone(),
        _ => String::new(),
    };
    assert!(said.is_empty(), "{said}");
    assert_eq!(added(h.state()), [("sales@example.org".to_string(), String::new())], "named as a typed address is");
    assert!(matches!(&h.state().dialog, Some(Dialog::SharedMailbox { checking: Some(_), .. })), "checked as a typed address is");
}

#[test]
fn enter_adds_the_highlighted_result_and_arrows_move_the_highlight() {
    let (mut app, _, _, _) = window();
    let _held = held_graph(&mut app);
    let mut h = harness(app);
    h.run_steps(3);
    found_two(&mut h);
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(1);
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(1);
    h.key_press(egui::Key::ArrowUp);
    h.run_steps(1);
    assert_eq!(field(h.state()).highlight, Some(0));
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(added(h.state()), [("helpdesk@example.org".to_string(), String::new())], "named as a typed address is");
}

#[test]
fn allow_takes_the_owners_own_click_and_never_input_a_script_put_in() {
    let (mut app, owner, _, sign_ins) = window();
    app.execute("ui.searchDirectory", json!({"text": "help", "answer": {"outcome": "consentNeeded"}})).unwrap();
    let mut h = harness(app);
    h.run_steps(3);
    let allow = h.state().hit.rect("directoryAllow", 0).expect("the question's Allow is drawn");
    h.state_mut().driven = true;
    click(&mut h, allow.center());
    assert!(sign_ins.lock().unwrap().is_empty(), "a script's click opens no page");
    assert_eq!(field(h.state()).area, Area::Offer);
    h.state_mut().driven = false;
    click(&mut h, allow.center());
    assert_eq!(sign_ins.lock().unwrap().len(), 1);
    assert_eq!(h.state().directory_search.perm.get(&owner), Some(&Permission::Asking));
}

#[test]
fn not_now_is_the_owners_too() {
    let (mut app, owner, _, _) = window();
    app.execute("ui.searchDirectory", json!({"text": "help", "answer": {"outcome": "consentNeeded"}})).unwrap();
    let mut h = harness(app);
    h.run_steps(3);
    let not_now = h.state().hit.rect("directoryNotNow", 0).expect("Not now is drawn");
    click(&mut h, not_now.center());
    assert_eq!(field(h.state()).area, Area::Declined);
    assert_eq!(h.state().directory_search.perm.get(&owner), Some(&Permission::Declined));
}

#[path = "directory_search_window_tests.rs"]
mod window_tests;
