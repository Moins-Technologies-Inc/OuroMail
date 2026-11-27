// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whose sign-in a Graph token in memory is, and the Graph job under way.
//!
//! `graph_tokens` holds an account's own Graph token under its id and, under a shared mailbox's
//! id, the token its owner's sign-in got for that mailbox's mail. The second kind is its owner's:
//! it is stamped with the sign-in it was had with, goes wherever that sign-in is let go of
//! ([`MailApp::drop_sign_in_of`], [`MailApp::drop_shared_tokens_for`]), and is read only through
//! [`MailApp::graph_token`], which looks at the stamp again at every use. A Graph job is bound
//! the same way to the sign-in it went with: when that is no longer the account's own as its
//! answer comes, what the answer says of a sign-in is nobody's.

use mailcraft_model::{Account, Id};

use crate::{GraphResult, MailApp, Tokens};

impl MailApp {
    /// Let go of everything this session holds for `account`'s sign-in, and of every job that
    /// went with it: a sign-in page waiting, its own tokens for mail and Graph, the tokens had
    /// with it for its shared mailboxes (their calendars' and their mail's), its calendar job or
    /// check, and what is held for its directory. The one place: an account removed, and an Undo
    /// or Redo that changes its sign-in ([`MailApp::same_sign_in_identity`]), call it. (An edit
    /// lets go of the same, but of the account's own tokens only when its address, its servers or
    /// its registration changed.)
    /// (The Graph run under way is bound to the sign-in it went with, and is looked at when its
    /// answer comes: [`MailApp::graph_job_holds`].)
    pub(crate) fn drop_sign_in_of(&mut self, account: Id) {
        self.cancel_pending_sign_in(account);
        self.tokens.remove(&account);
        self.graph_tokens.remove(&account);
        self.drop_shared_tokens_for(account);
        self.forget_calendar_jobs_for(account);
        self.forget_directory(Some(account));
    }

    /// Let go of the tokens had for shared mailboxes with `account`'s sign-in, their calendars'
    /// and their mail's: `account` as the owner of shared mailboxes, or as a shared mailbox
    /// itself. An owner's own tokens stay.
    pub(crate) fn drop_shared_tokens_for(&mut self, account: Id) {
        self.drop_shared_calendar_tokens_for(account);
        let (mailbox, stamps) = (&self.session.mailbox, &self.shared_mail_stamps);
        self.graph_tokens.retain(|id, _| {
            let stamped = stamps.get(id).is_some_and(|stamp| *id == account || stamp.owner.id == account);
            !stamped && mailbox.account(*id).is_none_or(|live| live.shared_of != Some(account))
        });
        self.shared_mail_stamps.retain(|id, stamp| *id != account && stamp.owner.id != account);
    }

    /// Let go of the mail token of the shared mailbox `shared` alone.
    pub(crate) fn drop_shared_mail_token(&mut self, shared: Id) {
        self.graph_tokens.remove(&shared);
        self.shared_mail_stamps.remove(&shared);
    }

    /// Let go of every shared mailbox's mail token whose stamp no longer holds: its mailbox went,
    /// is another's now, or its owner signs in otherwise.
    pub(crate) fn prune_stale_shared_mail_tokens(&mut self) {
        let stale: Vec<Id> = self
            .shared_mail_stamps
            .iter()
            .filter(|(shared, stamp)| !self.shared_calendar_stamp_holds(**shared, stamp))
            .map(|(shared, _)| *shared)
            .collect();
        for shared in stale {
            self.drop_shared_mail_token(shared);
        }
    }

    /// Keep the access token a run got for the mail of the shared mailbox `shared`, stamped with
    /// the mailbox opening, its owner's sign-in and its address as they are now. One that can't
    /// be stamped (the mailbox or its owner is gone) is not kept.
    pub(crate) fn keep_shared_mail_token(&mut self, shared: Id, tokens: Tokens) {
        let Some(stamp) = self.shared_calendar_stamp(shared) else {
            self.drop_shared_mail_token(shared);
            return;
        };
        let now = mailcraft_time::now();
        self.graph_tokens.insert(shared, Tokens { expires_at: tokens.expires_at.clamp(now, now.saturating_add(86_400)), ..tokens });
        self.shared_mail_stamps.insert(shared, stamp);
    }

    /// The Graph token held for `account`, if it may be used now: an account's own, or a shared
    /// mailbox's whose stamp still holds (the same opening, owner, owner's sign-in and address).
    /// A shared mailbox's without a stamp, or with one that no longer holds, is nobody's; so is
    /// one for an account that isn't there.
    pub(crate) fn graph_token(&self, account: Id) -> Option<&Tokens> {
        let live = self.session.mailbox.account(account)?;
        if live.is_shared() {
            let stamp = self.shared_mail_stamps.get(&account)?;
            if !self.shared_calendar_stamp_holds(account, stamp) {
                return None;
            }
        }
        self.graph_tokens.get(&account)
    }

    /// Whether the Graph job of `account` that went with `signer`'s sign-in, as it was then,
    /// still has an account to answer to: `account` is there, and its sign-in (its owner's, for a
    /// shared mailbox) is the one the job went with. A job nothing was noted for has none.
    pub(crate) fn graph_job_holds(&self, account: Id, signer: Option<&Account>) -> bool {
        let Some(signer) = signer else { return false };
        let Some(live) = self.session.mailbox.account(account) else { return false };
        let now = if live.is_shared() { self.session.mailbox.owner_of(account) } else { Some(live) };
        now.is_some_and(|now| self.same_sign_in_identity(signer, now))
    }

    /// Let go of the Graph run or Inbox check under way and of the checks that gave way: their
    /// answers are looked at no more. (The data was removed.)
    pub(crate) fn forget_graph_jobs(&mut self) {
        self.inbox_check.forget();
        (self.graph_rx, self.graph_signer, self.graph_pieces) = (None, None, None);
        (self.inbox_run, self.graph_after_sign_in) = (None, None);
    }
}

/// Take out of a Graph job's answer everything that is a sign-in or asks for one: the tokens,
/// the permissions it found missing, and the refusals. For an answer that is nobody's
/// ([`MailApp::graph_job_holds`]): nothing of it is kept, and no page is opened for it.
pub(crate) fn without_sign_in(result: &mut GraphResult) {
    result.tokens = None;
    (result.consent_needed, result.send_consent_needed, result.shared_calendars_consent_needed) = (false, false, false);
    (result.token_refused, result.sign_in_refused) = (false, false);
}
