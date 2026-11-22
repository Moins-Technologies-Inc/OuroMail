// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The organisation's directory, searched for a shared mailbox to open (Add Shared Mailbox): what
//! a search sends, and what of the answer is kept.
//!
//! Microsoft Graph lists the people and the shared mailboxes of the owner's organisation at
//! `/v1.0/users` (a work or school account's sign-in with `User.ReadBasic.All`), and can't tell
//! one from the other with that permission: every entry with a usable address is offered, and the
//! one the owner picks is checked when it is added, as a typed address is.
//!
//! Nothing here asks anybody anything: the request is made and sent by `mailcraft-net`, the
//! window holds the answer for as long as its dialog is open. Everything in an answer is hostile:
//! it is read as JSON with nothing assumed of its shape, at most [`MAX_SCANNED`] of its entries
//! are looked at and [`MAX_RESULTS`] kept, an address is kept only when it is a mailbox's own
//! ([`crate::mailbox_address`]), and a name is cleaned as every name coming in is
//! ([`mailcraft_names::incoming`]) and cut to [`MAX_NAME`] characters.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Id;

/// Fewest characters of typed text a search is asked with.
pub const MIN_QUERY: usize = 2;
/// Most characters of typed text a search is asked with: the rest is left out.
pub const MAX_QUERY: usize = 64;
/// Most entries an answer is asked for, and kept of it (`$top`).
pub const MAX_RESULTS: usize = 15;
/// Most characters of an entry's name that are kept.
pub const MAX_NAME: usize = 254;
/// Most entries of one answer looked at, whatever it holds (its size is capped in bytes too).
pub const MAX_SCANNED: usize = 1_000;
/// Most characters of what was typed that are looked at, however much was pasted.
const MAX_TYPED: usize = 4_096;

/// A search of the directory, as the window asks for it. Its `Debug` prints the text typed:
/// never log it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DirectoryQuery {
    /// The account whose organisation is searched, with its sign-in.
    pub account: Id,
    /// What the owner typed, as typed ([`search_text`] says what of it is sent).
    pub text: String,
    /// Which of the window's searches it is: an answer to an older one is not shown.
    pub ticket: u64,
}

impl DirectoryQuery {
    /// What of the typed text is sent ([`search_text`]).
    pub fn search_text(&self) -> Option<String> {
        search_text(&self.text)
    }
}

/// What of `typed` a search is asked with: without control characters (a line break or a tab is
/// white space), the characters that show nothing ([`mailcraft_names::is_format`],
/// [`mailcraft_names::is_ignorable`]) and `&` (which the directory's search can't take), its
/// words parted by one space, trimmed, and at most [`MAX_QUERY`] characters. `None` when fewer
/// than [`MIN_QUERY`] are left: then nothing is asked.
pub fn search_text(typed: &str) -> Option<String> {
    let kept: String = typed
        .chars()
        .take(MAX_TYPED)
        .filter(|c| *c != '&' && !mailcraft_names::is_format(*c) && !mailcraft_names::is_ignorable(*c) && (c.is_whitespace() || !c.is_control()))
        .collect();
    let words = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    let text: String = words.chars().take(MAX_QUERY).collect();
    let text = text.trim_end().to_string();
    (text.chars().count() >= MIN_QUERY).then_some(text)
}

/// One entry of the directory: who it is, and the address its mailbox is opened at. Its `Debug`
/// prints both: never log it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DirectoryEntry {
    /// Its name as the directory has it, cleaned; may be empty.
    pub name: String,
    /// A mailbox's own address ([`crate::mailbox_address`]).
    pub address: String,
}

impl DirectoryEntry {
    /// An entry the directory gave as `name` at `address`: `None` when the address isn't a
    /// mailbox's own; its name cleaned and cut to [`MAX_NAME`] characters (empty when nothing of
    /// it is left).
    pub fn found(name: &str, address: &str) -> Option<DirectoryEntry> {
        let address = crate::mailbox_address(address)?;
        Some(DirectoryEntry { name: mailcraft_names::incoming(name, MAX_NAME).unwrap_or_default(), address })
    }
}

/// One page of the directory's answer, read ([`read_page`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirectoryPage {
    /// At most [`MAX_RESULTS`], each address once whatever its letter case, in the answer's order.
    pub entries: Vec<DirectoryEntry>,
    /// The directory has more than these: the answer names a next page, or held more usable
    /// entries than are kept. A next page is never asked for: the owner types on instead.
    pub more: bool,
}

/// The answer to a search (`GET /v1.0/users`), as Graph writes a page of a list: `{"value":
/// [{"displayName", "mail", "userPrincipalName"}, …], "@odata.nextLink"?}`. An entry without a
/// usable `mail` (missing, not text, or not a mailbox's own address) is left out: a person or a
/// room without a mailbox (the sign-in name is no mailbox's address). One with a usable `mail`
/// whose `displayName` is missing or isn't text is kept with an empty name, as the dialog can
/// still list it by its address. `None` when the answer isn't such a page at all.
pub fn read_page(body: &[u8]) -> Option<DirectoryPage> {
    let page: Value = serde_json::from_slice(body).ok()?;
    let list = page.get("value")?.as_array()?;
    let mut more = page.get("@odata.nextLink").is_some_and(Value::is_string) || list.len() > MAX_SCANNED;
    let mut entries: Vec<DirectoryEntry> = Vec::new();
    for user in list.iter().take(MAX_SCANNED) {
        let text = |name: &str| user.get(name).and_then(Value::as_str);
        let Some(entry) = text("mail").and_then(|mail| DirectoryEntry::found(text("displayName").unwrap_or_default(), mail)) else { continue };
        if entries.iter().any(|kept| kept.address.eq_ignore_ascii_case(&entry.address)) {
            continue;
        }
        if entries.len() >= MAX_RESULTS {
            more = true;
            break;
        }
        entries.push(entry);
    }
    Some(DirectoryPage { entries, more })
}

/// How a search went.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DirectoryOutcome {
    /// The directory answered: its entries (perhaps none).
    Found,
    /// Nothing was asked: too little was typed ([`MIN_QUERY`]), or the account can't open a
    /// shared mailbox ([`crate::Account::can_open_shared`]: a personal Microsoft account has no
    /// directory to search).
    NothingAsked,
    /// The sign-in doesn't cover searching the directory yet: the owner is to be asked.
    ConsentNeeded,
    /// The organisation doesn't let this program search its directory (403; a guest can't).
    NotAllowed,
    /// The organisation's administrator has to approve it first.
    Approval,
    /// The sign-in was refused (it ran out or was taken back).
    SignInRefused,
    /// Microsoft is busy (429, 503, 504): try again in a moment.
    Busy,
    /// Microsoft couldn't be reached, or its answer made no sense.
    #[default]
    Unreachable,
}

/// What a search brought back, for the window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirectoryAnswer {
    pub account: Id,
    pub ticket: u64,
    /// What was found: empty unless the outcome is [`DirectoryOutcome::Found`].
    pub entries: Vec<DirectoryEntry>,
    /// The directory has more than these ([`DirectoryPage::more`]).
    pub more: bool,
    pub outcome: DirectoryOutcome,
}

#[cfg(test)]
#[path = "directory_tests.rs"]
mod tests;
