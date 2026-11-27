// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The background jobs that renew an account's sign-in for themselves (free/busy, automatic
//! replies, to-dos, rules, categories) and the read of the system's credential store: an answer of
//! one that went with another sign-in keeps nothing, in memory or in the store, whichever way it
//! ends, and shows nothing; one that went with the sign-in there still is lands, whatever else
//! changed; and once the data is removed none keeps anything or asks again. Fake in-memory
//! services and invented `.example` addresses only.

use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::outlook_colors::OUTLOOK_COLORS;
use mailcraft_model::remote::CategoriesDownload;
use mailcraft_model::sync::Tokens;
use mailcraft_model::{Category, Id};
use serde_json::json;

use super::integration_r2_tests::add_owner_b;
use super::integration_tests::{FakeWipe, OWNER_A, SignIns, fresh_tokens, setup_window};
use crate::auto_replies::{AutoRepliesJob, AutoRepliesResult};
use crate::category_sync::{CategoriesJob, CategoriesResult};
use crate::rule_sync::{RulesJob, RulesResult};
use crate::task_sync::{TasksJob, TasksResult};
use crate::{FreeBusyRequest, FreeBusyResult, MailApp, Services};

const NOW: i64 = 1_791_993_600;
const HYBRID: &str = "avery@mail.example";

/// The system's credential store, in memory. A read on the store's own thread (`secret-read`)
/// sees the entries as they were when [`Store::freeze`] was called, as a read that had got that
/// far before the window changed anything would; and it writes nothing.
#[derive(Clone, Default)]
struct Store {
    live: Arc<Mutex<HashMap<String, String>>>,
    frozen: Arc<Mutex<Option<HashMap<String, String>>>>,
}

impl Store {
    fn reader() -> bool {
        std::thread::current().name() == Some("secret-read")
    }

    fn freeze(&self) {
        *self.frozen.lock().unwrap() = Some(self.live.lock().unwrap().clone());
    }

    fn holds(&self, mark: &str) -> Vec<String> {
        self.live.lock().unwrap().iter().filter(|(_, v)| v.contains(mark)).map(|(k, _)| k.clone()).collect()
    }
}

impl crate::SecretStore for Store {
    fn get(&self, key: &str) -> Option<String> {
        if Self::reader()
            && let Some(frozen) = &*self.frozen.lock().unwrap()
        {
            return frozen.get(key).cloned();
        }
        self.live.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), mailcraft_host::SecretError> {
        if !Self::reader() {
            self.live.lock().unwrap().insert(key.to_string(), secret.to_string());
        }
        Ok(())
    }
    fn delete(&self, key: &str) {
        if !Self::reader() {
            self.live.lock().unwrap().remove(key);
        }
    }
}

/// Every place a token or secret with `mark` in it is kept: in memory or remembered.
fn kept_with(app: &MailApp, store: &Store, mark: &str) -> Vec<String> {
    let has = |t: &Tokens| t.access.contains(mark) || t.refresh.contains(mark);
    let mut found = Vec::new();
    found.extend(app.tokens.iter().filter(|(_, t)| has(t)).map(|(id, _)| format!("tokens[{id}]")));
    found.extend(app.graph_tokens.iter().filter(|(_, t)| has(t)).map(|(id, _)| format!("graph_tokens[{id}]")));
    found.extend(app.shared_calendar_tokens.iter().filter(|(_, e)| has(&e.tokens)).map(|(id, _)| format!("shared_calendar_tokens[{id}]")));
    found.extend(app.directory_search.access.iter().filter(|(_, t)| has(t)).map(|(id, _)| format!("directory[{id}]")));
    found.extend(app.secret_cache.iter().filter(|(_, v)| v.contains(mark)).map(|(k, _)| format!("secret_cache {k}")));
    found.extend(store.holds(mark).into_iter().map(|k| format!("system store {k}")));
    found.sort();
    found
}

/// The five kinds of background job that renew the sign-in they go with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    FreeBusy,
    AutoReplies,
    Tasks,
    Rules,
    Categories,
}

const KINDS: [Kind; 5] = [Kind::FreeBusy, Kind::AutoReplies, Kind::Tasks, Kind::Rules, Kind::Categories];

/// Where a job's answer goes.
enum Out {
    FreeBusy(Sender<FreeBusyResult>),
    AutoReplies(Sender<AutoRepliesResult>),
    Tasks(Sender<TasksResult>),
    Rules(Sender<RulesResult>),
    Categories(Sender<CategoriesResult>),
}

type Outs = Arc<Mutex<Vec<(Id, Out)>>>;

/// How a job's answer ends.
#[derive(Clone, Copy, Debug)]
enum How {
    /// With a renewed sign-in.
    Tokens,
    /// With a renewed sign-in and a refusal or a missing permission beside it.
    TokensRefused,
    /// With a refusal and nothing else.
    Refused,
    /// With a renewed sign-in and a problem.
    Error,
    /// Without an answer (the job's thread ended).
    Gone,
}

const HOWS: [How; 5] = [How::Tokens, How::TokensRefused, How::Refused, How::Error, How::Gone];

fn late(email: &str) -> Option<Tokens> {
    Some(fresh_tokens("LATE-access", "LATE-ref", email, ""))
}

/// A window with one Microsoft account that every one of the five jobs runs for, a store for
/// what is remembered, hosts that keep every job's answer back, and a sign-in page host.
struct Rig {
    kind: Kind,
    app: MailApp,
    owner: Id,
    store: Store,
    outs: Outs,
    sign_ins: SignIns,
    calendar: Id,
    day: u8,
    at: f64,
}

impl Rig {
    fn new(kind: Kind) -> Self {
        let store = Store::default();
        let outs: Outs = Default::default();
        let (mut app, owner, calendar, sign_ins) = if kind == Kind::FreeBusy {
            // (Free/busy needs a calendar the server has: an account of a mail server that signs
            // in with Microsoft, kept in step by a stand-in server.)
            let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
            app.autosave = false;
            let server = crate::calendar_sync::testing::serve(&mut app);
            let owner = crate::calendar_sync::testing::microsoft_account(&mut app);
            server.lock().unwrap().events = vec![crate::calendar_sync::testing::event("e-1", "Budget review", NOW - 3 * 3600)];
            assert!(app.calendar_tick_at(true, crate::now_ms()), "premise: the calendar is read");
            app.poll_calendar();
            app.session.driver.set_auto_sync(false);
            let calendar = app.session.mailbox.calendars.iter().find(|c| c.origin.account == owner).map(|c| c.id).expect("the server's calendar");
            let sign_ins: SignIns = Default::default();
            let log = sign_ins.clone();
            app.services.sign_in = Some(Box::new(move |req: crate::SignInRequest| {
                let (tx, rx) = mpsc::channel();
                log.lock().unwrap().push((req.graph, req.directory, req.cancel.clone(), tx));
                rx
            }));
            (app, owner, calendar, sign_ins)
        } else {
            let (mut app, owner, _shared, _jobs, sign_ins) = setup_window();
            // (The categories are kept in step for an account whose list was read once.)
            let categories = CategoriesDownload {
                account: owner,
                email: OWNER_A.into(),
                categories: vec![Category { name: "Red category".into(), color: OUTLOOK_COLORS[0] }],
                complete: true,
            };
            mailcraft_engine::cmd::remote::apply_categories(&mut app.session, &categories).unwrap();
            (app, owner, 0, sign_ins)
        };
        app.services.secrets = Some(Arc::new(store.clone()));
        app.ui.remember_sign_in = true;
        app.services.microsoft_client_id = "built-in-app".into();
        let log = outs.clone();
        app.services.free_busy = Some(Box::new(move |r: FreeBusyRequest| {
            let (tx, rx) = mpsc::channel();
            log.lock().unwrap().push((r.account.id, Out::FreeBusy(tx)));
            rx
        }));
        let log = outs.clone();
        app.services.graph_auto_replies = Some(Box::new(move |j: AutoRepliesJob| {
            let (tx, rx) = mpsc::channel();
            log.lock().unwrap().push((j.account.id, Out::AutoReplies(tx)));
            rx
        }));
        let log = outs.clone();
        app.services.graph_tasks = Some(Box::new(move |j: TasksJob| {
            let (tx, rx) = mpsc::channel();
            log.lock().unwrap().push((j.account.id, Out::Tasks(tx)));
            rx
        }));
        let log = outs.clone();
        app.services.graph_rules = Some(Box::new(move |j: RulesJob| {
            let (tx, rx) = mpsc::channel();
            log.lock().unwrap().push((j.account.id, Out::Rules(tx)));
            rx
        }));
        let log = outs.clone();
        app.services.graph_categories = Some(Box::new(move |j: CategoriesJob| {
            let (tx, rx) = mpsc::channel();
            log.lock().unwrap().push((j.account.id, Out::Categories(tx)));
            rx
        }));
        Self { kind, app, owner, store, outs, sign_ins, calendar, day: 10, at: 1_000.0 }
    }

    /// The owner signs in: its own tokens, marked.
    fn sign_in_as(&mut self, mark: &str) {
        let email = if self.kind == Kind::FreeBusy { HYBRID } else { OWNER_A };
        self.app.tokens.insert(self.owner, fresh_tokens(&format!("{mark}-mail"), &format!("{mark}-ref"), email, ""));
        self.app.graph_tokens.insert(self.owner, fresh_tokens(&format!("{mark}-graph"), &format!("{mark}-ref"), email, ""));
    }

    /// Start this rig's kind of job; whether one went to the host for the owner.
    fn start(&mut self) -> bool {
        let before = self.outs.lock().unwrap().len();
        self.at += 600_000.0;
        let at = self.at;
        match self.kind {
            Kind::FreeBusy => {
                self.day += 1;
                let day = mailcraft_time::Date::new(2026, 10, self.day).unwrap();
                let _ = self.app.free_busy_at(self.calendar, &["maya@lumen.example".to_string()], day, at);
            }
            Kind::AutoReplies => self.app.auto_replies_read_now(self.owner),
            Kind::Tasks => drop(self.app.tasks_tick_at(at)),
            Kind::Rules => drop(self.app.rules_tick_at(at)),
            Kind::Categories => drop(self.app.categories_tick_at(at)),
        }
        let outs = self.outs.lock().unwrap();
        outs.len() > before && outs.last().is_some_and(|(account, _)| *account == self.owner)
    }

    fn busy(&self) -> bool {
        match self.kind {
            Kind::FreeBusy => self.app.free_busy.busy(),
            Kind::AutoReplies => self.app.auto_replies.busy(),
            Kind::Tasks => self.app.tasks.busy(),
            Kind::Rules => self.app.rules.busy(),
            Kind::Categories => self.app.category_sync.busy(),
        }
    }

    /// The job last started answers `how`; the window looks a few times.
    fn answer(&mut self, how: How) {
        let (account, out) = self.outs.lock().unwrap().pop().expect("a job to answer");
        let tokens = late(if self.kind == Kind::FreeBusy { HYBRID } else { OWNER_A });
        let said = "LATE problem".to_string();
        match (out, how) {
            (_, How::Gone) => {}
            (Out::FreeBusy(tx), how) => drop(tx.send(match how {
                How::Tokens => FreeBusyResult { account, tokens, ..Default::default() },
                How::TokensRefused => FreeBusyResult { account, tokens, token_refused: true, consent_needed: true, ..Default::default() },
                How::Refused => FreeBusyResult { account, token_refused: true, error: said, ..Default::default() },
                _ => FreeBusyResult { account, tokens, error: said, ..Default::default() },
            })),
            (Out::AutoReplies(tx), how) => drop(tx.send(match how {
                How::Tokens => AutoRepliesResult { account, tokens, ..Default::default() },
                How::TokensRefused => AutoRepliesResult { account, tokens, sign_in_refused: true, consent_needed: true, ..Default::default() },
                How::Refused => AutoRepliesResult { account, sign_in_refused: true, error: said, ..Default::default() },
                _ => AutoRepliesResult { account, tokens, error: said, ..Default::default() },
            })),
            (Out::Tasks(tx), how) => drop(tx.send(match how {
                How::Tokens => TasksResult { account, tokens, ..Default::default() },
                How::TokensRefused => TasksResult { account, tokens, sign_in_refused: true, consent_needed: true, ..Default::default() },
                How::Refused => TasksResult { account, sign_in_refused: true, errors: vec![said], ..Default::default() },
                _ => TasksResult { account, tokens, errors: vec![said], ..Default::default() },
            })),
            (Out::Rules(tx), how) => drop(tx.send(match how {
                How::Tokens => RulesResult { account, tokens, ..Default::default() },
                How::TokensRefused => RulesResult { account, tokens, sign_in_refused: true, consent_needed: true, ..Default::default() },
                How::Refused => RulesResult { account, sign_in_refused: true, errors: vec![said], ..Default::default() },
                _ => RulesResult { account, tokens, errors: vec![said], ..Default::default() },
            })),
            (Out::Categories(tx), how) => drop(tx.send(match how {
                How::Tokens => CategoriesResult { account, tokens, ..Default::default() },
                How::TokensRefused => CategoriesResult { account, tokens, sign_in_refused: true, ..Default::default() },
                How::Refused => CategoriesResult { account, sign_in_refused: true, errors: vec![said], ..Default::default() },
                _ => CategoriesResult { account, tokens, errors: vec![said], ..Default::default() },
            })),
        }
        for _ in 0..3 {
            match self.kind {
                Kind::FreeBusy => self.app.poll_free_busy(),
                Kind::AutoReplies => self.app.poll_auto_replies(),
                Kind::Tasks => self.app.poll_tasks(),
                Kind::Rules => self.app.poll_rules(),
                Kind::Categories => self.app.poll_categories(),
            }
        }
    }

    fn edit(&mut self, params: serde_json::Value) {
        let mut params = params;
        params["id"] = json!(self.owner);
        self.app.execute("account.edit", params).unwrap();
    }

    /// What the owner was shown: the status line, the status bar, the account's problems, a dialog.
    fn shown(&self) -> (Option<String>, String, usize, bool) {
        let problems = self.app.sync_problems.iter().filter(|(a, ..)| *a == self.owner).count();
        (self.app.status_msg.as_ref().map(|(s, _)| s.clone()), self.app.session.sync_status.clone(), problems, self.app.dialog.is_some())
    }
}

/// What makes the sign-in a job went with another one, said in the steps before the job starts
/// and after it did.
const CHANGES: [&str; 5] =
    ["tenant edit", "application id edit", "Undo of a tenant edit", "Redo of a tenant edit", "the installation's application id"];

fn before_the_job(rig: &mut Rig, change: &str) {
    match change {
        "Undo of a tenant edit" => rig.edit(json!({"tenant": "tenant-new"})),
        "Redo of a tenant edit" => {
            rig.edit(json!({"tenant": "tenant-new"}));
            rig.app.execute("edit.undo", json!({})).unwrap();
        }
        _ => {}
    }
}

fn while_the_job_runs(rig: &mut Rig, change: &str) {
    match change {
        "tenant edit" => rig.edit(json!({"tenant": "tenant-new"})),
        "application id edit" => rig.edit(json!({"clientId": "11111111-2222-3333-4444-555555555555"})),
        "Undo of a tenant edit" => drop(rig.app.execute("edit.undo", json!({})).unwrap()),
        "Redo of a tenant edit" => drop(rig.app.execute("edit.redo", json!({})).unwrap()),
        _ => drop(rig.app.execute("ui.microsoftClientId", json!({"value": "other-app-0000"})).unwrap()),
    }
}

/// R2-1 and R2-3: a job that went with the sign-in before keeps nothing of its answer anywhere,
/// drops nothing of the sign-in there is now, opens no page and shows nothing, whichever way its
/// answer ends; for each of the five kinds and each way the sign-in can change under it.
#[test]
fn an_answer_of_a_job_that_went_with_the_sign_in_before_keeps_and_shows_nothing() {
    let mut wrong = Vec::new();
    let mut checked = 0;
    for kind in KINDS {
        for change in CHANGES {
            for how in HOWS {
                let mut rig = Rig::new(kind);
                before_the_job(&mut rig, change);
                rig.sign_in_as("old");
                if !rig.start() {
                    wrong.push(format!("PREMISE {kind:?} {change}: no job started"));
                    continue;
                }
                let tenant_then = rig.app.session.mailbox.account(rig.owner).and_then(|a| a.oauth.clone()).map(|o| (o.tenant, o.client_id));
                while_the_job_runs(&mut rig, change);
                let tenant_now = rig.app.session.mailbox.account(rig.owner).and_then(|a| a.oauth.clone()).map(|o| (o.tenant, o.client_id));
                if change != "the installation's application id" && tenant_then == tenant_now {
                    wrong.push(format!("PREMISE {kind:?} {change}: the sign-in did not change"));
                    continue;
                }
                // The sign-in there is now.
                rig.sign_in_as("new");
                let shown = rig.shown();
                rig.answer(how);
                checked += 1;
                let mut bad = Vec::new();
                let kept = kept_with(&rig.app, &rig.store, "LATE");
                if !kept.is_empty() {
                    bad.push(format!("kept {kept:?}"));
                }
                let own = (rig.app.tokens.get(&rig.owner).map(|t| t.refresh.clone()), rig.app.graph_tokens.get(&rig.owner).map(|t| t.access.clone()));
                if own != (Some("new-ref".to_string()), Some("new-graph".to_string())) {
                    bad.push(format!("the sign-in there is now lost its tokens: {own:?}"));
                }
                if !rig.sign_ins.lock().unwrap().is_empty() || rig.app.signing_in() {
                    bad.push("a sign-in page opened".into());
                }
                if rig.shown() != shown {
                    bad.push(format!("shown: {:?} -> {:?}", shown, rig.shown()));
                }
                if rig.busy() {
                    bad.push("the job is still waited for".into());
                }
                if !bad.is_empty() {
                    wrong.push(format!("{kind:?} | {change} | {how:?}: {bad:?}"));
                }
            }
        }
    }
    assert_eq!(checked, KINDS.len() * CHANGES.len() * HOWS.len(), "every combination ran: {wrong:#?}");
    assert!(wrong.is_empty(), "{} of {checked} combinations kept or showed something: {wrong:#?}", wrong.len());
}

/// Over-clearing: a job that went with the sign-in there still is (nothing changed, a name or a
/// colour changed, another account's sign-in changed or went) keeps its renewed sign-in, in
/// memory and in the store.
#[test]
fn an_answer_of_a_job_whose_sign_in_is_unchanged_lands_whatever_else_changed() {
    let mut wrong = Vec::new();
    for kind in KINDS {
        for change in ["nothing", "display name", "colour", "another owner's tenant edit", "another owner removed", "Undo of a display name"] {
            let mut rig = Rig::new(kind);
            let (other, _) = add_owner_b(&mut rig.app);
            if change == "Undo of a display name" {
                rig.edit(json!({"displayName": "Someone Else"}));
            }
            rig.sign_in_as("old");
            if !rig.start() {
                wrong.push(format!("PREMISE {kind:?} {change}: no job started"));
                continue;
            }
            match change {
                "display name" => rig.edit(json!({"displayName": "Someone Else"})),
                "colour" => rig.edit(json!({"color": "#2E8B57"})),
                "another owner's tenant edit" => drop(rig.app.execute("account.edit", json!({"id": other, "tenant": "tenant-gamma"})).unwrap()),
                "another owner removed" => drop(rig.app.execute("account.remove", json!({"id": other})).unwrap()),
                "Undo of a display name" => drop(rig.app.execute("edit.undo", json!({})).unwrap()),
                _ => {}
            }
            rig.answer(How::Tokens);
            let refresh = rig.app.tokens.get(&rig.owner).map(|t| t.refresh.clone());
            let remembered = rig.app.remembered("refresh", rig.owner);
            if refresh.as_deref() != Some("LATE-ref") || remembered.as_deref() != Some("LATE-ref") {
                wrong.push(format!("{kind:?} | {change}: the renewed sign-in was not kept: tokens {refresh:?}, remembered {remembered:?}"));
            }
            if kind == Kind::FreeBusy && rig.app.graph_tokens.get(&rig.owner).map(|t| t.access.as_str()) != Some("LATE-access") {
                wrong.push(format!("{kind:?} | {change}: the renewed Graph token was not kept"));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// R2-2: once the data is removed, a job still under way keeps nothing of its answer, and none of
/// the five asks the server again, even with a sign-in back in memory.
#[test]
fn after_the_data_is_removed_no_background_job_keeps_anything_or_asks_again() {
    let mut wrong = Vec::new();
    for kind in KINDS {
        let mut rig = Rig::new(kind);
        rig.sign_in_as("old");
        assert!(rig.start(), "premise: a {kind:?} job started");
        rig.app.services.data_wipe = Some(Box::new(FakeWipe));
        rig.app.execute("ui.removeAllData", json!({})).unwrap();
        if rig.busy() {
            wrong.push(format!("{kind:?}: still under way after the removal"));
        }
        rig.answer(How::Tokens);
        let kept = kept_with(&rig.app, &rig.store, "LATE");
        if !kept.is_empty() {
            wrong.push(format!("{kind:?}: kept after the removal: {kept:?}"));
        }
        if rig.busy() {
            wrong.push(format!("{kind:?}: still waited for after the removal"));
        }
        // (A sign-in that some store missed: none of the five goes with it.)
        rig.sign_in_as("missed");
        if rig.start() {
            wrong.push(format!("{kind:?}: asked the server again after the removal"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// The window's read of the store, started for this mailbox before an edit lets go of what is
/// remembered for the account: what the read brings for a name let go of meanwhile is not taken.
#[test]
fn a_read_of_the_store_brings_nothing_back_that_was_let_go_of_while_it_ran() {
    for change in ["tenant edit", "application id edit", "the installation's application id", "the data removed"] {
        let mut rig = Rig::new(Kind::Tasks);
        let owner = rig.app.session.mailbox.account(rig.owner).cloned().unwrap();
        let key = MailApp::secret_key("refresh", &owner);
        rig.store.live.lock().unwrap().insert(key.clone(), "LATE-remembered".into());
        rig.app.tokens.remove(&rig.owner);
        rig.app.graph_tokens.remove(&rig.owner);
        rig.store.freeze();
        rig.app.secrets_tick();
        assert!(rig.app.secret_read.is_some(), "premise ({change}): the read is under way");
        match change {
            "tenant edit" => rig.edit(json!({"tenant": "tenant-new"})),
            "application id edit" => rig.edit(json!({"clientId": "11111111-2222-3333-4444-555555555555"})),
            "the installation's application id" => drop(rig.app.execute("ui.microsoftClientId", json!({"value": "other-app-0000"})).unwrap()),
            _ => {
                rig.app.services.data_wipe = Some(Box::new(FakeWipe));
                rig.app.execute("ui.removeAllData", json!({})).unwrap();
            }
        }
        assert!(rig.store.holds("LATE").is_empty(), "premise ({change}): the store let go of it");
        let waited = std::time::Instant::now();
        while rig.app.secret_read.is_some() && waited.elapsed() < std::time::Duration::from_secs(5) {
            rig.app.poll_secret_read();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(rig.app.secret_read.is_none(), "premise ({change}): the read came back");
        let kept = kept_with(&rig.app, &rig.store, "LATE");
        assert!(kept.is_empty(), "{change}: the read brought back what was let go of: {kept:?}");
        assert_eq!(rig.app.remembered("refresh", rig.owner), None, "{change}: nothing to hand a job");
    }
}

/// Over-clearing: what is let go of for one account while the read runs leaves what it brings for
/// another, and a read during which nothing was let go of brings everything.
#[test]
fn a_read_of_the_store_keeps_what_it_brings_for_every_name_not_let_go_of() {
    for change in ["nothing", "another owner's tenant edit", "a display name"] {
        let mut rig = Rig::new(Kind::Tasks);
        let (other, _) = add_owner_b(&mut rig.app);
        let owner = rig.app.session.mailbox.account(rig.owner).cloned().unwrap();
        let key = MailApp::secret_key("refresh", &owner);
        rig.store.live.lock().unwrap().insert(key, "owner-remembered".into());
        rig.app.tokens.remove(&rig.owner);
        rig.store.freeze();
        rig.app.secrets_tick();
        assert!(rig.app.secret_read.is_some(), "premise ({change}): the read is under way");
        match change {
            "another owner's tenant edit" => drop(rig.app.execute("account.edit", json!({"id": other, "tenant": "tenant-gamma"})).unwrap()),
            "a display name" => rig.edit(json!({"displayName": "Someone Else"})),
            _ => {}
        }
        let waited = std::time::Instant::now();
        while rig.app.secret_read.is_some() && waited.elapsed() < std::time::Duration::from_secs(5) {
            rig.app.poll_secret_read();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            rig.app.remembered("refresh", rig.owner).as_deref(),
            Some("owner-remembered"),
            "{change}: the owner's remembered sign-in is taken"
        );
    }
}
