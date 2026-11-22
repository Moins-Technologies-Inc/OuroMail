// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Add Shared Mailbox's search on the host's thread: the token it takes, what it sends and when it
//! sends nothing; and the whole way from the window's typing to the Graph stand-in and back.
//! Nothing here leaves this machine, nobody is signed in, and every name and address is invented.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::directory::{DirectoryAnswer, DirectoryEntry, DirectoryOutcome, DirectoryQuery};
use mailcraft_model::sync::Tokens;
use mailcraft_model::{Account, AccountKind, Id, OAuth};
use mailcraft_net::NetError;
use mailcraft_net::graph::standin::{StandIn, TOKEN};
use mailcraft_net::oauth::{GraphTokens, Resource};
use mailcraft_ui_egui::directory_search::{DirectoryJob, PAUSE_MS};
use mailcraft_ui_egui::{MailApp, Services};
use serde_json::json;

use super::run_with;

fn owner() -> Account {
    Account {
        id: 7,
        email: "noor@example.org".into(),
        kind: AccountKind::Microsoft,
        oauth: Some(OAuth { provider: mailcraft_model::OAuthProvider::Microsoft, client_id: "client-1".into(), ..Default::default() }),
        ..Default::default()
    }
}

fn job(access: &str, refresh: &str) -> DirectoryJob {
    let owner = owner();
    DirectoryJob {
        oauth: owner.oauth.clone().unwrap_or_default(),
        owner,
        refresh: refresh.into(),
        access: access.into(),
        query: DirectoryQuery { account: 7, text: "help".into(), ticket: 3 },
        cancel: Arc::new(AtomicBool::new(false)),
    }
}

fn graph_tokens(access: &str) -> GraphTokens {
    GraphTokens {
        tokens: Tokens { access: access.into(), refresh: "re-fresh-2".into(), ..Default::default() },
        extras_missing: true,
        covers_favorites: false,
    }
}

/// What was renewed and searched with, in turn.
type Seen = Arc<Mutex<Vec<String>>>;

fn found(query: &DirectoryQuery) -> DirectoryAnswer {
    let entries = vec![DirectoryEntry::found("Help Desk", "helpdesk@example.org").unwrap()];
    DirectoryAnswer { account: query.account, ticket: query.ticket, entries, outcome: DirectoryOutcome::Found, ..Default::default() }
}

#[test]
fn a_token_of_this_session_is_searched_with_and_none_is_asked_for() {
    let seen: Seen = Arc::default();
    let (r, s) = (seen.clone(), seen.clone());
    let renew = move |_: &OAuth, _: &str, _: Resource| -> Result<GraphTokens, NetError> {
        r.lock().unwrap().push("renew".into());
        Ok(graph_tokens("new"))
    };
    let search = move |token: &str, _: &Account, q: &DirectoryQuery, _: &AtomicBool| {
        s.lock().unwrap().push(format!("search {token}"));
        found(q)
    };
    let out = run_with(job("kept", "re-fresh"), &renew, &search);
    assert_eq!(*seen.lock().unwrap(), ["search kept"]);
    assert_eq!((out.answer.outcome, out.answer.ticket, out.answer.entries.len()), (DirectoryOutcome::Found, 3, 1));
    assert!(out.tokens.is_none());
}

#[test]
fn without_a_token_one_is_asked_for_the_directory_first_and_handed_back() {
    let seen: Seen = Arc::default();
    let (r, s) = (seen.clone(), seen.clone());
    let renew = move |_: &OAuth, refresh: &str, mail: Resource| -> Result<GraphTokens, NetError> {
        r.lock().unwrap().push(format!("renew {refresh} {mail:?}"));
        Ok(graph_tokens("dir-access"))
    };
    let search = move |token: &str, _: &Account, q: &DirectoryQuery, _: &AtomicBool| {
        s.lock().unwrap().push(format!("search {token}"));
        found(q)
    };
    let out = run_with(job("", "re-fresh"), &renew, &search);
    assert_eq!(*seen.lock().unwrap(), ["renew re-fresh GraphMail", "search dir-access"]);
    assert_eq!(out.tokens.map(|t| (t.access, t.refresh)), Some(("dir-access".into(), "re-fresh-2".into())));
}

#[test]
fn a_sign_in_that_doesnt_cover_the_directory_sends_nothing_of_the_text() {
    let searched = Arc::new(Mutex::new(0));
    let s = searched.clone();
    let renew = |_: &OAuth, _: &str, _: Resource| -> Result<GraphTokens, NetError> { Err(NetError::ConsentNeeded("not given".into())) };
    let search = move |_: &str, _: &Account, q: &DirectoryQuery, _: &AtomicBool| {
        *s.lock().unwrap() += 1;
        found(q)
    };
    let out = run_with(job("", "re-fresh"), &renew, &search);
    assert_eq!((out.answer.outcome, *searched.lock().unwrap()), (DirectoryOutcome::ConsentNeeded, 0));
    // No refresh token at all: nothing is asked of anybody.
    let out = run_with(job("", ""), &renew, &search);
    assert_eq!((out.answer.outcome, *searched.lock().unwrap()), (DirectoryOutcome::SignInRefused, 0));
}

#[test]
fn a_dialog_closed_before_or_while_the_token_comes_has_nothing_sent() {
    let searched = Arc::new(Mutex::new(0));
    let s = searched.clone();
    let search = move |_: &str, _: &Account, q: &DirectoryQuery, _: &AtomicBool| {
        *s.lock().unwrap() += 1;
        found(q)
    };
    let closed = job("kept", "re-fresh");
    closed.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    let nothing = |_: &OAuth, _: &str, _: Resource| -> Result<GraphTokens, NetError> { Ok(graph_tokens("new")) };
    assert_eq!(run_with(closed, &nothing, &search).answer.outcome, DirectoryOutcome::NothingAsked);
    let closing = job("", "re-fresh");
    let flag = closing.cancel.clone();
    let renew = move |_: &OAuth, _: &str, _: Resource| -> Result<GraphTokens, NetError> {
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(graph_tokens("new"))
    };
    let out = run_with(closing, &renew, &search);
    assert_eq!(out.answer.outcome, DirectoryOutcome::NothingAsked);
    assert!(out.tokens.is_some(), "the token is kept all the same");
    assert_eq!(*searched.lock().unwrap(), 0);
}

#[test]
fn a_token_no_longer_taken_is_renewed_once_and_searched_with_again() {
    let seen: Seen = Arc::default();
    let s = seen.clone();
    let renew = |_: &OAuth, _: &str, _: Resource| -> Result<GraphTokens, NetError> { Ok(graph_tokens("new")) };
    let search = move |token: &str, _: &Account, q: &DirectoryQuery, _: &AtomicBool| {
        s.lock().unwrap().push(token.to_string());
        DirectoryAnswer { account: q.account, ticket: q.ticket, outcome: DirectoryOutcome::SignInRefused, ..Default::default() }
    };
    let out = run_with(job("old", "re-fresh"), &renew, &search);
    assert_eq!(*seen.lock().unwrap(), ["old", "new"], "once more, and no more");
    assert_eq!(out.answer.outcome, DirectoryOutcome::SignInRefused);
}

// ---- the window, the host's job and the Graph stand-in ----

const NOW: i64 = 1_791_993_600;

/// A window with a work account signed in, whose directory search runs the host's job on this
/// thread against the stand-in (the token for the directory is the stand-in's own).
fn window() -> (MailApp, Id, Arc<Mutex<usize>>) {
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    let account = app
        .execute("account.add", json!({"email": "noor@example.org", "kind": "microsoft", "auth": "oauth2", "directory": "organisation"}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    app.seed_graph_token(
        account,
        Tokens { access: "mail".into(), refresh: "ref-1".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() },
    );
    let renewals = Arc::new(Mutex::new(0));
    let counted = renewals.clone();
    app.services.graph_directory = Some(Box::new(move |job: DirectoryJob| {
        let counted = counted.clone();
        let renew = move |_: &OAuth, _: &str, _: Resource| -> Result<GraphTokens, NetError> {
            *counted.lock().unwrap() += 1;
            Ok(GraphTokens {
                tokens: Tokens { access: TOKEN.into(), refresh: "ref-2".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() },
                extras_missing: true,
                covers_favorites: false,
            })
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = tx.send(run_with(job, &renew, &mailcraft_net::graph::search_directory_unless));
        rx
    }));
    app.execute("ui.dialog", json!({"name": "sharedMailbox", "account": account})).unwrap();
    (app, account, renewals)
}

fn type_in(app: &mut MailApp, text: &str) {
    app.execute("ui.searchDirectory", json!({"text": text})).unwrap();
}

#[test]
fn what_the_owner_types_reaches_the_stand_in_after_the_pause_and_its_answer_the_dialog() {
    let server = StandIn::start().unwrap();
    {
        let mut state = server.state();
        state.add_directory_user("Help Desk", "helpdesk@example.org", "helpdesk@example.org");
        state.add_directory_user("Ada Lovelace", "ada@example.org", "ada@example.org");
    }
    let _route = server.route();
    let (mut app, _, renewals) = window();
    // One character: nothing goes.
    type_in(&mut app, "h");
    app.directory_tick_at(1_000_000.0);
    assert!(server.state().requests.iter().all(|r| r.path.last().map(String::as_str) != Some("users")), "nothing asked for one letter");
    type_in(&mut app, "help");
    app.directory_tick_at(1_000_000.0 + PAUSE_MS);
    app.directory_tick_at(1_000_000.0 + PAUSE_MS + 1.0);
    let asked: Vec<String> = server.state().requests.iter().filter(|r| r.target.contains("/v1.0/users")).map(|r| r.target.clone()).collect();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(asked[0].contains("help") && asked[0].contains("top=15"), "{}", asked[0]);
    assert_eq!(*renewals.lock().unwrap(), 1, "a token for the directory first");
    let Some(mailcraft_ui_egui::dialogs::Dialog::SharedMailbox { search, .. }) = &app.dialog else { panic!("the dialog is open") };
    let shown: Vec<(&str, &str)> = search.entries().iter().map(|e| (e.name.as_str(), e.address.as_str())).collect();
    assert_eq!(shown, [("Help Desk", "helpdesk@example.org")]);
    // Typed again: the token of this session is used, and no other is asked for.
    type_in(&mut app, "ada");
    app.directory_tick_at(2_000_000.0);
    app.directory_tick_at(2_000_001.0);
    assert_eq!(*renewals.lock().unwrap(), 1);
    assert_eq!(server.state().requests.iter().filter(|r| r.target.contains("/v1.0/users")).count(), 2);
}

/// The search is handed the job's own call-off, so that a dialog closed while it waits to ask
/// once more has nothing more sent (S-F3).
#[test]
fn the_search_is_handed_the_jobs_call_off_flag() {
    let job = job("kept", "re-fresh");
    let flag = job.cancel.clone();
    let seen = Arc::new(Mutex::new(None));
    let s = seen.clone();
    let renew = |_: &OAuth, _: &str, _: Resource| -> Result<GraphTokens, NetError> { Ok(graph_tokens("new")) };
    let search = move |_: &str, _: &Account, q: &DirectoryQuery, cancel: &AtomicBool| {
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
        *s.lock().unwrap() = Some(cancel.load(std::sync::atomic::Ordering::Relaxed));
        found(q)
    };
    run_with(job, &renew, &search);
    assert_eq!(*seen.lock().unwrap(), Some(true), "the search sees the job's own call-off");
}
