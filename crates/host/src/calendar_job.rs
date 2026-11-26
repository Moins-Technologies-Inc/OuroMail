// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar's job between Send/Receive runs, for every front end (the desktop's window and the
//! phone's core alike): the request a plan makes, the job in flight, and its answer taken in.
//!
//! What is due and what each job asks for is the engine's ([`mailcraft_engine::Session::calendar_due`],
//! [`CalendarPlan`]); which sign-in a job goes with (the account's, a shared mailbox's owner's, a
//! token kept from earlier) and whether the Favorites ride along are the front end's, handed in as
//! [`CalendarAuth`]. [`CalendarJobs::start`] makes the request ([`calendar_request`]), has the front
//! end start it its own way and tells the engine; [`CalendarJobs::poll`] hands the answer back, and
//! [`take_answer`] has the engine take it in ([`mailcraft_engine::Session::calendar_answered`]).
//! The front end keeps the tokens and says what the owner is told.
//!
//! Nothing of an event is logged, and no token appears in a `Debug` output.

use std::sync::mpsc::{Receiver, TryRecvError};

use mailcraft_engine::Session;
use mailcraft_engine::sync_driver::{CalendarCame, CalendarPlan, CalendarTaken};
use mailcraft_model::{Account, Id, OAuth};

use crate::graph::{GraphRequest, GraphResult};
use crate::job_sign_in::{ForeignSignIn, foreign_sign_in, normalized_address, same_sign_in_identity};

/// The sign-in a calendar job goes with, as the front end found it.
pub struct CalendarAuth {
    /// The sign-in settings of the account whose sign-in it is, with the application id filled in.
    pub oauth: OAuth,
    /// That account's refresh token, to get a Graph token with when `access` is empty.
    pub refresh: String,
    /// A Graph access token of this session that is still good; else empty.
    pub access: String,
    /// The Favorites as Outlook has them ride along ([`GraphRequest::favorites`]).
    pub favorites: bool,
}

impl std::fmt::Debug for CalendarAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CalendarAuth").field("favorites", &self.favorites).finish_non_exhaustive()
    }
}

/// The request for the calendar job `plan` says, with `auth`: its changes when it carries any, then
/// the days asked for (the wide stretch with the contacts when it brings the directory, counted
/// from this session's clock as its asks are). Nothing of the mail. The run is to check that a
/// token it renews is the signer's ([`GraphRequest::signer`]): the account's own, or for a shared
/// mailbox the owner's it is opened through. `None` when the plan's account (or that owner) is gone.
pub fn calendar_request(session: &Session, plan: &CalendarPlan, auth: CalendarAuth) -> Option<GraphRequest> {
    let account = session.mailbox.account(plan.account)?.clone();
    // A shared mailbox's calendars are read with the sign-in of the account it is opened through.
    let signer = if account.is_shared() { session.mailbox.owner_of(plan.account)?.clone() } else { account.clone() };
    Some(GraphRequest {
        account,
        oauth: auth.oauth,
        refresh: auth.refresh,
        access: auth.access,
        outbox: Vec::new(),
        directory: plan.directory,
        calendar: Some(plan.range(session.now())),
        changes: plan.changes.clone(),
        asks: session.calendar_asks_for(plan),
        settings: plan.settings,
        favorites: auth.favorites,
        favorites_scopes: auth.favorites,
        mail: None,
        mail_pieces: None,
        cancel: Default::default(),
        signer: Some(signer),
    })
}

/// The job under way.
struct Running {
    account: Id,
    /// Whether the job was launched for a shared mailbox.
    is_shared: bool,
    /// Normalized shared mailbox address at launch (empty when `!is_shared`).
    shared_email: String,
    /// The account whose sign-in the job went with (a shared mailbox's owner), as it was: the
    /// tokens that come back must be its own ([`foreign_sign_in`]), and the live account/owner
    /// must still have the same sign-in identity when the answer arrives.
    signer: Account,
    carried: bool,
    started: f64,
    opening: u64,
    rx: Receiver<GraphResult>,
}

impl Running {
    /// Whether `session` still has the same mailbox opening, target account kind/address, and
    /// signer credential identity as when this job started.
    fn matches_session(&self, session: &Session) -> bool {
        if self.opening != session.openings() {
            return false;
        }
        let Some(live_account) = session.mailbox.account(self.account) else {
            return false;
        };
        if live_account.is_shared() != self.is_shared {
            return false;
        }
        if self.is_shared {
            live_account.shared_of == Some(self.signer.id)
                && normalized_address(&live_account.email) == self.shared_email
                && session.mailbox.owner_of(self.account).is_some_and(|owner| same_sign_in_identity(&self.signer, owner))
        } else {
            same_sign_in_identity(&self.signer, live_account)
        }
    }
}

/// The calendar's job in flight, at most one.
#[derive(Default)]
pub struct CalendarJobs {
    running: Option<Running>,
}

impl std::fmt::Debug for CalendarJobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CalendarJobs").field("running", &self.running.as_ref().map(|r| r.account)).finish()
    }
}

/// What a calendar job's answer is, before the engine takes it in.
pub struct CalendarAnswer {
    pub account: Id,
    /// Whether the job was launched for a shared mailbox (captured at launch, never inferred
    /// post-hoc from a possibly removed account).
    pub is_shared: bool,
    /// The job carried changes up.
    pub carried: bool,
    /// When it started (milliseconds).
    pub started: f64,
    /// What it brought; an empty answer when it ended without one (`stopped_early`).
    pub result: GraphResult,
    /// The job ended without an answer (its thread ended early).
    pub stopped_early: bool,
    /// The answer came with somebody else's sign-in: it was refused whole (`result` is empty, and
    /// its token is not to be kept). The front end tells the owner; the engine takes it as a job
    /// that failed ([`take_answer`]).
    pub foreign: Option<ForeignSignIn>,
}

/// What [`CalendarJobs::poll`] found.
pub enum CalendarPolled {
    /// No job is in flight, or its answer hasn't come.
    Nothing,
    /// The job in flight was for a mailbox that is no longer open: it was let go of (AU3-V8).
    Stale,
    /// The answer.
    Answer(Box<CalendarAnswer>),
}

impl CalendarJobs {
    /// Whether a job is in flight.
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// The account of the job in flight.
    pub fn account(&self) -> Option<Id> {
        self.running.as_ref().map(|r| r.account)
    }

    /// Whether the job in flight belongs to `account` or uses `account`'s sign-in (for a shared
    /// mailbox owned by `account`).
    pub fn involves(&self, account: Id) -> bool {
        self.running.as_ref().is_some_and(|r| r.account == account || r.signer.id == account)
    }

    /// Whether the job in flight (if any) still matches `session`'s current mailbox opening,
    /// target account kind/address, and signer credential identity.
    pub fn matches_session(&self, session: &Session) -> bool {
        self.running.as_ref().is_none_or(|r| r.matches_session(session))
    }

    /// Let go of the job in flight (another mailbox took the place of the open one).
    pub fn forget(&mut self) {
        self.running = None;
    }

    /// Start the calendar job `plan` says, with `auth`, at `now` (milliseconds): `launch` starts it
    /// its front end's way, `None` when it can't (no host for it, Work Offline). Whether it was
    /// started; the engine is told when it was ([`Session::calendar_started`]).
    pub fn start(
        &mut self,
        session: &mut Session,
        plan: &CalendarPlan,
        auth: CalendarAuth,
        now: f64,
        launch: impl FnOnce(GraphRequest) -> Option<Receiver<GraphResult>>,
    ) -> bool {
        if self.running.is_some() {
            return false;
        }
        let Some(request) = calendar_request(session, plan, auth) else { return false };
        let is_shared = request.account.is_shared();
        let shared_email = if is_shared { normalized_address(&request.account.email) } else { String::new() };
        // (Checked again here when the answer comes: the run checks it before anything goes up.)
        let Some(signer) = request.signer.clone() else { return false };
        let Some(rx) = launch(request) else { return false };
        session.calendar_started(plan, now);
        self.running = Some(Running {
            account: plan.account,
            is_shared,
            shared_email,
            signer,
            carried: plan.changes.is_some(),
            started: now,
            opening: session.openings(),
            rx,
        });
        true
    }

    /// The answer of the job in flight, when it has come.
    pub fn poll(&mut self, session: &Session) -> CalendarPolled {
        let Some(running) = self.running.as_ref() else { return CalendarPolled::Nothing };
        // For a mailbox that has been closed since, or whose target account or signer identity
        // changed or was removed while the job was in flight: the answer is nobody's (AU3-V8).
        if !running.matches_session(session) {
            log::info!("calendar: the job's mailbox or sign-in identity changed while it ran; let go of");
            self.running = None;
            return CalendarPolled::Stale;
        }
        let (result, stopped_early) = match running.rx.try_recv() {
            Ok(result) => (result, false),
            Err(TryRecvError::Empty) => return CalendarPolled::Nothing,
            Err(TryRecvError::Disconnected) => {
                log::warn!("calendar: the job ended without an answer (its thread ended early)");
                (GraphResult { account: running.account, ..Default::default() }, true)
            }
        };
        let Some(running) = self.running.take() else { return CalendarPolled::Nothing };
        // Somebody else's sign-in: nothing of what it brought is this account's calendar, and its
        // token is not kept. (Logged without naming anybody.)
        let foreign = foreign_sign_in(&running.signer, result.tokens.as_ref());
        let result = if foreign.is_some() {
            log::warn!("calendar: the answer came with another account's sign-in; refused whole");
            GraphResult { account: running.account, ..Default::default() }
        } else {
            result
        };
        CalendarPolled::Answer(Box::new(CalendarAnswer {
            account: running.account,
            is_shared: running.is_shared,
            carried: running.carried,
            started: running.started,
            result,
            stopped_early,
            foreign,
        }))
    }
}

/// What the engine keeps of a calendar job's answer: `job_errors` says the front end has lines of
/// the job's own for the owner (the wide stretch and the contacts are then asked for again).
pub fn came(result: &GraphResult, job_errors: bool) -> CalendarCame<'_> {
    CalendarCame {
        uploaded: result.uploaded.as_ref(),
        events: result.events.as_ref(),
        more_events: &result.more_events,
        calendars: result.calendars.as_ref(),
        categories: result.categories.as_ref(),
        extras_missing: result.extras_missing,
        hours: result.mailbox_hours.as_ref(),
        contacts: result.contacts.as_ref(),
        job_errors,
        consent_needed: result.consent_needed,
        token_refused: result.token_refused,
        sign_in_refused: result.sign_in_refused,
    }
}

/// Have the engine take a calendar job's answer in at `now` (milliseconds): what the server said
/// to the changes and what it has in the days asked for, and where the owner's permission stands.
/// What the owner is to be told comes back. An answer refused for its sign-in
/// ([`CalendarAnswer::foreign`]) is a job that failed, whatever `job_errors` says.
pub fn take_answer(session: &mut Session, answer: &CalendarAnswer, job_errors: bool, now: f64) -> CalendarTaken {
    let job_errors = job_errors || answer.foreign.is_some();
    session.calendar_answered(answer.account, answer.carried, answer.started, now, &came(&answer.result, job_errors))
}
