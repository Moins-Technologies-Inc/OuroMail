// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One identity check for every token store, and a shared mailbox's mail token that goes with its
//! owner's sign-in: tenant-only and application-id-only edits, Undo and Redo; every event that
//! lets go of an owner's sign-in; and the check made when the token is used. Fake in-memory
//! services and invented `.example` addresses only.

use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use mailcraft_engine::cmd::graph_mail::apply_graph_mail;
use mailcraft_model::directory::DirectoryOutcome;
use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMailResult, MessagePage, Tokens};
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use super::integration_tests::{
    DirJobs, FakeWipe, OWNER_A, OWNER_B, SHARED_A, SHARED_A2, SHARED_B, answer_dir, fresh_tokens, launch_pending_dir, open_dir_dialog_and_type,
    setup_window, silence_owner,
};
use super::*;
use crate::{GraphRequest, GraphResult, MailApp};

const LINK: &str = "https://graph.microsoft.com/v1.0/me/mailFolders/in/messages/delta?$deltatoken=one";

/// What a Graph job was asked with: for whom, its tokens, and where its answer goes.
pub(super) struct Ask {
    pub(super) account: Id,
    pub(super) access: String,
    pub(super) refresh: String,
    pub(super) tx: Sender<GraphResult>,
}

pub(super) type Asked = Arc<Mutex<Vec<Ask>>>;
/// The tokens each directory search went with: access, refresh.
type DirAsked = Arc<Mutex<Vec<(String, String)>>>;

#[derive(Clone, Default)]
struct Store(Arc<Mutex<HashMap<String, String>>>);

impl crate::SecretStore for Store {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), mailcraft_host::SecretError> {
        self.0.lock().unwrap().insert(key.to_string(), secret.to_string());
        Ok(())
    }
    fn delete(&self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
}

/// A host that notes what every Graph job was asked with and keeps its answer back. (The mail's
/// pieces end at once: the job's own answer is what the tests send.)
pub(super) fn capture_graph(app: &mut MailApp) -> Asked {
    let asked: Asked = Default::default();
    let log = asked.clone();
    app.services.graph = Some(Box::new(move |req: GraphRequest| {
        let (tx, rx) = mpsc::channel();
        log.lock().unwrap().push(Ask { account: req.account.id, access: req.access.clone(), refresh: req.refresh.clone(), tx });
        rx
    }));
    asked
}

/// Note the tokens every directory search goes with, beside what `jobs` notes.
fn tap_directory(app: &mut MailApp, jobs: &DirJobs) -> DirAsked {
    let asked: DirAsked = Default::default();
    let (log, jobs) = (asked.clone(), jobs.clone());
    app.services.graph_directory = Some(Box::new(move |job: DirectoryJob| {
        let (tx, rx) = mpsc::channel();
        log.lock().unwrap().push((job.access.clone(), job.refresh.clone()));
        jobs.lock().unwrap().push((job.query.text.clone(), job.query.ticket, job.access.clone(), job.cancel.clone(), tx));
        rx
    }));
    asked
}

/// Give `account` an Inbox the server knows, with a link: it has an Inbox pass and a check.
pub(super) fn seed_inbox(app: &mut MailApp, account: Id, email: &str) {
    let folders = vec![GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }];
    let pages = vec![MessagePage { folder: "in".into(), link: LINK.into(), caught_up: true, ..Default::default() }];
    let seed =
        GraphMailResult { account, email: email.into(), listing: Some(FolderListing { folders, complete: true }), pages, ..Default::default() };
    apply_graph_mail(&mut app.session, seed).unwrap();
}

/// A second owner with a shared mailbox of its own, signed in.
pub(super) fn add_owner_b(app: &mut MailApp) -> (Id, Id) {
    let owner = app
        .execute(
            "account.add",
            json!({"email": OWNER_B, "kind": "microsoft", "auth": "oauth2", "provider": "microsoft", "tenant": "tenant-beta", "directory": "organisation"}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    app.tokens.insert(owner, fresh_tokens("owner-b-mail", "owner-b-ref", OWNER_B, "tenant-beta"));
    let shared = app.execute("account.addSharedMailbox", json!({"account": owner, "address": SHARED_B})).unwrap()["id"].as_u64().unwrap();
    (owner, shared)
}

/// A shared mailbox's mail token as its run leaves it: through the run's own end, never put into
/// the map by hand.
pub(super) fn shared_mail_token(app: &mut MailApp, shared: Id, access: &str, refresh: &str, email: &str) {
    let tokens = fresh_tokens(access, refresh, email, "");
    app.finish_shared_mail(shared, true, GraphResult { account: shared, tokens: Some(tokens), ..Default::default() });
    assert_eq!(app.graph_tokens.get(&shared).map(|t| t.access.as_str()), Some(access), "premise: the shared mailbox's mail token is kept");
}

/// A window with one owner and its shared mailbox, a store for what is remembered, and hosts that
/// note what they are asked with.
struct Rig {
    app: MailApp,
    owner: Id,
    shared: Id,
    jobs: DirJobs,
    asked: Asked,
    dir_asked: DirAsked,
    store: Store,
}

impl Rig {
    fn new() -> Self {
        let (mut app, owner, shared, jobs, _) = setup_window();
        let store = Store::default();
        app.services.secrets = Some(Arc::new(store.clone()));
        app.ui.remember_sign_in = true;
        seed_inbox(&mut app, owner, OWNER_A);
        seed_inbox(&mut app, shared, SHARED_A);
        let asked = capture_graph(&mut app);
        let dir_asked = tap_directory(&mut app, &jobs);
        Self { app, owner, shared, jobs, asked, dir_asked, store }
    }

    /// Every place a token whose text has `mark` in it is still kept, in memory or remembered.
    fn kept_with(&self, mark: &str) -> Vec<String> {
        let app = &self.app;
        let has = |t: &Tokens| t.access.contains(mark) || t.refresh.contains(mark);
        let mut found = Vec::new();
        found.extend(app.tokens.iter().filter(|(_, t)| has(t)).map(|(id, _)| format!("tokens[{id}]")));
        found.extend(app.graph_tokens.iter().filter(|(_, t)| has(t)).map(|(id, _)| format!("graph_tokens[{id}]")));
        found.extend(app.shared_calendar_tokens.iter().filter(|(_, e)| has(&e.tokens)).map(|(id, _)| format!("shared_calendar_tokens[{id}]")));
        found.extend(app.directory_search.access.iter().filter(|(_, t)| has(t)).map(|(id, _)| format!("directory access[{id}]")));
        found.extend(app.secret_cache.iter().filter(|(_, v)| v.contains(mark)).map(|(k, _)| format!("remembered, in memory: {k}")));
        found.extend(self.store.0.lock().unwrap().iter().filter(|(_, v)| v.contains(mark)).map(|(k, _)| format!("the system's store: {k}")));
        found.sort();
        found
    }

    /// The owner's tokens, the shared mailbox's two and the directory's, all marked `mark`.
    fn sign_in_as(&mut self, mark: &str, at: f64) {
        let (app, owner, shared) = (&mut self.app, self.owner, self.shared);
        let refresh = format!("{mark}-ref");
        app.tokens.insert(owner, fresh_tokens(&format!("{mark}-mail"), &refresh, OWNER_A, ""));
        app.graph_tokens.insert(owner, fresh_tokens(&format!("{mark}-graph"), &refresh, OWNER_A, ""));
        app.seed_shared_calendar_token(shared, fresh_tokens(&format!("{mark}-cal"), &refresh, OWNER_A, ""));
        assert!(app.shared_calendar_tokens.contains_key(&shared), "premise: shared calendar token kept");
        shared_mail_token(app, shared, &format!("{mark}-sharedmail"), &refresh, OWNER_A);
        let idx = launch_pending_dir(app, owner, &self.jobs, "help", at);
        answer_dir(&self.jobs, idx, owner, DirectoryOutcome::Found, Some(fresh_tokens(&format!("{mark}-dir"), &refresh, OWNER_A, "")));
        app.directory_tick_at(at + PAUSE_MS + 1.0);
        assert!(app.directory_search.access.contains_key(&owner), "premise: directory token kept");
    }

    /// Ask for everything that goes with a kept token (a directory search, the Inbox checks of
    /// the owner and of the shared mailbox, the calendar) and say what was handed a token marked
    /// `mark`.
    fn handed_with(&mut self, mark: &str, at: f64) -> Vec<String> {
        let (app, owner, shared) = (&mut self.app, self.owner, self.shared);
        let (graph_from, dir_from) = (self.asked.lock().unwrap().len(), self.dir_asked.lock().unwrap().len());
        let before = self.jobs.lock().unwrap().len();
        open_dir_dialog_and_type(app, owner, "sales", at);
        app.directory_tick_at(at + PAUSE_MS);
        if self.jobs.lock().unwrap().len() > before {
            answer_dir(&self.jobs, before, owner, DirectoryOutcome::Unreachable, None);
            app.directory_tick_at(at + PAUSE_MS + 1.0);
        }
        for account in [shared, owner] {
            if app.start_inbox_pass(account, false, true).is_some() {
                app.forget_mail_runs();
            }
        }
        app.calendar_list_due(owner);
        if app.calendar_tick_at(true, crate::now_ms() + at) {
            app.forget_calendar_jobs();
        }
        silence_owner(app, owner);
        let mut handed = Vec::new();
        for ask in self.asked.lock().unwrap().iter().skip(graph_from) {
            if ask.access.contains(mark) || ask.refresh.contains(mark) {
                handed.push(format!("graph job of {}: access {:?}, refresh {:?}", ask.account, ask.access, ask.refresh));
            }
        }
        for (access, refresh) in self.dir_asked.lock().unwrap().iter().skip(dir_from) {
            if access.contains(mark) || refresh.contains(mark) {
                handed.push(format!("directory job: access {access:?}, refresh {refresh:?}"));
            }
        }
        handed
    }

    /// Note in `wrong` every token marked `mark` that is still kept, and every one handed on.
    fn note(&mut self, wrong: &mut Vec<String>, when: &str, mark: &str, at: f64) {
        wrong.extend(self.kept_with(mark).into_iter().map(|what| format!("{when}: still kept: {what}")));
        wrong.extend(self.handed_with(mark, at).into_iter().map(|what| format!("{when}: handed on: {what}")));
    }

    fn oauth(&self) -> (String, String) {
        let oauth = self.app.session.mailbox.account(self.owner).and_then(|a| a.oauth.clone()).unwrap();
        (oauth.tenant, oauth.client_id)
    }
}

/// SEC-B1: a change of the tenant alone, or of the application id alone, by an edit, by Undo and
/// by Redo, leaves no token of the sign-in before it in any store, and hands none to a job.
#[test]
fn a_tenant_only_or_client_id_only_edit_undo_and_redo_leave_no_old_token_anywhere() {
    let mut wrong = Vec::new();
    for (field, value) in [("tenant", "tenant-beta"), ("clientId", "11111111-2222-3333-4444-555555555555")] {
        let mut rig = Rig::new();
        let first = rig.oauth();

        rig.sign_in_as("one", 10_000.0);
        rig.app.execute("account.edit", json!({"id": rig.owner, field: value})).unwrap();
        let second = rig.oauth();
        assert_ne!(first, second, "premise ({field}): the edit changed the sign-in");
        rig.note(&mut wrong, &format!("{field}, edit"), "one", 20_000.0);

        rig.sign_in_as("two", 30_000.0);
        rig.app.execute("edit.undo", json!({})).unwrap();
        assert_eq!(rig.oauth(), first, "premise ({field}): Undo put the sign-in back");
        rig.note(&mut wrong, &format!("{field}, Undo"), "two", 40_000.0);

        rig.sign_in_as("three", 50_000.0);
        rig.app.execute("edit.redo", json!({})).unwrap();
        assert_eq!(rig.oauth(), second, "premise ({field}): Redo made the change again");
        rig.note(&mut wrong, &format!("{field}, Redo"), "three", 60_000.0);
    }
    assert!(wrong.is_empty(), "tokens of the sign-in before, still kept or handed on: {wrong:#?}");
}

/// SEC-B1, the edit itself: another name to sign in with is another sign-in for what was had for
/// shared mailboxes and the directory, which all goes. The account's own tokens stay at the edit
/// (its address, servers and registration are the same: `google_credential_security_tests`),
/// and go with the rest at Undo, which has the one check for every store.
#[test]
fn an_edit_of_the_sign_in_name_alone_lets_go_of_what_was_had_for_shared_mailboxes_and_undo_of_everything() {
    let mut rig = Rig::new();
    // (What the system remembers is kept under the address and the servers, which are the same.)
    rig.app.ui.remember_sign_in = false;
    rig.sign_in_as("one", 10_000.0);
    let before = rig.app.session.mailbox.account(rig.owner).cloned().unwrap();
    rig.app.execute("account.edit", json!({"id": rig.owner, "username": "alice.other@alpha.example"})).unwrap();
    let after = rig.app.session.mailbox.account(rig.owner).cloned().unwrap();
    assert!(!rig.app.same_sign_in_identity(&before, &after), "premise: another name signs in");
    let own = vec![format!("graph_tokens[{}]", rig.owner), format!("tokens[{}]", rig.owner)];
    let kept_at_edit = rig.kept_with("one");
    // And back, by Undo.
    rig.sign_in_as("two", 30_000.0);
    rig.app.execute("edit.undo", json!({})).unwrap();
    assert_eq!(
        rig.app.session.mailbox.account(rig.owner).map(|a| a.username.clone()),
        Some(before.username.clone()),
        "premise: Undo put the name back"
    );
    let kept_at_undo = rig.kept_with("two");
    assert_eq!(
        (kept_at_edit, kept_at_undo),
        (own, Vec::new()),
        "what is still kept of the sign-in before: after the edit (the account's own only), after Undo (nothing)"
    );
}

/// Two owners, each with a shared mailbox whose mail token its run left.
fn two_owners() -> (MailApp, Id, Id, Id, Id, DirJobs) {
    let (mut app, owner, shared, jobs, _) = setup_window();
    let (owner_b, shared_b) = add_owner_b(&mut app);
    shared_mail_token(&mut app, shared, "a-shared-mail", "owner-ref-1", OWNER_A);
    shared_mail_token(&mut app, shared_b, "b-shared-mail", "owner-b-ref", OWNER_B);
    (app, owner, shared, owner_b, shared_b, jobs)
}

/// Note in `wrong` when `shared`'s mail token is still kept after `event`, or when another
/// owner's shared mailbox lost its own.
fn went(wrong: &mut Vec<String>, app: &MailApp, shared: Id, shared_b: Id, event: &str) {
    if app.graph_tokens.contains_key(&shared) {
        wrong.push(format!("{event}: the shared mailbox's mail token is still kept"));
    }
    if app.graph_tokens.get(&shared_b).map(|t| t.access.as_str()) != Some("b-shared-mail") {
        wrong.push(format!("{event}: another owner's shared mailbox lost its token"));
    }
}

/// SEC-B2 (1): the mail token of a shared mailbox is its owner's sign-in's, and goes wherever
/// that sign-in is let go of; another owner's stays.
#[test]
fn a_shared_mailboxs_mail_token_goes_with_every_event_that_lets_go_of_its_owners_sign_in() {
    let mut wrong = Vec::new();
    // The owner's sign-in is edited, and the edit undone and redone.
    let (mut app, owner, shared, _, shared_b, _) = two_owners();
    app.execute("account.edit", json!({"id": owner, "tenant": "tenant-gamma"})).unwrap();
    went(&mut wrong, &app, shared, shared_b, "account.edit of the owner");
    app.tokens.insert(owner, fresh_tokens("owner-mail-2", "owner-ref-2", OWNER_A, ""));
    shared_mail_token(&mut app, shared, "a-shared-mail-2", "owner-ref-2", OWNER_A);
    app.execute("edit.undo", json!({})).unwrap();
    went(&mut wrong, &app, shared, shared_b, "edit.undo of the owner's edit");
    app.tokens.insert(owner, fresh_tokens("owner-mail-3", "owner-ref-3", OWNER_A, ""));
    shared_mail_token(&mut app, shared, "a-shared-mail-3", "owner-ref-3", OWNER_A);
    app.execute("edit.redo", json!({})).unwrap();
    went(&mut wrong, &app, shared, shared_b, "edit.redo of the owner's edit");

    // The shared mailbox is removed, by its id and by its address: its own entry, no sibling's.
    let (mut app, owner, shared, _, shared_b, _) = two_owners();
    let sibling = app.execute("account.addSharedMailbox", json!({"account": owner, "address": SHARED_A2})).unwrap()["id"].as_u64().unwrap();
    shared_mail_token(&mut app, sibling, "a2-shared-mail", "owner-ref-1", OWNER_A);
    app.execute("account.removeSharedMailbox", json!({"id": shared})).unwrap();
    went(&mut wrong, &app, shared, shared_b, "account.removeSharedMailbox by id");
    assert_eq!(app.graph_tokens.get(&sibling).map(|t| t.access.as_str()), Some("a2-shared-mail"), "its sibling keeps its own");
    app.execute("account.removeSharedMailbox", json!({"account": owner, "address": SHARED_A2})).unwrap();
    went(&mut wrong, &app, sibling, shared_b, "account.removeSharedMailbox by address");

    // The owner is removed, and its shared mailboxes with it.
    let (mut app, owner, shared, _, shared_b, _) = two_owners();
    app.execute("account.remove", json!({"id": owner})).unwrap();
    assert!(app.session.mailbox.account(shared).is_none(), "premise: the shared mailbox went with its owner");
    went(&mut wrong, &app, shared, shared_b, "account.remove of the owner");

    // The owner's sign-in is refused, with nobody there and with the owner there; one that only
    // lacks a permission is no refusal.
    for quiet in [true, false] {
        let (mut app, owner, shared, _, shared_b, _) = two_owners();
        app.graph_mail_needs_sign_in(owner, true, false, true);
        assert!(app.graph_tokens.contains_key(&shared), "a permission missing is no refusal: the token stays");
        app.graph_mail_needs_sign_in(owner, quiet, false, false);
        went(&mut wrong, &app, shared, shared_b, &format!("sign-in refused (quiet: {quiet})"));
    }

    // A sign-in of the owner's came to nothing.
    let (mut app, owner, shared, _, shared_b, _) = two_owners();
    app.sign_in_came_to_nothing(owner, "sign in cancelled", false);
    went(&mut wrong, &app, shared, shared_b, "sign-in came to nothing");

    // The directory's search found the owner's sign-in refused.
    let (mut app, owner, shared, _, shared_b, jobs) = two_owners();
    let idx = launch_pending_dir(&mut app, owner, &jobs, "payroll", 50_000.0);
    answer_dir(&jobs, idx, owner, DirectoryOutcome::SignInRefused, None);
    app.directory_tick_at(50_000.0 + PAUSE_MS + 1.0);
    went(&mut wrong, &app, shared, shared_b, "directory sign-in refused");

    // Another mailbox takes this one's place; the data is removed.
    let (mut app, _, _, _, _, _) = two_owners();
    app.execute("file.new", json!({})).unwrap();
    assert!(app.graph_tokens.is_empty(), "file.new: every token went");
    let (mut app, _, _, _, _, _) = two_owners();
    app.services.data_wipe = Some(Box::new(FakeWipe));
    app.execute("ui.removeAllData", json!({})).unwrap();
    assert!(app.graph_tokens.is_empty(), "ui.removeAllData: every token went");

    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// SEC-B2 (1): Undo of adding a shared mailbox takes it away, and its mail token with it.
#[test]
fn undoing_the_adding_of_a_shared_mailbox_lets_go_of_its_mail_token() {
    let (mut app, owner, _, _, _) = setup_window();
    let added = app.execute("account.addSharedMailbox", json!({"account": owner, "address": SHARED_A2})).unwrap()["id"].as_u64().unwrap();
    shared_mail_token(&mut app, added, "a2-shared-mail", "owner-ref-1", OWNER_A);
    app.execute("edit.undo", json!({})).unwrap();
    assert!(app.session.mailbox.account(added).is_none(), "premise: Undo took the shared mailbox away");
    assert!(!app.graph_tokens.contains_key(&added), "its mail token went with it");
}

/// The access token the Inbox check of `shared` goes with now.
fn check_goes_with(app: &mut MailApp, asked: &Asked, shared: Id) -> String {
    let from = asked.lock().unwrap().len();
    assert!(app.start_inbox_pass(shared, false, true).is_some(), "premise: the Inbox check of the shared mailbox starts");
    app.forget_mail_runs();
    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), from + 1, "premise: one job");
    assert_eq!(asked[from].account, shared);
    asked[from].access.clone()
}

fn live(app: &mut MailApp, id: Id) -> &mut mailcraft_model::Account {
    app.session.mailbox.accounts.iter_mut().find(|a| a.id == id).unwrap()
}

/// SEC-B2 (2): the token is used only for the owner's sign-in it was had with. Every clean-up is
/// gone round here (the mailbox is changed underneath the window): the check at the use refuses.
#[test]
fn a_shared_mailboxs_mail_token_is_used_only_for_the_sign_in_it_was_had_with() {
    let window = || {
        let (mut app, owner, shared, _, _) = setup_window();
        seed_inbox(&mut app, shared, SHARED_A);
        let asked = capture_graph(&mut app);
        shared_mail_token(&mut app, shared, "a-shared-mail", "owner-ref-1", OWNER_A);
        (app, owner, shared, asked)
    };
    let mut wrong = Vec::new();
    let mut refused = |app: &mut MailApp, asked: &Asked, shared: Id, why: &str| {
        let went_with = check_goes_with(app, asked, shared);
        if !went_with.is_empty() {
            wrong.push(format!("{why}: the Inbox check went with {went_with:?}"));
        }
    };
    // As it was had: used.
    let (mut app, owner, shared, asked) = window();
    assert_eq!(check_goes_with(&mut app, &asked, shared), "a-shared-mail", "the sign-in it was had with: used");
    // The owner's tenant is another.
    live(&mut app, owner).oauth.as_mut().unwrap().tenant = "tenant-beta".into();
    refused(&mut app, &asked, shared, "another tenant");
    // The owner's application id is another.
    let (mut app, owner, shared, asked) = window();
    live(&mut app, owner).oauth.as_mut().unwrap().client_id = "11111111-2222-3333-4444-555555555555".into();
    refused(&mut app, &asked, shared, "another application id");
    // The shared mailbox is opened through another owner.
    let (mut app, _, shared, asked) = window();
    let (owner_b, _) = add_owner_b(&mut app);
    live(&mut app, shared).shared_of = Some(owner_b);
    refused(&mut app, &asked, shared, "another owner");
    // The id names another shared mailbox.
    let (mut app, _, shared, asked) = window();
    live(&mut app, shared).email = "other-team@alpha.example".into();
    seed_inbox(&mut app, shared, "other-team@alpha.example");
    refused(&mut app, &asked, shared, "another shared mailbox");
    // One that nothing says whose sign-in it was had with (put there by hand) is nobody's.
    let (mut app, _, shared, _, _) = setup_window();
    seed_inbox(&mut app, shared, SHARED_A);
    let asked = capture_graph(&mut app);
    app.graph_tokens.insert(shared, fresh_tokens("unstamped", "owner-ref-1", OWNER_A, ""));
    refused(&mut app, &asked, shared, "a token with nothing to say whose it is");
    assert!(wrong.is_empty(), "a shared mailbox's mail token was used for a sign-in it was not had with: {wrong:#?}");
}
