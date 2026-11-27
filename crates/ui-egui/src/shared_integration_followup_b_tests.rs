// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the owner did that a job whose sign-in changed while it ran (a voided job) took with it:
//! a message held because its send's answer was lost is said in the status line whether or not
//! the run was voided; a change to the automatic replies goes again with the sign-in there is
//! now, to the same address under the same login name (or the owner is told it wasn't made, and
//! for which account); a rule to be made is sent again once the server's
//! rules are read whole. Fake in-memory hosts and invented `.example` addresses only.

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use mailcraft_model::auto_replies::{AutoReplies, AutoReplyStatus};
use mailcraft_model::remote_rules::RulesDownload;
use mailcraft_model::{Address, FolderKind, Id, Message};
use serde_json::json;

use super::integration_r2_tests::{add_owner_b, capture_graph};
use super::integration_tests::{OWNER_A, OWNER_B, fresh_tokens, setup_window};
use crate::auto_replies::{AutoRepliesJob, AutoRepliesResult};
use crate::rule_sync::{RulesJob, RulesResult};
use crate::{GraphResult, MailApp};

/// The status line, when there is one.
fn status(app: &MailApp) -> String {
    app.status_msg.as_ref().map(|(s, _)| s.clone()).unwrap_or_default()
}

/// A message waiting in `account`'s Outbox.
fn queued(app: &mut MailApp, account: Id, subject: &str) -> Id {
    let outbox = app.session.mailbox.folder_of_kind(account, FolderKind::Outbox).expect("premise: an Outbox");
    let m = Message {
        account,
        folder: outbox,
        subject: subject.into(),
        to: vec![Address::new("", "kit@example.net")],
        message_id: format!("{}@example.org", subject.to_lowercase().replace(' ', "-")),
        ..Default::default()
    };
    let id = app.session.mailbox.add_message(m).unwrap();
    app.session.mailbox_changed();
    id
}

/// R3-2 / CORR F1 (mutant K08): a message whose send's answer was lost is held, and the status
/// line says so, whether or not the run's sign-in changed while it ran, as long as the account
/// is there. (An account that went has no message to say anything of.)
#[test]
fn a_message_held_for_a_lost_answer_is_said_whether_or_not_the_run_was_voided() {
    let mut wrong = Vec::new();
    for event in ["nothing", "tenant edit", "the installation's application id"] {
        let (mut app, a, _shared, _jobs, _sign_ins) = setup_window();
        app.services.microsoft_client_id = "built-in-app".into();
        app.services.sync = Some(Box::new(|req: mailcraft_model::sync::SyncRequest| {
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(mailcraft_model::sync::SyncResult { account: req.account.id, ..Default::default() });
            rx
        }));
        let message = queued(&mut app, a, "Purchase order");
        let asked = capture_graph(&mut app);
        app.execute("ui.sync", json!({"account": a})).unwrap();
        assert_eq!(asked.lock().unwrap().len(), 1, "premise ({event}): a Graph job went");
        match event {
            "tenant edit" => drop(app.execute("account.edit", json!({"id": a, "tenant": "tenant-delta"})).unwrap()),
            "the installation's application id" => drop(app.execute("ui.microsoftClientId", json!({"value": "other-app-0000"})).unwrap()),
            _ => {}
        }
        app.status_msg = None;
        let _ = asked.lock().unwrap()[0].tx.send(GraphResult { account: a, unsure: vec![message], ..Default::default() });
        for _ in 0..3 {
            app.poll_graph();
        }
        let m = app.session.mailbox.message(message).expect("still in the Outbox");
        if m.held.is_empty() || m.sending {
            wrong.push(format!("{event}: not held"));
        }
        let said = status(&app);
        if !(said.contains("Purchase order") && said.contains("may have been sent")) {
            wrong.push(format!("{event}: the status line: {said:?}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

type ArJobs = Arc<Mutex<Vec<(Id, String, Option<AutoReplies>, Sender<AutoRepliesResult>)>>>;

/// A window whose automatic replies' jobs are noted (account, refresh token, change) and wait
/// for the test's answer; with a second owner, signed in, whose application id is its own.
fn replies() -> (MailApp, Id, Id, ArJobs) {
    let (mut app, a, _shared, _jobs, _sign_ins) = setup_window();
    app.services.microsoft_client_id = "built-in-app".into();
    let (b, _) = add_owner_b(&mut app);
    app.execute("account.edit", json!({"id": b, "clientId": "b-own-app"})).unwrap();
    app.tokens.insert(b, fresh_tokens("bee-mail", "bee-ref", OWNER_B, "tenant-beta"));
    // (B's own replies were read a moment ago: only what a test asks for goes.)
    app.session.driver.auto_replies.started(b, f64::MAX);
    let jobs: ArJobs = Default::default();
    let log = jobs.clone();
    app.services.graph_auto_replies = Some(Box::new(move |j: AutoRepliesJob| {
        let (tx, rx) = mpsc::channel();
        log.lock().unwrap().push((j.account.id, j.refresh.clone(), j.change.clone(), tx));
        rx
    }));
    (app, a, b, jobs)
}

fn away(text: &str) -> AutoReplies {
    AutoReplies { status: Some(AutoReplyStatus::AlwaysEnabled), internal_reply_message: Some(text.into()), ..Default::default() }
}

/// The last job's answer: the change made, with a renewed sign-in.
fn answer_last(app: &mut MailApp, jobs: &ArJobs) {
    let (account, _, change, tx) = jobs.lock().unwrap().pop().expect("a job to answer");
    let _ = tx.send(AutoRepliesResult {
        account,
        tokens: Some(fresh_tokens("LATE-access", "LATE-ref", OWNER_A, "")),
        setting: change.clone(),
        changed: change.is_some(),
        ..Default::default()
    });
    for _ in 0..3 {
        app.poll_auto_replies();
    }
}

/// CORR F4: an owner's change to his automatic replies whose job was voided (a tenant edit or a
/// change of the installation's application id while it ran) goes again with the sign-in there
/// is now; with no sign-in yet he is told to sign in first, as for any change.
#[test]
fn a_change_to_the_automatic_replies_whose_job_was_voided_goes_again() {
    for event in ["tenant edit", "the installation's application id"] {
        for signed_in in [true, false] {
            let (mut app, a, _b, jobs) = replies();
            let change = away("Away until Monday");
            app.auto_replies_change(a, change.clone()).unwrap();
            assert_eq!(jobs.lock().unwrap().len(), 1, "premise ({event}): the change went");
            match event {
                "tenant edit" => drop(app.execute("account.edit", json!({"id": a, "tenant": "tenant-new"})).unwrap()),
                _ => drop(app.execute("ui.microsoftClientId", json!({"value": "other-app-0000"})).unwrap()),
            }
            answer_last(&mut app, &jobs);
            assert!(!app.auto_replies.busy(), "premise ({event}): the answer was taken");
            app.status_msg = None;
            if signed_in {
                app.tokens.insert(a, fresh_tokens("NEW-mail", "NEW-ref", OWNER_A, ""));
            }
            app.auto_replies_tick_at(crate::now_ms());
            let went: Vec<(Id, String, Option<AutoReplies>)> = jobs.lock().unwrap().iter().map(|(id, r, c, _)| (*id, r.clone(), c.clone())).collect();
            if signed_in {
                assert_eq!(went, [(a, "NEW-ref".to_string(), Some(change.clone()))], "{event}: the change went again with the new sign-in");
            } else {
                assert!(went.is_empty(), "{event}: nothing goes without a sign-in: {went:?}");
                assert!(status(&app).contains("Sign in with Send/Receive first"), "{event}: told: {:?}", status(&app));
            }
            assert!(!kept_late(&app), "{event}: the voided answer's sign-in is kept");
        }
    }
}

fn kept_late(app: &MailApp) -> bool {
    app.tokens.values().chain(app.graph_tokens.values()).any(|t| t.refresh.contains("LATE"))
}

/// Over-clearing and the edges of CORR F4: a change whose job was not voided lands and goes
/// once; a voided read (no change) starts nothing by itself; a voided change of an account that
/// went is let go of; one the owner made again meanwhile goes instead of it, once; and one that
/// can't wait because another account's change waits is said not to have been made.
#[test]
fn a_voided_change_goes_again_only_when_nothing_newer_or_other_takes_its_place() {
    for case in ["not voided", "a read", "account removed", "changed again meanwhile", "another account's change waits"] {
        let (mut app, a, b, jobs) = replies();
        let change = away("Away until Monday");
        let newer = away("Back on Tuesday");
        if case == "a read" {
            app.auto_replies_read_now(a);
        } else {
            app.auto_replies_change(a, change.clone()).unwrap();
        }
        assert_eq!(jobs.lock().unwrap().len(), 1, "premise ({case}): a job went");
        match case {
            "not voided" => {}
            "account removed" => drop(app.execute("account.remove", json!({"id": a})).unwrap()),
            _ => drop(app.execute("account.edit", json!({"id": a, "tenant": "tenant-new"})).unwrap()),
        }
        match case {
            "changed again meanwhile" => app.auto_replies_change(a, newer.clone()).unwrap(),
            "another account's change waits" => app.auto_replies_change(b, newer.clone()).unwrap(),
            _ => {}
        }
        answer_last(&mut app, &jobs);
        // (What the answer said, before the next look.)
        let said = status(&app);
        if case == "account removed" {
            assert!(app.session.driver.auto_replies.waiting.is_none(), "{case}: queued again for an account that went");
        }
        app.status_msg = None;
        app.tokens.insert(a, fresh_tokens("NEW-mail", "NEW-ref", OWNER_A, ""));
        app.auto_replies_tick_at(crate::now_ms());
        let went: Vec<(Id, Option<AutoReplies>)> = jobs.lock().unwrap().iter().map(|(id, _, c, _)| (*id, c.clone())).collect();
        match case {
            "changed again meanwhile" => assert_eq!(went, [(a, Some(newer.clone()))], "{case}: the newer change goes, once"),
            "another account's change waits" => {
                assert_eq!(went, [(b, Some(newer.clone()))], "{case}: the other account's change goes");
                assert!(said.contains("Your automatic replies weren't changed"), "{case}: told: {said:?}");
            }
            _ => {
                assert!(went.is_empty(), "{case}: nothing goes again: {went:?}");
                assert!(!said.contains("weren't changed"), "{case}: told: {said:?}");
            }
        }
    }
}

type RuleJobs = Arc<Mutex<Vec<(Id, usize, Sender<RulesResult>)>>>;

/// CORR F4, rules: a rule to be made whose job was voided is not sent twice, and is sent again
/// with the sign-in there is now once the server's rules, read whole, show it isn't there.
#[test]
fn a_rule_whose_job_was_voided_is_sent_again_after_a_whole_read() {
    let (mut app, a, _shared, _jobs, _sign_ins) = setup_window();
    let jobs: RuleJobs = Default::default();
    let log = jobs.clone();
    app.services.graph_rules = Some(Box::new(move |job: RulesJob| {
        let (tx, rx) = mpsc::channel();
        log.lock().unwrap().push((job.account.id, job.upload.ops.len(), tx));
        rx
    }));
    let made = app
        .execute(
            "rules.add",
            json!({"name": "Receipts", "account": a, "conditions": [{"type": "subjectContains", "value": "receipt"}], "actions": [{"type": "markRead"}]}),
        )
        .unwrap();
    let rule = made["id"].as_u64().unwrap();
    let mut at = 0.0;
    let mut start = |app: &mut MailApp| {
        at += 3.0 * 3_600_000.0;
        app.rules_tick_at(at)
    };
    assert!(start(&mut app), "premise: the rule goes");
    assert_eq!(jobs.lock().unwrap().last().map(|j| (j.0, j.1)), Some((a, 1)), "premise: with the rule to make");
    app.execute("account.edit", json!({"id": a, "tenant": "tenant-new"})).unwrap();
    let (_, _, tx) = jobs.lock().unwrap().pop().unwrap();
    let _ = tx.send(RulesResult { account: a, tokens: Some(fresh_tokens("LATE-access", "LATE-ref", OWNER_A, "")), ..Default::default() });
    app.poll_rules();
    assert!(!app.rules.busy(), "premise: the voided answer was taken");
    assert!(app.session.mailbox.rules.iter().any(|r| r.id == rule && r.unanswered), "the rule may be on the server: not sent again yet");
    // Signed in again: the next job only reads, and the server's rules, read whole, haven't it.
    app.tokens.insert(a, fresh_tokens("NEW-mail", "NEW-ref", OWNER_A, ""));
    assert!(start(&mut app), "a read goes");
    let (_, ops, tx) = jobs.lock().unwrap().pop().unwrap();
    assert_eq!(ops, 0, "the rule is not sent twice");
    let download = RulesDownload { account: a, email: OWNER_A.into(), complete: true, rules: Vec::new() };
    let _ = tx.send(RulesResult { account: a, download: Some(download), ..Default::default() });
    app.poll_rules();
    assert!(app.session.mailbox.rules.iter().any(|r| r.id == rule && !r.unanswered && r.waiting()), "it waits to go again");
    assert!(start(&mut app), "the next job goes");
    assert_eq!(jobs.lock().unwrap().last().map(|j| (j.0, j.1)), Some((a, 1)), "with the rule to make");
}

/// MFU-S3: a voided change of the automatic replies goes again only while the account's address
/// and login name are what they were when its job went (a tenant edit keeps both); after an edit
/// of either, nothing goes and the owner is told it wasn't made.
#[test]
fn a_voided_change_goes_again_only_to_the_same_address_and_login_name() {
    let mut wrong = Vec::new();
    for (what, edit, again) in [
        ("a tenant edit", json!({"tenant": "tenant-new"}), true),
        ("an address edit", json!({"email": "someone@other.example"}), false),
        ("a login-name edit", json!({"username": "alias@alpha.example"}), false),
    ] {
        let (mut app, a, _b, jobs) = replies();
        let change = away("Away until Monday");
        app.auto_replies_change(a, change.clone()).unwrap();
        assert_eq!(jobs.lock().unwrap().len(), 1, "premise ({what}): the change went");
        let mut p = edit;
        p["id"] = json!(a);
        app.execute("account.edit", p).unwrap();
        app.status_msg = None;
        answer_last(&mut app, &jobs);
        assert!(!app.auto_replies.busy(), "premise ({what}): the answer was taken");
        let said = status(&app);
        app.tokens.insert(a, fresh_tokens("NEW-mail", "NEW-ref", OWNER_A, ""));
        app.auto_replies_tick_at(crate::now_ms());
        let went: Vec<(Id, String, Option<AutoReplies>)> = jobs.lock().unwrap().iter().map(|(id, r, c, _)| (*id, r.clone(), c.clone())).collect();
        if again {
            if went != [(a, "NEW-ref".to_string(), Some(change.clone()))] || said.contains("weren't changed") {
                wrong.push(format!("{what}: went {went:?}, said {said:?}"));
            }
        } else {
            if !went.is_empty() || app.session.driver.auto_replies.waiting.is_some() {
                wrong.push(format!("{what}: went again: {went:?}"));
            }
            if said != "Your automatic replies weren't changed" {
                wrong.push(format!("{what}: said {said:?}"));
            }
        }
        assert!(!kept_late(&app), "{what}: the voided answer's sign-in is kept");
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// CORR C2: when another account's change holds the one place a change waits in, the owner is
/// told which account's replies weren't changed, and to make that change again.
#[test]
fn the_owner_is_told_which_accounts_replies_werent_changed() {
    let (mut app, a, b, jobs) = replies();
    app.auto_replies_change(a, away("Away until Monday")).unwrap();
    app.execute("account.edit", json!({"id": a, "tenant": "tenant-new"})).unwrap();
    app.auto_replies_change(b, away("Back on Tuesday")).unwrap();
    app.status_msg = None;
    answer_last(&mut app, &jobs);
    assert_eq!(
        status(&app),
        format!(
            "Your automatic replies weren't changed: another account's change was waiting, so make the change for {OWNER_A} again (File › Info › Automatic Replies)."
        ),
        "the line names the account"
    );
    assert_eq!(app.session.driver.auto_replies.waiting.as_ref().map(|(id, _)| *id), Some(b), "the other account's change still waits");
}
