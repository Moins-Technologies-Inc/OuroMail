// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The organisation's directory, searched for a shared mailbox to open (Add Shared Mailbox).
//!
//! One request, `GET /v1.0/users`, with the account's own Graph token and nothing else of the
//! owner: the text typed ([`mailcraft_model::directory::search_text`]) as three clauses of a
//! `$search` (`"displayName:…" OR "mail:…" OR "userPrincipalName:…"`, each `"` and `\` in it
//! escaped with a backslash), the header `ConsistencyLevel: eventual` that a search of the
//! directory needs, `$select=displayName,mail,userPrincipalName` (no more than what is shown, all
//! of them in what `User.ReadBasic.All` lets be read) and `$top=15`.
//!
//! The answer is hostile and bounded: at most [`MAX_ANSWER`] bytes (a longer one is refused
//! without being read on), ten seconds, its entries read by
//! [`mailcraft_model::directory::read_page`]. A next page is never asked for: that there is one
//! only says that the owner may type on. A busy Microsoft (429, 503) is asked once more when it
//! says to wait at most [`MAX_RETRY_WAIT`] seconds, and never otherwise. A personal Microsoft
//! account has no directory: nothing is asked for one, nor for a shared mailbox itself.
//!
//! Nothing here asks for a permission, and nothing calls it yet but the tests: the window's search
//! and the permission it needs come with it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mailcraft_model::Account;
use mailcraft_model::directory::{DirectoryAnswer, DirectoryOutcome, DirectoryQuery, MAX_RESULTS, read_page};

use super::{Graph, wait_of};
use crate::http;

/// Where the directory is listed.
const USERS: &str = "/v1.0/users";
/// What is asked of each entry.
pub(crate) const FIELDS: &str = "displayName,mail,userPrincipalName";
/// What each of the clauses of a search looks in.
const SEARCHED: [&str; 3] = ["displayName", "mail", "userPrincipalName"];
/// Largest answer taken: fifteen entries are a few kilobytes.
pub(crate) const MAX_ANSWER: usize = 256 * 1024;
/// How long an answer is waited for: the owner is waiting at a text field.
const TIMEOUT: Duration = Duration::from_secs(10);
/// How often a wait before the one more try looks whether the search was called off.
const CALL_OFF_STEP: Duration = Duration::from_millis(50);
/// Longest wait a busy Microsoft may ask for before the one more try; a longer one is busy.
pub(crate) const MAX_RETRY_WAIT: u32 = 5;

/// Search the directory of `owner`'s organisation for what `query` says, with `token` (a Graph
/// token of `owner`'s sign-in). Nothing is changed anywhere. It blocks: for the answer (ten
/// seconds at most) and, before its one more try, up to [`MAX_RETRY_WAIT`] seconds asleep on the
/// calling thread, so a caller runs it off the window's thread.
pub fn search_directory(token: &str, owner: &Account, query: &DirectoryQuery) -> DirectoryAnswer {
    search_directory_at(&super::graph_base(), token, owner, query)
}

/// [`search_directory`], called off by `cancel`: once it is set, nothing more is sent (a wait to
/// ask once more ends at once, and that request is never made). The request already on its way
/// finishes; what it brings is the caller's to drop.
pub fn search_directory_unless(token: &str, owner: &Account, query: &DirectoryQuery, cancel: &AtomicBool) -> DirectoryAnswer {
    search_directory_at_unless(&super::graph_base(), token, owner, query, cancel)
}

/// The `$search` of a search for `text`: its three clauses.
pub(crate) fn clauses(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '"' | '\\') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    SEARCHED.iter().map(|field| format!("\"{field}:{escaped}\"")).collect::<Vec<_>>().join(" OR ")
}

/// [`search_directory`] of Graph at `base`.
pub(super) fn search_directory_at(base: &str, token: &str, owner: &Account, query: &DirectoryQuery) -> DirectoryAnswer {
    search_directory_at_unless(base, token, owner, query, &AtomicBool::new(false))
}

/// [`search_directory_unless`] of Graph at `base`.
pub(super) fn search_directory_at_unless(base: &str, token: &str, owner: &Account, query: &DirectoryQuery, cancel: &AtomicBool) -> DirectoryAnswer {
    let answer = |outcome| DirectoryAnswer { account: query.account, ticket: query.ticket, outcome, ..Default::default() };
    // (A personal account, a shared mailbox, another account than the one the search is for.)
    if !owner.can_open_shared() || owner.id != query.account {
        return answer(DirectoryOutcome::NothingAsked);
    }
    let Some(text) = query.search_text() else { return answer(DirectoryOutcome::NothingAsked) };
    let Ok(graph) = Graph::at(base, token) else { return answer(DirectoryOutcome::SignInRefused) };
    let search = clauses(&text);
    let top = MAX_RESULTS.to_string();
    let url = graph.url(&format!("{USERS}?{}", http::form(&[("$search", &search), ("$select", FIELDS), ("$top", &top)])));
    let mut retried = false;
    loop {
        let reply = match graph.ask(&url, "GET", &[("ConsistencyLevel", "eventual")], None, TIMEOUT, MAX_ANSWER) {
            Ok(reply) => reply,
            Err(e) if e.is_refused_sign_in() => return answer(DirectoryOutcome::SignInRefused),
            // (An answer too long, cut off or late, or no connection.)
            Err(_) => return answer(DirectoryOutcome::Unreachable),
        };
        let outcome = match reply.status {
            200 => {
                return match read_page(&reply.body) {
                    Some(page) => DirectoryAnswer { entries: page.entries, more: page.more, ..answer(DirectoryOutcome::Found) },
                    None => answer(DirectoryOutcome::Unreachable),
                };
            }
            401 => DirectoryOutcome::SignInRefused,
            403 => DirectoryOutcome::NotAllowed,
            429 | 503 | 504 => DirectoryOutcome::Busy,
            _ => DirectoryOutcome::Unreachable,
        };
        // Asked once more, and only when it says to wait a little.
        let wait = wait_of(reply.retry_after.as_deref(), mailcraft_time::now());
        match (outcome, wait) {
            (DirectoryOutcome::Busy, Some(seconds)) if !retried && matches!(reply.status, 429 | 503) && seconds <= MAX_RETRY_WAIT => {
                retried = true;
                // Waited in short steps: called off meanwhile (the dialog went, or its text
                // changed), nothing more is sent.
                let until = Instant::now() + Duration::from_secs(u64::from(seconds));
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        return answer(DirectoryOutcome::NothingAsked);
                    }
                    let left = until.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    std::thread::sleep(left.min(CALL_OFF_STEP));
                }
            }
            _ => return answer(outcome),
        }
    }
}

#[cfg(test)]
#[path = "directory_tests.rs"]
mod tests;
