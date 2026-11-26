// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A shared mailbox's calendars in the window (#70): kept in step through Microsoft Graph with
//! the sign-in of the account the mailbox is opened through, at the mailbox's own address, and
//! shown, read and (with edit rights) changed here.
//!
//! A shared mailbox's calendar is there to be ticked from the moment it is added (the engine
//! keeps one for it, [`mailcraft_engine::cmd::remote::ensure_shared_calendars`]). Nothing of it is
//! asked of any server until the owner ticks one: then a calendar job reads its list and, of the
//! calendars that are ticked, their events (the window, polling and caps of the owner's own).
//! The first job finds out whether the owner's sign-in covers shared calendars
//! (`Calendars.ReadWrite.Shared`, which a sign-in's page asks for with everything else); when an
//! older sign-in doesn't, the window puts the one question for it ([`crate::server_sync`]), and
//! its page asks for this with the rest. Refused, or when an administrator has to approve first,
//! the window says what to do next. File › Info shows where it stands under the shared mailbox,
//! and the line of the account it is opened through has Ask again.
//!
//! Its access token is its own (it covers what the mailbox's mail token needn't), kept in memory
//! and never mixed with the owner's or the mail's; the refresh token that comes back is the
//! owner's, kept as his. Nothing of a shared mailbox's calendar is a reminder, an alert or part of
//! what the owner's own calendars show beside it. Where the owner may edit one it is changed as
//! his own is, at the mailbox's address; what makes the server send mail from the mailbox waits for
//! the permission to send from shared mailboxes, which the same one question asks for.

use mailcraft_model::{Account, Id};

use crate::{MailApp, Tokens};

pub(crate) use mailcraft_engine::sync_driver::SharedPermission as Permission;

/// Identity stamp binding a cached shared-mailbox calendar access token to the mailbox opening,
/// owning account sign-in identity, and normalized shared mailbox address it was issued for.
#[derive(Clone)]
pub(crate) struct SharedCalendarStamp {
    pub(crate) opening: u64,
    pub(crate) owner: Account,
    pub(crate) shared_email: String,
}

/// A shared mailbox's cached calendar access token bound to its [`SharedCalendarStamp`].
#[derive(Clone)]
pub(crate) struct StampedSharedCalendarToken {
    pub(crate) stamp: SharedCalendarStamp,
    pub(crate) tokens: Tokens,
}

impl MailApp {
    /// Ask the owner, once, for what a calendar ticked in a shared mailbox found missing: the one
    /// question for an older sign-in, with the calendar in front (the owner ticked it), then
    /// Microsoft's page. Called every look at the calendar.
    pub(crate) fn shared_calendars_ask_tick(&mut self) {
        if let Some(shared) = self.session.shared_calendars_question(&self.ask_window()) {
            self.ask_shared_calendars(shared);
        }
    }

    /// The one question, for the account `shared` is opened through. Not put again by itself
    /// once it went away unanswered in this session (the calendar then waits, declined).
    pub(crate) fn ask_shared_calendars(&mut self, shared: Id) -> bool {
        let Some(owner) = self.session.mailbox.owner_of(shared).map(|o| o.id) else { return false };
        if self.ask_sync(owner) {
            return true;
        }
        // (Not asked: the calendar is not looked at again until the owner asks.)
        let perm = &mut self.session.driver.calendar.shared_perm;
        if perm.get(&shared) == Some(&Permission::Needed) {
            perm.insert(shared, Permission::Declined);
        }
        false
    }

    /// What a calendar job of the shared mailbox `shared` said of sending from it. (Where the
    /// owner's permission for reading its calendars stands is the engine's, taken in with the rest
    /// of the answer: [`mailcraft_engine::Session::calendar_answered`].)
    pub(crate) fn shared_calendars_send_answered(&mut self, shared: Id, result: &crate::GraphResult, errors: &mut Vec<String>) {
        // What makes the server send mail from the shared mailbox (an answer to a meeting, one
        // called off or forwarded, one with people invited) found that sending from shared
        // mailboxes isn't allowed yet (#70): it waits, nothing goes out as the owner, and the
        // owner is asked as he is for mail (the same permission, the same page).
        if result.send_consent_needed && self.session.mailbox.account(shared).is_some() {
            self.session.driver.calendar.send_waits.insert(shared);
            let owner = self.session.mailbox.owner_of(shared).map(|o| o.id);
            if self.dialog.is_some() || !owner.is_some_and(|owner| self.ask_sync(owner)) {
                errors.push(
                    tl!("A meeting answer or invitation from this shared mailbox waits: sending from shared mailboxes needs your permission. Use Send/Receive.")
                        .to_string(),
                );
            }
        }
    }

    /// Build the identity stamp for `shared` from the live session, or `None` when `shared` is
    /// not a live shared mailbox with an existing owner account.
    pub(crate) fn shared_calendar_stamp(&self, shared: Id) -> Option<SharedCalendarStamp> {
        let account = self.session.mailbox.account(shared).filter(|a| a.is_shared())?;
        let owner = self.session.mailbox.owner_of(shared)?.clone();
        Some(SharedCalendarStamp {
            opening: self.session.openings(),
            owner,
            shared_email: mailcraft_host::job_sign_in::normalized_address(&account.email),
        })
    }

    /// Whether `stamp` still matches `shared` in the live mailbox opening: same opening, `shared`
    /// still exists as a shared mailbox owned by the same owner with the same normalized address,
    /// and the owner still has the same sign-in credential identity.
    pub(crate) fn shared_calendar_stamp_holds(&self, shared: Id, stamp: &SharedCalendarStamp) -> bool {
        if stamp.opening != self.session.openings() {
            return false;
        }
        let Some(account) = self.session.mailbox.account(shared).filter(|a| a.is_shared()) else {
            return false;
        };
        if account.shared_of != Some(stamp.owner.id) || mailcraft_host::job_sign_in::normalized_address(&account.email) != stamp.shared_email {
            return false;
        }
        self.session.mailbox.owner_of(shared).is_some_and(|owner| Self::same_sign_in_identity(&stamp.owner, owner))
    }

    /// Drop cached shared-calendar tokens for `account` (as the shared mailbox itself or as the
    /// owning account of any shared mailboxes).
    pub(crate) fn drop_shared_calendar_tokens_for(&mut self, account: Id) {
        self.shared_calendar_tokens.retain(|shared, entry| *shared != account && entry.stamp.owner.id != account);
    }

    /// Drop every cached shared-calendar token whose stamp no longer matches the live session.
    pub(crate) fn prune_stale_shared_calendar_tokens(&mut self) {
        let stale: Vec<Id> = self
            .shared_calendar_tokens
            .iter()
            .filter(|(shared, entry)| !self.shared_calendar_stamp_holds(**shared, &entry.stamp))
            .map(|(shared, _)| *shared)
            .collect();
        for id in stale {
            self.shared_calendar_tokens.remove(&id);
        }
    }

    /// Keep the tokens a calendar job or the page brought back: the access token is the shared
    /// mailbox's calendars' own, stamped to the live opening, owner identity, and shared address;
    /// the refresh token is the owner's newest, for mail and Graph alike.
    pub(crate) fn keep_shared_calendar_tokens(&mut self, shared: Id, owner: Id, tokens: Tokens) {
        if self.data_removed {
            self.shared_calendar_tokens.clear();
            return;
        }
        let Some(stamp) = self.shared_calendar_stamp(shared).filter(|s| s.owner.id == owner) else {
            self.shared_calendar_tokens.remove(&shared);
            return;
        };
        if !tokens.refresh.is_empty() {
            self.remember("refresh", owner, &tokens.refresh);
            if let Some(kept) = self.tokens.get_mut(&owner) {
                kept.refresh = tokens.refresh.clone();
            }
        }
        let now = mailcraft_time::now();
        let clamped = Tokens { expires_at: tokens.expires_at.clamp(now, now.saturating_add(86_400)), ..tokens };
        self.shared_calendar_tokens.insert(shared, StampedSharedCalendarToken { stamp, tokens: clamped });
    }

    /// The access token to hand a job for `shared`'s calendars: verified against its identity
    /// stamp at every use; on mismatch or missing account/owner, evicted and refused (empty).
    pub(crate) fn shared_calendar_access(&mut self, shared: Id) -> String {
        if self.data_removed {
            self.shared_calendar_tokens.clear();
            return String::new();
        }
        let valid = self.shared_calendar_tokens.get(&shared).is_some_and(|entry| self.shared_calendar_stamp_holds(shared, &entry.stamp));
        if !valid {
            self.shared_calendar_tokens.remove(&shared);
            return String::new();
        }
        self.shared_calendar_tokens
            .get(&shared)
            .filter(|entry| entry.tokens.fresh(mailcraft_time::now()))
            .map(|entry| entry.tokens.access.clone())
            .unwrap_or_default()
    }

    /// A shared mailbox's calendar token as the page leaves it, stamped to the current session,
    /// for a host's tests (which sign nobody in).
    #[doc(hidden)]
    pub fn seed_shared_calendar_token(&mut self, shared: Id, tokens: Tokens) {
        if self.data_removed {
            return;
        }
        let Some(stamp) = self.shared_calendar_stamp(shared) else {
            return;
        };
        self.shared_calendar_tokens.insert(shared, StampedSharedCalendarToken { stamp, tokens });
    }

    /// Make `account`'s list of calendars and their rights due at the next look, as a quarter of an
    /// hour going by does, for a host's tests (which have a stopped clock).
    #[doc(hidden)]
    pub fn calendar_list_due(&mut self, account: Id) {
        self.session.driver.calendar.checked.remove(&account);
        self.session.driver.calendar.book.forget_checked(account);
    }

    /// What File › Info says under a shared mailbox's card about its calendars' permission. (Ask
    /// again is on the line of the account it is opened through: one page for everything.)
    pub(crate) fn shared_calendar_line(&self, shared: Id) -> Option<String> {
        self.session.mailbox.account(shared).filter(|a| a.is_shared())?;
        Some(match self.session.driver.calendar.shared_perm.get(&shared) {
            Some(Permission::Given) => tl!("Calendars: allowed").to_string(),
            Some(Permission::Allowed) => tl!("Calendars: allowed, reading them…").to_string(),
            Some(Permission::Needed | Permission::Declined) => tl!("Calendars: waiting for your permission").to_string(),
            Some(Permission::Asking) => tl!("Calendars: asking for your permission…").to_string(),
            Some(Permission::Refused(why)) => {
                crate::i18n::fmt(tl!("Calendars: not allowed ({why})"), &[("why", crate::forms::kit::shown(why, 90).as_str())])
            }
            None => tl!("Calendars: not asked yet (ticking one of them asks)").to_string(),
        })
    }
}
