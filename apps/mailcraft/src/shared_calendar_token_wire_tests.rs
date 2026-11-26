// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! End-to-end loopback HTTP wire regression tests for `shared_calendar_tokens`: drives real
//! `MailApp` calendar scheduling -> `CalendarJobs` -> `mailcraft_host::graph::graph_in_with` ->
//! `mailcraft_net::graph` -> loopback `StandIn` HTTP server, asserting that stale shared calendar
//! bearer tokens are never transmitted on the wire across mailbox swaps or owner identity edits.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mailcraft_engine::Session;
use mailcraft_model::sync::Tokens;
use mailcraft_model::{Id, Mailbox};
use mailcraft_net::NetError;
use mailcraft_net::graph::standin::{CalendarAccess, StandIn};
use mailcraft_ui_egui::{GraphRequest, MailApp, Services};
use serde_json::json;

const NOW: i64 = 1_791_993_600;
const OWNER_A: &str = "alice@alpha.example";
const OWNER_B: &str = "bob@beta.example";
const SHARED_A: &str = "team-a@alpha.example";
const SHARED_B: &str = "ops@beta.example";
const CANARY_WIRE_A: &str = "canary-wire-alpha-shared-cal-token";
const CANARY_WIRE_B: &str = "canary-wire-beta-shared-cal-token";

fn utc(day: u8, hour: u8) -> i64 {
    mailcraft_time::DateTime::new(mailcraft_time::Date::new(2026, 10, day).unwrap(), hour, 0, 0).unwrap().to_timestamp(0)
}

fn wire_harness() -> (MailApp, StandIn, Id, Id, Arc<AtomicUsize>, mailcraft_net::graph::standin::Routed) {
    let server = StandIn::start().unwrap();
    {
        let mut state = server.state();
        state.token = CANARY_WIRE_A.into();
        state.owner = OWNER_A.into();
        for addr in [SHARED_A, SHARED_B] {
            state.add_other(addr, true);
            let main = state.add_other_calendar(addr, "Calendar", true);
            state.add_other_event(addr, &main, "Stand-up", utc(14, 17), "busy");
            state.share_calendars(addr, CalendarAccess::Open);
        }
    }
    let route = server.route();
    let url = server.url();
    let renew_calls = Arc::new(AtomicUsize::new(0));
    let renew_counter = renew_calls.clone();

    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    app.services.graph = Some(Box::new(move |request: GraphRequest| {
        let (tx, rx) = std::sync::mpsc::channel();
        let url = url.clone();
        let renew_counter = renew_counter.clone();
        std::thread::spawn(move || {
            let _route = mailcraft_net::graph::standin::route(&url);
            let result = super::graph_in_with(request, "UTC", &move |_, _| {
                renew_counter.fetch_add(1, Ordering::SeqCst);
                Err(NetError::Config("unexpected opened-calendar renew".into()))
            });
            let _ = tx.send(result);
        });
        rx
    }));

    let owner = app
        .execute("account.add", json!({"email": OWNER_A, "kind": "microsoft", "auth": "oauth2", "provider": "microsoft", "tenant": "tenant-alpha"}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let fresh = Tokens {
        access: CANARY_WIRE_A.into(),
        refresh: "canary-owner-a-ref".into(),
        email: OWNER_A.into(),
        tenant: "tenant-alpha".into(),
        expires_at: mailcraft_time::now() + 3_600,
        ..Default::default()
    };
    app.seed_graph_token(owner, fresh.clone());
    let shared = app.execute("account.addSharedMailbox", json!({"account": owner, "address": SHARED_A})).unwrap()["id"].as_u64().unwrap();
    app.seed_shared_calendar_token(shared, fresh);
    app.execute("view.calendar", json!({})).unwrap();
    if app.calendar_tick_at(true, mailcraft_ui_egui::now_ms()) {
        drain_calendar(&mut app);
    }
    app.session.driver.calendar.checked.insert(owner, f64::MAX);
    app.session.driver.calendar.book.started(owner, f64::MAX);
    server.state().requests.clear();
    (app, server, owner, shared, renew_calls, route)
}

fn drain_calendar(app: &mut MailApp) {
    for _ in 0..2_000 {
        app.poll_calendar();
        if !app.calendar_syncing() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("calendar job did not finish");
}

fn cal_id(app: &MailApp, account: Id, name: &str) -> Id {
    app.session
        .mailbox
        .calendars
        .iter()
        .find(|c| c.origin.account == account && c.name == name)
        .map(|c| c.id)
        .unwrap_or_else(|| panic!("missing calendar {name} for {account}"))
}

#[test]
fn wire_control_same_account_uses_expected_bearer() {
    let (mut app, server, _owner, shared, renew_calls, _route) = wire_harness();
    let placeholder = cal_id(&app, shared, SHARED_A);
    app.execute("calendar.showCalendar", json!({"id": placeholder})).unwrap();
    assert!(app.calendar_tick_at(true, mailcraft_ui_egui::now_ms() + 1_000.0));
    drain_calendar(&mut app);

    let state = server.state();
    assert!(!state.requests.is_empty());
    assert!(
        state.requests.iter().any(|r| {
            r.path.join("/").contains("users/team-a@alpha.example")
                && r.header("authorization") == format!("Bearer {CANARY_WIRE_A}")
                && r.status == 200
        }),
        "same-account request must reach StandIn with expected bearer token"
    );
    assert_eq!(renew_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn wire_mailbox_swap_file_new_never_sends_stale_bearer_over_http() {
    let (mut app, server, _owner_a, shared_a, renew_calls, _route) = wire_harness();
    let placeholder_a = cal_id(&app, shared_a, SHARED_A);
    app.execute("calendar.showCalendar", json!({"id": placeholder_a})).unwrap();
    assert!(app.calendar_tick_at(true, mailcraft_ui_egui::now_ms() + 1_000.0));
    drain_calendar(&mut app);
    server.state().requests.clear();

    app.execute("file.new", json!({})).unwrap();
    let owner_b = app
        .execute("account.add", json!({"email": OWNER_B, "kind": "microsoft", "auth": "oauth2", "provider": "microsoft", "tenant": "tenant-beta"}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    app.session.driver.calendar.checked.insert(owner_b, f64::MAX);
    app.session.driver.calendar.book.started(owner_b, f64::MAX);
    let shared_b = app.execute("account.addSharedMailbox", json!({"account": owner_b, "address": SHARED_B})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(shared_b, shared_a);
    app.execute("view.calendar", json!({})).unwrap();
    let placeholder_b = cal_id(&app, shared_b, SHARED_B);
    app.execute("calendar.showCalendar", json!({"id": placeholder_b})).unwrap();

    assert!(!app.calendar_tick_at(true, mailcraft_ui_egui::now_ms() + 5_000.0), "file.new must not launch a calendar job with Mailbox 1 token");
    assert!(server.state().requests.is_empty(), "zero HTTP requests may reach StandIn after file.new without sign-in");
    assert_eq!(renew_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn wire_owner_identity_edit_and_undo_never_send_stale_bearer_over_http() {
    let (mut app, server, owner, shared, renew_calls, _route) = wire_harness();
    let placeholder = cal_id(&app, shared, SHARED_A);
    app.execute("calendar.showCalendar", json!({"id": placeholder})).unwrap();
    assert!(app.calendar_tick_at(true, mailcraft_ui_egui::now_ms() + 1_000.0));
    drain_calendar(&mut app);
    server.state().requests.clear();

    // Forward edit: Owner A -> Owner B.
    app.execute("account.edit", json!({"id": owner, "email": OWNER_B, "tenant": "tenant-beta"})).unwrap();
    app.calendar_list_due(shared);
    assert!(!app.calendar_tick_at(true, mailcraft_ui_egui::now_ms() + 120_000.0));
    assert!(server.state().requests.is_empty(), "zero HTTP requests may carry Owner A bearer after account.edit");

    // Seed Owner B token, verify it works for Owner B, then edit.undo back to Owner A.
    server.state().token = CANARY_WIRE_B.into();
    let fresh_b = Tokens {
        access: CANARY_WIRE_B.into(),
        refresh: "canary-owner-b-ref".into(),
        email: OWNER_B.into(),
        tenant: "tenant-beta".into(),
        expires_at: mailcraft_time::now() + 3_600,
        ..Default::default()
    };
    app.seed_graph_token(owner, fresh_b.clone());
    app.seed_shared_calendar_token(shared, fresh_b);
    app.calendar_list_due(shared);
    assert!(app.calendar_tick_at(true, mailcraft_ui_egui::now_ms() + 240_000.0));
    drain_calendar(&mut app);
    assert!(server.state().requests.iter().any(|r| r.header("authorization") == format!("Bearer {CANARY_WIRE_B}")));
    server.state().requests.clear();

    app.execute("edit.undo", json!({})).unwrap();
    app.calendar_list_due(shared);
    assert!(!app.calendar_tick_at(true, mailcraft_ui_egui::now_ms() + 360_000.0));
    assert!(server.state().requests.is_empty(), "zero HTTP requests may carry Owner B bearer after edit.undo");
    assert_eq!(renew_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn wire_confirmed_file_open_never_sends_stale_bearer_over_http() {
    let mut tmp = std::env::temp_dir();
    tmp.push(format!("cal-wire-fix-mb2-{}.mailcraft", std::process::id()));
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

    let (mut app, server, _owner_a, shared_a, _renew_calls, _route) = wire_harness();
    app.execute("file.open", json!({"path": tmp.to_string_lossy()})).unwrap();
    let _ = std::fs::remove_file(&tmp);
    assert!(app.is_unconfirmed());
    let owner_b = app.session.mailbox.accounts.iter().find(|a| a.email == OWNER_B).unwrap().id;
    let shared_b = app.session.mailbox.accounts.iter().find(|a| a.email == SHARED_B).unwrap().id;
    assert_eq!(shared_b, shared_a);
    app.session.driver.calendar.checked.insert(owner_b, f64::MAX);
    app.session.driver.calendar.book.started(owner_b, f64::MAX);
    app.execute("view.calendar", json!({})).unwrap();
    app.execute("ui.confirmServers", json!({})).unwrap();

    server.state().token = "expected-beta-token".into();
    assert!(!app.calendar_tick_at(true, mailcraft_ui_egui::now_ms() + 10_000.0));
    assert!(server.state().requests.is_empty(), "zero HTTP requests may carry Mailbox 1 bearer after file.open + confirmServers");
}
