// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Add Shared Mailbox's search of the organisation's directory, on a background thread of the
//! host's (`Services::graph_directory`): a token for the directory of its own when the window has
//! none (`oauth::refresh_graph_directory`, which names `User.ReadBasic.All` and nothing else that a
//! sign-in doesn't have), then one search of Microsoft Graph's (`graph::search_directory`, which
//! may wait a few seconds before its one more try and so is never run on the window's thread).
//!
//! The typed text goes to Graph only with a token that covers the directory: a sign-in that
//! doesn't cover it gets no search at all (the window asks the owner). A dialog closed while the
//! token comes has nothing sent, and one closed while the search waits to ask once more (after a
//! busy answer) has nothing more sent; a request already on its way finishes, and the window drops
//! its answer. Nothing of the text, the token or what was found is logged.

use std::sync::atomic::{AtomicBool, Ordering};

use mailcraft_model::directory::{DirectoryAnswer, DirectoryOutcome, DirectoryQuery};
use mailcraft_model::{Account, OAuth};
use mailcraft_net::NetError;
use mailcraft_net::oauth::{GraphTokens, Resource};
use mailcraft_ui_egui::directory_search::{DirectoryJob, DirectoryResult};

/// How a token for the directory is had.
type Renew<'a> = &'a dyn Fn(&OAuth, &str, Resource) -> Result<GraphTokens, NetError>;
/// How the directory is searched with a token.
type Search<'a> = &'a dyn Fn(&str, &Account, &DirectoryQuery, &AtomicBool) -> DirectoryAnswer;

/// Run the job against Microsoft's own services.
pub(crate) fn run(job: DirectoryJob) -> DirectoryResult {
    run_with(job, &mailcraft_net::oauth::refresh_graph_directory, &mailcraft_net::graph::search_directory_unless)
}

/// [`run`], with `renew` and `search` in the place of Microsoft's.
pub(crate) fn run_with(job: DirectoryJob, renew: Renew<'_>, search: Search<'_>) -> DirectoryResult {
    let said = |outcome| DirectoryAnswer { account: job.query.account, ticket: job.query.ticket, outcome, ..Default::default() };
    let stopped = || job.cancel.load(Ordering::Relaxed);
    let mut out = DirectoryResult { answer: said(DirectoryOutcome::NothingAsked), tokens: None };
    if stopped() {
        return out;
    }
    let mut access = job.access.clone();
    let mut renewed = false;
    loop {
        if access.is_empty() {
            if job.refresh.is_empty() {
                out.answer = said(DirectoryOutcome::SignInRefused);
                return out;
            }
            match renew(&job.oauth, &job.refresh, mailcraft_host::tasks::mail_resource(&job.owner)) {
                Ok(got) => {
                    access = got.tokens.access.clone();
                    out.tokens = Some(got.tokens);
                    renewed = true;
                }
                Err(e) => {
                    let outcome = if e.is_consent_needed() {
                        DirectoryOutcome::ConsentNeeded
                    } else if e.is_refused_sign_in() {
                        DirectoryOutcome::SignInRefused
                    } else {
                        DirectoryOutcome::Unreachable
                    };
                    out.answer = said(outcome);
                    return out;
                }
            }
            // The dialog went while the token came: the text isn't sent.
            if stopped() || access.is_empty() {
                return out;
            }
        }
        let answer = search(&access, &job.owner, &job.query, &job.cancel);
        // A token of this session that is no longer taken: once more, with a new one.
        if answer.outcome == DirectoryOutcome::SignInRefused && !renewed && !job.refresh.is_empty() && !stopped() {
            access.clear();
            continue;
        }
        out.answer = answer;
        return out;
    }
}

#[cfg(test)]
#[path = "directory_job_tests.rs"]
mod tests;
