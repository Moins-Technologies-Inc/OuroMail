// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The installation's application id as part of a sign-in, Undo and Redo of a server change and
//! what the system remembers, the stamps that go with the shared mailboxes' mail tokens, a Graph
//! answer that is nobody's shown as nothing, and the lines no earlier test pinned: a refusal in
//! an answer of the sign-in before, a check given up after the change, a name or a colour edited
//! while a run is under way, an Undo that keeps the sign-in, and the Graph send leg of a mail
//! server's account. Fake in-memory services and invented `.example` addresses only.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::Id;
use mailcraft_model::sync::Tokens;
use serde_json::json;

use super::integration_r2_tests::{Asked, add_owner_b, capture_graph, seed_inbox, shared_mail_token};
use super::integration_tests::{
    FakeWipe, OWNER_A, OWNER_B, SHARED_A, SHARED_A2, SignIns, fresh_tokens, seed_directory_token, seed_shared_cal, setup_window, silence_owner,
    start_inflight_shared_cal_job,
};
use crate::{GraphResult, MailApp, Services};

const NOW: i64 = 1_791_993_600;

#[derive(Clone, Default)]
struct Store(Arc<Mutex<HashMap<String, String>>>);

impl Store {
    fn holds(&self, mark: &str) -> bool {
        self.0.lock().unwrap().values().any(|v| v.contains(mark))
    }
}

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

fn mail_host(app: &mut MailApp) {
    app.services.sync = Some(Box::new(|req: mailcraft_model::sync::SyncRequest| {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(mailcraft_model::sync::SyncResult { account: req.account.id, ..Default::default() });
        rx
    }));
}

/// Whether anything of a sign-in whose tokens have `mark` in them is still held anywhere.
fn holds(app: &MailApp, store: &Store, mark: &str) -> Vec<String> {
    let has = |t: &Tokens| t.access.contains(mark) || t.refresh.contains(mark);
    let mut found = Vec::new();
    found.extend(app.tokens.iter().filter(|(_, t)| has(t)).map(|(id, _)| format!("tokens[{id}]")));
    found.extend(app.graph_tokens.iter().filter(|(_, t)| has(t)).map(|(id, _)| format!("graph_tokens[{id}]")));
    found.extend(app.shared_calendar_tokens.iter().filter(|(_, e)| has(&e.tokens)).map(|(id, _)| format!("shared_calendar_tokens[{id}]")));
    found.extend(app.directory_search.access.iter().filter(|(_, t)| has(t)).map(|(id, _)| format!("directory[{id}]")));
    found.extend(app.secret_cache.iter().filter(|(_, v)| v.contains(mark)).map(|(k, _)| format!("secret_cache {k}")));
    if store.holds(mark) {
        found.push("system store".into());
    }
    found.sort();
    found
}

/// Owner A, whose sign-in takes the installation's application id, with everything a sign-in
/// brings (own tokens, a shared mailbox's mail and calendar tokens, the directory's, the system's
/// store); and owner B, which names an application id of its own, with the same.
struct Two {
    app: MailApp,
    store: Store,
    a: Id,
    sa: Id,
    b: Id,
    sb: Id,
    sign_ins: SignIns,
}

fn two() -> Two {
    let (mut app, a, sa, jobs, sign_ins) = setup_window();
    let store = Store::default();
    app.services.secrets = Some(Arc::new(store.clone()));
    app.ui.remember_sign_in = true;
    app.services.microsoft_client_id = "built-in-app".into();
    let (b, sb) = add_owner_b(&mut app);
    // (B's own calendar is not due: the calendar job a test starts is for A's shared mailbox.)
    silence_owner(&mut app, b);
    app.execute("account.edit", json!({"id": b, "clientId": "b-own-app"})).unwrap();
    app.tokens.insert(b, fresh_tokens("bee-mail", "bee-ref", OWNER_B, "tenant-beta"));
    app.graph_tokens.insert(b, fresh_tokens("bee-graph", "bee-ref", OWNER_B, "tenant-beta"));
    shared_mail_token(&mut app, sb, "bee-sharedmail", "bee-ref", OWNER_B);
    app.remember("refresh", b, "bee-remembered");
    app.tokens.insert(a, fresh_tokens("ay-mail", "ay-ref", OWNER_A, "tenant-alpha"));
    app.graph_tokens.insert(a, fresh_tokens("ay-graph", "ay-ref", OWNER_A, "tenant-alpha"));
    shared_mail_token(&mut app, sa, "ay-sharedmail", "ay-ref", OWNER_A);
    seed_shared_cal(&mut app, sa, "ay-cal");
    seed_directory_token(&mut app, a, &jobs, "ay-dir");
    app.remember("refresh", a, "ay-remembered");
    app.dialog = None;
    Two { app, store, a, sa, b, sb, sign_ins }
}

/// R2-3: a change of the installation's application id is a change of the sign-in of every
/// account that takes it, and of no other: everything had with it goes (memory, the system's
/// store, the jobs under way), and nothing of it goes with the next jobs.
#[test]
fn the_installations_application_id_is_part_of_the_sign_in_of_every_account_that_takes_it() {
    let mut t = two();
    mail_host(&mut t.app);
    let _held = start_inflight_shared_cal_job(&mut t.app, t.sa, SHARED_A, 1_000_000.0);
    seed_inbox(&mut t.app, t.sa, SHARED_A);
    let asked = capture_graph(&mut t.app);
    assert!(t.app.start_inbox_pass(t.sa, true, true).is_some(), "premise: a check of A's shared mailbox is under way");
    t.app.execute("ui.microsoftClientId", json!({"value": "other-app-0000"})).unwrap();
    assert_eq!(holds(&t.app, &t.store, "ay-"), Vec::<String>::new(), "everything had with A's sign-in goes");
    assert!(!t.app.calendar_syncing(), "A's calendar job under way is let go of");
    // B names its own application id: nothing of it changes.
    let b = (t.app.tokens.get(&t.b).map(|x| x.access.clone()), t.app.graph_token(t.b).map(|x| x.access.clone()));
    assert_eq!(b, (Some("bee-mail".into()), Some("bee-graph".into())), "B keeps its own tokens");
    assert_eq!(t.app.graph_token(t.sb).map(|x| x.access.as_str()), Some("bee-sharedmail"), "B's shared mailbox keeps its mail token");
    assert_eq!(t.app.remembered("refresh", t.b).as_deref(), Some("bee-remembered"), "B's remembered sign-in stays");
    // The check of A's shared mailbox answers with the sign-in had with the other application id.
    let _ = asked.lock().unwrap()[0].tx.send(GraphResult {
        account: t.sa,
        tokens: Some(fresh_tokens("ay-late", "ay-late-ref", OWNER_A, "")),
        ..Default::default()
    });
    for _ in 0..3 {
        t.app.poll_graph();
        t.app.inbox_check_tick();
    }
    assert_eq!(holds(&t.app, &t.store, "ay-"), Vec::<String>::new(), "a late answer had with the other application id keeps nothing");
    assert!(t.sign_ins.lock().unwrap().is_empty() && !t.app.signing_in(), "no page for it");
    // The next jobs of A go with nothing of the sign-in before.
    t.app.tokens.insert(t.a, fresh_tokens("", "fresh-ref", OWNER_A, ""));
    let from = asked.lock().unwrap().len();
    for account in [t.sa, t.a] {
        if t.app.start_inbox_pass(account, false, true).is_some() {
            t.app.forget_mail_runs();
        }
    }
    let went: Vec<(String, String)> = asked.lock().unwrap().iter().skip(from).map(|x| (x.access.clone(), x.refresh.clone())).collect();
    assert!(!went.is_empty(), "premise: the next jobs went");
    assert!(went.iter().all(|(access, refresh)| !access.contains("ay-") && !refresh.contains("ay-")), "the next jobs went with {went:?}");
    // The same value again, and an Undo, change nobody's sign-in.
    t.app.tokens.insert(t.a, fresh_tokens("again-mail", "again-ref", OWNER_A, ""));
    t.app.execute("ui.microsoftClientId", json!({"value": "other-app-0000"})).unwrap();
    assert_eq!(t.app.tokens.get(&t.a).map(|x| x.access.as_str()), Some("again-mail"), "the same application id again changes nothing");
    let _ = t.app.execute("edit.undo", json!({}));
    assert_eq!(t.app.ui.microsoft_client_id, "other-app-0000", "Undo is for the mailbox, not this setting");
    // Google's application id is no Microsoft account's.
    t.app.execute("ui.googleClientId", json!({"value": "google-app-0000"})).unwrap();
    assert_eq!(t.app.tokens.get(&t.a).map(|x| x.access.as_str()), Some("again-mail"), "Google's application id leaves Microsoft accounts alone");
    assert_eq!(t.app.tokens.get(&t.b).map(|x| x.access.as_str()), Some("bee-mail"));
}

/// R2-4: Undo and Redo of a change of servers forget what the system remembers for the servers
/// the account no longer goes to, as the edit does; another account's stays.
#[test]
fn undo_and_redo_of_a_server_change_forget_the_other_servers_remembered_sign_in() {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let store = Store::default();
    app.services.secrets = Some(Arc::new(store.clone()));
    app.ui.remember_sign_in = true;
    let id = crate::calendar_sync::testing::microsoft_account(&mut app);
    let (other, _) = add_owner_b(&mut app);
    app.remember("refresh", other, "other-remembered");
    app.remember("refresh", id, "first-server-ref");
    app.execute("account.edit", json!({"id": id, "incoming": {"host": "imap2.mail.example"}})).unwrap();
    assert!(!store.holds("first-server-ref"), "premise: the edit forgets the first server's");
    app.remember("refresh", id, "second-server-ref");
    app.execute("edit.undo", json!({})).unwrap();
    assert!(!store.holds("second-server-ref"), "Undo forgets what is remembered for the server it leaves");
    assert_eq!(app.remembered("refresh", id), None);
    app.remember("refresh", id, "first-again-ref");
    app.execute("edit.redo", json!({})).unwrap();
    assert!(!store.holds("first-again-ref"), "Redo forgets what is remembered for the server it leaves");
    assert_eq!(app.remembered("refresh", other).as_deref(), Some("other-remembered"), "another account's stays");
    assert!(store.holds("other-remembered"));
}

/// R2-5: the stamps go with the shared mailboxes' mail tokens when another mailbox opens and when
/// the data is removed.
#[test]
fn the_shared_mail_stamps_go_with_their_tokens_when_another_mailbox_opens_or_the_data_goes() {
    for event in ["file.new", "ui.removeAllData"] {
        let mut t = two();
        assert_eq!(t.app.shared_mail_stamps.len(), 2, "premise: two stamps");
        if event == "ui.removeAllData" {
            t.app.services.data_wipe = Some(Box::new(FakeWipe));
        }
        t.app.execute(event, json!({})).unwrap();
        assert!(t.app.graph_tokens.is_empty(), "premise ({event}): the tokens went");
        assert!(t.app.shared_mail_stamps.is_empty(), "{event}: the stamps go with them");
    }
}

/// N1: a shared mailbox's token refused takes its stamp with it, by its run and by its Inbox pass;
/// the sibling's token and stamp stay.
#[test]
fn a_refused_shared_mailbox_token_takes_its_stamp_and_leaves_the_siblings_alone() {
    for path in ["run", "inbox pass"] {
        let mut t = two();
        let sibling = t.app.execute("account.addSharedMailbox", json!({"account": t.a, "address": SHARED_A2})).unwrap()["id"].as_u64().unwrap();
        shared_mail_token(&mut t.app, sibling, "sibling-mail", "ay-ref", OWNER_A);
        let refused = GraphResult { account: t.sa, token_refused: true, ..Default::default() };
        if path == "run" {
            t.app.finish_shared_mail(t.sa, true, refused);
        } else {
            t.app.finish_inbox_pass(t.sa, refused);
        }
        assert!(!t.app.graph_tokens.contains_key(&t.sa), "premise ({path}): the token went");
        assert!(!t.app.shared_mail_stamps.contains_key(&t.sa), "{path}: its stamp goes with it");
        assert_eq!(t.app.graph_token(sibling).map(|x| x.access.as_str()), Some("sibling-mail"), "{path}: the sibling keeps its token");
        assert!(
            t.app.shared_mail_stamps.contains_key(&sibling) && t.app.shared_mail_stamps.contains_key(&t.sb),
            "{path}: and the others their stamps"
        );
    }
}

/// N2: a Graph answer that is nobody's (its account went, or its sign-in changed) shows nothing:
/// no status, no problem, no dialog, no page. The same answer of a run whose sign-in is unchanged
/// is shown, as before.
#[test]
fn a_graph_answer_that_is_nobodys_shows_nothing() {
    let mut wrong = Vec::new();
    for whose in ["owner", "shared"] {
        for event in ["nothing", "tenant edit", "account gone"] {
            for kind in ["refused", "errors only", "pieces only"] {
                let mut t = two();
                mail_host(&mut t.app);
                seed_inbox(&mut t.app, t.sa, SHARED_A);
                let asked = capture_graph(&mut t.app);
                let account = if whose == "owner" { t.a } else { t.sa };
                t.app.execute("ui.sync", json!({"account": account})).unwrap();
                assert_eq!(asked.lock().unwrap().len(), 1, "premise ({whose} {event} {kind}): a Graph job went");
                match event {
                    "tenant edit" => drop(t.app.execute("account.edit", json!({"id": t.a, "tenant": "tenant-delta"})).unwrap()),
                    "account gone" if whose == "owner" => drop(t.app.execute("account.remove", json!({"id": t.a})).unwrap()),
                    "account gone" => drop(t.app.execute("account.removeSharedMailbox", json!({"id": t.sa})).unwrap()),
                    _ => {}
                }
                t.app.status_msg = None;
                let problems = t.app.sync_problems.len();
                // (And what a piece of its mail found wrong, as it came.)
                t.app.graph_problems.push("PIECE problem".into());
                let errors = if kind == "pieces only" { Vec::new() } else { vec!["PROBLEM of the run".into()] };
                let mut answer = GraphResult { account, errors, ..Default::default() };
                if kind == "refused" {
                    (answer.sign_in_refused, answer.token_refused) = (true, true);
                }
                let _ = asked.lock().unwrap()[0].tx.send(answer);
                for _ in 0..3 {
                    t.app.poll_graph();
                }
                let status = t.app.status_msg.as_ref().map(|(s, _)| s.clone()).unwrap_or_default();
                let line = format!("status {status:?}, bar {:?}, problems {} -> {}", t.app.session.sync_status, problems, t.app.sync_problems.len());
                let said = !status.is_empty() || !t.app.session.sync_status.is_empty() || t.app.sync_problems.len() > problems;
                if event == "nothing" {
                    if !said {
                        wrong.push(format!("PREMISE {whose} {kind}: the run's own answer showed nothing: {line}"));
                    }
                    continue;
                }
                if said {
                    wrong.push(format!("{whose} | {event} | {kind}: shown: {line}"));
                }
                if t.app.dialog.is_some() || t.app.signing_in() || !t.sign_ins.lock().unwrap().is_empty() {
                    wrong.push(format!("{whose} | {event} | {kind}: a dialog or a page opened"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// R2-6 (`without_sign_in`'s refusals): a refusal in an answer of the sign-in before drops nothing
/// of the sign-in there is now and opens no page.
#[test]
fn a_refusal_in_an_answer_of_the_sign_in_before_drops_nothing_of_the_new_one() {
    for whose in ["owner", "shared"] {
        let mut t = two();
        mail_host(&mut t.app);
        seed_inbox(&mut t.app, t.sa, SHARED_A);
        let asked = capture_graph(&mut t.app);
        let account = if whose == "owner" { t.a } else { t.sa };
        t.app.execute("ui.sync", json!({"account": account})).unwrap();
        t.app.execute("account.edit", json!({"id": t.a, "tenant": "tenant-delta"})).unwrap();
        t.app.tokens.insert(t.a, fresh_tokens("new-mail", "new-ref", OWNER_A, ""));
        t.app.graph_tokens.insert(t.a, fresh_tokens("new-graph", "new-ref", OWNER_A, ""));
        shared_mail_token(&mut t.app, t.sa, "new-shared", "new-ref", OWNER_A);
        let answer = GraphResult { account, sign_in_refused: true, token_refused: true, ..Default::default() };
        let _ = asked.lock().unwrap()[0].tx.send(answer);
        for _ in 0..3 {
            t.app.poll_graph();
        }
        assert_eq!(t.app.graph_tokens.get(&t.a).map(|x| x.access.as_str()), Some("new-graph"), "{whose}: the owner's new Graph token stays");
        assert_eq!(t.app.tokens.get(&t.a).map(|x| x.refresh.as_str()), Some("new-ref"), "{whose}: the owner's new sign-in stays");
        assert_eq!(t.app.graph_token(t.sa).map(|x| x.access.as_str()), Some("new-shared"), "{whose}: the shared mailbox's new token stays");
        assert!(t.sign_ins.lock().unwrap().is_empty() && !t.app.signing_in(), "{whose}: no page for it");
    }
}

/// R2-6 (the check that gives way): a check launched before its owner's sign-in changed and given
/// up after it keeps nothing of its answer.
#[test]
fn a_check_launched_before_the_change_and_given_up_after_it_keeps_nothing() {
    let mut t = two();
    seed_inbox(&mut t.app, t.sa, SHARED_A);
    let asked = capture_graph(&mut t.app);
    assert!(t.app.start_inbox_pass(t.sa, true, true).is_some(), "premise: the Inbox check starts");
    t.app.execute("account.edit", json!({"id": t.a, "tenant": "tenant-beta"})).unwrap();
    t.app.tokens.insert(t.a, fresh_tokens("new-mail", "new-ref", OWNER_A, ""));
    t.app.give_up_inbox_check();
    let _ = asked.lock().unwrap()[0].tx.send(GraphResult {
        account: t.sa,
        tokens: Some(fresh_tokens("LATE-access", "LATE-ref", OWNER_A, "")),
        ..Default::default()
    });
    for _ in 0..3 {
        t.app.inbox_check_tick();
        t.app.poll_graph();
    }
    assert_eq!(holds(&t.app, &t.store, "LATE"), Vec::<String>::new(), "nothing of the sign-in before is kept");
    assert_eq!(t.app.tokens.get(&t.a).map(|x| x.refresh.as_str()), Some("new-ref"), "the sign-in there is now keeps its own refresh token");
}

/// N3 (`graph_job_holds`): a name or a colour edited while a run is under way leaves its answer
/// the account's own: the renewed tokens are kept.
#[test]
fn a_name_or_colour_edited_while_a_run_is_under_way_leaves_its_answer_its_own() {
    for whose in ["owner", "shared"] {
        for (field, value) in [("displayName", "Alice Q"), ("color", "#2E8B57")] {
            let mut t = two();
            mail_host(&mut t.app);
            seed_inbox(&mut t.app, t.sa, SHARED_A);
            let asked: Asked = capture_graph(&mut t.app);
            let account = if whose == "owner" { t.a } else { t.sa };
            t.app.execute("ui.sync", json!({"account": account})).unwrap();
            t.app.execute("account.edit", json!({"id": t.a, field: value})).unwrap();
            let _ = asked.lock().unwrap()[0].tx.send(GraphResult {
                account,
                tokens: Some(fresh_tokens("late-access", "late-ref", OWNER_A, "")),
                ..Default::default()
            });
            t.app.poll_graph();
            assert_eq!(t.app.graph_token(account).map(|x| x.access.as_str()), Some("late-access"), "{whose} {field}: the answer's token is kept");
            assert_eq!(t.app.tokens.get(&t.a).map(|x| x.refresh.as_str()), Some("late-ref"), "{whose} {field}: the renewed refresh token is kept");
        }
    }
}

/// N3 (Undo/Redo and the system's store): Undo of an edit that keeps the sign-in (a name, a
/// colour, another account's name) forgets nobody's remembered sign-in.
#[test]
fn undo_of_an_edit_that_keeps_the_sign_in_keeps_every_remembered_sign_in() {
    let mut t = two();
    for (account, field, value) in [(t.a, "displayName", "Alice Q"), (t.a, "color", "#2E8B57"), (t.b, "displayName", "Bob Q")] {
        t.app.execute("account.edit", json!({"id": account, field: value})).unwrap();
        t.app.execute("edit.undo", json!({})).unwrap();
        assert_eq!(t.app.remembered("refresh", t.a).as_deref(), Some("ay-remembered"), "A's stays ({field})");
        assert_eq!(t.app.remembered("refresh", t.b).as_deref(), Some("bee-remembered"), "B's stays ({field})");
        assert!(t.store.holds("ay-remembered") && t.store.holds("bee-remembered"), "the system's store keeps both ({field})");
        t.app.execute("edit.redo", json!({})).unwrap();
        assert!(t.store.holds("ay-remembered") && t.store.holds("bee-remembered"), "and after Redo ({field})");
    }
}

/// N3 (the send leg's signer): the Graph send leg of a mail server's account that signs in with
/// Microsoft keeps the token it renewed.
#[test]
fn the_graph_send_leg_of_a_mail_server_account_keeps_its_renewed_token() {
    let (mut app, _, _, _, _) = setup_window();
    mail_host(&mut app);
    let id = app
        .execute(
            "account.add",
            json!({"email": "hybrid@alpha.example", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "imap.alpha.example"}}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    app.tokens.insert(id, fresh_tokens("hybrid-mail", "hybrid-ref", "hybrid@alpha.example", ""));
    let asked = capture_graph(&mut app);
    app.execute("mail.new", json!({"to": "friend@alpha.example", "subject": "Synthetic hybrid send", "account": id})).unwrap();
    assert_eq!(app.execute("compose.send", json!({})).unwrap()["queued"], true, "premise: queued");
    assert_eq!(app.execute("ui.sync", json!({"account": id})).unwrap()["pending"], "graph", "premise: the Graph send leg started");
    assert_eq!(asked.lock().unwrap().len(), 1, "premise: one Graph request");
    let renewed = fresh_tokens("renewed-access", "renewed-ref", "hybrid@alpha.example", "");
    let _ = asked.lock().unwrap()[0].tx.send(GraphResult { account: id, tokens: Some(renewed), ..Default::default() });
    app.poll_graph();
    assert_eq!(app.graph_token(id).map(|x| x.access.as_str()), Some("renewed-access"), "the send leg keeps its renewed token");
}
