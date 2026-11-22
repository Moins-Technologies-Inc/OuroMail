// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The directory search (Add Shared Mailbox) against the Graph stand-in: what is asked, and what
//! of the answer is kept. Every name and address is invented.

use mailcraft_model::directory::{DirectoryAnswer, DirectoryOutcome, DirectoryQuery, MAX_RESULTS};
use mailcraft_model::{Account, AccountKind, Directory, OAuth};

use super::*;
use crate::graph::standin::{Asked, StandIn, TOKEN};

const OWNER: Id = 7;
type Id = mailcraft_model::Id;

/// A stand-in whose directory has a help desk, a person, an entry without a mailbox and a room.
fn standin() -> StandIn {
    let standin = StandIn::start().unwrap();
    {
        let mut state = standin.state();
        state.add_directory_user("Help Desk", "helpdesk@example.org", "helpdesk@example.org");
        state.add_directory_user("Lena Help", "lena@example.org", "lena@example.org");
        state.add_directory_user("Help Without Mail", "", "nomail@example.org");
        state.add_directory_user("Sales Team", "sales@example.org", "sales.team@example.org");
    }
    standin
}

fn owner() -> Account {
    Account { id: OWNER, kind: AccountKind::Microsoft, email: "owner@example.org".into(), ..Default::default() }
}

fn query(text: &str) -> DirectoryQuery {
    DirectoryQuery { account: OWNER, text: text.into(), ticket: 41 }
}

fn search(standin: &StandIn, text: &str) -> DirectoryAnswer {
    search_directory_at(&standin.url(), TOKEN, &owner(), &query(text))
}

/// The requests the stand-in was sent.
fn asked(standin: &StandIn) -> Vec<Asked> {
    standin.state().requests.clone()
}

fn param<'a>(asked: &'a Asked, name: &str) -> Option<&'a str> {
    asked.query.iter().find(|(have, _)| have == name).map(|(_, value)| value.as_str())
}

fn addresses(answer: &DirectoryAnswer) -> Vec<&str> {
    answer.entries.iter().map(|e| e.address.as_str()).collect()
}

#[test]
fn a_search_sends_consistency_level_eventual_and_three_clauses() {
    let standin = standin();
    // Through Graph's own address, routed to the stand-in for this thread.
    let answer = {
        let _routed = standin.route();
        search_directory(TOKEN, &owner(), &query(" help "))
    };
    assert_eq!((answer.outcome, answer.account, answer.ticket), (DirectoryOutcome::Found, OWNER, 41));
    assert_eq!(addresses(&answer), ["helpdesk@example.org", "lena@example.org"], "by a word of the name or the start of the address");
    let asked = asked(&standin);
    assert_eq!(asked.len(), 1);
    let one = &asked[0];
    assert_eq!(one.header("consistencylevel"), "eventual");
    assert_eq!(param(one, "$search"), Some(r#""displayName:help" OR "mail:help" OR "userPrincipalName:help""#));
    assert_eq!(one.header("authorization"), format!("Bearer {TOKEN}"));
    assert_eq!(one.status, 200);
}

#[test]
fn quotes_and_backslashes_are_escaped_and_ampersands_dropped() {
    assert_eq!(clauses(r#"a"b\c"#), r#""displayName:a\"b\\c" OR "mail:a\"b\\c" OR "userPrincipalName:a\"b\\c""#);
    let standin = standin();
    let answer = search(&standin, r#"Q&A "desk" \"#);
    let asked = asked(&standin);
    let search = param(&asked[0], "$search").unwrap();
    assert_eq!(search, r#""displayName:QA \"desk\" \\" OR "mail:QA \"desk\" \\" OR "userPrincipalName:QA \"desk\" \\""#);
    assert!(!asked[0].target.contains('&') || asked[0].target.matches('&').count() == 2, "only the parameters are parted by &: {}", asked[0].target);
    assert!(!asked[0].target.contains(' ') && !asked[0].target.contains('"'), "written as %XX");
    // The stand-in read it as three clauses, as Graph would.
    assert_eq!(answer.outcome, DirectoryOutcome::Found);
}

#[test]
fn the_query_is_cut_to_64_characters_and_control_characters_go() {
    let standin = standin();
    let long = format!("he\u{7}lp\u{202e}{}", "x".repeat(200));
    search(&standin, &long);
    let asked = asked(&standin);
    let clause = param(&asked[0], "$search").unwrap().split(" OR ").next().unwrap().to_string();
    let value = clause.trim_matches('"').trim_start_matches("displayName:").to_string();
    assert_eq!(value.chars().count(), 64);
    assert!(value.starts_with("helpx"), "{value}");
    // Fewer than two characters left, or nothing: nothing is asked.
    for short in ["", "a", " \u{7}b\u{7} ", "&"] {
        assert_eq!(search(&standin, short).outcome, DirectoryOutcome::NothingAsked, "{short:?}");
    }
    assert_eq!(standin.state().requests.len(), 1);
}

#[test]
fn only_display_name_mail_and_user_principal_name_are_selected() {
    let standin = standin();
    search(&standin, "sales");
    let asked = asked(&standin);
    assert_eq!(param(&asked[0], "$select"), Some("displayName,mail,userPrincipalName"));
    assert_eq!(param(&asked[0], "$top"), Some("15"));
    assert_eq!(MAX_RESULTS, 15);
    let names: Vec<&str> = asked[0].query.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["$search", "$select", "$top"], "no $count, no $orderby, no $filter");
}

#[test]
fn entries_without_a_usable_mail_address_are_dropped_and_duplicates_merged() {
    let standin = standin();
    standin.state().add_directory_user("Help Desk (old)", "HelpDesk@Example.org", "old.helpdesk@example.org");
    standin.state().add_directory_user("Help Odd", "help/../odd@example.org", "odd@example.org");
    let answer = search(&standin, "help");
    assert_eq!(addresses(&answer), ["helpdesk@example.org", "lena@example.org"]);
    assert!(answer.entries.iter().all(|e| e.address != "nomail@example.org"), "no mailbox, not offered by its sign-in name");
    assert_eq!(answer.entries[0].name, "Help Desk");
}

#[test]
fn at_most_15_entries_are_kept_and_next_link_only_says_more() {
    let standin = standin();
    for i in 0..30 {
        standin.state().add_directory_user(&format!("Group {i:02}"), &format!("group{i:02}@example.org"), &format!("group{i:02}@example.org"));
    }
    let answer = search(&standin, "group");
    assert_eq!(answer.entries.len(), 15);
    assert!(answer.more, "the stand-in has a next page");
    assert_eq!(answer.entries.first().map(|e| e.address.as_str()), Some("group00@example.org"));
    // Fifteen at most even when the server answers more than asked for.
    let many: Vec<serde_json::Value> = (0..40).map(|i| serde_json::json!({"displayName": "T", "mail": format!("t{i}@example.org")})).collect();
    standin.state().directory_answer = Some(serde_json::json!({"value": many}).to_string());
    let answer = search(&standin, "group");
    assert_eq!((answer.entries.len(), answer.more), (15, true));
    // Fewer than fifteen and no next page: no more.
    standin.state().directory_answer = None;
    let answer = search(&standin, "sales");
    assert_eq!((answer.entries.len(), answer.more), (1, false));
}

#[test]
fn next_link_is_never_followed() {
    let standin = standin();
    for i in 0..40 {
        standin.state().add_directory_user(&format!("Desk {i}"), &format!("desk{i}@example.org"), &format!("desk{i}@example.org"));
    }
    let answer = search(&standin, "desk");
    assert!(answer.more);
    let asked = asked(&standin);
    assert_eq!(asked.len(), 1, "one page, one request");
    assert!(param(&asked[0], "$skiptoken").is_none());
}

#[test]
fn nothing_but_v1_users_is_asked() {
    let standin = standin();
    for text in ["help", "sales", "nobody here", "lena@"] {
        search(&standin, text);
    }
    let asked = asked(&standin);
    assert_eq!(asked.len(), 4);
    for one in &asked {
        assert_eq!((one.method.as_str(), one.path.as_slice()), ("GET", &["v1.0".to_string(), "users".to_string()][..]), "{}", one.target);
        assert!(one.body.is_empty());
    }
}

#[test]
fn a_403_is_not_allowed_a_401_sign_in_refused_a_429_busy() {
    let standin = standin();
    standin.state().directory_denied = true;
    assert_eq!(search(&standin, "help").outcome, DirectoryOutcome::NotAllowed);
    standin.state().directory_denied = false;
    assert_eq!(search_directory_at(&standin.url(), "another.token-0000", &owner(), &query("help")).outcome, DirectoryOutcome::SignInRefused);
    assert_eq!(search_directory_at(&standin.url(), "", &owner(), &query("help")).outcome, DirectoryOutcome::SignInRefused, "no token, nothing sent");
    standin.state().retry_after = Some(60);
    standin.state().fail_next.push(("GET /v1.0/users".into(), 429, "TooManyRequests".into()));
    assert_eq!(search(&standin, "help").outcome, DirectoryOutcome::Busy);
    standin.state().fail_next.push(("GET /v1.0/users".into(), 504, "ServiceUnavailable".into()));
    assert_eq!(search(&standin, "help").outcome, DirectoryOutcome::Busy);
    standin.state().fail_next.push(("GET /v1.0/users".into(), 500, "ServiceUnavailable".into()));
    assert_eq!(search(&standin, "help").outcome, DirectoryOutcome::Unreachable);
    // A search without the header is refused by Graph (400): the stand-in says so, and a 400 is
    // "couldn't search".
    standin.state().fail_next.push(("GET /v1.0/users".into(), 400, "ErrorInvalidRequest".into()));
    let answer = search(&standin, "help");
    assert_eq!((answer.outcome, answer.entries.len()), (DirectoryOutcome::Unreachable, 0));
    // Not reachable at all.
    let gone = format!("http://127.0.0.1:{}", { std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port() });
    assert_eq!(search_directory_at(&gone, TOKEN, &owner(), &query("help")).outcome, DirectoryOutcome::Unreachable);
}

#[test]
fn one_retry_on_429_with_a_short_retry_after_and_none_with_a_long_one() {
    let standin = standin();
    let users = |s: &StandIn| s.state().requests.len();
    // Told to wait a second: asked once more, and found.
    standin.state().retry_after = Some(1);
    standin.state().fail_next.push(("GET /v1.0/users".into(), 429, "TooManyRequests".into()));
    let answer = search(&standin, "help");
    assert_eq!((answer.outcome, users(&standin)), (DirectoryOutcome::Found, 2));
    // Twice busy: one more try only.
    for _ in 0..2 {
        standin.state().fail_next.push(("GET /v1.0/users".into(), 503, "ServiceUnavailable".into()));
    }
    assert_eq!(search(&standin, "help").outcome, DirectoryOutcome::Busy);
    assert_eq!(users(&standin), 4);
    standin.state().fail_next.clear();
    // Told to wait longer than five seconds: not asked again.
    standin.state().retry_after = Some(MAX_RETRY_WAIT + 1);
    standin.state().fail_next.push(("GET /v1.0/users".into(), 429, "TooManyRequests".into()));
    assert_eq!(search(&standin, "help").outcome, DirectoryOutcome::Busy);
    assert_eq!(users(&standin), 5);
    assert!(standin.state().fail_next.is_empty());
}

/// A 504 (the gateway gave up) is busy and never asked again, even when it says to come back in
/// a second; a 503 that says the same is asked once more (the control).
#[test]
fn a_504_is_not_asked_again_even_with_a_short_retry_after() {
    let standin = standin();
    {
        let mut state = standin.state();
        state.retry_after = Some(1);
        state.retry_after_on_504 = true;
        state.fail_next.push(("GET /v1.0/users".into(), 504, "ServiceUnavailable".into()));
    }
    assert_eq!(search(&standin, "help").outcome, DirectoryOutcome::Busy);
    assert_eq!(standin.state().requests.len(), 1, "one request");
    standin.state().fail_next.push(("GET /v1.0/users".into(), 503, "ServiceUnavailable".into()));
    assert_eq!(search(&standin, "help").outcome, DirectoryOutcome::Found);
    assert_eq!(standin.state().requests.len(), 3, "the 503 was asked once more");
}

/// An answer of `bytes` bytes or a little more: a page whose one entry is a help desk, padded out
/// with a long name.
fn padded_page(bytes: usize) -> String {
    let pad = "p".repeat(bytes);
    serde_json::json!({"value": [{"displayName": "Help Desk", "mail": "helpdesk@example.org", "jobTitle": pad}]}).to_string()
}

#[test]
fn an_answer_over_256_kb_is_refused_without_reading_on() {
    let standin = standin();
    // Just under the cap: read.
    standin.state().directory_answer = Some(padded_page(MAX_ANSWER - 1_024));
    let answer = search(&standin, "help");
    assert_eq!((answer.outcome, addresses(&answer)), (DirectoryOutcome::Found, vec!["helpdesk@example.org"]));
    // Over it: refused, whatever it holds.
    for over in [MAX_ANSWER + 1, 4 * MAX_ANSWER, 16 * MAX_ANSWER] {
        standin.state().directory_answer = Some(padded_page(over));
        let answer = search(&standin, "help");
        assert_eq!((answer.outcome, answer.entries.len()), (DirectoryOutcome::Unreachable, 0), "{over}");
    }
    assert_eq!(MAX_ANSWER, 256 * 1024);
}

#[test]
fn a_personal_account_is_never_searched() {
    let standin = standin();
    let mut personal = owner();
    personal.oauth = Some(OAuth { directory: Directory::Personal, ..Default::default() });
    let mut outlook = owner();
    outlook.email = "ada@outlook.com".into();
    outlook.oauth = Some(OAuth::default());
    let shared = Account { shared_of: Some(3), ..owner() };
    let imap = Account { kind: AccountKind::Imap, ..owner() };
    for account in [personal, outlook, shared, imap] {
        let answer = search_directory_at(&standin.url(), TOKEN, &account, &query("help"));
        assert_eq!(answer.outcome, DirectoryOutcome::NothingAsked, "{}", account.email);
    }
    // Nor for another account than the one the search names.
    let other = Account { id: OWNER + 1, ..owner() };
    assert_eq!(search_directory_at(&standin.url(), TOKEN, &other, &query("help")).outcome, DirectoryOutcome::NothingAsked);
    assert!(standin.state().requests.is_empty());
}

#[test]
fn hostile_answers_never_panic() {
    let standin = standin();
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    let hostile = [
        String::new(),
        "not json".into(),
        "null".into(),
        r#"{"value": "x"}"#.into(),
        r#"{"value": [1, "a", null, {"mail": ["x"]}, {"mail": "a@example.org", "displayName": {"x": 1}}]}"#.into(),
        r#"{"value": [{"mail": "a@example.org\r\nBcc: b@example.org"}], "@odata.nextLink": 5}"#.into(),
        format!(r#"{{"value": [{{"mail": "a@example.org", "displayName": "{}"}}]}}"#, "\\u202e".repeat(5_000)),
        deep,
        "\u{feff}{\"value\": []}".into(),
    ];
    for body in hostile {
        standin.state().directory_answer = Some(body.clone());
        let answer = search(&standin, "help");
        assert!(answer.entries.len() <= MAX_RESULTS);
        assert!(matches!(answer.outcome, DirectoryOutcome::Found | DirectoryOutcome::Unreachable), "{:?}", body.get(..40));
        for entry in &answer.entries {
            assert!(!entry.name.chars().any(|c| c.is_control() || c == '\u{202e}'), "{:?}", entry.name);
        }
    }
}

// --- The stand-in's own rules for the directory, as the reference has them.

/// A search of the stand-in's directory with `query` (as written, already encoded) and the
/// headers `more`, through Graph as a client would ask.
fn raw(standin: &StandIn, query: &str, more: &[(&str, &str)]) -> (u16, serde_json::Value) {
    let graph = Graph::at(&standin.url(), TOKEN).unwrap();
    let reply = graph.ask(&graph.url(&format!("/v1.0/users?{query}")), "GET", more, None, TIMEOUT, MAX_ANSWER).unwrap();
    (reply.status, serde_json::from_slice(&reply.body).unwrap_or_default())
}

fn search_of(text: &str) -> String {
    http::form(&[("$search", &clauses(text)), ("$select", FIELDS), ("$top", "15")])
}

#[test]
fn standin_refuses_search_without_consistency_level() {
    let standin = standin();
    let (status, body) = raw(&standin, &search_of("help"), &[]);
    assert_eq!(status, 400);
    assert!(body["error"]["message"].as_str().unwrap().contains("ConsistencyLevel"), "{body}");
    let (status, body) = raw(&standin, &search_of("help"), &[("ConsistencyLevel", "eventual")]);
    // (The entry without a mailbox is listed too, its `mail` null: the client leaves it out.)
    assert_eq!((status, body["value"].as_array().map(Vec::len)), (200, Some(3)));
    // A listing without a search needs no header.
    let (status, _) = raw(&standin, "$select=displayName&$top=5", &[]);
    assert_eq!(status, 200);
}

#[test]
fn standin_matches_display_name_tokens_and_mail_prefixes() {
    let standin = standin();
    let eventual = [("ConsistencyLevel", "eventual")];
    let found = |query: &str| -> Vec<String> {
        let (status, body) = raw(&standin, &http::form(&[("$search", query), ("$select", "mail")]), &eventual);
        assert_eq!(status, 200, "{query}: {body}");
        body["value"].as_array().unwrap().iter().filter_map(|u| u["mail"].as_str().map(str::to_string)).collect()
    };
    assert_eq!(found(r#""displayName:desk""#), ["helpdesk@example.org"]);
    assert_eq!(found(r#""displayName:desk help""#), ["helpdesk@example.org"], "words in any order");
    assert_eq!(found(r#""displayName:DE""#), ["helpdesk@example.org"], "the start of a word, any case");
    assert!(found(r#""displayName:elp""#).is_empty(), "not inside a word");
    assert_eq!(found(r#""mail:sales""#), ["sales@example.org"]);
    assert!(found(r#""mail:example""#).is_empty(), "an address from its start");
    assert_eq!(found(r#""userPrincipalName:sales.t""#), ["sales@example.org"]);
    assert_eq!(found(r#""displayName:team" OR "mail:lena""#), ["lena@example.org", "sales@example.org"]);
    // What it can't read is a 400.
    for query in [r#"displayName:desk"#, r#""displayName:desk"#, r#""jobTitle:x""#, r#""displayName:a" AND "mail:b""#] {
        let (status, _) = raw(&standin, &http::form(&[("$search", query)]), &eventual);
        assert_eq!(status, 400, "{query}");
    }
}

#[test]
fn standin_refuses_a_select_outside_read_basic() {
    let standin = standin();
    let eventual = [("ConsistencyLevel", "eventual")];
    for select in ["displayName,jobTitle", "accountEnabled", "mail,userType", "showInAddressList"] {
        let (status, body) = raw(&standin, &http::form(&[("$select", select)]), &eventual);
        assert_eq!(status, 403, "{select}: {body}");
    }
    let (status, body) = raw(&standin, &http::form(&[("$select", "id,displayName,givenName,surname,mail,userPrincipalName")]), &eventual);
    assert_eq!(status, 200);
    let first = &body["value"][0];
    assert!(first.get("jobTitle").is_none() && first["id"].is_string() && first["givenName"].is_null(), "{first}");
    // Paging as Graph pages a list: a next link while there is more; $top of 1 to 999.
    let (_, body) = raw(&standin, "$top=2", &eventual);
    assert!(body["@odata.nextLink"].is_string());
    for top in ["0", "1000", "x"] {
        assert_eq!(raw(&standin, &format!("$top={top}"), &eventual).0, 400, "{top}");
    }
}
