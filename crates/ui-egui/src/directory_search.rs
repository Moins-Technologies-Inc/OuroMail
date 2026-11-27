// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Add Shared Mailbox's search of the organisation's directory, as the owner types.
//!
//! The dialog's search field asks Microsoft Graph's list of the organisation's people and shared
//! mailboxes (`mailcraft_net::graph::search_directory`, `GET /v1.0/users` with `$search`), which
//! can't tell one from the other with the one permission it takes (`User.ReadBasic.All`): every
//! entry with a usable address is listed, at most 15, with a line that says so, and the one the
//! owner picks is added and checked as a typed address is (`account.addSharedMailbox`, S1).
//!
//! **What goes, and when.** Only the text the owner typed into the search field, as
//! `sync.directoryRequest` cleans it (at most 64 characters), with a token of the account's own
//! for the directory, to Microsoft Graph and nowhere else; a search starts only while the dialog
//! is open, for a work or school account (never a personal one, nor a shared mailbox), not while
//! working offline, and only after at least two characters and a pause of [`PAUSE_MS`] in the
//! typing. One search is in flight at a time: text typed meanwhile is searched once that answer is
//! in. A request already on its way when the dialog closes or its text changes finishes, and its
//! answer is dropped; a wait to ask once more after a busy answer ends there, with nothing more
//! sent. An answer for older text, for a dialog that went, for another mailbox, or for an account
//! whose sign-in changed or went since it was asked is dropped, tokens and all; a search that
//! stays unanswered [`ANSWER_DEADLINE_MS`] is called off and said to have failed, and no other
//! search starts until the host's own one has ended (it may still be on its way). Keys and text
//! that an agent or a script types into the window (`MailApp::driven`: `ui.text`, `ui.key`) are
//! never searched; `ui.searchDirectory`, with which the owner's own scripts search, is never for
//! agents, nor are `sync.directoryRequest` and the answer to the permission
//! (`ui.allowDirectory`). Nothing of the text or of what was found is logged, put in the status
//! bar, kept in the mailbox or in `ui.json`, or shown to `ui.inspect` (the results are hit regions
//! by number, and only the rows in view); it all goes with the dialog. The token for the directory
//! is held in memory for the account's sign-in in the open mailbox only: it goes, with whether the
//! permission was given, when another mailbox is opened, the account is removed, its sign-in
//! changes (or an Undo changes it back), a mail sign-in is refused, and when MailCraft's data is
//! removed. Each is also stamped with the mailbox opening and the sign-in it was had for
//! (`Stamp`), and is used only while both are still those: a place that forgot to let go of it
//! changes nothing.
//!
//! **The permission.** It is asked for by itself, the first time a search finds the sign-in doesn't
//! cover it: the dialog asks the owner first, in its own words, with Allow and Not now, and only
//! Allow opens Microsoft's page, for that permission alone
//! (`mailcraft_net::oauth::sign_in_for_directory`). When it was given already (by the owner in
//! an earlier session, or by an administrator for everybody), the first search goes without a
//! question. The question keeps the keyboard until it is answered: the search field takes no keys
//! meanwhile (what was typed before stays), so typing answers nothing, and keys or clicks an agent
//! or a script puts into the window never move the keyboard onto Allow or Not now. Only the
//! owner answers it, a no included: his own Not now, Escape or close of the dialog is a no for
//! this session (nothing more is sent for it, and the address field and Add work as before); when
//! an agent or a script closes the dialog (`ui.closeDialog`, or a key or a click it puts in),
//! nothing is recorded and the question comes again at the next search. Allow that can't open the
//! page says why in the dialog and leaves the question up. A page called off before it answered
//! (Cancel, or Add pressed while it waited) records no answer either: the question comes again,
//! and File › Info says it wasn't answered, with Ask again. A no on the page, or one an administrator has to turn into a yes, is
//! said in the dialog with the approval help the other permissions have. File › Info says where it
//! stands in this session (nothing of it is kept across a restart, and nothing is asked of
//! Microsoft to say it), with Ask again.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Key, RichText, Sense, Ui, vec2};
use mailcraft_model::directory::{DirectoryAnswer, DirectoryEntry, DirectoryOutcome, DirectoryQuery, MAX_RESULTS};
use mailcraft_model::{Account, Id};
use serde_json::{Value, json};

use crate::dialogs::Dialog;
use crate::theme::Tokens as Theme;
use crate::{MailApp, Tokens};

/// How long the owner's typing pauses before a search goes, in milliseconds.
pub const PAUSE_MS: f64 = 300.0;
/// How long a search may go unanswered before the dialog stops waiting and says Microsoft couldn't
/// be reached, in milliseconds. The host's own worst path is longer (a stale token's search, its
/// wait and one more try, a new token and the same again: each request may take its patience to
/// connect and as long again for the rest, about two minutes in all), so the host's job is called
/// off then and no other search starts until it has ended.
pub(crate) const ANSWER_DEADLINE_MS: f64 = 45_000.0;
/// When a frame comes after a keystroke, so that the pause is seen to end though nothing moves.
const WAKE: std::time::Duration = std::time::Duration::from_millis(320);
/// How many rows of results the list shows before it scrolls.
const ROWS_SHOWN: f32 = 6.0;
/// The height of one row of results.
const ROW: f32 = 38.0;

/// What the host is asked to search ([`crate::Services::graph_directory`]). Deliberately without
/// `Debug`: it holds the typed text and the account's tokens.
pub struct DirectoryJob {
    /// The account whose organisation is searched, as it is now.
    pub owner: Account,
    /// Its sign-in settings, with the application id filled in.
    pub oauth: mailcraft_model::OAuth,
    /// Its refresh token: the job gets a token for the directory with it when `access` is empty
    /// or refused.
    pub refresh: String,
    /// A token for the directory of this session that is still good, else empty.
    pub access: String,
    /// What to search for, as `sync.directoryRequest` gave it, and the ticket of the search.
    pub query: DirectoryQuery,
    /// Set when the dialog went: nothing more is sent.
    pub cancel: Arc<AtomicBool>,
}

/// What a [`DirectoryJob`] came to. Never an error as a whole: the answer says how it went.
#[derive(Default)]
pub struct DirectoryResult {
    pub answer: DirectoryAnswer,
    /// The tokens the job got for the directory, when it got some: the refresh token is the
    /// account's newest.
    pub tokens: Option<Tokens>,
}

/// Where the directory's permission stands for an account, in this session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Permission {
    /// A search had a token for it.
    Given,
    /// Microsoft's page for it is open in the browser.
    Asking,
    /// The owner said Not now, closed the question, or said no on the page.
    Declined,
    /// An administrator has to approve it first.
    Approval,
    /// The organisation doesn't let its people give it.
    Organisation,
    /// Microsoft's page was called off before it answered: no answer is recorded, and the question
    /// comes again at the next search.
    Unanswered,
}

/// The search under way and what is known of the permission, in the window.
#[derive(Default)]
pub struct DirectorySearching {
    pending: Option<Pending>,
    pub(crate) perm: HashMap<Id, Permission>,
    /// Tokens for the directory, in memory for the session only (never stored).
    access: HashMap<Id, Tokens>,
    tickets: u64,
    /// The account whose dialog puts the question now: closed unanswered, it is a no.
    offering: Option<Id>,
    /// The account whose question the owner himself closed (his Escape or close, never an
    /// agent's or a script's): that is his no.
    pub(crate) declined_by_owner: Option<Id>,
    /// What `perm` and `access` were had for, each: used only while it still holds.
    perm_stamps: HashMap<Id, Stamp>,
    access_stamps: HashMap<Id, Stamp>,
    /// The host's job of a search the window stopped waiting for: no other starts until it ended.
    lingering: Option<Receiver<DirectoryResult>>,
}

/// What something held for an account's directory was had for: the mailbox opening and the
/// account as it was, its sign-in compared as the window compares it for its other tokens
/// ([`MailApp::same_sign_in_identity`]).
struct Stamp {
    opening: u64,
    owner: Account,
}

struct Pending {
    account: Id,
    opening: u64,
    /// The account as it was when the search went: an answer for a sign-in changed since is
    /// nobody's.
    owner: Account,
    ticket: u64,
    /// When it went (milliseconds, the window's clock).
    started: f64,
    cancel: Arc<AtomicBool>,
    rx: Receiver<DirectoryResult>,
    /// Let go of (the account, its sign-in or the mailbox changed): its answer is dropped.
    void: bool,
}

impl DirectorySearching {
    /// Whether a search is in flight.
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
}

/// What the dialog's result area shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Area {
    /// Nothing typed.
    #[default]
    Empty,
    /// One character, or nothing that is sent.
    TooShort,
    /// Waiting for the pause, or for the answer.
    Searching,
    /// The entries found.
    Found,
    /// The directory found nobody.
    NoMatch,
    /// The question: the permission is needed (Allow, Not now).
    Offer,
    /// Microsoft's page is open for it.
    Asking,
    /// Not now, or no on the page.
    Declined,
    /// An administrator has to approve it first.
    Approval,
    /// The organisation doesn't let MailCraft search its directory.
    NotAllowed,
    /// Microsoft is busy.
    Busy,
    /// Microsoft couldn't be reached.
    Unreachable,
    /// The window works offline.
    Offline,
    /// There is no sign-in to search with.
    SignIn,
}

/// The search field of an Add Shared Mailbox dialog: it lives and goes with the dialog.
/// Deliberately without `Debug`: it holds the typed text and the entries found.
#[derive(Default)]
pub struct DirectoryField {
    pub(crate) text: String,
    /// When the owner last changed the text, while it isn't searched yet.
    typed_at: Option<f64>,
    /// The search whose answer is to be shown: 0 for none.
    ticket: u64,
    pub(crate) area: Area,
    pub(crate) entries: Vec<DirectoryEntry>,
    pub(crate) more: bool,
    pub(crate) highlight: Option<usize>,
    /// Shown although no host can search: a script gave the answer (`ui.searchDirectory`).
    scripted: bool,
    focused: bool,
    /// Why Allow couldn't open Microsoft's page, said under the question (which stays).
    pub(crate) refusal: String,
    /// The question has taken the keyboard.
    question_focused: bool,
    /// A frame of input an agent or a script put in went by while the question was up: the
    /// keyboard goes back to the question.
    refocus: bool,
    /// The keyboard is in the search field (or left it with this frame's Enter) or on a row: the
    /// dialog's Enter adds the marked row only then.
    pub(crate) keyboard_here: bool,
    /// The height the list may take in the window as it is ([`list_room`]); none before the
    /// dialog was first drawn.
    pub(crate) room: Option<f32>,
    /// The height the list took when last drawn (0 without one).
    pub(crate) list_drawn: f32,
}

impl DirectoryField {
    /// The entries shown, at most 15 (for a host's tests).
    pub fn entries(&self) -> &[DirectoryEntry] {
        &self.entries
    }

    /// Whether a search waits for the pause in the owner's typing.
    pub(crate) fn waiting(&self) -> bool {
        self.typed_at.is_some()
    }
}

/// What the owner did in the result area.
pub(crate) enum Pick {
    /// Add the entry at this index.
    Add(usize),
}

/// The height the list of results may take: what the window leaves once the rest of the dialog
/// (as last drawn, `dialog` tall with a list `list` tall) and its lift above the middle are taken,
/// between one row and [`ROWS_SHOWN`] rows. The title, the fields and the buttons stay on screen
/// at the smallest window the app allows (760×480); the list scrolls inside.
pub(crate) fn list_room(screen: f32, dialog: Option<f32>, list: f32, lift: f32) -> f32 {
    let full = ROWS_SHOWN * ROW;
    let Some(dialog) = dialog else { return full };
    let rest = (dialog - list).max(0.0);
    // (Twice the dialog's lift above the middle, none in a short window, and a margin above and
    // below.)
    (screen - rest - 2.0 * lift - 2.0 * 8.0).clamp(ROW, full)
}

/// File › Info's line of an account: where the directory's permission stands in this session,
/// and the links beside it.
pub(crate) struct DirectoryLine {
    pub(crate) said: &'static str,
    /// Ask again.
    pub(crate) again: bool,
    /// What to do next (an administrator's approval, or the organisation's no).
    pub(crate) help: bool,
    /// Copy a message for my administrator.
    pub(crate) copy: bool,
}

/// The area that an answer's outcome shows.
fn area_of(outcome: DirectoryOutcome, found: bool) -> Area {
    match outcome {
        DirectoryOutcome::Found if found => Area::Found,
        DirectoryOutcome::Found => Area::NoMatch,
        DirectoryOutcome::NothingAsked => Area::TooShort,
        DirectoryOutcome::ConsentNeeded => Area::Offer,
        DirectoryOutcome::NotAllowed => Area::NotAllowed,
        DirectoryOutcome::Approval => Area::Approval,
        DirectoryOutcome::SignInRefused => Area::SignIn,
        DirectoryOutcome::Busy => Area::Busy,
        DirectoryOutcome::Unreachable => Area::Unreachable,
    }
}

/// What the area shows while the permission stands so for the account.
fn area_of_permission(perm: Option<Permission>) -> Option<Area> {
    match perm? {
        Permission::Given => None,
        Permission::Asking => Some(Area::Asking),
        Permission::Declined => Some(Area::Declined),
        Permission::Approval => Some(Area::Approval),
        Permission::Organisation => Some(Area::NotAllowed),
        Permission::Unanswered => None,
    }
}

impl MailApp {
    /// The stamp of what is had now for `account`.
    fn directory_stamp(&self, account: Id) -> Option<Stamp> {
        self.session.mailbox.account(account).map(|a| Stamp { opening: self.session.openings(), owner: self.sign_in_snapshot(a) })
    }

    /// Whether `stamp` is still the mailbox open now and `account`'s sign-in as it is now.
    fn stamp_holds(&self, account: Id, stamp: &Stamp) -> bool {
        stamp.opening == self.session.openings()
            && self.session.mailbox.account(account).is_some_and(|now| self.same_sign_in_identity(&stamp.owner, now))
    }

    /// Where the directory's permission stands for `account`, if what was recorded was recorded
    /// for the sign-in it has now in the mailbox open now. (One recorded without a stamp is a
    /// test's; the window records it with one, [`MailApp::set_directory_perm`].)
    pub(crate) fn held_perm(&self, account: Id) -> Option<Permission> {
        let perm = self.directory_search.perm.get(&account).copied()?;
        let holds = self.directory_search.perm_stamps.get(&account).is_none_or(|stamp| self.stamp_holds(account, stamp));
        holds.then_some(perm)
    }

    /// The token for the directory held for `account`, still good at `now` (seconds) and had for
    /// the sign-in it has now in the mailbox open now; else empty.
    fn held_access(&self, account: Id, now: i64) -> String {
        if self.data_removed {
            return String::new();
        }
        let holds = self.directory_search.access_stamps.get(&account).is_some_and(|stamp| self.stamp_holds(account, stamp));
        if !holds {
            return String::new();
        }
        self.directory_search.access.get(&account).filter(|t| t.fresh(now)).map(|t| t.access.clone()).unwrap_or_default()
    }

    /// Record where the directory's permission stands for `account`, stamped with the sign-in it
    /// stands for.
    pub(crate) fn set_directory_perm(&mut self, account: Id, perm: Permission) {
        self.directory_search.perm.insert(account, perm);
        match self.directory_stamp(account) {
            Some(stamp) => self.directory_search.perm_stamps.insert(account, stamp),
            None => self.directory_search.perm_stamps.remove(&account),
        };
    }

    /// Whether the dialog offers the search: a host that can search, or a script's answer.
    pub(crate) fn directory_offered(&self, field: &DirectoryField) -> bool {
        self.services.graph_directory.is_some() || field.scripted
    }

    /// The owner changed the search text at `now`: what was found for the text before goes, and
    /// the search waits for the pause. Text that a script or an agent put into the window is never
    /// searched.
    pub(crate) fn directory_typed(&self, account: Id, field: &mut DirectoryField, now: f64) {
        field.entries.clear();
        field.more = false;
        field.highlight = None;
        // (An answer on its way is for the text before.)
        field.ticket = 0;
        field.typed_at = None;
        if mailcraft_model::directory::search_text(&field.text).is_none() {
            field.area = if field.text.trim().is_empty() { Area::Empty } else { Area::TooShort };
            return;
        }
        if self.driven {
            field.area = Area::Empty;
            return;
        }
        if let Some(area) = area_of_permission(self.held_perm(account)) {
            field.area = area;
            return;
        }
        if self.offline() {
            field.area = Area::Offline;
            return;
        }
        field.typed_at = Some(now);
        field.area = Area::Searching;
    }

    /// Each frame, at `now` (milliseconds): take in an answer, let go of one whose dialog went, and
    /// start the search the owner's pause asks for. (Public for a host's tests.)
    pub fn directory_tick_at(&mut self, now: f64) {
        self.poll_directory(now);
        // Microsoft's page called off before it answered (Cancel, Add while it waited, another
        // mailbox): no answer is recorded. The question comes again, and File › Info offers Ask
        // again.
        let waiting = self.signing_in.as_ref().filter(|s| s.directory).map(|s| s.account);
        let called_off: Vec<Id> =
            self.directory_search.perm.iter().filter(|(id, perm)| **perm == Permission::Asking && waiting != Some(**id)).map(|(id, _)| *id).collect();
        for id in called_off {
            self.set_directory_perm(id, Permission::Unanswered);
            if let Some(Dialog::SharedMailbox { account, search, .. }) = &mut self.dialog
                && *account == id
                && search.area == Area::Asking
            {
                search.area = Area::Offer;
            }
        }
        let open = match &self.dialog {
            Some(Dialog::SharedMailbox { account, search, .. }) => Some((*account, search.area)),
            _ => None,
        };
        // The question went with its dialog. Only the owner's own Escape or close (as the dialog
        // marks it) is his no, for this session: an agent's or a script's close (`ui.closeDialog`,
        // a key or a click it put into the window) records nothing, and the question comes again
        // at the next search. Typing doesn't answer it while it is up.
        let declined = self.directory_search.declined_by_owner.take();
        if let Some(asked) = self.directory_search.offering
            && open.is_none_or(|(account, area)| account != asked || area != Area::Offer)
        {
            self.directory_search.offering = None;
            let unanswered = matches!(self.held_perm(asked), None | Some(Permission::Unanswered));
            if declined == Some(asked) && open.is_none_or(|(account, _)| account != asked) && unanswered {
                self.set_directory_perm(asked, Permission::Declined);
            }
        }
        if let Some((account, Area::Offer)) = open {
            self.directory_search.offering = Some(account);
        }
        // The dialog went, or is another account's: nothing more is sent for what is on its way.
        if let Some(pending) = &self.directory_search.pending
            && open.is_none_or(|(account, _)| account != pending.account)
        {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        if self.directory_search.pending.is_some() {
            return;
        }
        // A search the window stopped waiting for may still be on its way: none other goes until
        // the host's job has ended (what it brings then is nobody's).
        if let Some(rx) = &self.directory_search.lingering {
            if matches!(rx.try_recv(), Err(TryRecvError::Empty)) {
                return;
            }
            self.directory_search.lingering = None;
        }
        let Some(Dialog::SharedMailbox { account, search, checking, .. }) = &mut self.dialog else { return };
        let Some(at) = search.typed_at else { return };
        if now - at < PAUSE_MS || checking.is_some() {
            return;
        }
        search.typed_at = None;
        let (account, text) = (*account, search.text.clone());
        let started = self.start_directory_search(account, &text, now);
        if let Some(Dialog::SharedMailbox { search, .. }) = &mut self.dialog {
            match started {
                Ok(ticket) => {
                    search.ticket = ticket;
                    search.area = Area::Searching;
                }
                Err(area) => search.area = area,
            }
        }
    }

    /// Hand the search for `text` to the host: its ticket, or what the area says instead.
    fn start_directory_search(&mut self, account: Id, text: &str, now_ms: f64) -> Result<u64, Area> {
        if self.data_removed {
            return Err(Area::Empty);
        }
        if let Some(area) = area_of_permission(self.held_perm(account)) {
            return Err(area);
        }
        if self.offline() {
            return Err(Area::Offline);
        }
        if self.services.graph_directory.is_none() {
            return Err(Area::Empty);
        }
        let owner = self.session.mailbox.account(account).filter(|a| a.can_open_shared()).cloned().ok_or(Area::Empty)?;
        // What of the text is sent: the engine's to say (and refused while working offline).
        let asked = self.execute("sync.directoryRequest", json!({"account": account, "text": text})).map_err(|_| Area::TooShort)?;
        let text = asked.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
        let oauth = owner.oauth.clone().filter(|o| o.provider == mailcraft_model::OAuthProvider::Microsoft).ok_or(Area::SignIn)?;
        let now = mailcraft_time::now();
        let access = self.held_access(account, now);
        let kept = self.tokens.get(&account).or(self.graph_tokens.get(&account)).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
        let refresh = kept.or_else(|| self.remembered("refresh", account)).unwrap_or_default();
        if access.is_empty() && refresh.is_empty() {
            return Err(Area::SignIn);
        }
        let Some(search) = &self.services.graph_directory else { return Err(Area::Empty) };
        self.directory_search.tickets = self.directory_search.tickets.wrapping_add(1).max(1);
        let ticket = self.directory_search.tickets;
        let cancel = Arc::new(AtomicBool::new(false));
        let asked_for = self.sign_in_snapshot(&owner);
        let job = DirectoryJob {
            owner,
            oauth: self.effective_oauth(oauth),
            refresh,
            access,
            query: DirectoryQuery { account, text, ticket },
            cancel: cancel.clone(),
        };
        let rx = search(job);
        let opening = self.session.openings();
        self.directory_search.pending = Some(Pending { account, opening, owner: asked_for, ticket, started: now_ms, cancel, rx, void: false });
        Ok(ticket)
    }

    /// The host's answer, when it is in: the tokens kept, and what was found shown in the dialog it
    /// was asked for, for the text it was asked for; anything else is dropped.
    fn poll_directory(&mut self, now: f64) {
        let Some(pending) = &self.directory_search.pending else { return };
        let (result, late) = match pending.rx.try_recv() {
            Ok(result) => (Some(result), false),
            Err(TryRecvError::Empty) if now - pending.started < ANSWER_DEADLINE_MS => return,
            // No answer in time (the job may still be on its way), or the host's thread went
            // without one: Microsoft couldn't be reached, and Try again is offered.
            Err(TryRecvError::Empty) => (None, true),
            Err(TryRecvError::Disconnected) => (None, false),
        };
        let Some(Pending { account, opening, owner, ticket, cancel, rx, void, .. }) = self.directory_search.pending.take() else { return };
        let result = match result {
            Some(result) => result,
            None => {
                cancel.store(true, Ordering::Relaxed);
                if late {
                    self.directory_search.lingering = Some(rx);
                }
                DirectoryResult {
                    answer: DirectoryAnswer { account, ticket, outcome: DirectoryOutcome::Unreachable, ..Default::default() },
                    tokens: None,
                }
            }
        };
        // Another mailbox took this one's place, the account went or its sign-in changed, data was
        // removed, or what was held for it was let go of meanwhile: this answer is nobody's, tokens
        // and all.
        if self.data_removed {
            self.forget_directory(None);
            return;
        }
        let same = self.session.mailbox.account(account).is_some_and(|now| self.same_sign_in_identity(&owner, now));
        if void || opening != self.session.openings() || !same {
            return;
        }
        let answer = result.answer;
        // How it went, and never what was asked or found.
        log::info!("directory search: {} entries ({:?})", answer.entries.len(), answer.outcome);
        // (A sign-in that was refused keeps none of the tokens that came with the refusal, and
        // records no permission as given.)
        if let Some(tokens) = result.tokens.filter(|_| !matches!(answer.outcome, DirectoryOutcome::SignInRefused)) {
            self.keep_directory_tokens(account, tokens);
        }
        match answer.outcome {
            DirectoryOutcome::SignInRefused => {
                self.drop_directory_token(account);
                self.drop_shared_tokens_for(account);
                self.forget_calendar_jobs_for(account);
            }
            DirectoryOutcome::ConsentNeeded => {
                // (Given before, and taken back since: asked again.)
                if self.held_perm(account) == Some(Permission::Given) {
                    self.directory_search.perm.remove(&account);
                }
                self.drop_directory_token(account);
            }
            _ => {}
        }
        let perm = self.held_perm(account);
        let Some(Dialog::SharedMailbox { account: shown, search, .. }) = &mut self.dialog else { return };
        if *shown != account || search.ticket == 0 || search.ticket != answer.ticket || answer.account != account {
            return;
        }
        search.ticket = 0;
        search.highlight = None;
        let mut entries = answer.entries;
        entries.truncate(MAX_RESULTS);
        search.more = answer.more;
        search.area = match (answer.outcome, area_of_permission(perm)) {
            (DirectoryOutcome::ConsentNeeded, Some(area)) => area,
            (outcome, _) => area_of(outcome, !entries.is_empty()),
        };
        search.entries = if search.area == Area::Found { entries } else { Vec::new() };
    }

    /// Let go of what the window holds for searching `account`'s directory (every account's, with
    /// none): its token, whether it was allowed, the question, and the answer of a search on its
    /// way (called off). For another mailbox, an account removed, a sign-in changed and the data
    /// removed: wherever the window lets go of an account's other tokens.
    pub(crate) fn forget_directory(&mut self, account: Option<Id>) {
        let mine = |id: &Id| account.is_none_or(|a| a == *id);
        self.directory_search.access.retain(|id, _| !mine(id));
        self.directory_search.access_stamps.retain(|id, _| !mine(id));
        self.directory_search.perm.retain(|id, _| !mine(id));
        self.directory_search.perm_stamps.retain(|id, _| !mine(id));
        if self.directory_search.offering.as_ref().is_some_and(mine) {
            self.directory_search.offering = None;
        }
        if self.directory_search.declined_by_owner.as_ref().is_some_and(mine) {
            self.directory_search.declined_by_owner = None;
        }
        if let Some(pending) = &mut self.directory_search.pending
            && mine(&pending.account)
        {
            pending.cancel.store(true, Ordering::Relaxed);
            pending.void = true;
        }
    }

    /// Let go of the token for `account`'s directory alone (a mail sign-in refused, or the
    /// directory's own refused): the next search asks for a new one.
    pub(crate) fn drop_directory_token(&mut self, account: Id) {
        self.directory_search.access.remove(&account);
        self.directory_search.access_stamps.remove(&account);
        if let Some(pending) = &mut self.directory_search.pending
            && pending.account == account
        {
            pending.cancel.store(true, Ordering::Relaxed);
            pending.void = true;
        }
    }

    /// Keep what a search or the page brought: the refresh token as the account's newest (and
    /// remembered when the owner wants that), the access token for the directory in memory.
    fn keep_directory_tokens(&mut self, account: Id, tokens: Tokens) {
        if self.data_removed {
            self.forget_directory(None);
            return;
        }
        if self.signed_in_as_other(account, &tokens) {
            return;
        }
        if !tokens.refresh.is_empty() {
            self.remember("refresh", account, &tokens.refresh);
            for kept in [self.tokens.get_mut(&account), self.graph_tokens.get_mut(&account)].into_iter().flatten() {
                kept.refresh = tokens.refresh.clone();
            }
        }
        if !tokens.access.is_empty() {
            let now = mailcraft_time::now();
            let expires_at = tokens.expires_at.clamp(now, now.saturating_add(86_400));
            self.directory_search.access.insert(account, Tokens { expires_at, ..tokens });
            match self.directory_stamp(account) {
                Some(stamp) => self.directory_search.access_stamps.insert(account, stamp),
                None => self.directory_search.access_stamps.remove(&account),
            };
            self.set_directory_perm(account, Permission::Given);
        }
    }

    /// `ui.allowDirectory`: Microsoft's page for the directory's permission, because the owner said
    /// Allow (or Ask again in File › Info).
    pub(crate) fn begin_directory_sign_in(&mut self, account: Id) -> Result<Value, String> {
        if self.data_removed {
            return Err(Self::data_removed_sync_refused());
        }
        let owner = self.session.mailbox.account(account).filter(|a| a.can_open_shared()).cloned();
        let owner =
            owner.ok_or_else(|| tl!("shared mailboxes open under a work or school account kept in step through Microsoft Graph").to_string())?;
        let oauth = owner.oauth.clone().filter(|o| o.provider == mailcraft_model::OAuthProvider::Microsoft);
        let oauth = oauth.ok_or_else(|| tl!("This account has no sign-in with Microsoft").to_string())?;
        self.directory_next = true;
        let begun = self.begin_sign_in(owner.id, oauth, &owner.email, String::new(), false, true);
        self.directory_next = false;
        begun?;
        self.set_directory_perm(account, Permission::Asking);
        self.directory_search.offering = None;
        self.session.sync_status =
            tl!("Allow MailCraft to search your organisation's directory on the page that opened in your browser…").to_string();
        if let Some(Dialog::SharedMailbox { account: shown, search, .. }) = &mut self.dialog
            && *shown == account
        {
            search.area = Area::Asking;
        }
        Ok(json!({"pending": "signIn", "account": account}))
    }

    /// The page for the directory's permission came back: with the tokens, what was typed is
    /// searched at once; without, why not is said in the dialog.
    pub(crate) fn directory_signed_in(&mut self, account: Id, answer: Result<Tokens, String>) {
        self.session.sync_status.clear();
        if self.data_removed {
            self.forget_directory(None);
            return;
        }
        let perm = match answer {
            Ok(tokens) if !self.signed_in_as_other(account, &tokens) => {
                self.keep_directory_tokens(account, tokens);
                Permission::Given
            }
            Ok(_) => Permission::Declined,
            Err(e) if e == crate::favorites_sync::NEEDS_APPROVAL => Permission::Approval,
            Err(e) if e == crate::favorites_sync::REFUSED_BY_ORGANISATION || e == crate::favorites_sync::REFUSED_FOR_ACCOUNT => {
                Permission::Organisation
            }
            Err(_) => Permission::Declined,
        };
        self.set_directory_perm(account, perm);
        if let Some(Dialog::SharedMailbox { account: shown, search, .. }) = &mut self.dialog
            && *shown == account
        {
            match area_of_permission(Some(perm)) {
                Some(area) => search.area = area,
                // (What the owner typed before he was asked.)
                None if mailcraft_model::directory::search_text(&search.text).is_some() => {
                    search.typed_at = Some(f64::MIN);
                    search.area = Area::Searching;
                }
                None => search.area = Area::Empty,
            }
        }
    }

    /// File › Info's line for `account` (one that can open shared mailboxes): where searching its
    /// directory stands in this session. Nothing of it is kept across a restart and nothing is
    /// asked of Microsoft to say it: before the first search it is "not asked in this session"
    /// (when it was given before, by the owner or an administrator, that search asks nothing).
    pub(crate) fn directory_line(&self, account: Id) -> Option<DirectoryLine> {
        self.session.mailbox.account(account).filter(|a| a.can_open_shared())?;
        let line = |said, again, help, copy| Some(DirectoryLine { said, again, help, copy });
        match self.held_perm(account).as_ref() {
            None if self.services.graph_directory.is_some() => line(tl!("Directory search: not asked in this session"), false, false, false),
            None => None,
            Some(Permission::Given) => line(tl!("Directory search: allowed"), false, false, false),
            Some(Permission::Asking) => line(tl!("Directory search: asking on Microsoft's page…"), false, false, false),
            Some(Permission::Declined) => line(tl!("Directory search: not now (for this session)"), true, false, false),
            Some(Permission::Approval) => line(tl!("Directory search: needs your administrator's approval"), true, true, true),
            Some(Permission::Organisation) => line(tl!("Directory search: your organisation doesn't allow it"), true, true, false),
            Some(Permission::Unanswered) => line(tl!("Directory search: not answered in this session"), true, false, false),
        }
    }

    /// File › Info's Ask again: the dialog, with the question in it, whatever was answered before.
    pub(crate) fn directory_ask_again(&mut self, account: Id) -> bool {
        // (An agent's or a script's click: nothing, and nothing said.)
        if self.driven {
            return false;
        }
        if self.dialog.is_some() {
            self.status(tl!("Close the open dialog first, then press Ask again."));
            return false;
        }
        if self.signing_in.is_some() {
            self.status(tl!("Finish the sign-in in your browser first, then press Ask again."));
            return false;
        }
        self.directory_search.perm.remove(&account);
        if self.run("ui.dialog", json!({"name": "sharedMailbox", "account": account})).is_err() {
            return false;
        }
        if let Some(Dialog::SharedMailbox { search, .. }) = &mut self.dialog {
            search.area = Area::Offer;
        }
        true
    }

    /// `ui.searchDirectory`: the owner's own script types into the open dialog's search (`text`),
    /// which is searched at the next frame; with `answer`, nothing is asked of anybody, and it is
    /// shown as what the host brought (for scripts, and for looking at the dialog in each state).
    fn search_directory_command(&mut self, p: &Value) -> Result<Value, String> {
        let offline = self.offline();
        let perm = self.directory_search.perm.clone();
        let Some(Dialog::SharedMailbox { account, search, .. }) = &mut self.dialog else {
            return Err(tl!("Open Add Shared Mailbox first").to_string());
        };
        let account = *account;
        if let Some(text) = p.get("text").and_then(Value::as_str) {
            search.text = text.chars().take(4_096).collect();
            search.entries.clear();
            search.more = false;
            search.highlight = None;
            search.ticket = 0;
        }
        let Some(answer) = p.get("answer").filter(|a| a.is_object()) else {
            // As if the owner had typed it and paused.
            let state = if offline { Area::Offline } else { area_of_permission(perm.get(&account).copied()).unwrap_or(Area::Searching) };
            let searched = mailcraft_model::directory::search_text(&search.text).is_some();
            search.area = if searched { state } else { Area::TooShort };
            search.typed_at = (searched && state == Area::Searching).then_some(f64::MIN);
            return Ok(json!({"account": account, "pending": search.typed_at.is_some()}));
        };
        let outcome = match answer.get("outcome").and_then(Value::as_str).unwrap_or("found") {
            "found" => DirectoryOutcome::Found,
            "nothingAsked" => DirectoryOutcome::NothingAsked,
            "consentNeeded" => DirectoryOutcome::ConsentNeeded,
            "notAllowed" => DirectoryOutcome::NotAllowed,
            "approval" => DirectoryOutcome::Approval,
            "signInRefused" => DirectoryOutcome::SignInRefused,
            "busy" => DirectoryOutcome::Busy,
            "unreachable" => DirectoryOutcome::Unreachable,
            other => return Err(format!("unknown outcome `{}`", other.chars().take(40).collect::<String>())),
        };
        let entries: Vec<DirectoryEntry> = answer
            .get("entries")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|e| {
                let field = |k: &str| e.get(k).and_then(Value::as_str).unwrap_or_default();
                DirectoryEntry::found(field("name"), field("address"))
            })
            .take(MAX_RESULTS)
            .collect();
        search.scripted = true;
        search.typed_at = None;
        search.ticket = 0;
        search.highlight = None;
        search.more = answer.get("more").and_then(Value::as_bool).unwrap_or(false);
        search.area = match area_of(outcome, !entries.is_empty()) {
            // (Nothing typed: the dialog as it opens.)
            Area::TooShort if search.text.trim().is_empty() => Area::Empty,
            area => area,
        };
        search.entries = if search.area == Area::Found { entries } else { Vec::new() };
        Ok(json!({"account": account, "shown": search.entries.len()}))
    }
}

/// The window's commands of the directory's search.
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "ui.searchDirectory" => app.search_directory_command(p),
        "ui.allowDirectory" => {
            if p.get("confirmed").and_then(Value::as_bool) != Some(true) {
                return Some(Err(tl!("Say that this is meant with \"confirmed\": true.").to_string()));
            }
            let Some(account) = p.get("account").and_then(Value::as_u64) else { return Some(Err(tl!("`account` is required").to_string())) };
            app.begin_directory_sign_in(account)
        }
        _ => return None,
    })
}

/// The search part of the Add Shared Mailbox dialog of `account`: the field, the result area and
/// the list. `enabled` is false while an address is being checked; `enter` is the dialog's Enter.
/// Returns what the owner picked, and whether Enter was taken by a highlighted entry.
pub(crate) fn search_ui(app: &mut MailApp, ui: &mut Ui, t: &Theme, account: Id, field: &mut DirectoryField, enabled: bool) -> Option<Pick> {
    let mut picked = None;
    ui.label(tl!("Search your organisation"));
    let edit = egui::TextEdit::singleline(&mut field.text).hint_text(tl!("Name or e-mail address")).desired_width(f32::INFINITY).char_limit(256);
    // While the question is up the field takes no keys (it keeps what was typed before).
    let question = field.area == Area::Offer;
    let resp = crate::text_menu::field(ui.add_enabled(enabled && !question, edit));
    if !field.focused {
        if !question {
            resp.request_focus();
        }
        field.focused = true;
    }
    field.keyboard_here = resp.has_focus() || resp.lost_focus();
    field.list_drawn = 0.0;
    if !question {
        field.question_focused = false;
    }
    if resp.changed() {
        app.directory_typed(account, field, crate::now_ms());
    }
    if field.waiting() {
        // (A frame when the pause is over, though nothing moves.)
        ui.ctx().request_repaint_after(WAKE);
    }
    let n = field.entries.len();
    if resp.has_focus() && n > 0 {
        if ui.input(|i| i.key_pressed(Key::ArrowDown)) {
            field.highlight = Some(field.highlight.map_or(0, |h| (h + 1).min(n - 1)));
        }
        if ui.input(|i| i.key_pressed(Key::ArrowUp)) {
            field.highlight = field.highlight.and_then(|h| h.checked_sub(1));
        }
    }
    let dim = |text: &str| RichText::new(text).color(t.text_dim);
    match field.area {
        Area::Empty | Area::Found => {}
        Area::TooShort => {
            ui.label(dim(tl!("Type at least two letters of a name or address.")));
        }
        Area::Searching => {
            ui.label(dim(tl!("Searching your organisation's directory…")));
        }
        Area::NoMatch => {
            ui.label(dim(&crate::i18n::fmt(tl!("Nobody in your organisation's directory matches \"{query}\"."), &[("query", field.text.trim())])));
        }
        Area::Offer => {
            let asked = ui.label(tl!(
                "Searching your organisation's directory needs your permission. Microsoft's page opens in your browser to ask you, once."
            ));
            // The question takes the keyboard and keeps it until the owner answers: keys typed go
            // nowhere (his own Tab reaches Allow and Not now). Input an agent or a script puts into
            // the window never moves it: after such a frame it is back on the question.
            let held = ui.interact(asked.rect, ui.make_persistent_id("directoryQuestion"), Sense::focusable_noninteractive());
            held.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::Group,
                    true,
                    tl!("Searching your organisation's directory needs your permission. Microsoft's page opens in your browser to ask you, once."),
                )
            });
            if !field.question_focused || field.refocus || app.driven {
                held.request_focus();
            }
            field.question_focused = true;
            field.refocus = app.driven;
            ui.horizontal(|ui| {
                let allow = crate::widgets::primary_button(ui, tl!("Allow"), enabled);
                app.hit.add("directoryAllow", 0, allow.rect);
                let not_now = ui.add_enabled(enabled, egui::Button::new(tl!("Not now")));
                app.hit.add("directoryNotNow", 0, not_now.rect);
                // Where the keyboard is, on these two (the app's other buttons are as they were).
                for button in [&allow, &not_now] {
                    if button.has_focus() {
                        ui.painter().rect_stroke(button.rect.expand(2.0), 5.0, egui::Stroke::new(2.0, t.accent), egui::StrokeKind::Outside);
                    }
                }
                // The owner's own click: never input a script or an agent put into the window.
                if allow.clicked() && !app.driven {
                    match app.run("ui.allowDirectory", json!({"account": account, "confirmed": true})) {
                        Ok(_) => {
                            field.refusal.clear();
                            field.area = Area::Asking;
                        }
                        // The page couldn't be opened: why, here, and the question stays (no no).
                        Err(e) => field.refusal = e,
                    }
                } else if not_now.clicked() && !app.driven {
                    app.set_directory_perm(account, Permission::Declined);
                    field.area = Area::Declined;
                }
            });
            if field.area == Area::Offer && !field.refusal.is_empty() {
                ui.label(RichText::new(field.refusal.as_str()).color(t.red));
            }
        }
        Area::Asking => {
            ui.label(dim(tl!("Allow MailCraft to search your organisation's directory on the page that opened in your browser…")));
        }
        Area::Declined => {
            ui.label(dim(tl!("Searching your organisation's directory isn't allowed. Type the address below, or allow it in File › Info.")));
        }
        Area::Approval => {
            ui.label(dim(tl!(crate::favorites_sync::NEEDS_APPROVAL)));
            ui.horizontal(|ui| {
                if ui.link(tl!("What to do next")).clicked() {
                    let _ = app.run("ui.approvalHelp", json!({"account": account}));
                }
                if ui.link(tl!("Copy a message for my administrator")).clicked() {
                    let _ = app.run("ui.copyAdminMessage", json!({"account": account}));
                }
            });
        }
        Area::NotAllowed => {
            ui.label(dim(tl!("Your organisation doesn't let MailCraft search its directory. Type the shared mailbox's address instead.")));
        }
        Area::Busy | Area::Unreachable => {
            let said = if field.area == Area::Busy {
                tl!("Microsoft is busy. Try again in a moment.")
            } else {
                tl!("Couldn't reach Microsoft to search the directory. Type the address, or try again in a moment.")
            };
            ui.label(dim(said));
            let again = ui.add_enabled(enabled, egui::Button::new(tl!("Try again")));
            app.hit.add("directoryTryAgain", 0, again.rect);
            if again.clicked() && !app.driven {
                app.directory_typed(account, field, f64::MIN);
            }
        }
        Area::Offline => {
            ui.label(dim(tl!("You're working offline: type the address.")));
        }
        Area::SignIn => {
            ui.label(dim(tl!("Sign in again with Send/Receive to search your organisation's directory.")));
        }
    }
    if field.area == Area::Found && n > 0 {
        results_ui(app, ui, t, field, enabled, &mut picked);
        if field.more {
            ui.label(dim(&tln!(
                n,
                "Keep typing to narrow the list: only the first {n} is shown.",
                "Keep typing to narrow the list: only the first {n} are shown."
            )));
        }
        ui.label(
            RichText::new(tl!("The directory lists people and shared mailboxes alike; the one you pick is checked when it is added."))
                .color(t.text_dim)
                .size(11.0),
        );
    }
    picked
}

/// The list of entries found: one row each, its name over its address, cut to the row's width.
fn results_ui(app: &mut MailApp, ui: &mut Ui, t: &Theme, field: &mut DirectoryField, enabled: bool, picked: &mut Option<Pick>) {
    let room = field.room.unwrap_or(ROWS_SHOWN * ROW);
    let list =
        egui::Frame::new().fill(t.input).stroke(egui::Stroke::new(1.0, t.input_border)).corner_radius(4.0).inner_margin(egui::Margin::same(2)).show(
            ui,
            |ui| {
                // A bar beside the list whenever it is longer than the box, not only under the pointer.
                let style = ui.style_mut();
                style.spacing.scroll = egui::style::ScrollStyle {
                    bar_width: 8.0,
                    dormant_handle_opacity: 1.0,
                    active_handle_opacity: 1.0,
                    interact_handle_opacity: 1.0,
                    ..egui::style::ScrollStyle::solid()
                };
                style.visuals.widgets.inactive.bg_fill = crate::widgets::scroll_ink(t);
                style.visuals.widgets.hovered.bg_fill = t.text;
                style.visuals.widgets.active.bg_fill = t.accent;
                egui::ScrollArea::vertical()
                    .max_height(room)
                    .auto_shrink([false, true])
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (i, entry) in field.entries.iter().enumerate() {
                            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
                            let spoken = if entry.name.is_empty() { entry.address.clone() } else { format!("{}, {}", entry.name, entry.address) };
                            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, spoken.clone()));
                            // The row the keyboard is on is the one marked (and the one Enter adds).
                            if resp.has_focus() {
                                field.highlight = Some(i);
                                field.keyboard_here = true;
                            }
                            let lit = field.highlight == Some(i);
                            if lit {
                                ui.painter().rect_filled(rect, 3.0, t.selection);
                                ui.scroll_to_rect(rect, None);
                            } else if resp.hovered() && enabled {
                                ui.painter().rect_filled(rect, 3.0, t.hover);
                            }
                            // (A row with no name is its address alone, on one line in the middle of the row.)
                            let layout = if entry.name.is_empty() {
                                egui::Layout::left_to_right(egui::Align::Center)
                            } else {
                                egui::Layout::top_down(egui::Align::Min)
                            };
                            let mut row = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(8.0, 3.0))).layout(layout));
                            row.spacing_mut().item_spacing.y = 1.0;
                            let (first, second) =
                                if entry.name.is_empty() { (entry.address.as_str(), "") } else { (entry.name.as_str(), entry.address.as_str()) };
                            row.add(egui::Label::new(RichText::new(first).color(t.text)).truncate().selectable(false));
                            if !second.is_empty() {
                                row.add(egui::Label::new(RichText::new(second).color(t.text_dim).size(11.5)).truncate().selectable(false));
                            }
                            // A target only for a row in view: one scrolled away can't be clicked.
                            let shown = rect.intersect(ui.clip_rect());
                            if shown.is_positive() && shown.height() >= ROW / 2.0 {
                                app.hit.add("directoryResult", u64::try_from(i).unwrap_or(u64::MAX), shown);
                            }
                            if resp.clicked() && enabled {
                                field.highlight = Some(i);
                                *picked = Some(Pick::Add(i));
                            }
                        }
                    });
            },
        );
    field.list_drawn = list.response.rect.height();
}

#[cfg(test)]
#[path = "directory_search_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "shared_integration_tests.rs"]
mod integration_tests;

#[cfg(test)]
#[path = "shared_integration_r2_tests.rs"]
mod integration_r2_tests;

#[cfg(test)]
#[path = "shared_integration_r2b_tests.rs"]
mod integration_r2b_tests;

#[cfg(test)]
#[path = "shared_integration_r3_tests.rs"]
mod integration_r3_tests;

#[cfg(test)]
#[path = "shared_integration_r3b_tests.rs"]
mod integration_r3b_tests;

#[cfg(test)]
#[path = "shared_integration_followup_tests.rs"]
mod integration_followup_tests;

#[cfg(test)]
#[path = "shared_integration_followup_b_tests.rs"]
mod integration_followup_b_tests;
