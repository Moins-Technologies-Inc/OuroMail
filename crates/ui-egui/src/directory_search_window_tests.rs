// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The directory's search, second round: what is let go of when the mailbox, the account or its
//! sign-in changes; the question that keeps the keyboard until it is answered; Allow that can't
//! open the page; the dialog at the smallest window; Enter; File › Info's line; input an agent
//! or a script puts into the window; a host that never answers. Hosts are fakes; every name and
//! address is invented.

use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;

use super::*;

type H = egui_kittest::Harness<'static, MailApp>;

/// A time past the pause after anything the window was typed into just now.
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

/// The owner types `text` (the keys go where the keyboard is).
fn ty(h: &mut H, text: &str) {
    h.event(egui::Event::Text(text.into()));
    h.step();
}

/// Frames with what the control channel queued fed in as the window's own `raw_input_hook` does.
fn driven_steps(h: &mut H, n: usize) {
    for _ in 0..n {
        let mut raw = egui::RawInput::default();
        h.state_mut().raw_input_hook(&mut raw);
        h.input_mut().events.extend(raw.events);
        h.step();
    }
}

/// `n` entries found, with more on the server or not.
fn found(h: &mut H, n: usize, more: bool) {
    let entries: Vec<Value> =
        (0..n).map(|i| json!({"name": format!("Team mailbox {:02}", i + 1), "address": format!("team{:02}@example.org", i + 1)})).collect();
    h.state_mut().execute("ui.searchDirectory", json!({"text": "te", "answer": {"outcome": "found", "more": more, "entries": entries}})).unwrap();
    h.run_steps(4);
}

/// The dialog's address field: the text field right under "Or type its address".
fn address_field(h: &H) -> egui::Rect {
    let label = h.get_by_label(tl!("Or type its address")).rect();
    let below = h.query_all_by_role(Role::TextInput).map(|n| n.rect()).filter(|r| r.top() >= label.bottom() - 1.0);
    below.min_by(|a, b| a.top().total_cmp(&b.top())).expect("the address field")
}

fn error_said(app: &MailApp) -> String {
    match &app.dialog {
        Some(Dialog::SharedMailbox { error, .. }) => error.clone(),
        _ => "<closed>".into(),
    }
}

/// A search answered with a token for the directory: the window holds one for `owner`.
fn given(app: &mut MailApp, jobs: &Jobs, owner: Id, access: &str) {
    let n = jobs.lock().unwrap().len();
    typed(app, &format!("he{n}"), 0.0);
    app.directory_tick_at(PAUSE_MS);
    let mine = Tokens {
        access: access.into(),
        refresh: format!("{access}-ref"),
        email: OWNER.into(),
        expires_at: mailcraft_time::now() + 3_600,
        ..Default::default()
    };
    answer(jobs, n, owner, DirectoryOutcome::Found, &[], Some(mine));
    app.directory_tick_at(PAUSE_MS + 1.0);
    assert!(app.directory_search.access.contains_key(&owner), "premise: a token for the directory is held");
    assert_eq!(app.directory_search.perm.get(&owner), Some(&Permission::Given), "premise: allowed");
}

/// A search on its way for `owner` (its answer not in yet).
fn on_its_way(app: &mut MailApp, jobs: &Jobs) -> usize {
    let n = jobs.lock().unwrap().len();
    typed(app, "help", 10.0);
    app.directory_tick_at(10.0 + PAUSE_MS);
    assert_eq!(jobs.lock().unwrap().len(), n + 1, "premise: a search went");
    n
}

/// Nothing of the directory is held for `account` any more.
fn let_go(app: &MailApp, account: Id, why: &str) {
    assert!(!app.directory_search.access.contains_key(&account), "{why}: the token for the directory is let go of");
    assert_eq!(app.directory_search.perm.get(&account), None, "{why}: whether it was allowed is forgotten");
}

/// The answer to search `n`, with tokens, comes in after the change: none of it is kept.
fn late_answer_dropped(app: &mut MailApp, jobs: &Jobs, n: usize, account: Id, why: &str) {
    let late = Tokens {
        access: "late-access".into(),
        refresh: "late-ref".into(),
        email: OWNER.into(),
        expires_at: mailcraft_time::now() + 3_600,
        ..Default::default()
    };
    answer(jobs, n, account, DirectoryOutcome::Found, &[("Help Desk", "helpdesk@example.org")], Some(late));
    app.directory_tick_at(1.0e8);
    assert!(!app.directory_search.access.values().any(|t| t.access == "late-access"), "{why}: the late token isn't kept");
    assert!(!app.tokens.values().chain(app.graph_tokens.values()).any(|t| t.refresh == "late-ref"), "{why}: nor its refresh token");
    assert_eq!(app.directory_search.perm.get(&account), None, "{why}: nor taken as a yes");
}

// ---- S-F1: what is held for the directory goes when the mailbox, the account or its sign-in does

#[test]
fn another_mailbox_whose_account_has_the_same_id_never_gets_the_old_directory_token() {
    let (mut app, owner, jobs, _) = window();
    given(&mut app, &jobs, owner, "zqxtokenA");
    app.dialog = None;
    app.directory_tick_at(PAUSE_MS + 2.0);
    app.session.replace_mailbox(Session::empty().mailbox);
    app.drop_secrets_if_replaced();
    let cy = app
        .execute("account.add", json!({"email": "cy@other.example", "kind": "microsoft", "auth": "oauth2", "directory": "organisation"}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    assert_eq!(cy, owner, "premise: the same id in the other mailbox");
    assert_eq!(app.directory_search.perm.get(&cy), None, "File › Info says nothing was allowed for an account never asked");
    app.tokens.insert(cy, tokens("cy-access", "cy-ref"));
    app.execute("ui.dialog", json!({"name": "sharedMailbox", "account": cy})).unwrap();
    typed(&mut app, "zqx", 0.0);
    app.directory_tick_at(1.0e9);
    let jobs = jobs.lock().unwrap();
    let last = jobs.last().unwrap();
    assert_eq!(last.0, "zqx", "premise: the other mailbox's search went");
    assert_ne!(last.2, "zqxtokenA", "the other mailbox's account searched with the old mailbox's directory token");
    assert!(last.2.is_empty(), "it gets a token of its own");
}

#[test]
fn a_mailbox_opened_while_a_search_is_on_its_way_keeps_none_of_its_answer() {
    let (mut app, owner, jobs, _) = window();
    let n = on_its_way(&mut app, &jobs);
    app.dialog = None;
    app.session.replace_mailbox(Session::empty().mailbox);
    app.drop_secrets_if_replaced();
    late_answer_dropped(&mut app, &jobs, n, owner, "another mailbox");
}

#[test]
fn removing_the_account_lets_go_of_what_is_held_for_its_directory() {
    let (mut app, owner, jobs, _) = window();
    given(&mut app, &jobs, owner, "tok-removed");
    let n = on_its_way(&mut app, &jobs);
    app.dialog = None;
    app.execute("account.remove", json!({"id": owner})).unwrap();
    let_go(&app, owner, "account removed");
    late_answer_dropped(&mut app, &jobs, n, owner, "account removed");
}

#[test]
fn a_changed_sign_in_lets_go_of_the_token_and_drops_the_answer_on_its_way() {
    let (mut app, owner, jobs, _) = window();
    given(&mut app, &jobs, owner, "tok-tenant-a");
    let n = on_its_way(&mut app, &jobs);
    app.execute("account.edit", json!({"id": owner, "tenant": "contoso.example"})).unwrap();
    let_go(&app, owner, "tenant changed");
    late_answer_dropped(&mut app, &jobs, n, owner, "tenant changed");
}

#[test]
fn undo_of_a_sign_in_change_lets_go_of_the_token_given_since() {
    let (mut app, owner, jobs, _) = window();
    app.execute("account.edit", json!({"id": owner, "tenant": "contoso.example"})).unwrap();
    // (The edit let go of the sign-in's tokens: signed in again.)
    app.tokens.insert(owner, tokens("mail-access-b", "ref-b"));
    given(&mut app, &jobs, owner, "tok-tenant-b");
    app.execute("edit.undo", json!({})).unwrap();
    assert!(app.session.mailbox.account(owner).is_some_and(|a| a.oauth.as_ref().is_none_or(|o| o.tenant != "contoso.example")), "premise: undone");
    let_go(&app, owner, "undo");
}

struct NoWipe;

impl crate::data_removal::DataWipe for NoWipe {
    fn erase(&self, _: Option<&std::path::Path>) -> crate::data_removal::WipeReport {
        crate::data_removal::WipeReport::default()
    }
}

#[test]
fn removing_mailcrafts_data_lets_go_of_what_is_held_for_the_directory() {
    let (mut app, owner, jobs, _) = window();
    given(&mut app, &jobs, owner, "tok-wiped");
    let n = on_its_way(&mut app, &jobs);
    app.dialog = None;
    app.services.data_wipe = Some(Box::new(NoWipe));
    app.execute("ui.removeAllData", json!({})).unwrap();
    let_go(&app, owner, "data removed");
    late_answer_dropped(&mut app, &jobs, n, owner, "data removed");
}

// ---- C-F1: the question keeps the keyboard until it is answered

#[test]
fn typing_on_while_the_question_is_up_neither_answers_it_nor_is_searched() {
    let (app, owner, jobs, sign_ins) = window();
    let mut h = harness(app);
    h.run_steps(3);
    ty(&mut h, "h");
    ty(&mut h, "e");
    ty(&mut h, "l");
    h.state_mut().directory_tick_at(soon());
    assert_eq!(texts(&jobs), ["hel"], "premise: three keys, one search");
    answer(&jobs, 0, owner, DirectoryOutcome::ConsentNeeded, &[], None);
    h.run_steps(3);
    assert_eq!(field(h.state()).area, Area::Offer, "premise: the question is up");
    ty(&mut h, "p");
    ty(&mut h, "x");
    h.key_press(egui::Key::Space);
    h.run_steps(2);
    h.state_mut().directory_tick_at(soon());
    h.run_steps(2);
    assert_eq!(field(h.state()).area, Area::Offer, "the question is still up");
    assert_eq!(field(h.state()).text, "hel", "the field keeps what was typed before the question; keys typed while it is up go nowhere");
    assert_eq!(h.state().directory_search.perm.get(&owner), None, "nothing was recorded");
    assert_eq!(texts(&jobs).len(), 1, "nothing more was sent");
    assert!(sign_ins.lock().unwrap().is_empty(), "no page opened");
}

/// The question up after three keys: the owner's own Tab reaches Allow, whose Enter opens the
/// page (the question has the keyboard); a click into the search field lets no key in.
fn question_after_typing() -> (H, Id, Jobs, SignIns) {
    let (app, owner, jobs, sign_ins) = window();
    let mut h = harness(app);
    h.run_steps(3);
    ty(&mut h, "hel");
    h.state_mut().directory_tick_at(soon());
    answer(&jobs, 0, owner, DirectoryOutcome::ConsentNeeded, &[], None);
    h.run_steps(3);
    assert_eq!(field(h.state()).area, Area::Offer, "premise: the question is up");
    (h, owner, jobs, sign_ins)
}

#[test]
fn the_question_has_the_keyboard_and_tab_reaches_allow() {
    let (mut h, _, _, sign_ins) = question_after_typing();
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(sign_ins.lock().unwrap().len(), 1, "Tab from the question is Allow, and Enter on it opens the page");
    assert_eq!(field(h.state()).area, Area::Asking);
}

#[test]
fn while_the_question_is_up_a_click_into_the_search_field_lets_no_key_in() {
    let (mut h, owner, jobs, sign_ins) = question_after_typing();
    let label = h.get_by_label(tl!("Search your organisation")).rect();
    let search = h
        .query_all_by_role(Role::TextInput)
        .map(|n| n.rect())
        .filter(|r| r.top() >= label.bottom() - 1.0)
        .min_by(|a, b| a.top().total_cmp(&b.top()))
        .expect("the search field");
    click(&mut h, search.center());
    ty(&mut h, "p");
    h.run_steps(2);
    h.state_mut().directory_tick_at(soon());
    assert_eq!(field(h.state()).text, "hel", "the field takes no keys while the question is up");
    assert_eq!((field(h.state()).area, h.state().directory_search.perm.get(&owner)), (Area::Offer, None));
    assert!(texts(&jobs).len() == 1 && sign_ins.lock().unwrap().is_empty());
}

#[test]
fn typing_after_ask_again_neither_answers_the_question_nor_is_searched() {
    let (mut app, owner, jobs, sign_ins) = window();
    app.dialog = None;
    app.directory_search.perm.insert(owner, Permission::Declined);
    let mut h = harness(app);
    h.run_steps(3);
    assert!(h.state_mut().directory_ask_again(owner));
    h.run_steps(3);
    assert_eq!(field(h.state()).area, Area::Offer, "premise: Ask again puts the question");
    ty(&mut h, "h");
    ty(&mut h, "e");
    h.run_steps(2);
    h.state_mut().directory_tick_at(soon());
    h.run_steps(2);
    assert_eq!(field(h.state()).area, Area::Offer, "the question is still up");
    assert_eq!(h.state().directory_search.perm.get(&owner), None, "nothing was recorded");
    assert!(texts(&jobs).is_empty() && sign_ins.lock().unwrap().is_empty(), "nothing sent, no page");
}

// ---- C-F2: Allow that can't open Microsoft's page says why, and is no no

#[test]
fn allow_that_cannot_open_the_page_says_why_in_the_dialog_and_records_no_no() {
    for case in ["no sign-in here", "offline"] {
        let (mut app, owner, _, sign_ins) = window();
        app.execute("ui.searchDirectory", json!({"text": "help", "answer": {"outcome": "consentNeeded"}})).unwrap();
        let why = if case == "offline" {
            app.execute("sync.offline", json!({"value": true})).unwrap();
            crate::send_receive::said()
        } else {
            app.services.sign_in = None;
            tl!("This build can't sign in to this kind of account. Use the desktop app.").to_string()
        };
        let mut h = harness(app);
        h.run_steps(3);
        let allow = h.state().hit.rect("directoryAllow", 0).expect("Allow is drawn");
        click(&mut h, allow.center());
        h.run_steps(2);
        assert!(sign_ins.lock().unwrap().is_empty(), "{case}: premise: no page");
        assert_eq!(field(h.state()).area, Area::Offer, "{case}: the question stays");
        assert_eq!(h.state().directory_search.perm.get(&owner), None, "{case}: no no recorded");
        assert!(h.query_by_label(&why).is_some(), "{case}: the reason is said in the dialog: {why}");
    }
}

// ---- C-F3: the dialog fits every window the app allows

#[test]
fn the_dialog_with_fifteen_results_keeps_its_title_search_field_and_buttons_on_screen() {
    for (width, height) in [(760.0, 480.0), (1366.0 / 1.25, 768.0 / 1.25), (1280.0, 800.0)] {
        let (app, _, _, _) = window();
        let mut h = harness_at(app, width, height);
        h.run_steps(3);
        found(&mut h, 15, true);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height));
        for label in [tl!("Add Shared Mailbox"), tl!("Search your organisation"), tl!("Add"), tl!("Cancel")] {
            let rects: Vec<egui::Rect> = h.query_all_by_label(label).map(|n| n.rect()).collect();
            assert!(!rects.is_empty(), "{label} is drawn at {width}x{height}");
            assert!(rects.iter().all(|r| screen.contains_rect(*r)), "{label} at {rects:?} is on screen at {width}x{height}");
        }
        let inputs: Vec<egui::Rect> = h.query_all_by_role(Role::TextInput).map(|n| n.rect()).collect();
        assert!(inputs.len() >= 2, "the search field and the address field (and the window's own)");
        assert!(inputs.iter().all(|r| screen.contains_rect(*r)), "every field on screen at {width}x{height}: {inputs:?}");
        assert!(screen.contains_rect(address_field(&h)), "the address field on screen at {width}x{height}");
        assert!(h.state().hit.rect("directoryResult", 0).is_some(), "at least a row of the list shows at {width}x{height}");
    }
}

// ---- C-F4: Enter

#[test]
fn enter_adds_the_typed_address_once_the_owner_is_in_the_address_field() {
    let (mut app, _, _, _) = window();
    let _held = held_graph(&mut app);
    let mut h = harness(app);
    h.run_steps(3);
    found_two(&mut h);
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(1);
    assert_eq!(field(h.state()).highlight, Some(0), "premise: a row is highlighted");
    let address = address_field(&h);
    click(&mut h, address.center());
    ty(&mut h, "support@example.org");
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let added: Vec<String> = added(h.state()).into_iter().map(|(a, _)| a).collect();
    assert_eq!(added, ["support@example.org"], "Enter in the address field adds what is typed there");
}

#[test]
fn enter_with_nothing_chosen_and_no_address_does_nothing() {
    let (mut app, _, _, _) = window();
    let _held = held_graph(&mut app);
    let mut h = harness(app);
    h.run_steps(3);
    found_two(&mut h);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(added(h.state()).is_empty(), "nothing added");
    assert_eq!(error_said(h.state()), "", "and nothing said in red");
}

// ---- S-F5: a row out of view has no click target

#[test]
fn rows_scrolled_out_of_view_have_no_click_target() {
    let (app, _, _, _) = window();
    let mut h = harness(app);
    h.run_steps(3);
    found(&mut h, 15, false);
    assert!(h.state().hit.rect("directoryResult", 0).is_some(), "the first row is a target");
    assert!(h.state().hit.rect("directoryResult", 14).is_none(), "the last, out of view, is none");
}

// ---- S-F7: a picked entry is named as a typed address is

#[test]
fn a_picked_entry_is_named_as_the_same_address_typed_is() {
    let typed_name = {
        let (mut app, owner, _, _) = window();
        let id = app.execute("account.addSharedMailbox", json!({"account": owner, "address": "zero@example.org"})).unwrap()["id"].as_u64().unwrap();
        app.session.mailbox.account(id).map(|a| a.display_name.clone()).unwrap()
    };
    let (mut app, _, _, _) = window();
    let _held = held_graph(&mut app);
    let mut h = harness(app);
    h.run_steps(3);
    let entries = json!([{"name": "Ze\u{200d}r\u{fe0f}o\u{e000}", "address": "zero@example.org"}]);
    h.state_mut().execute("ui.searchDirectory", json!({"text": "ze", "answer": {"outcome": "found", "entries": entries}})).unwrap();
    h.run_steps(3);
    let row = h.state().hit.rect("directoryResult", 0).expect("the row is drawn");
    click(&mut h, row.center());
    assert_eq!(added(h.state()), [("zero@example.org".to_string(), typed_name)]);
}

// ---- S-F8 / C-F6: Not now and Ask again are the owner's own

#[test]
fn not_now_refuses_a_click_an_agent_or_a_script_puts_into_the_window() {
    let (mut app, owner, _, _) = window();
    app.execute("ui.searchDirectory", json!({"text": "help", "answer": {"outcome": "consentNeeded"}})).unwrap();
    let mut h = harness(app);
    h.run_steps(3);
    as_agent(h.state_mut(), "ui.click", json!({"kind": "directoryNotNow", "id": 0}));
    driven_steps(&mut h, 6);
    assert_eq!(field(h.state()).area, Area::Offer, "the question still stands");
    assert_eq!(h.state().directory_search.perm.get(&owner), None, "not answered by an agent");
    let not_now = h.state().hit.rect("directoryNotNow", 0).expect("Not now is drawn");
    click(&mut h, not_now.center());
    assert_eq!(h.state().directory_search.perm.get(&owner), Some(&Permission::Declined), "control: the owner's click");
}

#[test]
fn ask_again_refuses_a_click_an_agent_or_a_script_puts_into_the_window() {
    let (mut app, owner, _, _) = window();
    app.dialog = None;
    app.directory_search.perm.insert(owner, Permission::Declined);
    app.execute("ui.backstage", json!({"value": true, "page": "info"})).unwrap();
    let mut h = harness(app);
    h.run_steps(4);
    assert!(h.state().hit.rect("directoryAskAgain", owner).is_some(), "premise: Ask again is drawn");
    as_agent(h.state_mut(), "ui.click", json!({"kind": "directoryAskAgain", "id": owner}));
    driven_steps(&mut h, 6);
    assert!(h.state().dialog.is_none(), "an agent's click opens nothing");
    assert_eq!(h.state().directory_search.perm.get(&owner), Some(&Permission::Declined), "and forgets nothing");
    let link = h.state().hit.rect("directoryAskAgain", owner).expect("Ask again");
    click(&mut h, link.center());
    assert!(matches!(h.state().dialog, Some(Dialog::SharedMailbox { .. })), "control: the owner's click");
}

// ---- C-F8: a host that never answers

#[test]
fn a_host_thread_that_goes_without_answering_ends_the_search_with_try_again() {
    let (mut app, _, jobs, _) = window();
    typed(&mut app, "help", 0.0);
    app.directory_tick_at(PAUSE_MS);
    assert_eq!(texts(&jobs).len(), 1);
    jobs.lock().unwrap().clear();
    app.directory_tick_at(PAUSE_MS + 1.0);
    assert_eq!(field(&app).area, Area::Unreachable, "Try again is offered");
    assert!(!app.directory_search.busy());
}

#[test]
fn a_search_that_stays_unanswered_ends_after_the_deadline_and_is_called_off() {
    let (mut app, _, jobs, _) = window();
    typed(&mut app, "help", 0.0);
    app.directory_tick_at(PAUSE_MS);
    app.directory_tick_at(PAUSE_MS + ANSWER_DEADLINE_MS - 1.0);
    assert_eq!(field(&app).area, Area::Searching, "premise: still waiting before the deadline");
    app.directory_tick_at(PAUSE_MS + ANSWER_DEADLINE_MS + 1.0);
    assert_eq!(field(&app).area, Area::Unreachable, "Try again is offered");
    assert!(!app.directory_search.busy(), "a new search may go");
    assert!(jobs.lock().unwrap()[0].3.load(Ordering::Relaxed), "the host is told to stop");
}

// ---- C-F9, C-F10

#[test]
fn a_row_the_keyboard_is_on_is_the_one_marked() {
    let (app, _, _, _) = window();
    let mut h = harness(app);
    h.run_steps(3);
    found_two(&mut h);
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    assert_eq!(field(h.state()).highlight, Some(0), "Tab from the search field marks the first row");
}

#[test]
fn ask_again_says_why_when_it_can_do_nothing() {
    let (mut app, owner, _, _) = window();
    app.directory_search.perm.insert(owner, Permission::Declined);
    assert!(!app.directory_ask_again(owner), "premise: a dialog is open");
    let said = app.status_msg.as_ref().map(|s| s.0.clone()).unwrap_or_default();
    assert_eq!(said, tl!("Close the open dialog first, then press Ask again."));
}

// ---- C-F5 / S-F4: File › Info's line

#[test]
fn file_info_says_where_the_permission_stands_in_words_of_its_own() {
    let cases = [
        (None, "Directory search: not asked in this session", vec![]),
        (Some(Permission::Given), "Directory search: allowed", vec![]),
        (Some(Permission::Asking), "Directory search: asking on Microsoft's page…", vec![]),
        (Some(Permission::Declined), "Directory search: not now (for this session)", vec!["Ask again"]),
        (
            Some(Permission::Approval),
            "Directory search: needs your administrator's approval",
            vec!["What to do next", "Copy a message for my administrator", "Ask again"],
        ),
        (Some(Permission::Organisation), "Directory search: your organisation doesn't allow it", vec!["What to do next", "Ask again"]),
        (Some(Permission::Unanswered), "Directory search: not answered in this session", vec!["Ask again"]),
    ];
    for (perm, said, links) in cases {
        let (mut app, owner, _, _) = window();
        app.dialog = None;
        match perm {
            // (Asking: Microsoft's page is open for it.)
            Some(Permission::Asking) => {
                app.begin_directory_sign_in(owner).unwrap();
            }
            Some(perm) => {
                app.directory_search.perm.insert(owner, perm);
            }
            None => {}
        }
        app.execute("ui.backstage", json!({"value": true, "page": "info"})).unwrap();
        let mut h = harness(app);
        h.run_steps(4);
        assert!(h.query_by_label(said).is_some(), "{perm:?}: {said}");
        for link in links {
            assert!(h.query_by_label(link).is_some(), "{perm:?}: {link}");
        }
    }
}

// ---- C-F11: the blank under the last row

#[test]
fn the_blank_under_the_last_row_is_a_trackpad_held_past_the_end_and_goes_when_it_lets_go() {
    let (app, _, _, _) = window();
    let mut h = harness(app);
    h.run_steps(3);
    found(&mut h, 15, true);
    let over = h.state().hit.rect("directoryResult", 0).expect("the list").center();
    let note = |h: &H| h.get_by_label_contains("Keep typing").rect().top();
    let last = |h: &H| h.state().hit.rect("directoryResult", 14).map(|r| r.bottom());
    h.event(egui::Event::PointerMoved(over));
    h.step();
    let wheel = |phase| egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -2_000.0),
        phase,
        source: egui::MouseWheelSource::Trackpad,
        modifiers: egui::Modifiers::NONE,
    };
    // As the reviewer's screenshot: fingers still on the trackpad, past the end.
    h.event(wheel(egui::TouchPhase::Move));
    h.run_steps(10);
    let held = note(&h) - last(&h).expect("the last row shows");
    assert!(held > 40.0, "held past the end, a blank shows under the last row: {held}");
    // They let go: the list springs back to its end.
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::Vec2::ZERO,
        phase: egui::TouchPhase::End,
        source: egui::MouseWheelSource::Trackpad,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(120);
    let after = note(&h) - last(&h).expect("the last row shows");
    assert!(after < 20.0, "let go, the last row sits at the end of the list: {after}");
}
