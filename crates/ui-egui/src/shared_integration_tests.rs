// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Integration tests between shared-mailbox directory search / Opened before and shared-calendar
//! token lifecycle cleanup, identity binding, sign-in refusal symmetry, and post-wipe guards.
//! Runs strictly against fake in-memory services and synthetic `.example` addresses.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use mailcraft_engine::{Module, Session};
use mailcraft_model::directory::{DirectoryAnswer, DirectoryOutcome};
use mailcraft_model::sync::Tokens;
use mailcraft_model::{Id, Mailbox};
use serde_json::json;

use super::*;
use crate::data_removal::{DataWipe, WipeReport};
use crate::dialogs::Dialog;
use crate::{GraphRequest, GraphResult, MailApp, Services, SignInRequest};

const NOW: i64 = 1_791_993_600;
pub(super) const OWNER_A: &str = "alice@alpha.example";
pub(super) const OWNER_B: &str = "bob@beta.example";
pub(super) const SHARED_A: &str = "team-a@alpha.example";
pub(super) const SHARED_A2: &str = "help-a@alpha.example";
pub(super) const SHARED_B: &str = "ops@beta.example";

pub(super) type DirJobs = Arc<Mutex<Vec<(String, u64, String, Arc<AtomicBool>, Sender<DirectoryResult>)>>>;
pub(super) type SignIns = Arc<Mutex<Vec<(bool, bool, Arc<AtomicBool>, Sender<Result<Tokens, String>>)>>>;
pub(super) type HeldGraph = Arc<Mutex<Vec<Sender<GraphResult>>>>;

pub(super) struct FakeWipe;

impl DataWipe for FakeWipe {
    fn erase(&self, _mailbox: Option<&std::path::Path>) -> WipeReport {
        WipeReport::default()
    }
}

pub(super) fn fresh_tokens(access: &str, refresh: &str, email: &str, tenant: &str) -> Tokens {
    Tokens {
        access: access.into(),
        refresh: refresh.into(),
        email: email.into(),
        tenant: tenant.into(),
        expires_at: mailcraft_time::now() + 3_600,
        ..Default::default()
    }
}

pub(super) fn calendar_of(app: &MailApp, account: Id, name: &str) -> Id {
    app.session
        .mailbox
        .calendars
        .iter()
        .find(|c| c.origin.account == account && c.name == name)
        .map(|c| c.id)
        .unwrap_or_else(|| panic!("missing calendar {name} for account {account}"))
}

pub(super) fn silence_owner(app: &mut MailApp, owner: Id) {
    app.session.driver.calendar.checked.insert(owner, f64::MAX);
    app.session.driver.calendar.book.started(owner, f64::MAX);
}

pub(super) fn setup_window() -> (MailApp, Id, Id, DirJobs, SignIns) {
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    let owner = app
        .execute(
            "account.add",
            json!({
                "email": OWNER_A,
                "kind": "microsoft",
                "auth": "oauth2",
                "provider": "microsoft",
                "tenant": "tenant-alpha",
                "directory": "organisation"
            }),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    app.tokens.insert(owner, fresh_tokens("owner-mail", "owner-ref-1", OWNER_A, "tenant-alpha"));
    app.graph_tokens.insert(owner, fresh_tokens("owner-graph", "owner-ref-1", OWNER_A, "tenant-alpha"));
    let shared = app.execute("account.addSharedMailbox", json!({"account": owner, "address": SHARED_A})).unwrap()["id"].as_u64().unwrap();

    let dir_jobs: DirJobs = Default::default();
    let dir_log = dir_jobs.clone();
    app.services.graph_directory = Some(Box::new(move |job: DirectoryJob| {
        let (tx, rx) = mpsc::channel();
        dir_log.lock().unwrap().push((job.query.text.clone(), job.query.ticket, job.access.clone(), job.cancel.clone(), tx));
        rx
    }));

    let sign_ins: SignIns = Default::default();
    let sign_log = sign_ins.clone();
    app.services.sign_in = Some(Box::new(move |req: SignInRequest| {
        let (tx, rx) = mpsc::channel();
        sign_log.lock().unwrap().push((req.graph, req.directory, req.cancel.clone(), tx));
        rx
    }));

    app.services.graph = Some(Box::new(move |req: GraphRequest| {
        let (tx, rx) = mpsc::channel();
        tx.send(GraphResult { account: req.account.id, ..Default::default() }).unwrap();
        rx
    }));

    app.session.view.module = Module::Calendar;
    if app.calendar_tick_at(true, crate::now_ms()) {
        app.poll_calendar();
    }
    silence_owner(&mut app, owner);
    (app, owner, shared, dir_jobs, sign_ins)
}

pub(super) fn open_dir_dialog_and_type(app: &mut MailApp, owner: Id, text: &str, at: f64) {
    if !matches!(&app.dialog, Some(Dialog::SharedMailbox { account, .. }) if *account == owner) {
        app.execute("ui.dialog", json!({"name": "sharedMailbox", "account": owner})).unwrap();
    }
    let mut dialog = app.dialog.take();
    if let Some(Dialog::SharedMailbox { account, search, .. }) = &mut dialog {
        search.text = text.into();
        app.directory_typed(*account, search, at);
    }
    app.dialog = dialog;
}

pub(super) fn answer_dir(jobs: &DirJobs, idx: usize, account: Id, outcome: DirectoryOutcome, tokens: Option<Tokens>) {
    let jobs = jobs.lock().unwrap();
    let (_, ticket, _, _, tx) = &jobs[idx];
    let answer = DirectoryAnswer { account, ticket: *ticket, outcome, ..Default::default() };
    tx.send(DirectoryResult { answer, tokens }).unwrap();
}

pub(super) fn seed_directory_token(app: &mut MailApp, owner: Id, jobs: &DirJobs, access: &str) -> usize {
    let idx = jobs.lock().unwrap().len();
    let t0 = (idx as f64) * 10_000.0;
    open_dir_dialog_and_type(app, owner, "help", t0);
    app.directory_tick_at(t0 + PAUSE_MS);
    assert_eq!(jobs.lock().unwrap().len(), idx + 1, "premise: directory search launched");
    answer_dir(jobs, idx, owner, DirectoryOutcome::Found, Some(fresh_tokens(access, "owner-ref-dir", OWNER_A, "tenant-alpha")));
    app.directory_tick_at(t0 + PAUSE_MS + 1.0);
    assert!(app.directory_search.access.contains_key(&owner), "premise: directory token cached");
    idx
}

pub(super) fn launch_pending_dir(app: &mut MailApp, owner: Id, jobs: &DirJobs, text: &str, at: f64) -> usize {
    let idx = jobs.lock().unwrap().len();
    open_dir_dialog_and_type(app, owner, text, at);
    app.directory_tick_at(at + PAUSE_MS);
    assert_eq!(jobs.lock().unwrap().len(), idx + 1, "premise: pending directory search launched");
    assert!(app.directory_search.busy(), "premise: directory search in flight");
    idx
}

pub(super) fn seed_shared_cal(app: &mut MailApp, shared: Id, access: &str) {
    app.seed_shared_calendar_token(shared, fresh_tokens(access, "owner-ref-cal", OWNER_A, "tenant-alpha"));
    assert!(app.shared_calendar_tokens.contains_key(&shared), "premise: shared calendar token cached");
}

pub(super) fn install_held_graph(app: &mut MailApp) -> HeldGraph {
    let held: HeldGraph = Default::default();
    let held_clone = held.clone();
    app.services.graph = Some(Box::new(move |_req: GraphRequest| {
        let (tx, rx) = mpsc::channel();
        held_clone.lock().unwrap().push(tx);
        rx
    }));
    held
}

pub(super) fn start_inflight_shared_cal_job(app: &mut MailApp, shared: Id, shared_email: &str, offset_ms: f64) -> HeldGraph {
    let held = install_held_graph(app);
    let cal_id = calendar_of(app, shared, shared_email);
    if app.session.mailbox.calendars.iter().find(|c| c.id == cal_id).is_some_and(|c| !c.visible) {
        app.execute("calendar.showCalendar", json!({"id": cal_id})).unwrap();
    }
    app.calendar_list_due(shared);
    assert!(app.calendar_tick_at(true, crate::now_ms() + offset_ms), "premise: shared calendar job started");
    assert!(app.calendar_syncing(), "premise: shared calendar job in flight");
    assert_eq!(held.lock().unwrap().len(), 1, "premise: graph request captured");
    held
}

#[test]
fn quiet_and_after_sign_in_refusal_drop_directory_token_and_cancel_pending_search() {
    let (mut app, owner, shared, jobs, _) = setup_window();
    seed_directory_token(&mut app, owner, &jobs, "dir-tok-quiet");
    seed_shared_cal(&mut app, shared, "cal-tok-quiet");
    let pending_idx = launch_pending_dir(&mut app, owner, &jobs, "sales", 50_000.0);

    // Quiet sign-in refusal (!consent): must clear BOTH shared_calendar_tokens and directory token/pending.
    app.graph_mail_needs_sign_in(owner, true, false, false);
    assert!(!app.shared_calendar_tokens.contains_key(&shared), "shared calendar token cleared on quiet refusal");
    assert!(!app.directory_search.access.contains_key(&owner), "directory access token must be cleared on quiet sign-in refusal");
    assert!(!app.directory_search.access_stamps.contains_key(&owner), "directory access stamp must be cleared on quiet sign-in refusal");
    assert!(jobs.lock().unwrap()[pending_idx].3.load(Ordering::Relaxed), "in-flight directory search must be cancelled on quiet refusal");
    answer_dir(&jobs, pending_idx, owner, DirectoryOutcome::Found, Some(fresh_tokens("dir-tok-late", "ref-late", OWNER_A, "tenant-alpha")));
    app.directory_tick_at(50_000.0 + PAUSE_MS + 1.0);
    assert!(!app.directory_search.access.contains_key(&owner), "late directory answer after quiet refusal must not repopulate token");

    // after_sign_in = true, !consent: must also clear directory token and cancel pending search.
    seed_directory_token(&mut app, owner, &jobs, "dir-tok-after");
    let pending_idx2 = launch_pending_dir(&mut app, owner, &jobs, "support", 90_000.0);
    app.graph_mail_needs_sign_in(owner, false, true, false);
    assert!(!app.directory_search.access.contains_key(&owner), "directory access token must be cleared on after_sign_in refusal");
    assert!(jobs.lock().unwrap()[pending_idx2].3.load(Ordering::Relaxed), "in-flight directory search must be cancelled on after_sign_in refusal");
    answer_dir(&jobs, pending_idx2, owner, DirectoryOutcome::Found, Some(fresh_tokens("dir-tok-late2", "ref-late2", OWNER_A, "tenant-alpha")));
    app.directory_tick_at(90_000.0 + PAUSE_MS + 1.0);
    assert!(!app.directory_search.access.contains_key(&owner), "late directory answer after after_sign_in refusal must not repopulate token");

    // Control: consent == true (missing scope only, not refused sign-in) must preserve valid directory token.
    seed_directory_token(&mut app, owner, &jobs, "dir-tok-consent-control");
    app.graph_mail_needs_sign_in(owner, true, false, true);
    assert!(app.directory_search.access.contains_key(&owner), "consent-only notice must not evict valid directory token");
}

#[test]
fn sign_in_came_to_nothing_drops_directory_token_and_cancels_pending_search() {
    let (mut app, owner, shared, jobs, _) = setup_window();
    seed_directory_token(&mut app, owner, &jobs, "dir-tok-nothing");
    seed_shared_cal(&mut app, shared, "cal-tok-nothing");
    let pending_idx = launch_pending_dir(&mut app, owner, &jobs, "billing", 50_000.0);

    app.sign_in_came_to_nothing(owner, "sign in cancelled", true);

    assert!(!app.shared_calendar_tokens.contains_key(&shared), "shared calendar token cleared when sign-in came to nothing");
    assert!(!app.directory_search.access.contains_key(&owner), "directory token must be cleared when sign-in came to nothing");
    assert!(!app.directory_search.access_stamps.contains_key(&owner), "directory access stamp must be cleared when sign-in came to nothing");
    assert!(jobs.lock().unwrap()[pending_idx].3.load(Ordering::Relaxed), "in-flight directory search must be cancelled when sign-in came to nothing");
    answer_dir(&jobs, pending_idx, owner, DirectoryOutcome::Found, Some(fresh_tokens("dir-tok-late", "ref-late", OWNER_A, "tenant-alpha")));
    app.directory_tick_at(50_000.0 + PAUSE_MS + 1.0);
    assert!(!app.directory_search.access.contains_key(&owner), "late directory answer after sign_in_came_to_nothing must be dropped");
}

#[test]
fn remove_all_data_blocks_directory_search_sign_in_and_late_completion() {
    let (mut app, owner, _shared, jobs, sign_ins) = setup_window();
    seed_directory_token(&mut app, owner, &jobs, "dir-tok-pre-wipe");
    // Start a directory OAuth sign-in before Remove All Data.
    app.execute("ui.allowDirectory", json!({"account": owner, "confirmed": true})).unwrap();
    assert_eq!(sign_ins.lock().unwrap().len(), 1, "premise: directory sign-in started");
    assert!(app.signing_in(), "premise: signing_in in flight");

    app.services.data_wipe = Some(Box::new(FakeWipe));
    app.execute("ui.removeAllData", json!({})).unwrap();
    assert!(app.data_removed, "premise: data_removed set");

    // Late directory sign-in completion after wipe must be dropped completely.
    let (_, _, cancel, tx) = sign_ins.lock().unwrap().remove(0);
    assert!(cancel.load(Ordering::Relaxed), "the sign-in waiting in the browser is called off when the data is removed");
    let _ = tx.send(Ok(fresh_tokens("dir-tok-post-wipe", "ref-post-wipe", OWNER_A, "tenant-alpha")));
    app.poll_sign_in();
    assert!(app.directory_search.access.is_empty(), "late directory sign-in must not store access token after data_removed");
    assert!(app.directory_search.perm.is_empty(), "late directory sign-in must not store permission after data_removed");

    // Direct directory_signed_in and keep_directory_tokens after wipe must store nothing.
    app.directory_signed_in(owner, Ok(fresh_tokens("dir-tok-direct", "ref-direct", OWNER_A, "tenant-alpha")));
    assert!(app.directory_search.access.is_empty(), "directory_signed_in must be blocked after data_removed");
    assert!(app.directory_search.perm.is_empty(), "directory_signed_in must not record perm after data_removed");
    app.keep_directory_tokens(owner, fresh_tokens("dir-tok-keep", "ref-keep", OWNER_A, "tenant-alpha"));
    assert!(app.directory_search.access.is_empty(), "keep_directory_tokens must be blocked after data_removed");

    // New ui.allowDirectory after wipe must be refused without starting a sign-in.
    let err = app.execute("ui.allowDirectory", json!({"account": owner, "confirmed": true})).unwrap_err();
    assert_eq!(err, MailApp::data_removed_sync_refused(), "ui.allowDirectory must refuse with data_removed_sync_refused");
    assert!(sign_ins.lock().unwrap().is_empty(), "no browser sign-in launched after data_removed");

    // New directory search after wipe must not launch any host job even if tokens were injected.
    let before_jobs = jobs.lock().unwrap().len();
    app.tokens.insert(owner, fresh_tokens("injected-mail", "injected-ref", OWNER_A, "tenant-alpha"));
    open_dir_dialog_and_type(&mut app, owner, "finance", 80_000.0);
    app.directory_tick_at(80_000.0 + PAUSE_MS);
    assert_eq!(jobs.lock().unwrap().len(), before_jobs, "no directory search may launch after data_removed");
}

#[test]
fn directory_sign_in_refused_clears_shared_calendar_tokens_and_cancels_calendar_jobs() {
    let (mut app, owner_a, shared_a, jobs, _) = setup_window();
    // Add an unrelated owner B + shared mailbox B to prove isolation.
    let owner_b = app
        .execute(
            "account.add",
            json!({
                "email": OWNER_B,
                "kind": "microsoft",
                "auth": "oauth2",
                "provider": "microsoft",
                "tenant": "tenant-beta",
                "directory": "organisation"
            }),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    app.tokens.insert(owner_b, fresh_tokens("owner-b-mail", "owner-b-ref", OWNER_B, "tenant-beta"));
    let shared_b = app.execute("account.addSharedMailbox", json!({"account": owner_b, "address": SHARED_B})).unwrap()["id"].as_u64().unwrap();
    silence_owner(&mut app, owner_b);

    seed_directory_token(&mut app, owner_a, &jobs, "dir-tok-a");
    seed_shared_cal(&mut app, shared_a, "cal-tok-a");
    app.seed_shared_calendar_token(shared_b, fresh_tokens("cal-tok-b", "owner-b-ref", OWNER_B, "tenant-beta"));
    let _held = start_inflight_shared_cal_job(&mut app, shared_a, SHARED_A, 1_000_000.0);

    // Owner A's directory search comes back with SignInRefused.
    let idx = launch_pending_dir(&mut app, owner_a, &jobs, "payroll", 50_000.0);
    answer_dir(&jobs, idx, owner_a, DirectoryOutcome::SignInRefused, None);
    app.directory_tick_at(50_000.0 + PAUSE_MS + 1.0);

    assert!(!app.directory_search.access.contains_key(&owner_a), "directory token cleared on SignInRefused");
    assert!(!app.shared_calendar_tokens.contains_key(&shared_a), "owner A's shared calendar token must be cleared on directory SignInRefused");
    assert!(app.shared_calendar_access(shared_a).is_empty(), "owner A's shared_calendar_access must be empty after directory SignInRefused");
    assert!(!app.calendar_syncing(), "owner A's in-flight shared calendar job must be cancelled on directory SignInRefused");
    assert_eq!(app.shared_calendar_access(shared_b), "cal-tok-b", "unrelated owner B's shared calendar token must remain intact");
}

#[test]
fn shared_mailbox_remember_remove_readd_check_drop_and_forget_before_keep_calendar_tokens_clean() {
    let (mut app, owner, shared_a, _, _) = setup_window();
    let shared_a2 = app.execute("account.addSharedMailbox", json!({"account": owner, "address": SHARED_A2})).unwrap()["id"].as_u64().unwrap();
    seed_shared_cal(&mut app, shared_a, "cal-tok-a1");
    seed_shared_cal(&mut app, shared_a2, "cal-tok-a2");
    let _held = start_inflight_shared_cal_job(&mut app, shared_a, SHARED_A, 1_000_000.0);

    // 1. Remove by address with default remember=true: clears calendar token & job, remembers in shared_before.
    app.execute("account.removeSharedMailbox", json!({"account": owner, "address": SHARED_A})).unwrap();
    assert!(!app.shared_calendar_tokens.contains_key(&shared_a), "removed shared mailbox token cleared");
    assert!(!app.calendar_syncing(), "removed shared mailbox calendar job cancelled");
    assert_eq!(app.session.mailbox.account(owner).unwrap().shared_before.iter().map(|e| e.address.as_str()).collect::<Vec<_>>(), vec![SHARED_A]);
    assert_eq!(app.shared_calendar_access(shared_a2), "cal-tok-a2", "live sibling shared mailbox token preserved");

    // 2. Re-add from Opened before (note_from_before), seed token + job, then drop_shared_check:
    //    must clear calendar token & job AND re-remember in shared_before.
    let readded = app.execute("account.addSharedMailbox", json!({"account": owner, "address": SHARED_A})).unwrap()["id"].as_u64().unwrap();
    app.note_from_before(readded, true);
    seed_shared_cal(&mut app, readded, "cal-tok-readded");
    let _held2 = start_inflight_shared_cal_job(&mut app, readded, SHARED_A, 2_000_000.0);
    app.drop_shared_check(readded);
    assert!(!app.shared_calendar_tokens.contains_key(&readded), "drop_shared_check cleared shared calendar token");
    assert!(!app.calendar_syncing(), "drop_shared_check cancelled in-flight calendar job");
    assert_eq!(
        app.session.mailbox.account(owner).unwrap().shared_before.iter().map(|e| e.address.as_str()).collect::<Vec<_>>(),
        vec![SHARED_A],
        "failed re-add from Opened before stayed remembered"
    );

    // 3. Remove with remember=false: clears token, does NOT add to shared_before.
    let temp = app.execute("account.addSharedMailbox", json!({"account": owner, "address": "temp@alpha.example"})).unwrap()["id"].as_u64().unwrap();
    seed_shared_cal(&mut app, temp, "cal-tok-temp");
    app.execute("account.removeSharedMailbox", json!({"id": temp, "remember": false})).unwrap();
    assert!(!app.shared_calendar_tokens.contains_key(&temp), "remember=false removal cleared shared calendar token");
    assert_eq!(app.session.mailbox.account(owner).unwrap().shared_before.iter().map(|e| e.address.as_str()).collect::<Vec<_>>(), vec![SHARED_A]);

    // 4. Add a second remembered entry, then test Forget (single) and Forget all:
    //    neither disrupts live shared_a2's valid calendar token.
    let extra = app.execute("account.addSharedMailbox", json!({"account": owner, "address": "extra@alpha.example"})).unwrap()["id"].as_u64().unwrap();
    seed_shared_cal(&mut app, extra, "cal-tok-extra");
    app.execute("account.removeSharedMailbox", json!({"id": extra})).unwrap();
    assert!(!app.shared_calendar_tokens.contains_key(&extra));

    app.execute("account.forgetSharedBefore", json!({"account": owner, "address": SHARED_A})).unwrap();
    assert_eq!(
        app.session.mailbox.account(owner).unwrap().shared_before.iter().map(|e| e.address.as_str()).collect::<Vec<_>>(),
        vec!["extra@alpha.example"]
    );
    assert_eq!(app.shared_calendar_access(shared_a2), "cal-tok-a2", "single forgetSharedBefore preserved live shared mailbox token");

    app.execute("account.forgetSharedBefore", json!({"account": owner})).unwrap();
    assert!(app.session.mailbox.account(owner).unwrap().shared_before.is_empty(), "Forget all emptied shared_before");
    assert_eq!(app.shared_calendar_access(shared_a2), "cal-tok-a2", "Forget all preserved live shared mailbox token");
}

#[test]
fn combined_lifecycle_clears_both_shared_calendar_and_directory_state() {
    let (mut app, owner, shared, jobs, _) = setup_window();

    // 1. account.edit (tenant change) clears both shared_calendar_tokens + job AND directory state.
    seed_directory_token(&mut app, owner, &jobs, "dir-tok-edit");
    seed_shared_cal(&mut app, shared, "cal-tok-edit");
    let _held = start_inflight_shared_cal_job(&mut app, shared, SHARED_A, 1_000_000.0);
    let pending_edit = launch_pending_dir(&mut app, owner, &jobs, "team", 50_000.0);
    app.execute("account.edit", json!({"id": owner, "tenant": "tenant-beta"})).unwrap();
    assert!(!app.shared_calendar_tokens.contains_key(&shared), "account.edit cleared shared calendar token");
    assert!(!app.calendar_syncing(), "account.edit cancelled calendar job");
    assert!(!app.directory_search.access.contains_key(&owner), "account.edit cleared directory token");
    assert!(!app.directory_search.perm.contains_key(&owner), "account.edit cleared directory permission");
    assert!(jobs.lock().unwrap()[pending_edit].3.load(Ordering::Relaxed), "account.edit cancelled pending directory search");
    answer_dir(&jobs, pending_edit, owner, DirectoryOutcome::Found, Some(fresh_tokens("dir-tok-edit-late", "ref-late", OWNER_A, "tenant-alpha")));
    app.directory_tick_at(50_000.0 + PAUSE_MS + 1.0);
    assert!(!app.directory_search.access.contains_key(&owner), "late directory answer after account.edit must be dropped");

    // 2. edit.undo clears both after re-seeding under tenant-beta.
    app.tokens.insert(owner, fresh_tokens("owner-mail-b", "owner-ref-b", OWNER_A, "tenant-beta"));
    seed_directory_token(&mut app, owner, &jobs, "dir-tok-undo");
    seed_shared_cal(&mut app, shared, "cal-tok-undo");
    let _held2 = start_inflight_shared_cal_job(&mut app, shared, SHARED_A, 2_000_000.0);
    app.execute("edit.undo", json!({})).unwrap();
    assert!(!app.shared_calendar_tokens.contains_key(&shared), "edit.undo cleared shared calendar token");
    assert!(!app.calendar_syncing(), "edit.undo cancelled calendar job");
    assert!(!app.directory_search.access.contains_key(&owner), "edit.undo cleared directory token");
    assert!(!app.directory_search.perm.contains_key(&owner), "edit.undo cleared directory permission");

    // 3. edit.redo clears both after re-seeding under tenant-alpha.
    app.tokens.insert(owner, fresh_tokens("owner-mail-a", "owner-ref-a", OWNER_A, "tenant-alpha"));
    seed_directory_token(&mut app, owner, &jobs, "dir-tok-redo");
    seed_shared_cal(&mut app, shared, "cal-tok-redo");
    let _held3 = start_inflight_shared_cal_job(&mut app, shared, SHARED_A, 3_000_000.0);
    app.execute("edit.redo", json!({})).unwrap();
    assert!(!app.shared_calendar_tokens.contains_key(&shared), "edit.redo cleared shared calendar token");
    assert!(!app.calendar_syncing(), "edit.redo cancelled calendar job");
    assert!(!app.directory_search.access.contains_key(&owner), "edit.redo cleared directory token");
    assert!(!app.directory_search.perm.contains_key(&owner), "edit.redo cleared directory permission");

    // 4. Mailbox swap (file.new -> drop_secrets_if_replaced) clears both.
    app.tokens.insert(owner, fresh_tokens("owner-mail-c", "owner-ref-c", OWNER_A, "tenant-beta"));
    seed_directory_token(&mut app, owner, &jobs, "dir-tok-swap");
    seed_shared_cal(&mut app, shared, "cal-tok-swap");
    let _held4 = start_inflight_shared_cal_job(&mut app, shared, SHARED_A, 4_000_000.0);
    app.execute("file.new", json!({})).unwrap();
    assert!(app.shared_calendar_tokens.is_empty(), "file.new cleared shared_calendar_tokens");
    assert!(!app.calendar_syncing(), "file.new cancelled calendar jobs");
    assert!(app.directory_search.access.is_empty(), "file.new cleared directory tokens");
    assert!(app.directory_search.perm.is_empty(), "file.new cleared directory permissions");
}
