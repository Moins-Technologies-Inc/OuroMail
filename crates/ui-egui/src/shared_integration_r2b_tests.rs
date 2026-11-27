// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Graph work under way when the data is removed, a mailbox goes or a sign-in changes; one
//! owner's calendar work let go of without another's; a refused directory sign-in's tokens; a
//! refusal nobody saw; and each guard reached by itself. Fake in-memory services and invented
//! `.example` addresses only.

use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use mailcraft_model::Id;
use mailcraft_model::directory::DirectoryOutcome;
use serde_json::json;

use super::integration_r2_tests::{Asked, add_owner_b, capture_graph, seed_inbox, shared_mail_token};
use super::integration_tests::{
    DirJobs, FakeWipe, OWNER_A, OWNER_B, SHARED_A, answer_dir, fresh_tokens, launch_pending_dir, seed_directory_token, seed_shared_cal, setup_window,
    silence_owner, start_inflight_shared_cal_job,
};
use super::*;
use crate::{CalendarCheckRequest, CalendarCheckResult, GraphResult, MailApp};

type Checks = Arc<Mutex<Vec<Sender<CalendarCheckResult>>>>;

/// A host that keeps every check of the calendar connection waiting.
fn hold_checks(app: &mut MailApp) -> Checks {
    let checks: Checks = Default::default();
    let log = checks.clone();
    app.services.check_calendar = Some(Box::new(move |_req: CalendarCheckRequest| {
        let (tx, rx) = mpsc::channel();
        log.lock().unwrap().push(tx);
        rx
    }));
    checks
}

fn check_calendar_of(app: &mut MailApp, owner: Id) {
    app.execute("ui.checkCalendar", json!({"account": owner, "confirmed": true})).unwrap();
    assert!(app.checking_calendar(), "premise: the calendar connection is being checked");
}

/// A calendar job of `owner`'s own, kept waiting.
fn start_owner_calendar_job(app: &mut MailApp, owner: Id, offset_ms: f64) -> Asked {
    let asked = capture_graph(app);
    app.calendar_list_due(owner);
    assert!(app.calendar_tick_at(true, crate::now_ms() + offset_ms), "premise: a calendar job started");
    assert_eq!(asked.lock().unwrap().last().map(|ask| ask.account), Some(owner), "premise: it is this owner's");
    assert!(app.calendar_syncing(), "premise: the calendar job is under way");
    asked
}

/// A host for mail connections, which Send/Receive wants there before it starts anything.
fn mail_host(app: &mut MailApp) {
    app.services.sync = Some(Box::new(|req: mailcraft_model::sync::SyncRequest| {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(mailcraft_model::sync::SyncResult { account: req.account.id, ..Default::default() });
        rx
    }));
}

fn late(account: Id) -> GraphResult {
    GraphResult { account, tokens: Some(fresh_tokens("late-access", "late-ref", OWNER_A, "")), consent_needed: true, ..Default::default() }
}

fn kept_late(app: &MailApp) -> bool {
    app.tokens.values().chain(app.graph_tokens.values()).any(|t| t.access.contains("late") || t.refresh.contains("late"))
}

/// SEC-NB1: the Graph run under way is let go of when the data is removed, and its answer,
/// should it still be looked at, stores nothing and opens no sign-in page.
#[test]
fn a_graph_answer_that_comes_after_the_data_was_removed_is_nobodys() {
    for (whose, by_command) in [("owner", true), ("owner", false), ("shared", true), ("shared", false)] {
        let (mut app, owner, shared, _, sign_ins) = setup_window();
        mail_host(&mut app);
        let asked = capture_graph(&mut app);
        let account = if whose == "owner" { owner } else { shared };
        app.execute("ui.sync", json!({"account": account})).unwrap();
        assert!(app.graph_rx.is_some(), "premise ({whose}): a Graph run is under way");
        assert_eq!(asked.lock().unwrap().len(), 1, "premise ({whose}): one job");
        if by_command {
            app.services.data_wipe = Some(Box::new(FakeWipe));
            app.execute("ui.removeAllData", json!({})).unwrap();
        } else {
            // (The removal's own clean-up gone round: the answer's path refuses by itself.)
            app.data_removed = true;
            app.tokens.clear();
            app.graph_tokens.clear();
        }
        let _ = asked.lock().unwrap()[0].tx.send(late(account));
        app.poll_graph();
        assert!(!kept_late(&app), "{whose}, by the command: {by_command}: a token that came after the data was removed is kept");
        assert!(app.tokens.is_empty() && app.graph_tokens.is_empty(), "{whose}, by the command: {by_command}: nothing is kept");
        assert!(!app.signing_in() && sign_ins.lock().unwrap().is_empty(), "{whose}, by the command: {by_command}: a sign-in page was opened");
        if by_command {
            assert!(app.graph_rx.is_none(), "{whose}: the Graph run under way is let go of");
        }
    }
}

/// SEC-NB1: `ui.removeAllData` lets go of the Graph run at once, before any answer.
#[test]
fn removing_the_data_lets_go_of_the_graph_run_under_way() {
    let (mut app, _, shared, _, _) = setup_window();
    mail_host(&mut app);
    let _asked = capture_graph(&mut app);
    app.execute("ui.sync", json!({"account": shared})).unwrap();
    assert!(app.graph_rx.is_some(), "premise: a Graph run is under way");
    app.services.data_wipe = Some(Box::new(FakeWipe));
    app.execute("ui.removeAllData", json!({})).unwrap();
    assert!(app.graph_rx.is_none(), "the Graph run under way is let go of");
}

/// SEC-NB1: a shared mailbox removed while its run is under way gets no token from that run.
#[test]
fn a_shared_mailbox_removed_while_its_run_is_under_way_gets_no_token_from_it() {
    let (mut app, _, shared, _, sign_ins) = setup_window();
    mail_host(&mut app);
    let asked = capture_graph(&mut app);
    app.execute("ui.sync", json!({"account": shared})).unwrap();
    assert!(app.graph_rx.is_some(), "premise: the shared mailbox's run is under way");
    app.execute("account.removeSharedMailbox", json!({"id": shared})).unwrap();
    assert!(app.session.mailbox.account(shared).is_none(), "premise: the shared mailbox is gone");
    let _ = asked.lock().unwrap()[0].tx.send(late(shared));
    app.poll_graph();
    assert!(!app.graph_tokens.contains_key(&shared), "a token is kept for a mailbox that is gone");
    assert!(!kept_late(&app), "nothing the run brought is kept");
    assert!(!app.signing_in() && sign_ins.lock().unwrap().is_empty(), "no sign-in page for it");
}

/// SEC-NB1 and SEC-B2: a run or an Inbox check under way when the owner's sign-in changes was
/// started with the sign-in before: what it brings is nobody's.
#[test]
fn a_run_under_way_when_its_owners_sign_in_changes_brings_no_token() {
    for whose in ["owner", "shared", "check"] {
        let (mut app, owner, shared, _, sign_ins) = setup_window();
        mail_host(&mut app);
        seed_inbox(&mut app, shared, SHARED_A);
        let asked = capture_graph(&mut app);
        let account = if whose == "owner" { owner } else { shared };
        if whose == "check" {
            assert!(app.start_inbox_pass(shared, true, true).is_some(), "premise: the Inbox check starts");
        } else {
            app.execute("ui.sync", json!({"account": account})).unwrap();
        }
        assert!(app.graph_rx.is_some(), "premise ({whose}): a Graph job is under way");
        app.execute("account.edit", json!({"id": owner, "tenant": "tenant-beta"})).unwrap();
        let _ = asked.lock().unwrap()[0].tx.send(late(account));
        app.poll_graph();
        assert!(!kept_late(&app), "{whose}: a token had with the sign-in before is kept");
        assert!(!app.graph_tokens.contains_key(&shared), "{whose}: nothing is kept for the shared mailbox");
        assert!(!app.signing_in() && sign_ins.lock().unwrap().is_empty(), "{whose}: no sign-in page for a job of the sign-in before");
    }
}

/// A check that gave way to a run keeps the sign-in it renewed only while it is the same one.
#[test]
fn a_check_that_gave_way_keeps_nothing_once_its_owners_sign_in_changed() {
    let (mut app, owner, shared, _, _) = setup_window();
    seed_inbox(&mut app, shared, SHARED_A);
    let asked = capture_graph(&mut app);
    assert!(app.start_inbox_pass(shared, true, true).is_some(), "premise: the Inbox check starts");
    app.give_up_inbox_check();
    app.execute("account.edit", json!({"id": owner, "tenant": "tenant-beta"})).unwrap();
    let _ = asked.lock().unwrap()[0].tx.send(late(shared));
    app.inbox_check_tick();
    assert!(!kept_late(&app), "a token had with the sign-in before is kept");
}

/// SEC-NB1: the end of a shared mailbox's run and a check that gave way, each looked at by
/// itself once the data is removed, keep nothing and open no page.
#[test]
fn nothing_is_kept_for_a_shared_mailbox_once_the_data_is_removed() {
    let mut wrong = Vec::new();
    let (mut app, _, shared, _, sign_ins) = setup_window();
    seed_inbox(&mut app, shared, SHARED_A);
    let asked = capture_graph(&mut app);
    assert!(app.start_inbox_pass(shared, true, true).is_some(), "premise: the Inbox check starts");
    app.give_up_inbox_check();
    // (The removal's own clean-up gone round.)
    app.data_removed = true;
    app.tokens.clear();
    app.graph_tokens.clear();
    app.finish_shared_mail(shared, false, late(shared));
    if kept_late(&app) || !app.graph_tokens.is_empty() {
        wrong.push("the end of a shared mailbox's run kept a token");
    }
    if app.signing_in() || !sign_ins.lock().unwrap().is_empty() {
        wrong.push("the end of a shared mailbox's run opened a sign-in page");
    }
    app.graph_tokens.clear();
    let _ = asked.lock().unwrap()[0].tx.send(late(shared));
    app.inbox_check_tick();
    if kept_late(&app) || !app.graph_tokens.is_empty() {
        wrong.push("a check that gave way kept a token");
    }
    assert!(wrong.is_empty(), "once the data is removed: {wrong:#?}");
}

/// SEC-NB2 and CORR F2: letting go of one owner's calendar work lets go of that owner's job or
/// check, and of nobody else's.
#[test]
fn letting_go_of_one_owners_calendar_work_leaves_another_owners_alone() {
    // A's check and B's job: A's directory sign-in is refused.
    let (mut app, owner_a, _, jobs, _) = setup_window();
    let (owner_b, _) = add_owner_b(&mut app);
    let _checks = hold_checks(&mut app);
    check_calendar_of(&mut app, owner_a);
    let _asked = start_owner_calendar_job(&mut app, owner_b, 1_000_000.0);
    refuse_directory_sign_in(&mut app, owner_a, &jobs);
    assert!(app.calendar_syncing(), "A refused: B's calendar job goes on");
    assert!(!app.checking_calendar(), "A refused: A's check is let go of");

    // A's job and B's check: A's mail sign-in is refused.
    let (mut app, owner_a, shared_a, _, _) = setup_window();
    let (owner_b, _) = add_owner_b(&mut app);
    silence_owner(&mut app, owner_b);
    let _checks = hold_checks(&mut app);
    check_calendar_of(&mut app, owner_b);
    let _held = start_inflight_shared_cal_job(&mut app, shared_a, SHARED_A, 1_000_000.0);
    app.graph_mail_needs_sign_in(owner_a, true, false, false);
    assert!(!app.calendar_syncing(), "A refused: A's calendar job is let go of");
    assert!(app.checking_calendar(), "A refused: B's check goes on");

    // B's job alone: A's directory sign-in is refused.
    let (mut app, owner_a, _, jobs, _) = setup_window();
    let (owner_b, _) = add_owner_b(&mut app);
    let _asked = start_owner_calendar_job(&mut app, owner_b, 1_000_000.0);
    refuse_directory_sign_in(&mut app, owner_a, &jobs);
    assert!(app.calendar_syncing(), "A's directory sign-in refused: B's calendar job goes on");
}

fn refuse_directory_sign_in(app: &mut MailApp, owner: Id, jobs: &DirJobs) {
    let idx = launch_pending_dir(app, owner, jobs, "payroll", 50_000.0);
    answer_dir(jobs, idx, owner, DirectoryOutcome::SignInRefused, None);
    app.directory_tick_at(50_000.0 + PAUSE_MS + 1.0);
}

/// SEC-NB3: a search whose sign-in was refused keeps none of the tokens that came with it, and
/// records no permission as given.
#[test]
fn a_directory_search_whose_sign_in_was_refused_keeps_none_of_the_tokens_it_came_with() {
    let (mut app, owner, _, jobs, _) = setup_window();
    let idx = launch_pending_dir(&mut app, owner, &jobs, "payroll", 50_000.0);
    answer_dir(&jobs, idx, owner, DirectoryOutcome::SignInRefused, Some(fresh_tokens("refused-access", "refused-ref", OWNER_A, "")));
    app.directory_tick_at(50_000.0 + PAUSE_MS + 1.0);
    assert!(!app.tokens.values().chain(app.graph_tokens.values()).any(|t| t.refresh == "refused-ref"), "the refused sign-in's refresh token is kept");
    assert!(!app.directory_search.access.contains_key(&owner), "the refused sign-in's access token is kept");
    assert_ne!(app.held_perm(owner), Some(Permission::Given), "a refused sign-in is recorded as a permission given");
}

/// SEC-NB4: a refusal nobody saw leaves the owner's own sign-in for the Send/Receive that asks,
/// and lets go of what was had with it for shared mailboxes; asked for, the refused tokens go
/// and the page opens.
#[test]
fn a_quiet_refusal_keeps_the_owners_own_sign_in_and_drops_what_it_had_for_shared_mailboxes() {
    let (mut app, owner, shared, _, sign_ins) = setup_window();
    shared_mail_token(&mut app, shared, "a-shared-mail", "owner-ref-1", OWNER_A);
    seed_shared_cal(&mut app, shared, "a-shared-cal");
    app.graph_mail_needs_sign_in(owner, true, false, false);
    assert!(!app.graph_tokens.contains_key(&shared), "quiet refusal: the shared mailbox's mail token went");
    assert!(!app.shared_calendar_tokens.contains_key(&shared), "quiet refusal: the shared mailbox's calendar token went");
    assert!(app.tokens.contains_key(&owner) && app.graph_tokens.contains_key(&owner), "quiet refusal: the owner's own sign-in stays");
    assert!(!app.signing_in() && sign_ins.lock().unwrap().is_empty(), "quiet refusal: no page");
    app.graph_mail_needs_sign_in(owner, false, false, false);
    assert!(!app.tokens.contains_key(&owner) && !app.graph_tokens.contains_key(&owner), "asked for: the refused tokens went");
    assert_eq!(sign_ins.lock().unwrap().len(), 1, "asked for: the page opens");
}

/// CORR F1: one owner's refusal lets go of that owner's directory token and search alone.
#[test]
fn a_refusal_for_one_owner_leaves_another_owners_directory_token_and_search_alone() {
    let (mut app, owner_a, _, jobs, _) = setup_window();
    let (owner_b, _) = add_owner_b(&mut app);
    let idx = launch_pending_dir(&mut app, owner_b, &jobs, "help", 10_000.0);
    answer_dir(&jobs, idx, owner_b, DirectoryOutcome::Found, Some(fresh_tokens("dir-b", "owner-b-ref", OWNER_B, "")));
    app.directory_tick_at(10_000.0 + PAUSE_MS + 1.0);
    assert!(app.directory_search.access.contains_key(&owner_b), "premise: B's directory token is kept");
    let pending = launch_pending_dir(&mut app, owner_b, &jobs, "sales", 50_000.0);

    app.graph_mail_needs_sign_in(owner_a, true, false, false);
    app.sign_in_came_to_nothing(owner_a, "sign in cancelled", false);

    assert_eq!(app.directory_search.access.get(&owner_b).map(|t| t.access.as_str()), Some("dir-b"), "A refused: B's directory token stays");
    assert!(app.directory_search.access_stamps.contains_key(&owner_b), "A refused: and what it was had for");
    assert!(!jobs.lock().unwrap()[pending].3.load(Ordering::Relaxed), "A refused: B's search is not called off");
    answer_dir(&jobs, pending, owner_b, DirectoryOutcome::Found, Some(fresh_tokens("dir-b-2", "owner-b-ref", OWNER_B, "")));
    app.directory_tick_at(50_000.0 + PAUSE_MS + 1.0);
    assert_eq!(app.directory_search.access.get(&owner_b).map(|t| t.access.as_str()), Some("dir-b-2"), "A refused: B's answer is still taken");
}

/// SEC-NB5 (M8): Undo and Redo of a sign-in change let go of that account's check of the calendar
/// connection, which nothing else looks at again.
#[test]
fn undo_and_redo_of_a_sign_in_change_let_go_of_the_accounts_calendar_check() {
    let (mut app, owner, _, _, _) = setup_window();
    let _checks = hold_checks(&mut app);
    app.execute("account.edit", json!({"id": owner, "tenant": "tenant-beta"})).unwrap();
    app.tokens.insert(owner, fresh_tokens("mail-2", "ref-2", OWNER_A, ""));
    check_calendar_of(&mut app, owner);
    app.execute("edit.undo", json!({})).unwrap();
    assert!(!app.checking_calendar(), "Undo: the check started with the sign-in before is let go of");
    app.tokens.insert(owner, fresh_tokens("mail-3", "ref-3", OWNER_A, ""));
    check_calendar_of(&mut app, owner);
    app.execute("edit.redo", json!({})).unwrap();
    assert!(!app.checking_calendar(), "Redo: the check started with the sign-in before is let go of");
    app.tokens.insert(owner, fresh_tokens("mail-4", "ref-4", OWNER_A, ""));
    check_calendar_of(&mut app, owner);
    app.execute("account.edit", json!({"id": owner, "tenant": "tenant-gamma"})).unwrap();
    assert!(!app.checking_calendar(), "the edit: the check started with the sign-in before is let go of");
}

/// SEC-NB5 (M12): a shared mailbox being added that isn't kept lets go of its calendar token and
/// job by itself, when the mailbox is gone already and no command's clean-up runs for it.
#[test]
fn a_shared_mailbox_not_kept_lets_go_of_its_calendar_token_and_job_when_it_is_gone_already() {
    let (mut app, _, shared, _, _) = setup_window();
    seed_shared_cal(&mut app, shared, "cal-tok");
    shared_mail_token(&mut app, shared, "a-shared-mail", "owner-ref-1", OWNER_A);
    let _held = start_inflight_shared_cal_job(&mut app, shared, SHARED_A, 1_000_000.0);
    app.session.run("account.removeSharedMailbox", &json!({"id": shared})).unwrap();
    assert!(app.calendar_syncing() && app.shared_calendar_tokens.contains_key(&shared), "premise: removed underneath, nothing cleaned up");
    app.drop_shared_check(shared);
    assert!(!app.shared_calendar_tokens.contains_key(&shared), "its calendar token went");
    assert!(!app.graph_tokens.contains_key(&shared), "its mail token went");
    assert!(!app.calendar_syncing(), "its calendar job is let go of");
}

/// SEC-NB5 (M13): the directory's token is never used once the data is removed, even when the
/// removal's own clean-up was gone round.
#[test]
fn the_directory_token_is_not_used_once_the_data_is_removed() {
    let (mut app, owner, _, jobs, _) = setup_window();
    seed_directory_token(&mut app, owner, &jobs, "dir-tok");
    assert_eq!(app.held_access(owner, mailcraft_time::now()), "dir-tok", "premise: held and good");
    app.data_removed = true;
    assert_eq!(app.held_access(owner, mailcraft_time::now()), "", "not used once the data is removed");
}

/// SEC-NB5 (M15): a directory answer that comes once the data is removed lets go of everything
/// held for the directory, even when it brings no token and the removal's clean-up was gone round.
#[test]
fn a_directory_answer_after_the_data_was_removed_lets_go_of_what_was_held() {
    let (mut app, owner, _, jobs, _) = setup_window();
    seed_directory_token(&mut app, owner, &jobs, "dir-tok");
    let idx = launch_pending_dir(&mut app, owner, &jobs, "sales", 50_000.0);
    assert_eq!(app.held_perm(owner), Some(Permission::Given), "premise: the permission is recorded");
    app.data_removed = true;
    answer_dir(&jobs, idx, owner, DirectoryOutcome::Found, None);
    app.directory_tick_at(50_000.0 + PAUSE_MS + 1.0);
    assert!(app.directory_search.perm.is_empty() && app.directory_search.access.is_empty(), "what was held for the directory went");
}
