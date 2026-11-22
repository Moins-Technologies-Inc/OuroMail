// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Searching the organisation's directory for a shared mailbox (Add Shared Mailbox): its one
//! permission, `User.ReadBasic.All`, is on the administrator's list, is asked for on a page of its
//! own and in a token request of its own, and never on a sign-in's page nor with any other token.
//! Every other scope string is byte for byte what it was before the permission came ([`BEFORE`] was
//! printed from the base bytes). Nothing here leaves this machine.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::*;
use crate::fake::{FakeHttp, json_answer};

const DIRECTORY: &str = "https://graph.microsoft.com/User.ReadBasic.All";
const SCHEME: &str = "org.example.mail";
const PATH: &str = "/oauth2redirect";
const TOKENS: &str = r#"{"token_type":"Bearer","expires_in":3599,"access_token":"acc-ess","refresh_token":"re-fresh"}"#;
const PATIENCE: Duration = Duration::from_secs(5);

const YES: &str = "code=c";
const NO_SUCH_SCOPE: &str = "error=invalid_scope&error_description=AADSTS70011%3A+The+provided+value+for+scope+is+not+valid.";
const ADMIN_ONLY: &str = "error=access_denied&error_description=AADSTS90094%3A+An+administrator+of+the+organisation+has+to+approve.";
const BLOCKED: &str = "error=access_denied&error_description=AADSTS53003%3A+Access+has+been+blocked+by+Conditional+Access+policies.";
const DECLINED: &str = "error=access_denied&error_description=AADSTS65004%3A+User+declined+to+consent.";

fn oauth() -> OAuth {
    OAuth { provider: OAuthProvider::Microsoft, client_id: "client-1".into(), ..Default::default() }
}

fn has(scope: &str, permission: &str) -> bool {
    scope.split_whitespace().any(|s| s == permission)
}

/// Every scope string a request of this module is built with but the directory's own, as
/// `label=value` (the value as `Debug` prints it): what a sign-in's page asks, on the desktop and
/// on the phone, of an organisation's account and of a personal one; the one page of an older
/// sign-in; and every token request of a job that asks for a permission of its own.
fn every_other_scope() -> Vec<String> {
    let org = oauth();
    let personal = OAuth { tenant: "consumers".into(), ..org.clone() };
    let mut lines = Vec::new();
    for (who, o) in [("org", &org), ("personal", &personal)] {
        for resource in [Resource::Mail, Resource::Graph, Resource::GraphMail] {
            for (how, e) in [("sync", Endpoints::for_resource(o, resource)), ("phone", Endpoints::for_resource_without_sync(o, resource))] {
                lines.push(format!("{who} {resource:?} {how} scope={:?}", e.scope));
                lines.push(format!("{who} {resource:?} {how} consent={:?}", e.consent));
                lines.push(format!("{who} {resource:?} {how} fewer={:?}", e.fewer));
                lines.push(format!("{who} {resource:?} {how} lesser={:?}", e.lesser));
            }
        }
        for mail_write in [false, true] {
            let e = sync_endpoints(o, "ada@example.org", mail_write);
            lines.push(format!("{who} sync_endpoints {mail_write} scope={:?}", e.scope));
            lines.push(format!("{who} sync_endpoints {mail_write} consent={:?}", e.consent));
        }
    }
    for (name, s) in [
        ("shared", shared_scope()),
        ("shared_send", shared_send_scope()),
        ("shared_calendar", shared_calendar_scope()),
        ("shared_calendar_send", shared_calendar_send_scope()),
        ("opened_calendars", opened_calendars_scope()),
        ("tasks", tasks_scope()),
        ("rules", rules_scope()),
        ("replies", replies_scope()),
        ("sync_false", sync_scope(false)),
        ("sync_true", sync_scope(true)),
    ] {
        lines.push(format!("fn {name}={s:?}"));
    }
    lines
}

/// What [`every_other_scope`] gave at the base bytes, before the directory's permission came
/// (`logs/dump0.log` of the task's artifacts).
const BEFORE: &[&str] = &[
    r#"org Mail sync scope="https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send offline_access openid profile email""#,
    r#"org Mail sync consent="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/MailboxSettings.Read https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite https://graph.microsoft.com/Mail.ReadWrite.Shared https://graph.microsoft.com/Mail.Send.Shared https://graph.microsoft.com/Calendars.ReadWrite.Shared""#,
    r#"org Mail sync fewer="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/MailboxSettings.Read https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite""#,
    r#"org Mail sync lesser="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/MailboxSettings.Read""#,
    r#"org Mail phone scope="https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send offline_access openid profile email""#,
    r#"org Mail phone consent="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/MailboxSettings.Read https://graph.microsoft.com/Mail.ReadWrite.Shared https://graph.microsoft.com/Mail.Send.Shared""#,
    r#"org Mail phone fewer="""#,
    r#"org Mail phone lesser="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/MailboxSettings.Read""#,
    r#"org Graph sync scope="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read offline_access openid profile email""#,
    r#"org Graph sync consent="https://graph.microsoft.com/MailboxSettings.Read https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite https://graph.microsoft.com/Mail.ReadWrite.Shared https://graph.microsoft.com/Mail.Send.Shared https://graph.microsoft.com/Calendars.ReadWrite.Shared""#,
    r#"org Graph sync fewer="https://graph.microsoft.com/MailboxSettings.Read https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite""#,
    r#"org Graph sync lesser="https://graph.microsoft.com/MailboxSettings.Read""#,
    r#"org Graph phone scope="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read offline_access openid profile email""#,
    r#"org Graph phone consent="https://graph.microsoft.com/MailboxSettings.Read https://graph.microsoft.com/Mail.ReadWrite.Shared https://graph.microsoft.com/Mail.Send.Shared""#,
    r#"org Graph phone fewer="""#,
    r#"org Graph phone lesser="https://graph.microsoft.com/MailboxSettings.Read""#,
    r#"org GraphMail sync scope="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/Mail.ReadWrite offline_access openid profile email""#,
    r#"org GraphMail sync consent="https://graph.microsoft.com/MailboxSettings.Read https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite https://graph.microsoft.com/Mail.ReadWrite.Shared https://graph.microsoft.com/Mail.Send.Shared https://graph.microsoft.com/Calendars.ReadWrite.Shared""#,
    r#"org GraphMail sync fewer="https://graph.microsoft.com/MailboxSettings.Read https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite""#,
    r#"org GraphMail sync lesser="https://graph.microsoft.com/MailboxSettings.Read""#,
    r#"org GraphMail phone scope="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/Mail.ReadWrite offline_access openid profile email""#,
    r#"org GraphMail phone consent="https://graph.microsoft.com/MailboxSettings.Read https://graph.microsoft.com/Mail.ReadWrite.Shared https://graph.microsoft.com/Mail.Send.Shared""#,
    r#"org GraphMail phone fewer="""#,
    r#"org GraphMail phone lesser="https://graph.microsoft.com/MailboxSettings.Read""#,
    r#"org sync_endpoints false scope="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite offline_access openid profile email""#,
    r#"org sync_endpoints false consent="https://graph.microsoft.com/Mail.ReadWrite.Shared https://graph.microsoft.com/Mail.Send.Shared https://graph.microsoft.com/Calendars.ReadWrite.Shared""#,
    r#"org sync_endpoints true scope="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite offline_access openid profile email""#,
    r#"org sync_endpoints true consent="https://graph.microsoft.com/Mail.ReadWrite.Shared https://graph.microsoft.com/Mail.Send.Shared https://graph.microsoft.com/Calendars.ReadWrite.Shared""#,
    r#"personal Mail sync scope="https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send offline_access openid profile email""#,
    r#"personal Mail sync consent="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/MailboxSettings.Read https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite""#,
    r#"personal Mail sync fewer="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/MailboxSettings.Read https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite""#,
    r#"personal Mail sync lesser="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/MailboxSettings.Read""#,
    r#"personal Mail phone scope="https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send offline_access openid profile email""#,
    r#"personal Mail phone consent="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/MailboxSettings.Read""#,
    r#"personal Mail phone fewer="""#,
    r#"personal Mail phone lesser="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/MailboxSettings.Read""#,
    r#"personal Graph sync scope="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read offline_access openid profile email""#,
    r#"personal Graph sync consent="https://graph.microsoft.com/MailboxSettings.Read https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite""#,
    r#"personal Graph sync fewer="https://graph.microsoft.com/MailboxSettings.Read https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite""#,
    r#"personal Graph sync lesser="https://graph.microsoft.com/MailboxSettings.Read""#,
    r#"personal Graph phone scope="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read offline_access openid profile email""#,
    r#"personal Graph phone consent="https://graph.microsoft.com/MailboxSettings.Read""#,
    r#"personal Graph phone fewer="""#,
    r#"personal Graph phone lesser="https://graph.microsoft.com/MailboxSettings.Read""#,
    r#"personal GraphMail sync scope="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/Mail.ReadWrite offline_access openid profile email""#,
    r#"personal GraphMail sync consent="https://graph.microsoft.com/MailboxSettings.Read https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite""#,
    r#"personal GraphMail sync fewer="https://graph.microsoft.com/MailboxSettings.Read https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite""#,
    r#"personal GraphMail sync lesser="https://graph.microsoft.com/MailboxSettings.Read""#,
    r#"personal GraphMail phone scope="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/Mail.ReadWrite offline_access openid profile email""#,
    r#"personal GraphMail phone consent="https://graph.microsoft.com/MailboxSettings.Read""#,
    r#"personal GraphMail phone fewer="""#,
    r#"personal GraphMail phone lesser="https://graph.microsoft.com/MailboxSettings.Read""#,
    r#"personal sync_endpoints false scope="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite offline_access openid profile email""#,
    r#"personal sync_endpoints false consent="""#,
    r#"personal sync_endpoints true scope="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite offline_access openid profile email""#,
    r#"personal sync_endpoints true consent="""#,
    r#"fn shared="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Mail.ReadWrite.Shared offline_access openid profile email""#,
    r#"fn shared_send="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Mail.ReadWrite.Shared https://graph.microsoft.com/Mail.Send.Shared offline_access openid profile email""#,
    r#"fn shared_calendar="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Mail.ReadWrite.Shared https://graph.microsoft.com/Calendars.ReadWrite.Shared offline_access openid profile email""#,
    r#"fn shared_calendar_send="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Mail.ReadWrite.Shared https://graph.microsoft.com/Calendars.ReadWrite.Shared https://graph.microsoft.com/Mail.Send.Shared offline_access openid profile email""#,
    r#"fn opened_calendars="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/Calendars.ReadWrite.Shared offline_access openid profile email""#,
    r#"fn tasks="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/Tasks.ReadWrite offline_access openid profile email""#,
    r#"fn rules="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite offline_access openid profile email""#,
    r#"fn replies="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/MailboxSettings.ReadWrite offline_access openid profile email""#,
    r#"fn sync_false="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite offline_access openid profile email""#,
    r#"fn sync_true="https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite offline_access openid profile email""#,
];
/// The administrator's list as it was before the directory came.
const ADMIN_BEFORE: &str = r#"https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/MailboxSettings.Read https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/MailboxConfigItem.Read https://graph.microsoft.com/Mail.ReadBasic https://graph.microsoft.com/Mail.ReadWrite.Shared https://graph.microsoft.com/Mail.Send.Shared https://graph.microsoft.com/Calendars.ReadWrite.Shared https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/MailboxSettings.ReadWrite offline_access openid profile email"#;

#[test]
fn every_other_scope_string_is_byte_for_byte_what_it_was() {
    let now = every_other_scope();
    assert_eq!(now.len(), BEFORE.len());
    for (now, was) in now.iter().zip(BEFORE) {
        assert_eq!(now, was);
    }
}

#[test]
fn no_sign_in_page_and_no_other_token_asks_for_the_directory() {
    for line in every_other_scope() {
        assert!(!line.contains("User.ReadBasic.All"), "{line}");
    }
    // Nor the Favorites' page, the other one asked for by itself.
    let favorites = format!("{MICROSOFT_GRAPH} {MICROSOFT_GRAPH_EXTRAS} {MICROSOFT_GRAPH_FAVORITES} {SIGN_IN_SCOPE}");
    assert!(!has(&favorites, DIRECTORY));
}

#[test]
fn the_admin_list_names_user_read_basic_all_once_and_nothing_else_new() {
    let admin = admin_scope();
    assert_eq!(admin.split(' ').filter(|p| *p == DIRECTORY).count(), 1, "{admin}");
    let before_and_it = ADMIN_BEFORE.replace(" offline_access openid profile email", &format!(" {DIRECTORY} offline_access openid profile email"));
    assert_eq!(admin, before_and_it);
    // What the directory's page and its token ask for is all on the list: one approval covers it.
    for asked in format!("{} {}", directory_endpoints(&oauth()).scope, directory_scope()).split_whitespace() {
        assert!(has(&admin, asked), "{asked}");
    }
}

#[test]
fn the_directory_has_a_token_request_of_its_own_and_nothing_less() {
    assert_eq!(directory_scope(), format!("{MICROSOFT_GRAPH} {DIRECTORY} offline_access openid profile email"));
    let page = directory_endpoints(&oauth());
    assert_eq!(page.scope, format!("{MICROSOFT_GRAPH} {MICROSOFT_GRAPH_EXTRAS} {DIRECTORY} offline_access openid profile email"));
    // Nothing beside it on the page, and nothing asked again for less.
    assert!(page.consent.is_empty() && page.fewer.is_empty() && page.lesser.is_empty(), "{page:?}");
}

// ---- the page ----

/// The pages opened, by what each asked for.
type Pages = Arc<Mutex<Vec<String>>>;

fn query(url: &str) -> HashMap<String, String> {
    http::pairs(url.split_once('?').map_or("", |(_, q)| q)).into_iter().collect()
}

/// A way back that answers the pages in turn with `answers`, what opens a page, and the pages'
/// `scope`s as they were asked.
fn answering(answers: &[&str]) -> (Box<Handback>, Box<dyn Fn(&str)>, Pages) {
    let pages: Pages = Arc::default();
    let opened: Arc<Mutex<String>> = Arc::default();
    let (log, last) = (pages.clone(), opened.clone());
    let answers: Vec<String> = answers.iter().map(|a| format!("{SCHEME}:{PATH}?{a}&state={{state}}")).collect();
    let handback = move |_: &AtomicBool, _: Duration| {
        let page = query(&last.lock().unwrap());
        let mut log = log.lock().unwrap();
        log.push(page.get("scope").cloned().unwrap_or_default());
        let answer = answers.get(log.len() - 1).or(answers.last()).cloned().unwrap_or_default();
        Ok(answer.replace("{state}", page.get("state").map_or("", String::as_str)))
    };
    let open = move |url: &str| *opened.lock().unwrap() = url.to_string();
    (Box::new(handback), Box::new(open), pages)
}

fn via(handback: &Handback) -> RedirectStrategy<'_> {
    RedirectStrategy::CustomScheme(CustomScheme::new(SCHEME, PATH, handback).unwrap())
}

fn page_at(server: &FakeHttp, oauth: &OAuth) -> Endpoints {
    Endpoints { authorize: server.url("/authorize"), token: server.url("/token"), ..directory_endpoints(oauth) }
}

fn ask_page(server: &FakeHttp, settings: &OAuth, answers: &[&str]) -> (Result<Tokens, FavoritesRefusal>, Vec<String>) {
    let (back, open, pages) = answering(answers);
    let got =
        sign_in_for_directory_at(&page_at(server, settings), settings, "noor@example.org", &*open, &AtomicBool::new(false), PATIENCE, &via(&*back));
    let pages = pages.lock().unwrap().clone();
    (got, pages)
}

#[test]
fn the_directory_page_asks_for_what_graph_had_before_and_the_directory() {
    let server = FakeHttp::answering(json_answer("200 OK", TOKENS));
    let (got, pages) = ask_page(&server, &oauth(), &[YES]);
    let tokens = got.unwrap();
    assert_eq!((tokens.access.as_str(), tokens.refresh.as_str()), ("acc-ess", "re-fresh"));
    assert_eq!(pages, [directory_endpoints(&oauth()).scope], "one page, for that alone");
    assert!(has(&pages[0], DIRECTORY) && !pages[0].contains("Shared") && !has(&pages[0], MICROSOFT_GRAPH_TASKS), "{}", pages[0]);
    // The code is traded for exactly that.
    let form: HashMap<String, String> = http::pairs(server.request(0).split_once("\r\n\r\n").unwrap().1).into_iter().collect();
    assert_eq!(form["scope"], directory_endpoints(&oauth()).scope);
}

#[test]
fn a_refused_directory_page_says_whose_no_it_is_and_is_never_asked_again_for_less() {
    for (answer, whose) in
        [(ADMIN_ONLY, FavoritesRefusal::Approval), (BLOCKED, FavoritesRefusal::Organisation), (NO_SUCH_SCOPE, FavoritesRefusal::Account)]
    {
        let server = FakeHttp::answering(json_answer("200 OK", TOKENS));
        let (got, pages) = ask_page(&server, &oauth(), &[answer, YES]);
        assert_eq!(got.err(), Some(whose), "{answer}");
        assert_eq!(pages.len(), 1, "{answer}: {pages:?}");
        assert_eq!(server.asked(), 0, "nothing traded");
    }
    // The owner's own no on the page.
    let server = FakeHttp::answering(json_answer("200 OK", TOKENS));
    let (got, pages) = ask_page(&server, &oauth(), &[DECLINED, YES]);
    assert!(matches!(got, Err(FavoritesRefusal::Other(_))), "{got:?}");
    assert_eq!(pages.len(), 1);
}

#[test]
fn a_personal_account_is_never_put_the_page() {
    for settings in [OAuth { tenant: "consumers".into(), ..oauth() }, OAuth { directory: mailcraft_model::Directory::Personal, ..oauth() }] {
        let server = FakeHttp::answering(json_answer("200 OK", TOKENS));
        let (got, pages) = ask_page(&server, &settings, &[YES]);
        assert_eq!(got.err(), Some(FavoritesRefusal::Account));
        assert!(pages.is_empty(), "{pages:?}");
        assert_eq!(server.asked(), 0);
    }
}

#[test]
fn tokens_of_a_page_that_dont_cover_the_directory_are_consent_needed() {
    let partly = r#"{"token_type":"Bearer","expires_in":3599,"access_token":"acc-ess","refresh_token":"re-fresh","scope":"https://graph.microsoft.com/Mail.Send"}"#;
    let server = FakeHttp::answering(json_answer("200 OK", partly));
    let (got, _) = ask_page(&server, &oauth(), &[YES]);
    assert!(matches!(&got, Err(FavoritesRefusal::Other(e)) if e.is_consent_needed()), "{got:?}");
}

// ---- the token ----

fn graph_at(server: &FakeHttp, resource: Resource) -> Endpoints {
    Endpoints { authorize: server.url("/authorize"), token: server.url("/token"), ..Endpoints::for_resource(&oauth(), resource) }
}

fn refresh_at(server: &FakeHttp) -> Result<GraphTokens, NetError> {
    let mail = graph_at(server, Resource::GraphMail);
    refresh_graph_directory_at(&graph_at(server, Resource::Graph), (&mail, Resource::GraphMail), &oauth(), "re-fresh-1")
}

fn form_of(server: &FakeHttp, n: usize) -> HashMap<String, String> {
    http::pairs(server.request(n).split_once("\r\n\r\n").unwrap().1).into_iter().collect()
}

#[test]
fn a_directory_token_is_asked_for_with_the_directory_and_hands_back_the_newest_refresh_token() {
    let covered = r#"{"token_type":"Bearer","expires_in":3599,"access_token":"dir-access","refresh_token":"re-fresh-2","scope":"https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/User.ReadBasic.All"}"#;
    let server = FakeHttp::answering(json_answer("200 OK", covered));
    let got = refresh_at(&server).unwrap();
    assert_eq!((got.tokens.access.as_str(), got.tokens.refresh.as_str(), server.asked()), ("dir-access", "re-fresh-2", 1));
    assert_eq!(form_of(&server, 0)["scope"], directory_scope());
    // (An answer that leaves out what it granted granted what was asked: RFC 6749 §5.1.)
    let server = FakeHttp::answering(json_answer("200 OK", TOKENS));
    assert_eq!(refresh_at(&server).unwrap().tokens.access, "acc-ess");
}

#[test]
fn a_directory_token_without_the_scope_is_consent_needed() {
    let without = r#"{"token_type":"Bearer","expires_in":3599,"access_token":"dir-access","refresh_token":"re-fresh-2","scope":"https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite"}"#;
    let server = FakeHttp::answering(json_answer("200 OK", without));
    let got = refresh_at(&server);
    assert!(got.as_ref().is_err_and(NetError::is_consent_needed), "{:?}", got.map(|g| g.tokens.access));
    // A sign-in that never agreed, which Microsoft says in the words of one that is over, while
    // the same refresh token still gets mail's: the permission, and nothing more is asked.
    let over = r#"{"error":"invalid_grant","error_description":"AADSTS70000: The user could not be authenticated as the grant is expired. The user must sign in again."}"#;
    let server = FakeHttp::start(move |n, _| if n == 0 { json_answer("400 Bad Request", over) } else { json_answer("200 OK", TOKENS) });
    let got = refresh_at(&server);
    assert!(got.as_ref().is_err_and(NetError::is_consent_needed), "{:?}", got.map(|g| g.tokens.access));
    assert_eq!(server.asked(), 2);
    // Over for mail too: the sign-in is over, and never taken for a missing permission.
    let server = FakeHttp::answering(json_answer("400 Bad Request", over));
    let got = refresh_at(&server);
    assert!(got.as_ref().is_err_and(|e| e.is_refused_sign_in() && !e.is_consent_needed()), "{:?}", got.map(|g| g.tokens.access));
}
