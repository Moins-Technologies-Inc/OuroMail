// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The organisation's directory in the stand-in (Add Shared Mailbox): `GET /v1.0/users` as the
//! reference has it for a sign-in with `User.ReadBasic.All`.
//!
//! - **Search.** `$search` needs `ConsistencyLevel: eventual` (400 without, in the words of the
//!   reference on advanced queries). Each clause is `"property:value"` in double quotes, a `"`
//!   or `\` in it written `\"` or `\\`, the clauses joined by `OR`; `displayName` is matched by
//!   its words (each word of the value the start of a word of the name, in any order, whatever
//!   the case), `mail` and `userPrincipalName` from their start. Anything else in a `$search`
//!   is a 400 here (the service has more; a client of this program asks no more).
//! - **What is read.** `$select` of the basic profile alone (`displayName`, `givenName`, `id`,
//!   `mail`, `photo`, `securityIdentifier`, `surname`, `userPrincipalName`): a property outside
//!   it is a 403, as for a sign-in that may read no more. Without `$select`, those that a user
//!   has.
//! - **Pages.** `$top` from 1 to 999 (100 when not said), and `@odata.nextLink` while there is
//!   more (`$skiptoken`, which is the stand-in's own).
//! - **Knobs.** [`State::directory_denied`] (403 to everything: a guest, or an organisation that
//!   doesn't let the program read it), [`State::directory_answer`] (served as it is, for hostile
//!   answers), and `fail_next` and `retry_after` as for every request.

use serde_json::{Map, Value, json};

use super::{Answer, Asked, Refusal, State, bad, refuse};

/// What a sign-in with `User.ReadBasic.All` may read of a user.
pub const READ_BASIC: &[&str] = &["displayName", "givenName", "id", "mail", "photo", "securityIdentifier", "surname", "userPrincipalName"];
/// What is answered when `$select` doesn't say.
const SHOWN: &[&str] = &["displayName", "givenName", "id", "mail", "surname", "userPrincipalName"];
/// What a `$search` may look in.
const SEARCHED: &[&str] = &["displayName", "mail", "userPrincipalName"];
/// What a search without `ConsistencyLevel: eventual` is told.
pub const NEEDS_EVENTUAL: &str =
    "Request with $search query parameter only works through MSGraph with a special request header: 'ConsistencyLevel: eventual'";
const MAX_TOP: usize = 999;
const TOP: usize = 100;

/// An entry of the stand-in's directory: a person or a shared mailbox, which it can't tell apart.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DirectoryUser {
    pub id: String,
    pub display_name: String,
    /// Empty for an entry without a mailbox (`mail` is null).
    pub mail: String,
    pub upn: String,
}

impl DirectoryUser {
    fn property(&self, name: &str) -> Value {
        match name {
            "id" => json!(self.id),
            "displayName" => json!(self.display_name),
            "mail" if !self.mail.is_empty() => json!(self.mail),
            "userPrincipalName" => json!(self.upn),
            _ => Value::Null,
        }
    }

    /// Whether `value` finds it by `property`, as a directory search does.
    fn found_by(&self, property: &str, value: &str) -> bool {
        let value = value.to_lowercase();
        match property {
            "displayName" => {
                let words: Vec<String> =
                    self.display_name.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect();
                let asked: Vec<&str> = value.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
                !asked.is_empty() && asked.iter().all(|a| words.iter().any(|w| w.starts_with(a)))
            }
            "mail" => !self.mail.is_empty() && self.mail.to_lowercase().starts_with(&value),
            "userPrincipalName" => self.upn.to_lowercase().starts_with(&value),
            _ => false,
        }
    }
}

/// The clauses of a `$search`: `"property:value" OR …`, each a property and what is looked for.
fn clauses(search: &str) -> Result<Vec<(String, String)>, Refusal> {
    let unreadable = || bad("Syntax error: the $search query isn't one this stand-in reads.");
    let mut found = Vec::new();
    let mut chars = search.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.next() != Some('"') {
            return Err(unreadable());
        }
        let mut clause = String::new();
        loop {
            match chars.next() {
                Some('\\') => clause.push(chars.next().ok_or_else(unreadable)?),
                Some('"') => break,
                Some(c) => clause.push(c),
                None => return Err(unreadable()),
            }
        }
        let (property, value) = clause.split_once(':').ok_or_else(unreadable)?;
        if !SEARCHED.contains(&property) {
            return Err(bad(format!("Unsupported property '{property}' in $search.")));
        }
        found.push((property.to_string(), value.to_string()));
        let mut between = String::new();
        while let Some(c) = chars.next_if(|c| *c != '"') {
            between.push(c);
        }
        match between.trim() {
            "" if chars.peek().is_none() => return Ok(found),
            "OR" if chars.peek().is_some() => {}
            _ => return Err(unreadable()),
        }
    }
}

impl State {
    /// An entry of the directory called `name`, at `mail` (empty: it has no mailbox), signing in
    /// as `upn`: its id.
    pub fn add_directory_user(&mut self, name: &str, mail: &str, upn: &str) -> String {
        self.made += 1;
        let id = format!("{:08x}-1111-4222-8333-{:012x}", self.made, self.made);
        self.directory.push(DirectoryUser { id: id.clone(), display_name: name.into(), mail: mail.into(), upn: upn.into() });
        id
    }

    /// `GET /v1.0/users`.
    pub(super) fn directory_search(&mut self, asked: &Asked) -> Result<Answer, Refusal> {
        if self.directory_denied {
            return Err(refuse(403, "Authorization_RequestDenied", "Insufficient privileges to complete the operation."));
        }
        if let Some(raw) = self.directory_answer.clone() {
            self.raw = Some(("application/json".into(), raw));
            return Ok((200, None));
        }
        if let Some((name, _)) = asked.query.iter().find(|(name, _)| !matches!(name.as_str(), "$search" | "$select" | "$top" | "$skiptoken")) {
            return Err(bad(format!("The query parameter '{name}' isn't supported by this stand-in.")));
        }
        let search = match asked.param("$search") {
            Some(_) if !asked.header("consistencylevel").eq_ignore_ascii_case("eventual") => {
                return Err(refuse(400, "Request_UnsupportedQuery", NEEDS_EVENTUAL));
            }
            Some(text) => Some(clauses(text)?),
            None => None,
        };
        let select: Vec<String> = match asked.param("$select") {
            Some(names) => names.split(',').map(|n| n.trim().to_string()).collect(),
            None => SHOWN.iter().map(|n| (*n).to_string()).collect(),
        };
        if let Some(beyond) = select.iter().find(|n| !READ_BASIC.contains(&n.as_str())) {
            return Err(refuse(403, "Authorization_RequestDenied", format!("Insufficient privileges to complete the operation ('{beyond}').")));
        }
        let number = |name: &str, default: usize| {
            asked.param(name).map_or(Ok(default), |n| n.parse::<usize>().map_err(|_| bad(format!("Invalid value for '{name}'."))))
        };
        let top = number("$top", TOP)?;
        if !(1..=MAX_TOP).contains(&top) {
            return Err(bad("Invalid page size specified: the value of '$top' must be between 1 and 999."));
        }
        let skip = number("$skiptoken", 0)?;
        let matching: Vec<&DirectoryUser> = self
            .directory
            .iter()
            .filter(|user| search.as_ref().is_none_or(|clauses| clauses.iter().any(|(property, value)| user.found_by(property, value))))
            .collect();
        let page: Vec<Value> = matching
            .iter()
            .skip(skip)
            .take(top)
            .map(|user| Value::Object(select.iter().map(|name| (name.clone(), user.property(name))).collect::<Map<String, Value>>()))
            .collect();
        let mut list = json!({"@odata.context": format!("{}/v1.0/$metadata#users", self.base), "value": page});
        if skip.saturating_add(top) < matching.len() {
            let mut query: Vec<(String, String)> = asked.query.iter().filter(|(name, _)| name != "$skiptoken").cloned().collect();
            query.push(("$skiptoken".into(), skip.saturating_add(top).to_string()));
            let pairs: Vec<(&str, &str)> = query.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
            list["@odata.nextLink"] = json!(format!("{}/v1.0/users?{}", self.base, crate::http::form(&pairs)));
        }
        Ok((200, Some(list)))
    }
}
