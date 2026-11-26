// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Regression tests for `MailApp::shared_calendar_tokens` lifecycle cleanup, use-time identity
//! stamping, in-flight job cancellation/completion binding, and post-wipe (`data_removed`) guards.
//! Runs strictly against fake in-memory services and synthetic `.example` addresses.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use mailcraft_engine::{Module, Session};
use mailcraft_host::secrets::{SecretError, SecretStore};
use mailcraft_model::remote::{CalendarsDownload, RemoteCalendar};
use mailcraft_model::sync::Tokens;
use mailcraft_model::{CalendarOwner, Id, Mailbox, Rights};
use serde_json::json;

use crate::data_removal::{DataWipe, Why, WipeReport};
use crate::{GraphRequest, GraphResult, MailApp, Services, SignInRequest};

const NOW: i64 = 1_791_993_600;
const OWNER_A: &str = "alice@alpha.example";
const OWNER_B: &str = "bob@beta.example";
const SHARED_A: &str = "team-a@alpha.example";
const SHARED_B: &str = "ops@beta.example";
const CANARY_A: &str = "canary-alpha-shared-cal-access";
const CANARY_A2: &str = "canary-alpha-shared-cal-access-v2";
const CANARY_B: &str = "canary-beta-shared-cal-access";
const CANARY_LATE: &str = "canary-late-inflight-shared-cal";

#[derive(Clone, Debug)]
struct CapturedGraph {
    account_id: Id,
    account_email: String,
    signer_id: Option<Id>,
    signer_email: String,
    signer_tenant: String,
    refresh: String,
    access: String,
}

type CapturedLog = Arc<Mutex<Vec<CapturedGraph>>>;
type AnswerQueue = Arc<Mutex<VecDeque<GraphResult>>>;
type HeldSenders = Arc<Mutex<Vec<Sender<GraphResult>>>>;

fn fresh_tokens(access: &str, refresh: &str, email: &str, tenant: &str) -> Tokens {
    Tokens {
        access: access.into(),
        refresh: refresh.into(),
        email: email.into(),
        tenant: tenant.into(),
        expires_at: mailcraft_time::now() + 3_600,
        ..Default::default()
    }
}

fn expired_tokens(access: &str, refresh: &str, email: &str) -> Tokens {
    Tokens { access: access.into(), refresh: refresh.into(), email: email.into(), expires_at: mailcraft_time::now() - 60, ..Default::default() }
}

fn listing(shared: Id, address: &str) -> CalendarsDownload {
    let owner = Some(CalendarOwner { address: address.into(), name: "Shared".into() });
    CalendarsDownload {
        account: shared,
        email: address.into(),
        complete: true,
        calendars: vec![RemoteCalendar {
            id: "sh-main".into(),
            name: "Calendar".into(),
            main: true,
            rights: Rights::Read,
            owner,
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn calendar_of(app: &MailApp, account: Id, name: &str) -> Id {
    app.session
        .mailbox
        .calendars
        .iter()
        .find(|c| c.origin.account == account && c.name == name)
        .map(|c| c.id)
        .unwrap_or_else(|| panic!("missing calendar {name} for account {account}"))
}

fn make_graph_service(answers: AnswerQueue, log: CapturedLog) -> Box<dyn Fn(GraphRequest) -> mpsc::Receiver<GraphResult>> {
    Box::new(move |req: GraphRequest| {
        let signer_id = req.signer.as_ref().map(|s| s.id);
        let signer_email = req.signer.as_ref().map(|s| s.email.clone()).unwrap_or_default();
        let signer_tenant = req.signer.as_ref().and_then(|s| s.oauth.as_ref()).map(|o| o.tenant.clone()).unwrap_or_default();
        log.lock().unwrap().push(CapturedGraph {
            account_id: req.account.id,
            account_email: req.account.email.clone(),
            signer_id,
            signer_email,
            signer_tenant,
            refresh: req.refresh.clone(),
            access: req.access.clone(),
        });
        let (tx, rx) = mpsc::channel();
        let res = answers.lock().unwrap().pop_front().unwrap_or_else(|| GraphResult { account: req.account.id, ..Default::default() });
        tx.send(res).unwrap();
        rx
    })
}

fn make_held_graph_service(log: CapturedLog, held: HeldSenders) -> Box<dyn Fn(GraphRequest) -> mpsc::Receiver<GraphResult>> {
    Box::new(move |req: GraphRequest| {
        let signer_id = req.signer.as_ref().map(|s| s.id);
        let signer_email = req.signer.as_ref().map(|s| s.email.clone()).unwrap_or_default();
        let signer_tenant = req.signer.as_ref().and_then(|s| s.oauth.as_ref()).map(|o| o.tenant.clone()).unwrap_or_default();
        log.lock().unwrap().push(CapturedGraph {
            account_id: req.account.id,
            account_email: req.account.email.clone(),
            signer_id,
            signer_email,
            signer_tenant,
            refresh: req.refresh.clone(),
            access: req.access.clone(),
        });
        let (tx, rx) = mpsc::channel();
        held.lock().unwrap().push(tx);
        rx
    })
}

fn silence_owner(app: &mut MailApp, owner: Id) {
    app.session.driver.calendar.checked.insert(owner, f64::MAX);
    app.session.driver.calendar.book.started(owner, f64::MAX);
}

fn make_window() -> (MailApp, Id, Id, AnswerQueue, CapturedLog) {
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    let owner = app
        .execute("account.add", json!({"email": OWNER_A, "kind": "microsoft", "auth": "oauth2", "provider": "microsoft", "tenant": "tenant-alpha"}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    app.tokens.insert(owner, fresh_tokens("canary-owner-a-mail", "canary-owner-a-ref-1", OWNER_A, "tenant-alpha"));
    app.graph_tokens.insert(owner, fresh_tokens("canary-owner-a-graph", "canary-owner-a-ref-1", OWNER_A, "tenant-alpha"));

    let shared = app.execute("account.addSharedMailbox", json!({"account": owner, "address": SHARED_A})).unwrap()["id"].as_u64().unwrap();
    app.session.view.module = Module::Calendar;

    let answers: AnswerQueue = Default::default();
    let log: CapturedLog = Default::default();
    app.services.graph = Some(make_graph_service(answers.clone(), log.clone()));
    app.services.sign_in = Some(Box::new(|_req: SignInRequest| {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(Err("cancelled".into()));
        rx
    }));

    if app.calendar_tick_at(true, crate::now_ms()) {
        app.poll_calendar();
    }
    silence_owner(&mut app, owner);
    log.lock().unwrap().clear();
    (app, owner, shared, answers, log)
}

fn acquire_shared_token(
    app: &mut MailApp,
    shared: Id,
    shared_email: &str,
    answers: &AnswerQueue,
    log: &CapturedLog,
    access: &str,
    refresh: &str,
    owner_email: &str,
    tenant: &str,
) {
    let cal_id = calendar_of(app, shared, shared_email);
    app.execute("calendar.showCalendar", json!({"id": cal_id})).unwrap();
    answers.lock().unwrap().push_back(GraphResult {
        account: shared,
        tokens: Some(fresh_tokens(access, refresh, owner_email, tenant)),
        calendars: Some(listing(shared, shared_email)),
        ..Default::default()
    });
    assert!(app.calendar_tick_at(true, crate::now_ms()), "initial shared calendar job must start");
    app.poll_calendar();
    assert!(app.shared_calendar_tokens.contains_key(&shared), "real poll_calendar must populate shared_calendar_tokens");
    log.lock().unwrap().clear();
}

struct FakeWipe {
    report: WipeReport,
}

impl DataWipe for FakeWipe {
    fn erase(&self, _mailbox: Option<&std::path::Path>) -> WipeReport {
        self.report.clone()
    }
}

#[derive(Default)]
struct SpySecretStore {
    writes: Mutex<Vec<(String, String)>>,
    values: Mutex<HashMap<String, String>>,
}

impl SecretStore for SpySecretStore {
    fn get(&self, key: &str) -> Option<String> {
        self.values.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        self.writes.lock().unwrap().push((key.to_string(), secret.to_string()));
        self.values.lock().unwrap().insert(key.to_string(), secret.to_string());
        Ok(())
    }
    fn delete(&self, key: &str) {
        self.values.lock().unwrap().remove(key);
    }
}

#[test]
fn control_valid_same_account_cache_hit() {
    let (mut app, owner, shared, answers, log) = make_window();
    acquire_shared_token(&mut app, shared, SHARED_A, &answers, &log, CANARY_A, "canary-ref-2", OWNER_A, "tenant-alpha");

    app.calendar_list_due(shared);
    assert!(app.calendar_tick_at(true, crate::now_ms() + 120_000.0));
    let jobs = log.lock().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].account_id, shared);
    assert_eq!(jobs[0].account_email, SHARED_A);
    assert_eq!(jobs[0].signer_id, Some(owner));
    assert_eq!(jobs[0].signer_email, OWNER_A);
    assert_eq!(jobs[0].signer_tenant, "tenant-alpha");
    assert_eq!(jobs[0].access, CANARY_A);
    assert_eq!(jobs[0].refresh, "canary-ref-2");
}

#[test]
fn control_expired_token_is_rejected() {
    let (mut app, owner, shared, _answers, log) = make_window();
    app.tokens.remove(&owner);
    app.seed_shared_calendar_token(shared, expired_tokens(CANARY_A, "", OWNER_A));
    let cal_id = calendar_of(&app, shared, SHARED_A);
    app.execute("calendar.showCalendar", json!({"id": cal_id})).unwrap();
    assert!(!app.calendar_tick_at(true, crate::now_ms() + 10_000.0));
    assert!(log.lock().unwrap().is_empty());
}

#[test]
fn control_live_token_refused_evicts_shared_entry() {
    let (mut app, _owner, shared, answers, log) = make_window();
    acquire_shared_token(&mut app, shared, SHARED_A, &answers, &log, CANARY_A, "canary-ref-2", OWNER_A, "tenant-alpha");
    app.calendar_list_due(shared);
    answers.lock().unwrap().push_back(GraphResult { account: shared, token_refused: true, ..Default::default() });
    assert!(app.calendar_tick_at(true, crate::now_ms() + 120_000.0));
    app.poll_calendar();
    assert!(!app.shared_calendar_tokens.contains_key(&shared));
}

#[test]
fn control_non_identity_owner_edit_keeps_shared_calendar_token() {
    let (mut app, owner, shared, answers, log) = make_window();
    acquire_shared_token(&mut app, shared, SHARED_A, &answers, &log, CANARY_A, "canary-ref-2", OWNER_A, "tenant-alpha");

    app.execute("account.edit", json!({"id": owner, "displayName": "Alice Renamed"})).unwrap();
    assert!(app.shared_calendar_tokens.contains_key(&shared));
    app.calendar_list_due(shared);
    assert!(app.calendar_tick_at(true, crate::now_ms() + 120_000.0));
    let jobs = log.lock().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].access, CANARY_A);
}

#[test]
fn mailbox_swap_file_new_clears_and_blocks_cross_mailbox_reuse() {
    let (mut app, owner_a, shared_a, answers, log) = make_window();
    acquire_shared_token(&mut app, shared_a, SHARED_A, &answers, &log, CANARY_A, "canary-ref-2", OWNER_A, "tenant-alpha");
    let opening_before = app.session.openings();

    app.execute("file.new", json!({})).unwrap();
    assert!(app.session.openings() > opening_before);
    assert!(app.shared_calendar_tokens.is_empty(), "drop_secrets_if_replaced must clear shared_calendar_tokens");

    let owner_b = app
        .execute("account.add", json!({"email": OWNER_B, "kind": "microsoft", "auth": "oauth2", "provider": "microsoft", "tenant": "tenant-beta"}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    assert_eq!(owner_b, owner_a);
    silence_owner(&mut app, owner_b);
    let shared_b = app.execute("account.addSharedMailbox", json!({"account": owner_b, "address": SHARED_B})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(shared_b, shared_a);
    app.session.view.module = Module::Calendar;
    let cal_b = calendar_of(&app, shared_b, SHARED_B);
    app.execute("calendar.showCalendar", json!({"id": cal_b})).unwrap();

    assert!(!app.calendar_tick_at(true, crate::now_ms() + 10_000.0), "cross-mailbox stale token must never launch a job");
    assert!(log.lock().unwrap().is_empty());
}

#[test]
fn mailbox_swap_file_open_and_confirm_clears_and_blocks_cross_mailbox_reuse() {
    let mut tmp = std::env::temp_dir();
    tmp.push(format!("cal-fix-mb2-{}.mailcraft", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    {
        let mut s2 = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
        let o2 = s2
            .run("account.add", &json!({"email": OWNER_B, "kind": "microsoft", "auth": "oauth2", "provider": "microsoft", "tenant": "tenant-beta"}))
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let sh2 = s2.run("account.addSharedMailbox", &json!({"account": o2, "address": SHARED_B})).unwrap()["id"].as_u64().unwrap();
        let cal2 = s2.mailbox.calendars.iter().find(|c| c.origin.account == sh2).unwrap().id;
        s2.run("calendar.showCalendar", &json!({"id": cal2})).unwrap();
        s2.run("file.saveAs", &json!({"path": tmp.to_string_lossy()})).unwrap();
    }

    let (mut app, _owner_a, shared_a, answers, log) = make_window();
    acquire_shared_token(&mut app, shared_a, SHARED_A, &answers, &log, CANARY_A, "canary-ref-2", OWNER_A, "tenant-alpha");

    app.execute("file.open", json!({"path": tmp.to_string_lossy()})).unwrap();
    let _ = std::fs::remove_file(&tmp);
    assert!(app.is_unconfirmed());
    assert!(app.shared_calendar_tokens.is_empty(), "file.open must clear shared_calendar_tokens");

    let owner_b = app.session.mailbox.accounts.iter().find(|a| a.email == OWNER_B).unwrap().id;
    let shared_b = app.session.mailbox.accounts.iter().find(|a| a.email == SHARED_B).unwrap().id;
    assert_eq!(shared_b, shared_a);
    silence_owner(&mut app, owner_b);
    app.session.view.module = Module::Calendar;

    app.execute("ui.confirmServers", json!({})).unwrap();
    assert!(!app.is_unconfirmed());
    assert!(!app.calendar_tick_at(true, crate::now_ms() + 20_000.0), "confirming file.open must never dispatch old mailbox token");
    assert!(log.lock().unwrap().is_empty());
}

#[test]
fn use_time_stamp_evicts_and_blocks_stale_entry_without_cleanup_hooks() {
    // 1. Opening mismatch: replace_mailbox directly on Session (bypassing drop_secrets_if_replaced).
    {
        let (mut app, _owner_a, shared_a, answers, log) = make_window();
        acquire_shared_token(&mut app, shared_a, SHARED_A, &answers, &log, CANARY_A, "canary-ref-2", OWNER_A, "tenant-alpha");

        let mut s2 = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
        let o2 = s2
            .run("account.add", &json!({"email": OWNER_B, "kind": "microsoft", "auth": "oauth2", "provider": "microsoft", "tenant": "tenant-beta"}))
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let sh2 = s2.run("account.addSharedMailbox", &json!({"account": o2, "address": SHARED_B})).unwrap()["id"].as_u64().unwrap();
        assert_eq!(sh2, shared_a);
        let cal2 = s2.mailbox.calendars.iter().find(|c| c.origin.account == sh2).unwrap().id;
        s2.run("calendar.showCalendar", &json!({"id": cal2})).unwrap();

        app.tokens.clear();
        app.graph_tokens.clear();
        app.session.replace_mailbox(s2.mailbox);
        silence_owner(&mut app, o2);
        app.session.view.module = Module::Calendar;
        assert!(app.shared_calendar_tokens.contains_key(&sh2), "precondition: entry retained when replace bypassed");

        assert!(!app.calendar_tick_at(true, crate::now_ms() + 10_000.0), "use-time stamp must block cross-opening entry");
        assert!(log.lock().unwrap().is_empty());
        assert!(!app.shared_calendar_tokens.contains_key(&sh2), "use-time stamp mismatch must evict stale entry");
    }

    // 2. Owner identity mismatch: edit owner directly on Session (bypassing execute cleanup).
    {
        let (mut app, owner, shared, answers, log) = make_window();
        acquire_shared_token(&mut app, shared, SHARED_A, &answers, &log, CANARY_A, "canary-ref-2", OWNER_A, "tenant-alpha");
        app.tokens.remove(&owner);
        app.graph_tokens.remove(&owner);
        app.session.run("account.edit", &json!({"id": owner, "tenant": "tenant-beta"})).unwrap();
        assert!(app.shared_calendar_tokens.contains_key(&shared));

        app.calendar_list_due(shared);
        assert!(!app.calendar_tick_at(true, crate::now_ms() + 120_000.0), "use-time stamp must block tenant-edited owner");
        assert!(log.lock().unwrap().is_empty());
        assert!(!app.shared_calendar_tokens.contains_key(&shared), "use-time stamp mismatch must evict entry");
    }

    // 3. Shared mailbox email mismatch: mutate shared mailbox email directly.
    {
        let (mut app, owner, shared, answers, log) = make_window();
        acquire_shared_token(&mut app, shared, SHARED_A, &answers, &log, CANARY_A, "canary-ref-2", OWNER_A, "tenant-alpha");
        app.tokens.remove(&owner);
        app.graph_tokens.remove(&owner);
        app.session.mailbox.account_mut(shared).unwrap().email = SHARED_B.into();
        assert!(app.shared_calendar_tokens.contains_key(&shared));

        app.calendar_list_due(shared);
        assert!(!app.calendar_tick_at(true, crate::now_ms() + 120_000.0), "use-time stamp must block changed shared email");
        assert!(log.lock().unwrap().is_empty());
        assert!(!app.shared_calendar_tokens.contains_key(&shared), "use-time stamp mismatch must evict entry");
    }
}

#[test]
fn account_edit_and_undo_redo_clear_tokens_and_block_reuse() {
    let (mut app, owner, shared, answers, log) = make_window();
    acquire_shared_token(&mut app, shared, SHARED_A, &answers, &log, CANARY_A, "canary-ref-2", OWNER_A, "tenant-alpha");

    // Forward edit: email + tenant change.
    app.execute("account.edit", json!({"id": owner, "email": OWNER_B, "tenant": "tenant-beta"})).unwrap();
    assert!(!app.shared_calendar_tokens.contains_key(&shared), "account.edit must clear owned shared_calendar_tokens");

    app.calendar_list_due(shared);
    assert!(!app.calendar_tick_at(true, crate::now_ms() + 120_000.0), "edited owner without sign-in must not reuse old token");
    assert!(log.lock().unwrap().is_empty());

    // Sign in Owner B and acquire CANARY_B for shared mailbox.
    app.tokens.insert(owner, fresh_tokens("own-b", "ref-b", OWNER_B, "tenant-beta"));
    answers.lock().unwrap().push_back(GraphResult {
        account: shared,
        tokens: Some(fresh_tokens(CANARY_B, "canary-ref-b", OWNER_B, "tenant-beta")),
        calendars: Some(listing(shared, SHARED_A)),
        ..Default::default()
    });
    assert!(app.calendar_tick_at(true, crate::now_ms() + 130_000.0));
    app.poll_calendar();
    assert!(app.shared_calendar_tokens.contains_key(&shared));
    log.lock().unwrap().clear();

    // edit.undo reverts Owner B -> Owner A.
    app.execute("edit.undo", json!({})).unwrap();
    assert_eq!(app.session.mailbox.account(owner).unwrap().email, OWNER_A);
    assert!(!app.shared_calendar_tokens.contains_key(&shared), "edit.undo must clear owned shared_calendar_tokens");
    app.calendar_list_due(shared);
    assert!(!app.calendar_tick_at(true, crate::now_ms() + 240_000.0), "edit.undo must not reuse Owner B token");
    assert!(log.lock().unwrap().is_empty());

    // Re-sign Owner A, acquire token, then edit.redo back to Owner B.
    app.tokens.insert(owner, fresh_tokens("own-a", "ref-a2", OWNER_A, "tenant-alpha"));
    answers.lock().unwrap().push_back(GraphResult {
        account: shared,
        tokens: Some(fresh_tokens(CANARY_A2, "ref-a3", OWNER_A, "tenant-alpha")),
        calendars: Some(listing(shared, SHARED_A)),
        ..Default::default()
    });
    assert!(app.calendar_tick_at(true, crate::now_ms() + 250_000.0));
    app.poll_calendar();
    assert!(app.shared_calendar_tokens.contains_key(&shared));
    log.lock().unwrap().clear();

    app.execute("edit.redo", json!({})).unwrap();
    assert_eq!(app.session.mailbox.account(owner).unwrap().email, OWNER_B);
    assert!(!app.shared_calendar_tokens.contains_key(&shared), "edit.redo must clear owned shared_calendar_tokens");
    app.calendar_list_due(shared);
    assert!(!app.calendar_tick_at(true, crate::now_ms() + 360_000.0));
    assert!(log.lock().unwrap().is_empty());
}

#[test]
fn tenant_only_undo_clears_shared_calendar_tokens() {
    let (mut app, owner, shared, answers, log) = make_window();
    acquire_shared_token(&mut app, shared, SHARED_A, &answers, &log, CANARY_A, "canary-ref-2", OWNER_A, "tenant-alpha");

    app.execute("account.edit", json!({"id": owner, "tenant": "tenant-beta"})).unwrap();
    assert!(!app.shared_calendar_tokens.contains_key(&shared));

    app.tokens.insert(owner, fresh_tokens("own-b", "ref-b", OWNER_A, "tenant-beta"));
    answers.lock().unwrap().push_back(GraphResult {
        account: shared,
        tokens: Some(fresh_tokens(CANARY_B, "ref-b2", OWNER_A, "tenant-beta")),
        calendars: Some(listing(shared, SHARED_A)),
        ..Default::default()
    });
    app.calendar_list_due(shared);
    assert!(app.calendar_tick_at(true, crate::now_ms() + 120_000.0));
    app.poll_calendar();
    assert!(app.shared_calendar_tokens.contains_key(&shared));

    app.execute("edit.undo", json!({})).unwrap();
    assert!(!app.shared_calendar_tokens.contains_key(&shared), "tenant-only edit.undo must clear shared_calendar_tokens");
}

#[test]
fn account_remove_and_remove_shared_mailbox_clear_tokens_and_cancel_jobs() {
    // 1. account.removeSharedMailbox by id.
    {
        let (mut app, _owner, shared, answers, log) = make_window();
        acquire_shared_token(&mut app, shared, SHARED_A, &answers, &log, CANARY_A, "canary-ref-2", OWNER_A, "tenant-alpha");
        let held: HeldSenders = Default::default();
        app.services.graph = Some(make_held_graph_service(log.clone(), held.clone()));
        app.calendar_list_due(shared);
        assert!(app.calendar_tick_at(true, crate::now_ms() + 120_000.0));
        assert!(app.calendar_syncing());

        app.execute("account.removeSharedMailbox", json!({"id": shared})).unwrap();
        assert!(!app.shared_calendar_tokens.contains_key(&shared), "removeSharedMailbox(id) must clear token");
        assert!(!app.calendar_syncing(), "removeSharedMailbox(id) must cancel in-flight job");
    }

    // 2. account.removeSharedMailbox by address.
    {
        let (mut app, _owner, shared, answers, log) = make_window();
        acquire_shared_token(&mut app, shared, SHARED_A, &answers, &log, CANARY_A, "canary-ref-2", OWNER_A, "tenant-alpha");
        let held: HeldSenders = Default::default();
        app.services.graph = Some(make_held_graph_service(log.clone(), held.clone()));
        app.calendar_list_due(shared);
        assert!(app.calendar_tick_at(true, crate::now_ms() + 120_000.0));
        assert!(app.calendar_syncing());

        app.execute("account.removeSharedMailbox", json!({"address": SHARED_A})).unwrap();
        assert!(!app.shared_calendar_tokens.contains_key(&shared), "removeSharedMailbox(address) must clear token");
        assert!(!app.calendar_syncing(), "removeSharedMailbox(address) must cancel in-flight job");
    }

    // 3. account.remove on owner clears child shared mailbox tokens and cancels child job.
    {
        let (mut app, owner, shared, answers, log) = make_window();
        acquire_shared_token(&mut app, shared, SHARED_A, &answers, &log, CANARY_A, "canary-ref-2", OWNER_A, "tenant-alpha");
        let held: HeldSenders = Default::default();
        app.services.graph = Some(make_held_graph_service(log.clone(), held.clone()));
        app.calendar_list_due(shared);
        assert!(app.calendar_tick_at(true, crate::now_ms() + 120_000.0));
        assert!(app.calendar_syncing());

        app.execute("account.remove", json!({"id": owner})).unwrap();
        assert!(app.shared_calendar_tokens.is_empty(), "account.remove on owner must clear child shared_calendar_tokens");
        assert!(!app.calendar_syncing(), "account.remove on owner must cancel child shared calendar job");
    }
}

#[test]
fn remove_all_data_clears_tokens_forgets_jobs_and_blocks_calendar_job_around() {
    // 1. Full ui.removeAllData path with partial wipe report leaving window open.
    {
        let (mut app, _owner, shared, answers, log) = make_window();
        acquire_shared_token(&mut app, shared, SHARED_A, &answers, &log, CANARY_A, "canary-ref-2", OWNER_A, "tenant-alpha");

        app.services.data_wipe = Some(Box::new(FakeWipe {
            report: WipeReport { removed: 2, failed: vec![("synthetic-leftover".into(), Why::Io("simulated busy".into()))], left: vec![] },
        }));
        app.execute("ui.removeAllData", json!({})).unwrap();
        assert!(app.data_removed);
        assert!(app.shared_calendar_tokens.is_empty(), "ui.removeAllData must clear shared_calendar_tokens");

        let day = mailcraft_time::Date::new(2026, 10, 15).unwrap();
        assert!(app.calendar_job_around(shared, day).is_err(), "calendar_job_around must refuse after ui.removeAllData");
        assert!(log.lock().unwrap().is_empty());
    }

    // 2. Direct data_removed guard in calendar_job_around & calendar_job even when tokens are still present.
    {
        let (mut app, _owner, shared, answers, log) = make_window();
        acquire_shared_token(&mut app, shared, SHARED_A, &answers, &log, CANARY_A, "canary-ref-2", OWNER_A, "tenant-alpha");
        app.data_removed = true;

        let day = mailcraft_time::Date::new(2026, 10, 15).unwrap();
        let err =
            app.calendar_job_around(shared, day).expect_err("calendar_job_around must check data_removed directly even if tokens were not cleared");
        assert!(err.contains("data has been removed"), "unexpected error: {err}");
        app.calendar_list_due(shared);
        assert!(!app.calendar_tick_at(true, crate::now_ms() + 120_000.0), "calendar_job must refuse when data_removed is set");
        assert!(log.lock().unwrap().is_empty());
    }
}

#[test]
fn late_inflight_completion_after_remove_all_data_is_dropped() {
    // 1. Full ui.removeAllData forgets in-flight calendar job immediately.
    {
        let (mut app, _owner, shared, _answers, log) = make_window();
        let held: HeldSenders = Default::default();
        app.services.graph = Some(make_held_graph_service(log.clone(), held.clone()));
        let secrets = Arc::new(SpySecretStore::default());
        app.services.secrets = Some(secrets.clone());
        app.ui.remember_sign_in = true;
        app.services.data_wipe = Some(Box::new(FakeWipe { report: WipeReport { removed: 1, failed: vec![], left: vec![] } }));

        let cal_id = calendar_of(&app, shared, SHARED_A);
        app.execute("calendar.showCalendar", json!({"id": cal_id})).unwrap();
        assert!(app.calendar_tick_at(true, crate::now_ms()));
        assert!(app.calendar_syncing());
        let tx = held.lock().unwrap().pop().unwrap();

        app.execute("ui.removeAllData", json!({})).unwrap();
        assert!(app.data_removed);
        assert!(!app.calendar_syncing(), "ui.removeAllData must call forget_calendar_jobs");
        let _ = tx.send(GraphResult {
            account: shared,
            tokens: Some(fresh_tokens(CANARY_LATE, "canary-late-ref", OWNER_A, "tenant-alpha")),
            calendars: Some(listing(shared, SHARED_A)),
            ..Default::default()
        });
        app.take_in_sync_answers(crate::now_ms() + 1_000.0);
        assert!(app.shared_calendar_tokens.is_empty());
        assert!(secrets.writes.lock().unwrap().is_empty());
    }

    // 2. Direct poll_calendar data_removed guard (bypassing remove_all_data's forget_calendar_jobs).
    {
        let (mut app, _owner, shared, _answers, log) = make_window();
        let held: HeldSenders = Default::default();
        app.services.graph = Some(make_held_graph_service(log.clone(), held.clone()));

        let cal_id = calendar_of(&app, shared, SHARED_A);
        app.execute("calendar.showCalendar", json!({"id": cal_id})).unwrap();
        assert!(app.calendar_tick_at(true, crate::now_ms()));
        assert!(app.calendar_syncing());
        let tx = held.lock().unwrap().pop().unwrap();

        app.data_removed = true;
        tx.send(GraphResult {
            account: shared,
            tokens: Some(fresh_tokens(CANARY_LATE, "canary-late-ref", OWNER_A, "tenant-alpha")),
            calendars: Some(listing(shared, SHARED_A)),
            ..Default::default()
        })
        .unwrap();
        app.poll_calendar();
        assert!(!app.calendar_syncing(), "poll_calendar must forget job when data_removed is set");
        assert!(app.shared_calendar_tokens.is_empty(), "poll_calendar must not store tokens when data_removed is set");
        assert!(
            !app.session.mailbox.calendars.iter().any(|c| c.origin.account == shared && c.name == "Calendar"),
            "poll_calendar must drop late completion whole when data_removed is set"
        );
    }
}

#[test]
fn mid_job_remove_shared_mailbox_never_poisons_graph_tokens_or_misses_eviction() {
    // 1. Removed via session.run (bypassing execute cancellation) + late tokens -> never populates graph_tokens!
    {
        let (mut app, _owner, shared, _answers, log) = make_window();
        let held: HeldSenders = Default::default();
        app.services.graph = Some(make_held_graph_service(log.clone(), held.clone()));
        let cal_id = calendar_of(&app, shared, SHARED_A);
        app.execute("calendar.showCalendar", json!({"id": cal_id})).unwrap();
        assert!(app.calendar_tick_at(true, crate::now_ms()));
        let tx = held.lock().unwrap().pop().unwrap();

        app.session.run("account.removeSharedMailbox", &json!({"id": shared})).unwrap();
        tx.send(GraphResult {
            account: shared,
            tokens: Some(fresh_tokens(CANARY_LATE, "canary-ref", OWNER_A, "tenant-alpha")),
            ..Default::default()
        })
        .unwrap();
        app.poll_calendar();
        assert!(!app.shared_calendar_tokens.contains_key(&shared));
        assert!(!app.graph_tokens.contains_key(&shared), "removed shared mailbox completion must NEVER populate graph_tokens");
    }

    // 2. Removed mid-flight + token_refused -> stale shared_calendar_tokens entry is still evicted.
    {
        let (mut app, _owner, shared, answers, log) = make_window();
        acquire_shared_token(&mut app, shared, SHARED_A, &answers, &log, CANARY_A, "canary-ref-2", OWNER_A, "tenant-alpha");
        let held: HeldSenders = Default::default();
        app.services.graph = Some(make_held_graph_service(log.clone(), held.clone()));
        app.calendar_list_due(shared);
        assert!(app.calendar_tick_at(true, crate::now_ms() + 120_000.0));
        let tx = held.lock().unwrap().pop().unwrap();

        app.session.run("account.removeSharedMailbox", &json!({"id": shared})).unwrap();
        tx.send(GraphResult { account: shared, token_refused: true, ..Default::default() }).unwrap();
        app.poll_calendar();
        assert!(!app.shared_calendar_tokens.contains_key(&shared), "stale shared token must be evicted when account removed mid-job");
    }
}

#[test]
fn mid_job_account_edit_rejects_late_completion_with_empty_email_or_tenant_change() {
    // 1. execute("account.edit") cancels in-flight job immediately.
    {
        let (mut app, owner, shared, _answers, log) = make_window();
        let held: HeldSenders = Default::default();
        app.services.graph = Some(make_held_graph_service(log.clone(), held.clone()));
        let cal_id = calendar_of(&app, shared, SHARED_A);
        app.execute("calendar.showCalendar", json!({"id": cal_id})).unwrap();
        assert!(app.calendar_tick_at(true, crate::now_ms()));
        assert!(app.calendar_syncing());

        app.execute("account.edit", json!({"id": owner, "email": OWNER_B, "tenant": "tenant-beta"})).unwrap();
        assert!(!app.calendar_syncing(), "account.edit changing identity must cancel in-flight shared calendar job");
    }

    // 2. Direct session.run("account.edit") (bypassing execute cancellation) + late empty-email tokens.
    {
        let (mut app, owner, shared, _answers, log) = make_window();
        let held: HeldSenders = Default::default();
        app.services.graph = Some(make_held_graph_service(log.clone(), held.clone()));
        let cal_id = calendar_of(&app, shared, SHARED_A);
        app.execute("calendar.showCalendar", json!({"id": cal_id})).unwrap();
        assert!(app.calendar_tick_at(true, crate::now_ms()));
        let tx = held.lock().unwrap().pop().unwrap();

        app.session.run("account.edit", &json!({"id": owner, "email": OWNER_B, "tenant": "tenant-beta"})).unwrap();
        tx.send(GraphResult {
            account: shared,
            tokens: Some(fresh_tokens(CANARY_LATE, "canary-stale-ref", "", "tenant-alpha")),
            calendars: Some(listing(shared, SHARED_A)),
            ..Default::default()
        })
        .unwrap();
        app.poll_calendar();
        assert!(!app.shared_calendar_tokens.contains_key(&shared), "late completion with empty email after owner identity edit must be rejected");
    }

    // 3. Direct session.run("account.edit") changing tenant ONLY + late completion naming same email.
    {
        let (mut app, owner, shared, _answers, log) = make_window();
        let held: HeldSenders = Default::default();
        app.services.graph = Some(make_held_graph_service(log.clone(), held.clone()));
        let cal_id = calendar_of(&app, shared, SHARED_A);
        app.execute("calendar.showCalendar", json!({"id": cal_id})).unwrap();
        assert!(app.calendar_tick_at(true, crate::now_ms()));
        let tx = held.lock().unwrap().pop().unwrap();

        app.session.run("account.edit", &json!({"id": owner, "tenant": "tenant-beta"})).unwrap();
        tx.send(GraphResult {
            account: shared,
            tokens: Some(fresh_tokens(CANARY_LATE, "canary-stale-ref", OWNER_A, "tenant-alpha")),
            calendars: Some(listing(shared, SHARED_A)),
            ..Default::default()
        })
        .unwrap();
        app.poll_calendar();
        assert!(!app.shared_calendar_tokens.contains_key(&shared), "late completion after owner tenant-only edit must be rejected");
    }
}

#[test]
fn owner_sign_in_refusal_clears_shared_calendar_tokens_and_blocks_dispatch() {
    // 1. Interactive refusal (!consent) + failed re-sign-in.
    {
        let (mut app, owner, shared, answers, log) = make_window();
        acquire_shared_token(&mut app, shared, SHARED_A, &answers, &log, CANARY_A, "canary-ref-2", OWNER_A, "tenant-alpha");

        app.graph_mail_needs_sign_in(owner, false, false, false);
        assert!(app.signing_in());
        app.poll_sign_in();
        assert!(!app.signing_in());
        assert!(!app.shared_calendar_tokens.contains_key(&shared), "owner sign-in refusal must clear shared_calendar_tokens");

        app.calendar_list_due(shared);
        assert!(!app.calendar_tick_at(true, crate::now_ms() + 120_000.0));
        assert!(log.lock().unwrap().is_empty());
    }

    // 2. Quiet refusal (!consent) also clears shared_calendar_tokens.
    {
        let (mut app, owner, shared, answers, log) = make_window();
        acquire_shared_token(&mut app, shared, SHARED_A, &answers, &log, CANARY_A, "canary-ref-2", OWNER_A, "tenant-alpha");

        app.graph_mail_needs_sign_in(owner, true, false, false);
        assert!(!app.shared_calendar_tokens.contains_key(&shared), "quiet owner sign-in refusal must clear shared_calendar_tokens");
    }
}
#[test]
fn apply_calendar_answer_for_removed_shared_mailbox_never_falls_through_to_graph_tokens() {
    use mailcraft_host::calendar_job::CalendarAnswer;

    // 1. Late tokens for a removed shared mailbox must never populate graph_tokens.
    {
        let (mut app, _owner, shared, _answers, _log) = make_window();
        app.execute("account.removeSharedMailbox", json!({"id": shared})).unwrap();
        let answer = CalendarAnswer {
            account: shared,
            is_shared: true,
            carried: false,
            started: 0.0,
            result: GraphResult {
                account: shared,
                tokens: Some(fresh_tokens(CANARY_LATE, "canary-ref", OWNER_A, "tenant-alpha")),
                ..Default::default()
            },
            stopped_early: false,
            foreign: None,
        };
        app.apply_calendar_answer(answer);
        assert!(!app.graph_tokens.contains_key(&shared), "removed shared mailbox answer must NEVER populate graph_tokens");
        assert!(!app.shared_calendar_tokens.contains_key(&shared));
    }

    // 2. token_refused for a removed shared mailbox evicts shared_calendar_tokens, not graph_tokens.
    {
        let (mut app, _owner, shared, answers, log) = make_window();
        acquire_shared_token(&mut app, shared, SHARED_A, &answers, &log, CANARY_A, "canary-ref-2", OWNER_A, "tenant-alpha");
        app.graph_tokens.insert(shared, fresh_tokens("unrelated-graph", "ref", OWNER_A, "tenant-alpha"));
        app.session.run("account.removeSharedMailbox", &json!({"id": shared})).unwrap();
        let answer = CalendarAnswer {
            account: shared,
            is_shared: true,
            carried: false,
            started: 0.0,
            result: GraphResult { account: shared, token_refused: true, ..Default::default() },
            stopped_early: false,
            foreign: None,
        };
        app.apply_calendar_answer(answer);
        assert!(!app.shared_calendar_tokens.contains_key(&shared), "token_refused must evict shared_calendar_tokens for shared job");
        assert!(app.graph_tokens.contains_key(&shared), "token_refused for a shared job must not evict graph_tokens");
    }
}
