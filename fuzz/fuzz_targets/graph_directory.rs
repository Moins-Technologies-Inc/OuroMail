// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fuzzes the reader of the directory's answers (Add Shared Mailbox, `GET /v1.0/users`) and the
//! text a search is asked with.
//!
//! Invariants: nothing panics; at most fifteen entries are kept, each at a mailbox's own address
//! and each address once, with a name that is cleaned and capped; what is sent of typed text is
//! within its caps and holds no `&`.

#![no_main]

use mailcraft_model::directory::{MAX_NAME, MAX_QUERY, MAX_RESULTS, MIN_QUERY, read_page, search_text};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    if let Some(page) = read_page(data) {
        assert!(page.entries.len() <= MAX_RESULTS);
        for (i, entry) in page.entries.iter().enumerate() {
            assert_eq!(mailcraft_model::mailbox_address(&entry.address).as_deref(), Some(entry.address.as_str()));
            assert!(entry.name.chars().count() <= MAX_NAME && !entry.name.chars().any(char::is_control));
            assert!(page.entries.iter().skip(i + 1).all(|other| !other.address.eq_ignore_ascii_case(&entry.address)));
        }
    }
    if let Ok(typed) = std::str::from_utf8(data)
        && let Some(text) = search_text(typed)
    {
        assert!((MIN_QUERY..=MAX_QUERY).contains(&text.chars().count()) && !text.contains('&'));
    }
});
