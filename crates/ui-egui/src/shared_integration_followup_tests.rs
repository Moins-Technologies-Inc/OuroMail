// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window's read of the system's credential store that moves a sign-in from its old name to
//! the name bound to its servers, while the sign-in is let go of (a tenant edit, a change of the
//! installation's application id, Remove all data): the move never leaves the old secret in the
//! store under the name let go of, whether it wrote before or after the name was let go of; and a
//! read keeps everything nothing let go of. A read dropped by File › Open writes nothing back,
//! and a sign-in remembered again under a name let go of while a read ran stays in the store.
//! Fake in-memory stores and invented `.example` addresses only.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;

use super::integration_r2_tests::add_owner_b;
use super::integration_tests::{FakeWipe, OWNER_A, setup_window};
use crate::MailApp;
use crate::secret_read::SecretRead;

/// What lets go of the account's sign-in while the read waits on the store.
const EVENTS: [&str; 3] = ["tenant edit", "the installation's application id", "the data removed"];

/// What changes while the read waits on the store and lets go of nothing of the owner's sign-in.
const KEEPS: [&str; 3] = ["nothing", "a display name", "another owner's tenant edit"];

/// Where the store's own thread (`secret-read`) stops until it is let go.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// Inside its read of the old name, the secret in hand.
    InGetOfOld,
    /// Inside its write of the new name, before anything is written.
    InSetOfNew,
}

/// A store whose background reader stops once, where [`Stop`] says, until [`Gated::release`];
/// it notes every name the reader writes, and when the read is over.
#[derive(Clone)]
struct Gated {
    live: Arc<Mutex<HashMap<String, String>>>,
    stop: Stop,
    // 0 armed, 1 the reader waits, 2 let go
    gate: Arc<(Mutex<u8>, Condvar)>,
    written: Arc<Mutex<Vec<String>>>,
    over: Arc<AtomicBool>,
}

impl Gated {
    fn new(stop: Stop) -> Self {
        Self { live: Default::default(), stop, gate: Default::default(), written: Default::default(), over: Default::default() }
    }

    fn reader() -> bool {
        std::thread::current().name() == Some("secret-read")
    }

    fn wait_here(&self) {
        let (m, c) = &*self.gate;
        let mut g = m.lock().unwrap();
        if *g == 0 {
            *g = 1;
            c.notify_all();
            while *g == 1 {
                g = c.wait(g).unwrap();
            }
        }
    }

    /// Whether the reader stopped where it was to, within a few seconds.
    fn stopped(&self) -> bool {
        let (m, c) = &*self.gate;
        let g = m.lock().unwrap();
        let (g, _) = c.wait_timeout_while(g, Duration::from_secs(5), |g| *g != 1).unwrap();
        *g == 1
    }

    fn release(&self) {
        *self.gate.0.lock().unwrap() = 2;
        self.gate.1.notify_all();
    }

    /// Wait until the reader is through (its last call), a few seconds at most.
    fn read_over(&self) -> bool {
        let waited = Instant::now();
        while !self.over.load(Ordering::SeqCst) && waited.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(2));
        }
        self.over.load(Ordering::SeqCst)
    }

    fn value(&self, key: &str) -> Option<String> {
        self.live.lock().unwrap().get(key).cloned()
    }

    fn holds(&self, mark: &str) -> Vec<String> {
        self.live.lock().unwrap().iter().filter(|(_, v)| v.contains(mark)).map(|(k, _)| k.clone()).collect()
    }
}

impl crate::SecretStore for Gated {
    fn get(&self, key: &str) -> Option<String> {
        let v = self.live.lock().unwrap().get(key).cloned();
        // (An old name has no `|`: it is the address alone.)
        if Self::reader() && self.stop == Stop::InGetOfOld && !key.contains('|') && v.is_some() {
            self.wait_here();
        }
        v
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), mailcraft_host::SecretError> {
        if Self::reader() {
            if self.stop == Stop::InSetOfNew {
                self.wait_here();
            }
            self.written.lock().unwrap().push(key.to_string());
        }
        self.live.lock().unwrap().insert(key.to_string(), secret.to_string());
        Ok(())
    }
    fn delete(&self, key: &str) {
        self.live.lock().unwrap().remove(key);
    }
    fn take_trouble(&self) -> Option<mailcraft_host::StoreTrouble> {
        if Self::reader() {
            self.over.store(true, Ordering::SeqCst);
        }
        None
    }
}

/// A window whose first start finds its owner's sign-in under the old name only, so that its read
/// of the store moves it; remembering is on. Gives the window, the owner, the new and the old name.
fn moving(store: &Gated) -> (MailApp, u64, String, String) {
    let (mut app, owner, _shared, _jobs, _sign_ins) = setup_window();
    app.services.secrets = Some(Arc::new(store.clone()));
    app.ui.remember_sign_in = true;
    app.services.microsoft_client_id = "built-in-app".into();
    app.ui.sign_ins_bound = false;
    app.secrets_asked = None;
    let account = app.session.mailbox.account(owner).cloned().unwrap();
    let key = MailApp::secret_key("refresh", &account);
    let old = MailApp::legacy_secret_key("refresh", OWNER_A);
    store.live.lock().unwrap().insert(old.clone(), "OLDNAME-ref".into());
    app.tokens.remove(&owner);
    app.graph_tokens.remove(&owner);
    (app, owner, key, old)
}

fn happen(app: &mut MailApp, owner: u64, event: &str) {
    match event {
        "tenant edit" => drop(app.execute("account.edit", json!({"id": owner, "tenant": "tenant-new"})).unwrap()),
        "the installation's application id" => drop(app.execute("ui.microsoftClientId", json!({"value": "other-app-0000"})).unwrap()),
        "the data removed" => {
            app.services.data_wipe = Some(Box::new(FakeWipe));
            app.execute("ui.removeAllData", json!({})).unwrap();
        }
        "a display name" => drop(app.execute("account.edit", json!({"id": owner, "displayName": "Someone Else"})).unwrap()),
        "another owner's tenant edit" => {
            let (other, _) = add_owner_b(app);
            app.execute("account.edit", json!({"id": other, "tenant": "tenant-gamma"})).unwrap();
        }
        _ => {}
    }
}

/// What is left of a moved sign-in that was let go of: in the store (under any name), in memory,
/// or noted as remembered.
fn left_of_it(app: &MailApp, store: &Gated, key: &str, event: &str) -> Vec<String> {
    let mut left = Vec::new();
    for name in store.holds("OLDNAME").into_iter().chain(store.holds("MOVED")) {
        left.push(format!("{event}: the store holds it under {name}"));
    }
    if app.secret_cache.contains_key(key) {
        left.push(format!("{event}: in memory"));
    }
    if app.ui.remembered_keys.iter().any(|k| k == key) {
        left.push(format!("{event}: the name is noted as remembered"));
    }
    left
}

/// Poll until the window has taken the read's answer in (a few seconds at most).
fn take_the_answer(app: &mut MailApp) {
    let waited = Instant::now();
    while app.secret_read.is_some() && waited.elapsed() < Duration::from_secs(5) {
        app.poll_secret_read();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(app.secret_read.is_none(), "premise: the read came back");
}

/// R3-1, the reading thread: a name let go of while the read waited on the old one is not
/// written at all, and nothing of the old secret is left anywhere once the read is over.
#[test]
fn a_moving_read_writes_no_name_let_go_of_while_it_waited() {
    let mut wrong = Vec::new();
    for event in EVENTS {
        let store = Gated::new(Stop::InGetOfOld);
        let (mut app, owner, key, old) = moving(&store);
        app.secrets_tick();
        assert!(app.secret_read.is_some() && store.stopped(), "premise ({event}): the read waits inside the old name");
        happen(&mut app, owner, event);
        assert_eq!((store.value(&key), store.value(&old)), (None, None), "premise ({event}): the store let go of both names");
        store.release();
        assert!(store.read_over(), "premise ({event}): the read ended");
        if store.written.lock().unwrap().contains(&key) {
            wrong.push(format!("{event}: the read wrote the name let go of"));
        }
        take_the_answer(&mut app);
        wrong.extend(left_of_it(&app, &store, &key, event));
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// R3-1, the reading thread: a write of the new name that was let go of while it was being made
/// is deleted again by the reader itself, before the window takes its answer.
#[test]
fn a_moving_read_deletes_what_it_wrote_as_the_name_was_let_go_of() {
    let mut wrong = Vec::new();
    for event in EVENTS {
        let store = Gated::new(Stop::InSetOfNew);
        let (mut app, owner, key, _old) = moving(&store);
        app.secrets_tick();
        assert!(app.secret_read.is_some() && store.stopped(), "premise ({event}): the read waits inside the write of the new name");
        happen(&mut app, owner, event);
        store.release();
        assert!(store.read_over(), "premise ({event}): the read ended");
        if let Some(left) = store.value(&key) {
            wrong.push(format!("{event}: the reader left its write of the name let go of in the store: {left}"));
        }
        take_the_answer(&mut app);
        wrong.extend(left_of_it(&app, &store, &key, event));
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Over-clearing: a moving read during which nothing of the owner's sign-in was let go of moves
/// it, wherever it waited, and the window takes it.
#[test]
fn a_moving_read_moves_the_sign_in_when_nothing_of_it_was_let_go_of() {
    for stop in [Stop::InGetOfOld, Stop::InSetOfNew] {
        for change in KEEPS {
            let store = Gated::new(stop);
            let (mut app, owner, key, old) = moving(&store);
            app.secrets_tick();
            assert!(app.secret_read.is_some() && store.stopped(), "premise ({change}): the read waits");
            happen(&mut app, owner, change);
            store.release();
            take_the_answer(&mut app);
            assert_eq!((store.value(&key).as_deref(), store.value(&old)), (Some("OLDNAME-ref"), None), "{change}: moved");
            assert_eq!(app.remembered("refresh", owner).as_deref(), Some("OLDNAME-ref"), "{change}: taken");
            assert!(app.ui.remembered_keys.contains(&key), "{change}: noted");
            assert!(app.ui.sign_ins_bound, "{change}: the move is done");
        }
    }
}

/// A window with a read of the store under way whose answer the test gives ([`SecretRead`]), so
/// that the window's side is seen alone; the store holds nothing yet.
fn reading() -> (MailApp, u64, String, Gated, mpsc::Sender<SecretRead>) {
    let store = Gated::new(Stop::InGetOfOld);
    let (mut app, owner, key, old) = moving(&store);
    store.live.lock().unwrap().remove(&old);
    let (send, receive) = mpsc::channel();
    app.secret_read = Some((app.session.openings(), receive));
    (app, owner, key, store, send)
}

fn moved_to(key: &str) -> SecretRead {
    SecretRead { answers: vec![(key.to_string(), Some("MOVED-ref".into()))], moved: vec![key.to_string()], migrated: true, trouble: None }
}

/// R3-1, the window: what the read says it moved to a name let go of while it ran (or to any
/// name, once the data is removed) is deleted from the store again when its answer comes, and
/// is neither taken nor noted.
#[test]
fn the_answer_of_a_moving_read_deletes_again_what_it_moved_to_a_name_let_go_of() {
    let mut wrong = Vec::new();
    for event in EVENTS {
        let (mut app, owner, key, store, send) = reading();
        happen(&mut app, owner, event);
        // (The reader's write landed after the window let the name go.)
        store.live.lock().unwrap().insert(key.clone(), "MOVED-ref".into());
        send.send(moved_to(&key)).unwrap();
        app.poll_secret_read();
        assert!(app.secret_read.is_none(), "premise ({event}): the answer was taken in");
        wrong.extend(left_of_it(&app, &store, &key, event));
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Over-clearing: what the read moved to a name nothing let go of stays in the store, and is
/// taken and noted.
#[test]
fn the_answer_of_a_moving_read_keeps_what_it_moved_to_a_name_not_let_go_of() {
    for change in KEEPS {
        let (mut app, owner, key, store, send) = reading();
        happen(&mut app, owner, change);
        store.live.lock().unwrap().insert(key.clone(), "MOVED-ref".into());
        send.send(moved_to(&key)).unwrap();
        app.poll_secret_read();
        assert_eq!(store.value(&key).as_deref(), Some("MOVED-ref"), "{change}: the store");
        assert_eq!(app.remembered("refresh", owner).as_deref(), Some("MOVED-ref"), "{change}: taken");
        assert!(app.ui.remembered_keys.contains(&key), "{change}: noted");
    }
}

/// R3-5 (mutant M24): a refresh token the provider refused, let go of while a read of the store
/// was under way, is not taken back from what the read brings.
#[test]
fn a_refused_refresh_token_let_go_of_while_a_read_ran_is_not_taken_back() {
    let store = Gated::new(Stop::InSetOfNew);
    let mut app = MailApp::new(
        mailcraft_engine::Session::at(mailcraft_engine::sample::mailbox(1_791_993_600, 0), 1_791_993_600, 0),
        crate::Services::default(),
    );
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    app.services.secrets = Some(Arc::new(store.clone()));
    app.ui.remember_sign_in = true;
    app.ui.sign_ins_bound = true;
    let google = app
        .execute(
            "account.add",
            json!({
                "email": "gail@gmail.example", "auth": "oauth2", "provider": "google",
                "incoming": {"host": "imap.gmail.com", "port": 993, "security": "tls"},
                "outgoing": {"host": "smtp.gmail.com", "port": 465, "security": "tls"}
            }),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let account = app.session.mailbox.account(google).cloned().unwrap();
    let key = MailApp::secret_key("refresh", &account);
    store.live.lock().unwrap().insert(key.clone(), "REFUSED-ref".into());
    app.secrets_asked = None;
    // (No move: the reader is through before the provider's answer comes; the answer waits.)
    app.secrets_tick();
    assert!(app.secret_read.is_some() && store.read_over(), "premise: the read has its answer, not yet taken");
    app.services.sign_in = Some(Box::new(|_req| {
        let (tx, rx) = mpsc::channel();
        tx.send(Err(crate::approval::GOOGLE_REAUTH_REQUIRED.into())).unwrap();
        rx
    }));
    let oauth = account.oauth.clone().unwrap();
    app.begin_sign_in(google, oauth, &account.email, "REFUSED-ref".into(), true, false).unwrap();
    app.poll_sign_in();
    assert_eq!(store.value(&key), None, "premise: the refused token was let go of in the store");
    take_the_answer(&mut app);
    assert_eq!(app.secret_cache.get(&key), None, "the refused token is taken back from the read");
    assert_eq!(app.remembered("refresh", google), None, "nothing to hand a job");
}

/// CORR F2 (mutant K10): a read dropped because another mailbox took the place of the one it was
/// for, after a name was let go of while it ran, leaves nothing let go of for the next read.
#[test]
fn a_read_dropped_for_another_mailbox_leaves_nothing_let_go_of_for_the_next_one() {
    let store = Gated::new(Stop::InSetOfNew);
    let (mut app, owner, _shared, _jobs, _sign_ins) = setup_window();
    app.services.secrets = Some(Arc::new(store.clone()));
    app.ui.remember_sign_in = true;
    app.ui.sign_ins_bound = true;
    app.services.microsoft_client_id = "built-in-app".into();
    app.secrets_asked = None;
    let account = app.session.mailbox.account(owner).cloned().unwrap();
    let key = MailApp::secret_key("refresh", &account);
    app.tokens.remove(&owner);
    app.graph_tokens.remove(&owner);
    app.secrets_tick();
    assert!(app.secret_read.is_some(), "premise: a read is under way");
    // The installation's id lets the name go (the account is the same, so is its name).
    happen(&mut app, owner, "the installation's application id");
    // The owner signs in again, and remembers it; then the same mailbox is opened again.
    app.remember("refresh", owner, "AGAIN-ref");
    assert_eq!(store.value(&key).as_deref(), Some("AGAIN-ref"), "premise: remembered");
    let same = app.session.mailbox.clone();
    app.session.replace_mailbox(same);
    app.drop_secrets_if_replaced();
    assert!(app.secret_read.is_none() && app.secret_cache.is_empty(), "premise: the read went with the mailbox, unpolled");
    if app.is_unconfirmed() {
        app.execute("ui.confirmServers", json!({})).unwrap();
    }
    app.secrets_tick();
    assert!(app.secret_read.is_some(), "premise: the next read is under way");
    take_the_answer(&mut app);
    assert_eq!(app.remembered("refresh", owner).as_deref(), Some("AGAIN-ref"), "the next read takes what it brings");
}

/// Where the store stops its reader, once, in the move of the owner's refresh token from its old
/// name to its new one (the security review's probes, MFU-S1 and MFU-S2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum At {
    /// In its read of the old name: the secret in hand, nothing written.
    GetOld,
    /// In its write of the new name, before anything is written.
    SetBefore,
    /// In its write of the new name, after it is written (before the reader looks again).
    SetAfter,
    /// In its read of the new name once the move is done (written, looked at, old name deleted).
    GetNew,
}

/// A store whose reader stops once where [`At`] says, by the exact old and new names of the
/// owner's refresh token, until [`Paused::release`]; it counts the reads that ended, and can
/// refuse the window's deletes of the new name.
#[derive(Clone)]
struct Paused {
    live: Arc<Mutex<HashMap<String, String>>>,
    at: At,
    new: String,
    old: String,
    // 0 armed, 1 the reader waits, 2 let go
    gate: Arc<(Mutex<u8>, Condvar)>,
    finished: Arc<AtomicUsize>,
    refuse_delete: Arc<AtomicBool>,
}

impl Paused {
    fn wait_here(&self) {
        let (m, c) = &*self.gate;
        let mut g = m.lock().unwrap();
        if *g == 0 {
            *g = 1;
            c.notify_all();
            while *g == 1 {
                g = c.wait(g).unwrap();
            }
        }
    }

    fn stopped(&self) -> bool {
        let (m, c) = &*self.gate;
        let g = m.lock().unwrap();
        let (g, _) = c.wait_timeout_while(g, Duration::from_secs(5), |g| *g != 1).unwrap();
        *g == 1
    }

    fn release(&self) {
        *self.gate.0.lock().unwrap() = 2;
        self.gate.1.notify_all();
    }

    /// Whether `n` reads have ended, within a few seconds.
    fn finished_at_least(&self, n: usize) -> bool {
        let waited = Instant::now();
        while self.finished.load(Ordering::SeqCst) < n && waited.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(2));
        }
        self.finished.load(Ordering::SeqCst) >= n
    }

    fn value(&self, key: &str) -> Option<String> {
        self.live.lock().unwrap().get(key).cloned()
    }

    fn holds(&self, mark: &str) -> Vec<String> {
        self.live.lock().unwrap().iter().filter(|(_, v)| v.contains(mark)).map(|(k, _)| k.clone()).collect()
    }
}

impl crate::SecretStore for Paused {
    fn get(&self, key: &str) -> Option<String> {
        let v = self.live.lock().unwrap().get(key).cloned();
        if Gated::reader() {
            if self.at == At::GetOld && key == self.old && v.is_some() {
                self.wait_here();
            }
            if self.at == At::GetNew && key == self.new {
                self.wait_here();
            }
        }
        v
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), mailcraft_host::SecretError> {
        let mine = Gated::reader() && key == self.new;
        if mine && self.at == At::SetBefore {
            self.wait_here();
        }
        self.live.lock().unwrap().insert(key.to_string(), secret.to_string());
        if mine && self.at == At::SetAfter {
            self.wait_here();
        }
        Ok(())
    }
    fn delete(&self, key: &str) {
        if !Gated::reader() && key == self.new && self.refuse_delete.load(Ordering::SeqCst) {
            return;
        }
        self.live.lock().unwrap().remove(key);
    }
    fn take_trouble(&self) -> Option<mailcraft_host::StoreTrouble> {
        if Gated::reader() {
            self.finished.fetch_add(1, Ordering::SeqCst);
        }
        None
    }
}

/// [`moving`] with a [`Paused`] store: the first start finds the owner's sign-in under its old
/// name only. Gives the window, the owner and the store.
fn paused_moving(at: At) -> (MailApp, u64, Paused) {
    let (mut app, owner, _shared, _jobs, _sign_ins) = setup_window();
    let account = app.session.mailbox.account(owner).cloned().unwrap();
    let (new, old) = (MailApp::secret_key("refresh", &account), MailApp::legacy_secret_key("refresh", OWNER_A));
    let store = Paused {
        live: Default::default(),
        at,
        new,
        old: old.clone(),
        gate: Default::default(),
        finished: Default::default(),
        refuse_delete: Default::default(),
    };
    store.live.lock().unwrap().insert(old, "OLDNAME-ref".into());
    app.services.secrets = Some(Arc::new(store.clone()));
    app.ui.remember_sign_in = true;
    app.services.microsoft_client_id = "built-in-app".into();
    app.ui.sign_ins_bound = false;
    app.secrets_asked = None;
    app.tokens.remove(&owner);
    app.graph_tokens.remove(&owner);
    (app, owner, store)
}

/// Poll until no read is under way (a few seconds at most).
fn take_soon(app: &mut MailApp) {
    let waited = Instant::now();
    while app.secret_read.is_some() && waited.elapsed() < Duration::from_secs(5) {
        app.poll_secret_read();
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Everything left of the old secret: in the store under any name, in memory, noted as
/// remembered, and in what the next read of the store and the next automatic-replies job of the
/// account go with.
fn old_secret_left(app: &mut MailApp, owner: u64, store: &Paused, data_removed: bool) -> Vec<String> {
    let mut left: Vec<String> = store.holds("OLDNAME").into_iter().map(|k| format!("the store holds it under {k}")).collect();
    if app.secret_cache.values().any(|v| v.contains("OLDNAME")) {
        left.push("in memory".into());
    }
    if app.ui.remembered_keys.contains(&store.new) && store.value(&store.new).is_some_and(|v| v.contains("OLDNAME")) {
        left.push("noted as remembered".into());
    }
    if !data_removed && app.session.mailbox.account(owner).is_some() {
        let went: Arc<Mutex<Vec<String>>> = Default::default();
        let log = went.clone();
        app.services.graph_auto_replies = Some(Box::new(move |job: crate::auto_replies::AutoRepliesJob| {
            log.lock().unwrap().push(job.refresh.clone());
            mpsc::channel().1
        }));
        app.secrets_asked = None;
        app.secrets_tick();
        take_soon(app);
        app.auto_replies_read_now(owner);
        if let Some(refresh) = went.lock().unwrap().last().filter(|r| r.contains("OLDNAME")) {
            left.push(format!("the next automatic-replies job went with {refresh}"));
        }
    }
    left
}

/// MFU-S1 (the security review's P04): File › Open (here of the same mailbox again, as a copy of
/// its file brings the same names) lands while the first start's moving read waits on the store,
/// with or without the servers confirmed (and so a second read); then the sign-in is let go of,
/// or nothing happens. The dropped read writes nothing back: once the sign-in is let go of,
/// nothing of the old secret is left anywhere; with nothing let go of, it is still in the store.
#[test]
fn a_read_dropped_by_file_open_writes_nothing_back() {
    let mut wrong = Vec::new();
    let mut ran = 0;
    for at in [At::GetOld, At::SetAfter] {
        for confirm in [false, true] {
            for event in ["nothing", "tenant edit", "the installation's application id", "the data removed"] {
                let (mut app, owner, store) = paused_moving(at);
                app.secrets_tick();
                assert!(app.secret_read.is_some() && store.stopped(), "premise ({at:?}, {event}): the read waits");
                let same = app.session.mailbox.clone();
                app.session.replace_mailbox(same);
                app.drop_secrets_if_replaced();
                assert!(app.secret_read.is_none(), "premise ({at:?}, {event}): the read went with the mailbox");
                if confirm && app.is_unconfirmed() {
                    app.execute("ui.confirmServers", json!({})).unwrap();
                }
                app.secrets_tick();
                let second = app.secret_read.is_some();
                assert_eq!(second, confirm, "premise ({at:?}, {event}): a second read once the servers are confirmed");
                happen(&mut app, owner, event);
                store.release();
                assert!(store.finished_at_least(if second { 2 } else { 1 }), "premise ({at:?}, {event}): the reads ended");
                take_soon(&mut app);
                let label = format!("{at:?}, confirmed={confirm}, {event}");
                if event == "nothing" {
                    if store.holds("OLDNAME").is_empty() {
                        wrong.push(format!("{label}: the sign-in is gone from the store"));
                    }
                } else {
                    wrong.extend(old_secret_left(&mut app, owner, &store, event == "the data removed").into_iter().map(|l| format!("{label}: {l}")));
                }
                ran += 1;
            }
        }
    }
    assert_eq!(ran, 16);
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// MFU-S2 / CORR C1 (the security review's P05): the owner's sign-in let go of while a moving read
/// ran (a tenant edit, the installation's application id, the account removed and added again
/// with the same address) and remembered again under the same name before the read's answer is
/// taken stays in the store, in memory and noted, wherever the read was; and the old secret is
/// nowhere.
#[test]
fn a_sign_in_remembered_again_while_a_moving_read_ran_stays_in_the_store() {
    let mut wrong = Vec::new();
    let mut ran = 0;
    for at in [At::GetOld, At::SetBefore, At::SetAfter, At::GetNew] {
        for event in ["tenant edit", "the installation's application id", "removed and added again"] {
            let (mut app, owner, store) = paused_moving(at);
            app.secrets_tick();
            assert!(app.secret_read.is_some() && store.stopped(), "premise ({at:?}, {event}): the read waits");
            let who = if event == "removed and added again" {
                app.execute("account.remove", json!({"id": owner})).unwrap();
                let added = app.execute(
                    "account.add",
                    json!({"email": OWNER_A, "kind": "microsoft", "auth": "oauth2", "provider": "microsoft", "tenant": "tenant-alpha", "directory": "organisation"}),
                );
                added.unwrap()["id"].as_u64().unwrap()
            } else {
                happen(&mut app, owner, event);
                owner
            };
            let name = app.session.mailbox.account(who).map(|a| MailApp::secret_key("refresh", a));
            assert_eq!(name.as_deref(), Some(store.new.as_str()), "premise ({at:?}, {event}): the same name");
            app.remember("refresh", who, "NEW-ref");
            assert_eq!(store.value(&store.new).as_deref(), Some("NEW-ref"), "premise ({at:?}, {event}): remembered again");
            store.release();
            assert!(store.finished_at_least(1), "premise ({at:?}, {event}): the read ended");
            take_the_answer(&mut app);
            let now = (store.value(&store.new), app.remembered("refresh", who), app.ui.remembered_keys.contains(&store.new));
            if now != (Some("NEW-ref".into()), Some("NEW-ref".into()), true) {
                wrong.push(format!("{at:?}, {event}: (store, memory, noted) = {now:?}"));
            }
            for name in store.holds("OLDNAME") {
                wrong.push(format!("{at:?}, {event}: the old secret is in the store under {name}"));
            }
            ran += 1;
        }
    }
    assert_eq!(ran, 12);
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// CORR C1 (its P1a), the window: what the read says it moved to a name let go of and remembered
/// again meanwhile is the owner's new sign-in, which its answer leaves in the store.
#[test]
fn the_answer_of_a_moving_read_keeps_a_sign_in_remembered_again() {
    let (mut app, owner, key, store, send) = reading();
    happen(&mut app, owner, "the installation's application id");
    app.remember("refresh", owner, "AGAIN-ref");
    assert_eq!(store.value(&key).as_deref(), Some("AGAIN-ref"), "premise: remembered again under the same name");
    send.send(moved_to(&key)).unwrap();
    app.poll_secret_read();
    assert!(app.secret_read.is_none(), "premise: the answer was taken in");
    let now = (store.value(&key), app.remembered("refresh", owner), app.ui.remembered_keys.contains(&key));
    assert_eq!(now, (Some("AGAIN-ref".into()), Some("AGAIN-ref".into()), true), "(store, memory, noted)");
}

/// CORR C1 (its P1b), the reading thread: the same with the read stopped inside its read of the
/// old name and inside its write of the new one.
#[test]
fn a_sign_in_remembered_again_while_the_reader_waits_stays_in_the_store() {
    let mut wrong = Vec::new();
    for stop in [Stop::InGetOfOld, Stop::InSetOfNew] {
        let store = Gated::new(stop);
        let (mut app, owner, key, _old) = moving(&store);
        app.secrets_tick();
        assert!(app.secret_read.is_some() && store.stopped(), "premise: the read waits");
        happen(&mut app, owner, "the installation's application id");
        app.remember("refresh", owner, "AGAIN-ref");
        store.release();
        assert!(store.read_over(), "premise: the read ended");
        take_the_answer(&mut app);
        let label = if stop == Stop::InGetOfOld { "in its read" } else { "in its write" };
        if store.value(&key).as_deref() != Some("AGAIN-ref") || !store.holds("OLDNAME").is_empty() {
            wrong.push(format!("{label}: the store holds {:?} under the name, the old secret under {:?}", store.value(&key), store.holds("OLDNAME")));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// The limits of writing back (over-writing): a sign-in remembered again and then let go of
/// again (by another tenant edit, even one whose delete the store refused) or removed with all
/// the data is not written back when the read's answer is taken.
#[test]
fn a_sign_in_remembered_again_and_let_go_of_again_is_not_written_back() {
    for case in ["let go of again", "let go of again, its delete refused", "the data removed"] {
        let (mut app, owner, store) = paused_moving(At::SetAfter);
        app.secrets_tick();
        assert!(app.secret_read.is_some() && store.stopped(), "premise ({case}): the read waits");
        happen(&mut app, owner, "tenant edit");
        app.remember("refresh", owner, "NEW-ref");
        assert_eq!(store.value(&store.new).as_deref(), Some("NEW-ref"), "premise ({case}): remembered again");
        match case {
            "the data removed" => happen(&mut app, owner, case),
            _ => {
                store.refuse_delete.store(case.contains("refused"), Ordering::SeqCst);
                app.execute("account.edit", json!({"id": owner, "tenant": "tenant-third"})).unwrap();
                store.refuse_delete.store(false, Ordering::SeqCst);
            }
        }
        store.release();
        assert!(store.finished_at_least(1), "premise ({case}): the read ended");
        take_the_answer(&mut app);
        assert_eq!(store.value(&store.new), None, "{case}: written back");
        assert!(store.holds("OLDNAME").is_empty(), "{case}: the old secret is in the store");
    }
}
