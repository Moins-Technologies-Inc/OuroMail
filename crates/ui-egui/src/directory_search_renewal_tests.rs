// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The directory's search, after the renewal reviews:
//! - only the owner answers the permission's question, a no included;
//! - a page called off records no answer;
//! - an agent's keys never reach the question;
//! - what is held for a sign-in is checked against that sign-in where it is used;
//! - tokens for somebody else are never kept;
//! - a refused mail sign-in drops the directory's token;
//! - no second search goes while one may still be on its way;
//! - File › Info at narrow widths;
//! - the question's name for a screen reader;
//! - Enter while the question is up;
//! - more rows in a short window.
//!
//! Hosts are fakes; every name and address is invented.

use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;

use super::*;

type H = egui_kittest::Harness<'static, MailApp>;

const QUESTION: &str = "Searching your organisation's directory needs your permission. Microsoft's page opens in your browser to ask you, once.";

fn soon() -> f64 {
    crate::now_ms() + PAUSE_MS * 10.0
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

fn ty(h: &mut H, text: &str) {
    h.event(egui::Event::Text(text.into()));
    h.step();
}

/// Frames with what the control channel queued fed in, as the window's `raw_input_hook` does.
fn driven_steps(h: &mut H, n: usize) {
    for _ in 0..n {
        let mut raw = egui::RawInput::default();
        h.state_mut().raw_input_hook(&mut raw);
        h.input_mut().events.extend(raw.events);
        h.step();
    }
}

/// An agent's control request with these grants (none: `vec![]`).
fn agent_with(app: &mut MailApp, grants: Vec<Class>, method: &str, params: Value) -> Value {
    let (req, _rx) = ControlRequest::new(method, params);
    let req = req.from_agent(grants);
    match crate::control::handle(app, &egui::Context::default(), &req) {
        Outcome::Done(v) => v,
        _ => json!({"ok": "asked"}),
    }
}

/// The question up after "hel", without a window drawn.
fn question(app: &mut MailApp, jobs: &Jobs, owner: Id) {
    typed(app, "hel", 0.0);
    app.directory_tick_at(PAUSE_MS);
    let n = jobs.lock().unwrap().len() - 1;
    answer(jobs, n, owner, DirectoryOutcome::ConsentNeeded, &[], None);
    app.directory_tick_at(PAUSE_MS + 1.0);
    app.directory_tick_at(PAUSE_MS + 2.0);
    assert_eq!(field(app).area, Area::Offer, "premise: the question is up");
}

/// The question up after "hel", typed in a drawn window.
fn question_drawn(app: MailApp, jobs: &Jobs, owner: Id) -> H {
    let mut h = harness(app);
    h.run_steps(3);
    ty(&mut h, "hel");
    h.state_mut().directory_tick_at(soon());
    let n = jobs.lock().unwrap().len() - 1;
    answer(jobs, n, owner, DirectoryOutcome::ConsentNeeded, &[], None);
    h.run_steps(3);
    assert_eq!(field(h.state()).area, Area::Offer, "premise: the question is up");
    h
}

/// The dialog opened again and searched: the question comes again, as the first time.
fn asked_again_at_the_next_search(app: &mut MailApp, jobs: &Jobs, owner: Id, why: &str) {
    app.dialog = None;
    app.execute("ui.dialog", json!({"name": "sharedMailbox", "account": owner})).unwrap();
    let before = jobs.lock().unwrap().len();
    typed(app, "help", 1_000.0);
    app.directory_tick_at(1_000.0 + PAUSE_MS);
    assert_eq!(jobs.lock().unwrap().len(), before + 1, "{why}: the next search goes");
    answer(jobs, before, owner, DirectoryOutcome::ConsentNeeded, &[], None);
    app.directory_tick_at(1_000.0 + PAUSE_MS + 1.0);
    assert_eq!(field(app).area, Area::Offer, "{why}: and the question comes again");
}

fn address_below(h: &H, label: &str) -> egui::Rect {
    let top = h.get_by_label(label).rect().bottom();
    let below = h.query_all_by_role(Role::TextInput).map(|n| n.rect()).filter(|r| r.top() >= top - 1.0);
    below.min_by(|a, b| a.top().total_cmp(&b.top())).expect("a field under the label")
}

// ---- SEC-N1: a no is the owner's own

#[test]
fn an_agents_close_dialog_without_any_grant_records_no_answer() {
    let (mut app, owner, jobs, _) = window();
    question(&mut app, &jobs, owner);
    agent_with(&mut app, vec![], "engine.execute", json!({"command": "ui.closeDialog", "params": {}}));
    assert!(app.dialog.is_none(), "premise: the dialog was closed");
    app.directory_tick_at(LATER);
    assert_eq!(app.directory_search.perm.get(&owner), None, "nothing recorded for the owner");
    assert_eq!(app.directory_line(owner).map(|l| l.said), Some(tl!("Directory search: not asked in this session")));
    asked_again_at_the_next_search(&mut app, &jobs, owner, "after an agent's close");
}

#[test]
fn an_agents_escape_or_click_on_the_close_records_no_answer() {
    for how in ["escape", "close"] {
        let (app, owner, jobs, _) = window();
        let mut h = question_drawn(app, &jobs, owner);
        if how == "escape" {
            agent_with(h.state_mut(), vec![Class::Input], "ui.key", json!({"key": "Escape"}));
        } else {
            let close = h.get_by_label("Close window").rect();
            agent_with(h.state_mut(), vec![Class::Input], "ui.click", json!({"x": close.center().x, "y": close.center().y}));
        }
        driven_steps(&mut h, 6);
        assert!(h.state().dialog.is_none(), "{how}: premise: the dialog went");
        h.state_mut().directory_tick_at(LATER);
        assert_eq!(h.state().directory_search.perm.get(&owner), None, "{how}: nothing recorded for the owner");
    }
}

#[test]
fn the_owners_own_escape_is_his_no() {
    let (app, owner, jobs, _) = window();
    let mut h = question_drawn(app, &jobs, owner);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.state().dialog.is_none(), "premise: the dialog went");
    h.state_mut().directory_tick_at(LATER);
    assert_eq!(h.state().directory_search.perm.get(&owner), Some(&Permission::Declined), "his Escape is his no, for this session");
}

// ---- SEC-N2: a page called off records no answer and offers Ask again

#[test]
fn a_page_called_off_records_no_answer_and_offers_ask_again() {
    for how in ["agent", "owner", "add"] {
        let (mut app, owner, jobs, sign_ins) = window();
        let _held = held_graph(&mut app);
        question(&mut app, &jobs, owner);
        app.begin_directory_sign_in(owner).unwrap();
        assert_eq!(sign_ins.lock().unwrap().len(), 1, "{how}: premise: the page is open");
        match how {
            "agent" => {
                agent_with(&mut app, vec![], "engine.execute", json!({"command": "ui.cancelSignIn", "params": {}}));
            }
            "owner" => {
                app.run("ui.cancelSignIn", json!({})).unwrap();
            }
            _ => {
                // The owner types an address and presses Add while the page waits.
                let id = app.execute("account.addSharedMailbox", json!({"account": owner, "address": "support@example.org"})).unwrap()["id"]
                    .as_u64()
                    .unwrap();
                app.begin_shared_check(id).unwrap();
            }
        }
        app.poll_sign_in();
        app.directory_tick_at(LATER);
        let perm = app.directory_search.perm.get(&owner).copied();
        assert_ne!(perm, Some(Permission::Asking), "{how}: not left asking");
        assert_ne!(perm, Some(Permission::Declined), "{how}: and no no recorded");
        let line = app.directory_line(owner).map(|l| (l.said, l.again));
        assert_eq!(line, Some((tl!("Directory search: not answered in this session"), true)), "{how}: File › Info offers Ask again");
        if matches!(app.dialog, Some(Dialog::SharedMailbox { .. })) && how != "add" {
            assert_eq!(field(&app).area, Area::Offer, "{how}: the dialog asks again");
        }
    }
}

// ---- SEC-N3: an agent's keys never reach the question

#[test]
fn an_agents_tab_never_puts_the_owners_next_key_on_allow() {
    let (app, owner, jobs, sign_ins) = window();
    let mut h = question_drawn(app, &jobs, owner);
    for _ in 0..2 {
        agent_with(h.state_mut(), vec![Class::Input], "ui.key", json!({"key": "Tab"}));
        driven_steps(&mut h, 3);
    }
    assert!(sign_ins.lock().unwrap().is_empty(), "premise: the agent's own keys open nothing");
    h.key_press(egui::Key::Space);
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(sign_ins.lock().unwrap().is_empty(), "the owner's next keys find the keyboard on the question, not on Allow");
    assert_eq!((field(h.state()).area, h.state().directory_search.perm.get(&owner)), (Area::Offer, None));
    // The control: his own Tab reaches Allow.
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    h.key_press(egui::Key::Space);
    h.run_steps(3);
    assert_eq!(sign_ins.lock().unwrap().len(), 1, "his own Tab and Space");
}

// ---- SEC-N4: checked where it is used

#[test]
fn a_token_held_for_a_sign_in_that_changed_where_nothing_let_go_of_it_is_never_sent() {
    let (mut app, owner, jobs, _) = window();
    typed(&mut app, "he", 0.0);
    app.directory_tick_at(PAUSE_MS);
    let mine = Tokens {
        access: "tok-stale".into(),
        refresh: "ref-stale".into(),
        email: OWNER.into(),
        expires_at: mailcraft_time::now() + 3_600,
        ..Default::default()
    };
    answer(&jobs, 0, owner, DirectoryOutcome::Found, &[], Some(mine));
    app.directory_tick_at(PAUSE_MS + 1.0);
    assert_eq!(app.directory_search.perm.get(&owner), Some(&Permission::Given), "premise: allowed");
    // The sign-in changes in place, past every place that lets go of what is held for it.
    if let Some(oauth) = app.session.mailbox.account_mut(owner).ok().and_then(|a| a.oauth.as_mut()) {
        oauth.tenant = "contoso.example".into();
    }
    assert_eq!(app.directory_line(owner).map(|l| l.said), Some(tl!("Directory search: not asked in this session")), "not allowed any more");
    typed(&mut app, "zqx", 1_000.0);
    app.directory_tick_at(1_000.0 + PAUSE_MS);
    let jobs = jobs.lock().unwrap();
    assert_eq!(jobs.len(), 2, "premise: the search went");
    assert_ne!(jobs[1].2, "tok-stale", "the old sign-in's token is never sent");
}

#[test]
fn a_token_held_for_another_mailbox_where_nothing_let_go_of_it_is_never_sent() {
    let (mut app, owner, jobs, _) = window();
    typed(&mut app, "he", 0.0);
    app.directory_tick_at(PAUSE_MS);
    let mine = Tokens {
        access: "tok-other-box".into(),
        refresh: "ref-o".into(),
        email: OWNER.into(),
        expires_at: mailcraft_time::now() + 3_600,
        ..Default::default()
    };
    answer(&jobs, 0, owner, DirectoryOutcome::Found, &[], Some(mine));
    app.directory_tick_at(PAUSE_MS + 1.0);
    app.dialog = None;
    // Another mailbox, without the window's letting go of what it held.
    app.session.replace_mailbox(Session::empty().mailbox);
    let cy = app.execute("account.add", json!({"email": OWNER, "kind": "microsoft", "auth": "oauth2", "directory": "organisation"})).unwrap()["id"]
        .as_u64()
        .unwrap();
    assert_eq!(cy, owner, "premise: the same id, the same address");
    assert_eq!(app.directory_line(cy).map(|l| l.said), Some(tl!("Directory search: not asked in this session")));
    // (Signed in, in the other mailbox.)
    app.tokens.insert(cy, tokens("cy-access", "cy-ref"));
    app.execute("ui.dialog", json!({"name": "sharedMailbox", "account": cy})).unwrap();
    typed(&mut app, "zqx", 1_000.0);
    app.directory_tick_at(1_000.0 + PAUSE_MS);
    let jobs = jobs.lock().unwrap();
    assert_eq!(jobs.len(), 2, "premise: the search went");
    assert_ne!(jobs[1].2, "tok-other-box", "another mailbox's token is never sent");
}

// ---- SEC-N7

#[test]
fn tokens_that_come_back_for_somebody_else_are_never_kept() {
    let (mut app, owner, jobs, _) = window();
    typed(&mut app, "he", 0.0);
    app.directory_tick_at(PAUSE_MS);
    let theirs = Tokens {
        access: "tok-mallory".into(),
        refresh: "ref-mallory".into(),
        email: "mallory@other.example".into(),
        expires_at: mailcraft_time::now() + 3_600,
        ..Default::default()
    };
    answer(&jobs, 0, owner, DirectoryOutcome::Found, &[], Some(theirs));
    app.directory_tick_at(PAUSE_MS + 1.0);
    assert!(!app.directory_search.access.values().any(|t| t.access == "tok-mallory"), "no token kept");
    assert!(!app.tokens.values().chain(app.graph_tokens.values()).any(|t| t.refresh == "ref-mallory"), "no refresh token kept");
    assert_ne!(app.directory_search.perm.get(&owner), Some(&Permission::Given), "not taken as a yes");
}

// ---- SEC-N6

#[test]
fn a_refused_mail_sign_in_drops_the_directorys_token_too() {
    let (mut app, owner, jobs, _) = window();
    typed(&mut app, "he", 0.0);
    app.directory_tick_at(PAUSE_MS);
    let mine = Tokens {
        access: "tok-refused".into(),
        refresh: "ref-r".into(),
        email: OWNER.into(),
        expires_at: mailcraft_time::now() + 3_600,
        ..Default::default()
    };
    answer(&jobs, 0, owner, DirectoryOutcome::Found, &[], Some(mine));
    app.directory_tick_at(PAUSE_MS + 1.0);
    assert!(app.directory_search.access.contains_key(&owner), "premise");
    app.graph_mail_needs_sign_in(owner, false, false, false);
    assert!(!app.directory_search.access.contains_key(&owner), "the directory's token goes with the refused sign-in");
}

// ---- SEC-N5 / CORR-N3

#[test]
fn after_the_deadline_no_other_search_starts_until_the_one_on_its_way_has_ended() {
    let (mut app, _, jobs, _) = window();
    typed(&mut app, "help", 0.0);
    app.directory_tick_at(PAUSE_MS);
    app.directory_tick_at(PAUSE_MS + ANSWER_DEADLINE_MS + 1.0);
    assert_eq!(field(&app).area, Area::Unreachable, "premise: the dialog stopped waiting");
    // The owner tries again while the host's thread still holds its answer.
    typed(&mut app, "helpdesk", PAUSE_MS + ANSWER_DEADLINE_MS + 2.0);
    app.directory_tick_at(LATER);
    assert_eq!(texts(&jobs).len(), 1, "no second search while the first may be on its way");
    // The host's thread ends.
    jobs.lock().unwrap()[0].4.send(DirectoryResult::default()).unwrap();
    app.directory_tick_at(LATER + 1.0);
    app.directory_tick_at(LATER + 2.0);
    assert_eq!(texts(&jobs), ["help", "helpdesk"], "then it goes");
}

// ---- CORR-N1: File › Info at narrow widths

#[test]
fn file_infos_directory_links_stay_on_screen_and_beside_their_neighbours_at_narrow_widths() {
    for width in [760.0, 900.0, 1100.0] {
        let (mut app, owner, _, _) = window();
        app.dialog = None;
        app.execute("account.addSharedMailbox", json!({"account": owner, "address": "support@example.org"})).unwrap();
        app.directory_search.perm.insert(owner, Permission::Approval);
        app.execute("ui.backstage", json!({"value": true, "page": "info"})).unwrap();
        let mut h = harness_at(app, width, 900.0);
        h.run_steps(4);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 900.0));
        for label in ["Directory search: needs your administrator's approval", "What to do next", "Copy a message for my administrator", "Ask again"]
        {
            let rects: Vec<egui::Rect> = h.query_all_by_label(label).map(|n| n.rect()).collect();
            assert!(!rects.is_empty(), "{label} at {width}");
            assert!(rects.iter().all(|r| screen.contains_rect(*r)), "{label} on screen at {width}: {rects:?}");
        }
        let line = h.get_by_label("Directory search: needs your administrator's approval").rect();
        let calendars = h.get_by_label_contains("Calendars: ").rect();
        assert!((line.left() - calendars.left()).abs() < 1.0, "the same indent as the shared mailbox's line at {width}: {line:?} {calendars:?}");
        assert!((line.height() - calendars.height()).abs() < 1.0, "the same size at {width}");
        let link = h.get_by_label("Ask again").rect();
        assert!(link.top() >= line.bottom() - 1.0, "the links on a line of their own at {width}");
    }
}

// ---- CORR-N2: the question has a name, and its buttons a focus mark

#[test]
fn the_question_holding_the_keyboard_has_a_name_and_a_role() {
    let (app, owner, jobs, _) = window();
    let h = question_drawn(app, &jobs, owner);
    let node = h.query_by_role_and_label(Role::Group, QUESTION).expect("the question's node, named");
    assert!(node.is_focused(), "and it has the keyboard");
}

// ---- CORR-N4: Enter while the question is up

#[test]
fn enter_on_allow_with_an_address_typed_opens_the_page_and_adds_nothing() {
    let (mut app, owner, jobs, sign_ins) = window();
    let _held = held_graph(&mut app);
    let mut h = question_drawn(app, &jobs, owner);
    let address = address_below(&h, tl!("Or type its address"));
    click(&mut h, address.center());
    ty(&mut h, "support@example.org");
    // Back to the question's buttons with the owner's own keys: Not now, then Allow.
    for _ in 0..2 {
        h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Tab);
        h.run_steps(2);
    }
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(sign_ins.lock().unwrap().len(), 1, "Enter on Allow opens the page");
    assert!(added(h.state()).is_empty(), "and adds nothing");
}

#[test]
fn enter_in_the_address_field_while_the_question_is_up_adds_the_typed_address() {
    let (mut app, owner, jobs, sign_ins) = window();
    let _held = held_graph(&mut app);
    let mut h = question_drawn(app, &jobs, owner);
    let address = address_below(&h, tl!("Or type its address"));
    click(&mut h, address.center());
    ty(&mut h, "support@example.org");
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let added: Vec<String> = added(h.state()).into_iter().map(|(a, _)| a).collect();
    assert_eq!(added, ["support@example.org"], "Enter in the address field adds it");
    assert!(sign_ins.lock().unwrap().is_empty(), "and answers nothing");
}

// ---- CORR-N6: a short window shows more rows

#[test]
fn a_short_window_shows_three_rows_of_fifteen() {
    let (app, _, _, _) = window();
    let mut h = harness_at(app, 760.0, 480.0);
    h.run_steps(3);
    let entries: Vec<Value> = (0..15).map(|i| json!({"name": format!("Team {i:02}"), "address": format!("team{i:02}@example.org")})).collect();
    h.state_mut().execute("ui.searchDirectory", json!({"text": "te", "answer": {"outcome": "found", "more": true, "entries": entries}})).unwrap();
    h.run_steps(5);
    let shown = (0..15u64).filter(|i| h.state().hit.rect("directoryResult", *i).is_some()).count();
    assert!(shown >= 3, "at 760x480, {shown} rows show");
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(760.0, 480.0));
    for label in [tl!("Add Shared Mailbox"), tl!("Search your organisation"), tl!("Add"), tl!("Cancel")] {
        assert!(h.query_all_by_label(label).all(|n| screen.contains_rect(n.rect())), "{label} on screen");
    }
}

#[test]
fn allow_and_not_now_show_where_the_keyboard_is() {
    let (app, owner, jobs, _) = window();
    let mut h = question_drawn(app, &jobs, owner);
    // A line of two pixels or more round the button, close to it.
    let marked = |h: &H, around: egui::Rect| {
        h.output().shapes.iter().any(|c| {
            matches!(&c.shape, egui::Shape::Rect(r) if r.stroke.width >= 2.0 && r.rect.contains_rect(around) && r.rect.width() < around.width() + 10.0)
        })
    };
    let allow = h.state().hit.rect("directoryAllow", 0).expect("Allow");
    let not_now = h.state().hit.rect("directoryNotNow", 0).expect("Not now");
    assert!(!marked(&h, allow) && !marked(&h, not_now), "no mark while the question has the keyboard");
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    assert!(marked(&h, allow) && !marked(&h, not_now), "Allow marked");
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    assert!(marked(&h, not_now) && !marked(&h, allow), "Not now marked");
}
