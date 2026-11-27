// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reading the remembered sign-ins off the window's thread (#613).
//!
//! The system's credential store may be locked: asking it then puts its unlock prompt in front of
//! the owner, and the call waits until the prompt is answered (the macOS keychain, GNOME Keyring,
//! KWallet). On the window's own thread that is a window that can't be clicked, scrolled or
//! closed, and a Cancel brings the prompt straight back because the next check asks again.
//!
//! So the window never calls [`SecretStore::get`]. When the mailbox opens, one background thread
//! reads what is remembered for its accounts, and the answers are kept in memory
//! (`MailApp::secret_cache`: what was found; no entry is a miss, a refusal or an answer still to
//! come, which all mean "not ready to sync quietly"). The store is asked again only when the
//! owner acts: Send/Receive is held until it has answered ([`MailApp::hold_for_secrets`]), and a
//! secret written this session goes into the answers as it is written.
//!
//! The one-time move of secrets filed under their old names to the names bound to servers
//! (`ui.sign_ins_bound`) is done by the same thread, since it too reads the store.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};

use serde_json::Value;

use super::{Id, MailApp, SecretStore, StoreTrouble};

/// What one background read of the store brought back.
pub(crate) struct SecretRead {
    /// Each key asked for, with what was found under it (nothing when it isn't there or the
    /// store refused).
    pub(crate) answers: Vec<(String, Option<String>)>,
    /// The new names of secrets moved from their old names.
    pub(crate) moved: Vec<String>,
    /// Whether the move from the old names was part of it.
    pub(crate) migrated: bool,
    /// Why the store couldn't be used, when it said.
    pub(crate) trouble: Option<StoreTrouble>,
}

/// The names let go of in the store while a read is under way, shared with the reading thread so
/// that its move from an old name never writes one of them back (a tenant edit, a change of the
/// installation's application id or Remove all data can land while the store keeps the read
/// waiting, R3-1); and whether every name is let go of (the data was removed, or another
/// mailbox took the place of the one it reads for).
#[derive(Default)]
pub(crate) struct LetGo {
    names: Mutex<HashSet<String>>,
    all: AtomicBool,
    /// The names let go of that a sign-in was remembered under again since: the window's
    /// alone, written back once the read is over (MFU-S2).
    again: Mutex<HashSet<String>>,
}

impl LetGo {
    /// `key` is let go of. (Noted before it is deleted from the store, and the reader looks again
    /// once it has written: whichever comes second sees the other.)
    pub(crate) fn note(&self, key: &str) {
        self.names.lock().unwrap_or_else(PoisonError::into_inner).insert(key.to_string());
        self.again.lock().unwrap_or_else(PoisonError::into_inner).remove(key);
    }

    /// A sign-in is remembered under `key` now: when `key` was let go of, it is the new one's.
    pub(crate) fn remembered_again(&self, key: &str) {
        let let_go = self.names.lock().unwrap_or_else(PoisonError::into_inner).contains(key);
        if let_go {
            self.again.lock().unwrap_or_else(PoisonError::into_inner).insert(key.to_string());
        }
    }

    /// Every name is let go of: the data was removed, or another mailbox took the place of the
    /// one the read is for.
    pub(crate) fn everything(&self) {
        self.all.store(true, Ordering::SeqCst);
    }

    /// Whether `key` is let go of.
    pub(crate) fn has(&self, key: &str) -> bool {
        self.all.load(Ordering::SeqCst) || self.names.lock().unwrap_or_else(PoisonError::into_inner).contains(key)
    }
}

/// One key to read, and the old name to move a secret from first.
struct Ask {
    key: String,
    old: Option<String>,
}

/// The reading itself; runs on a thread of its own, and may wait on the store for a long time.
fn read(store: &dyn SecretStore, asks: Vec<Ask>, migrated: bool, let_go: &LetGo) -> SecretRead {
    let (mut answers, mut moved) = (Vec::new(), Vec::new());
    for Ask { key, old } in asks {
        if let Some(old) = old
            && let Some(secret) = store.get(&old).filter(|s| !s.is_empty())
        {
            // (Let go of while the store kept this read waiting: not written again.)
            if let_go.has(&key) {
                continue;
            }
            match store.set(&key, &secret) {
                Ok(()) => {
                    // Let go of while it was being written: deleted again before anyone reads it.
                    if let_go.has(&key) {
                        store.delete(&key);
                        continue;
                    }
                    store.delete(&old);
                    moved.push(key.clone());
                }
                Err(_) => {
                    // The store may echo the credential in its error. Keep the old entry and
                    // do not expose or adopt a partially written replacement.
                    log::warn!("couldn't move a remembered sign-in to the operating system's credential store");
                    continue;
                }
            }
        }
        let secret = store.get(&key).filter(|s| !s.is_empty());
        answers.push((key, secret));
    }
    SecretRead { answers, moved, migrated, trouble: store.take_trouble() }
}

impl MailApp {
    /// Start reading the store for this mailbox's accounts, unless a read is under way.
    /// `migrate`: move secrets kept under their old names first.
    pub(crate) fn start_secret_read(&mut self, migrate: bool) {
        // (Nothing is read for a mailbox whose servers nobody has confirmed: one from a file, or
        // one where two accounts have the same address, AU3-03.)
        if self.secret_read.is_some() || self.unconfirmed {
            return;
        }
        let Some(store) = self.services.secrets.clone() else { return };
        let mut asks: Vec<Ask> = self
            .session
            .mailbox
            .accounts
            .iter()
            .flat_map(|a| {
                ["password", "refresh"].map(|kind| Ask {
                    key: Self::secret_key(kind, a),
                    // (An Exchange server account is newer than the old names: nothing moves to it;
                    // nor to either of two accounts with one address: the old name doesn't say
                    // whose it was, AU3-03.)
                    old: (migrate && a.kind != mailcraft_model::AccountKind::Exchange && !self.address_shared(&a.email))
                        .then(|| Self::legacy_secret_key(kind, &a.email)),
                })
            })
            .collect();
        // The addresses of the calendars subscribed to, which may hold a secret (#209): kept like a password.
        let subscribed = self
            .session
            .mailbox
            .calendars
            .iter()
            .filter_map(|c| c.subscription.as_ref().map(|sub| (c.id, sub)))
            .take(mailcraft_model::subscription::MAX_SUBSCRIBED);
        asks.extend(subscribed.map(|(id, sub)| Ask { key: sub.secret_name(id), old: None }));
        let (send, receive) = mpsc::channel();
        let let_go = Arc::new(LetGo::default());
        let told = Arc::clone(&let_go);
        let job = std::thread::Builder::new().name("secret-read".into()).spawn(move || {
            let _ = send.send(read(&*store, asks, migrate, &told));
        });
        match job {
            Ok(_) => {
                self.secrets_let_go = let_go;
                self.secret_read = Some((self.session.openings(), receive));
            }
            Err(e) => log::warn!("couldn't start reading the credential store: {e}"),
        }
    }

    /// Once a frame: take in what the read brought, and start the read for a mailbox that has
    /// not had one (and again when remembering is switched on).
    pub(crate) fn secrets_tick(&mut self) {
        self.poll_dav_forgetting();
        self.poll_secret_read();
        if self.secret_read.is_some() || self.data_removed || self.unconfirmed || self.services.secrets.is_none() {
            return;
        }
        let (opening, remember) = (self.session.openings(), self.ui.remember_sign_in);
        // The mailbox the window started with: secrets kept the old way are filed under its servers.
        let migrate = opening == 0 && !self.ui.sign_ins_bound;
        if (remember || migrate) && self.secrets_asked != Some((opening, remember)) {
            self.secrets_asked = Some((opening, remember));
            self.start_secret_read(migrate);
        }
    }

    pub(crate) fn poll_secret_read(&mut self) {
        let Some((opening, receive)) = &self.secret_read else { return };
        let opening = *opening;
        let brought = match receive.try_recv() {
            Ok(read) => Some(read),
            Err(mpsc::TryRecvError::Empty) => return,
            // The thread ended without an answer (the store panicked): nothing was found.
            Err(mpsc::TryRecvError::Disconnected) => None,
        };
        self.secret_read = None;
        // What was let go of while it ran is not taken back from it (a sign-in changed, an account
        // went); and once the data is removed, nothing is.
        let let_go = std::mem::take(&mut self.secrets_let_go);
        // What it moved to a name let go of meanwhile (to any, once the data is removed) is
        // deleted again: the move may have written it after the name was let go of (R3-1).
        if let Some(brought) = &brought {
            self.forget_moved_again(&brought.moved, &let_go);
        }
        if self.data_removed {
            self.sync_after_secrets.clear();
            return;
        }
        // A sign-in remembered again under a name let go of while it ran is the owner's new one:
        // the read's clean-up (the reader's delete of its own write, or the one above) may have
        // taken it out of the store, so it is written back (MFU-S2).
        self.keep_remembered_again(&let_go);
        // A mailbox took the place of the one it was started for: its accounts are not these.
        if opening == self.session.openings()
            && let Some(brought) = brought
        {
            // (A calendar and contacts server's password is read where it is used. One that a read
            // brings anyway, started before its settings changed, is let go if no account's
            // settings give that name any more.)
            let dav_now: std::collections::HashSet<String> = self.session.mailbox.accounts.iter().flat_map(Self::dav_secret_keys).collect();
            for (key, secret) in brought.answers {
                let stale = (key.starts_with(mailcraft_host::secrets::DAV_PASSWORD) && !dav_now.contains(&key)) || let_go.has(&key);
                if let Some(secret) = secret.filter(|_| !stale) {
                    // Noted, as one written here is, so that Remove all data finds it whichever
                    // mailbox is open then (AU3-18).
                    self.index_secret_key(key.clone());
                    self.secret_cache.insert(key, secret);
                }
            }
            for key in brought.moved.into_iter().filter(|key| !let_go.has(key)) {
                self.index_secret_key(key);
            }
            if brought.migrated {
                self.ui.sign_ins_bound = true;
            }
            if let Some(trouble) = brought.trouble {
                self.note_secret_trouble(trouble);
            }
        }
        // The Send/Receive the owner pressed while it was out goes on, with the answer in.
        for mut p in std::mem::take(&mut self.sync_after_secrets) {
            if let Some(object) = p.as_object_mut() {
                object.insert("secretsAsked".into(), Value::Bool(true));
            }
            if let Err(e) = self.start_sync(&p) {
                self.status(e);
            }
        }
    }

    /// Delete from the store again what a read moved to a name let go of while it ran. (Not noted
    /// either: the name is not the account's any more, or the data is gone.)
    fn forget_moved_again(&self, moved: &[String], let_go: &LetGo) {
        let Some(store) = &self.services.secrets else { return };
        for key in moved.iter().filter(|key| let_go.has(key)) {
            store.delete(key);
        }
    }

    /// Write back to the store what was remembered again under a name let go of while a read ran,
    /// as memory has it (nothing when it was let go of again since).
    fn keep_remembered_again(&mut self, let_go: &LetGo) {
        let Some(store) = self.services.secrets.clone() else { return };
        for key in let_go.again.lock().unwrap_or_else(PoisonError::into_inner).iter() {
            let Some(secret) = self.secret_cache.get(key) else { continue };
            if store.set(key, secret).is_err() {
                // (As `remember` does: a store error can echo its input, and a partial write
                // must not look kept.)
                log::warn!("couldn't keep a sign-in in the operating system's credential store");
                store.delete(key);
                self.secret_cache.remove(key);
            }
        }
    }

    /// Whether the store could still say something about what the account needs to sign in that
    /// this session doesn't have: remembered sign-ins are on, and neither memory nor the answers
    /// so far hold its secret.
    fn store_may_know(&self, account: Id) -> bool {
        if !self.ui.remember_sign_in || self.services.secrets.is_none() {
            return false;
        }
        let account = self.session.mailbox.owner_of(account).map_or(account, |owner| owner.id);
        let Some(a) = self.session.mailbox.account(account) else { return false };
        // (A JMAP account's secret is never with the store; an Exchange server account's is, under
        // the name that holds its server's address and its user name, #252.)
        if a.kind == mailcraft_model::AccountKind::Jmap {
            return false;
        }
        if a.oauth.is_some() {
            let kept = self.tokens.get(&account).or(self.graph_tokens.get(&account)).is_some_and(|t| !t.refresh.is_empty());
            !kept && !self.secret_cache.contains_key(&Self::secret_key("refresh", a))
        } else {
            !self.passwords.contains_key(&account) && !self.secret_cache.contains_key(&Self::secret_key("password", a))
        }
    }

    /// Send/Receive started by the owner: when the store might know a sign-in nothing here has,
    /// ask it again (this is the owner's act: a locked store may ask to be unlocked) and hold the
    /// run until it has answered. Gives whether the run was held.
    pub(crate) fn hold_for_secrets(&mut self, p: &Value) -> bool {
        if p.get("secretsAsked").and_then(Value::as_bool).unwrap_or(false) || p.get("password").is_some() {
            return false;
        }
        let accounts = match p.get("account").and_then(Value::as_u64) {
            Some(account) => vec![account],
            None => self.server_accounts(),
        };
        if !accounts.into_iter().any(|a| self.store_may_know(a)) {
            return false;
        }
        self.start_secret_read(false);
        // (Few: a button pressed again and again holds one run, not a queue of them.)
        if self.secret_read.is_none() || self.sync_after_secrets.len() >= 4 {
            return false;
        }
        self.sync_after_secrets.push(p.clone());
        true
    }
}
