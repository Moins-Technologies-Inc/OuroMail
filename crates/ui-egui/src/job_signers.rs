// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whose sign-in a background job went with, and the installation's application id as a part of
//! every sign-in that takes it.
//!
//! A job that renews its sign-in for itself (free/busy, the automatic replies, the to-dos, the
//! rules, the categories; Graph mail notes its own, [`crate::shared_mail_tokens`]) notes at its
//! launch the sign-in it goes with ([`MailApp::job_signer`]). Its answer keeps something of a
//! sign-in, or says anything, only while that is still the account's own
//! ([`MailApp::graph_job_holds`]); one that is nobody's is let go of whole. Sign-ins are compared
//! by [`MailApp::same_sign_in_identity`], which sees the application id an account signs in with
//! ([`MailApp::sign_in_snapshot`]): one that names none takes this installation's
//! (`ui.microsoftClientId`, `ui.googleClientId`), so a change of that is a change of its sign-in
//! ([`MailApp::application_ids_changed`]).

use mailcraft_model::{Account, Id};

use crate::MailApp;

impl MailApp {
    /// `account` as its sign-in goes now: its registration with the application id it signs in
    /// with filled in ([`MailApp::effective_oauth`]). What a job or a stamp notes of a sign-in.
    pub(crate) fn sign_in_snapshot(&self, account: &Account) -> Account {
        let mut snapshot = account.clone();
        snapshot.oauth = snapshot.oauth.take().map(|oauth| self.effective_oauth(oauth));
        snapshot
    }

    /// The sign-in a job of `account` goes with, as it is now: the account's own, or its owner's
    /// for a shared mailbox. `None` when there is no account (or no owner) for it.
    pub(crate) fn job_signer(&self, account: Id) -> Option<Account> {
        let live = self.session.mailbox.account(account)?;
        let signer = if live.is_shared() { self.session.mailbox.owner_of(account)? } else { live };
        Some(self.sign_in_snapshot(signer))
    }

    /// The application id each account that signs in on a provider's page signs in with now.
    pub(crate) fn application_ids(&self) -> Vec<(Id, String)> {
        let accounts = self.session.mailbox.accounts.iter();
        accounts.filter_map(|a| a.oauth.clone().map(|oauth| (a.id, self.effective_oauth(oauth).client_id))).collect()
    }

    /// This installation's application id changed; `before` is [`MailApp::application_ids`] as
    /// it was. Every account that signs in with another one now has another sign-in, and lets go
    /// of everything had with the one before, as an edit of its own application id does: its
    /// tokens and those had with it for its shared mailboxes and its directory, its sign-in page
    /// and calendar work under way, what the system remembers for it, and its Favorites' state.
    /// (A job still under way finds its sign-in changed when it answers.) An account that names
    /// an application id of its own, or signs in with the other provider, keeps everything.
    pub(crate) fn application_ids_changed(&mut self, before: &[(Id, String)]) {
        let changed: Vec<Id> = self.application_ids().into_iter().filter(|now| !before.contains(now)).map(|(id, _)| id).collect();
        for account in changed {
            let Some(was) = self.session.mailbox.account(account).cloned() else { continue };
            log::info!("sign-in, account {account}: this installation's application id changed; the sign-in had with the one before is let go of");
            self.drop_sign_in_of(account);
            self.forget_mail_secrets(&was);
            self.forget_favorites(account);
            self.ews_signed_in.remove(&account);
        }
    }
}
