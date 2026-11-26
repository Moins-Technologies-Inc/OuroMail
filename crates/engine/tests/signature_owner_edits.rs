// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's own editing of a signature open in File › Signatures (ouromail-signature-guard):
//! every command an agent is now refused there still works for the owner. With
//! `SIG_GUARD_OWNER_OUT` set, the test writes what each command left (its answer, the editor's
//! text and formatting, the signature kept) to that file, so that the same test run on the tree
//! before the guard shows the results are byte for byte the same.
//!
//! The people and the text are invented (`example.org`, `.example`).

use mailcraft_engine::{Session, sample};
use serde_json::{Value, json};

const SIG: &str = "Studio";
const SIG_TEXT: &str = "Kind regards\nAna Example, Studio Example";
/// A 1×1 PNG.
const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

fn opened() -> Session {
    let mut s = Session::at(sample::mailbox(1_750_000_000, 0), 1_750_000_000, 0);
    s.mailbox.signatures.clear();
    s.run("signature.set", &json!({"name": SIG, "text": SIG_TEXT, "default": true})).unwrap();
    s.run("signature.open", &json!({"name": SIG})).unwrap();
    s
}

fn cases() -> Vec<(&'static str, Value)> {
    let z = "Owner\u{a0}text";
    vec![
        ("compose.type", json!({"text": z})),
        ("compose.replaceText", json!({"from": 0, "to": 4, "expected": "Kind", "text": z})),
        ("compose.delete", json!({"from": 0, "to": 5})),
        ("edit.cut", json!({"from": 0, "to": 5})),
        ("edit.paste", json!({"text": z})),
        ("edit.paste", json!({"html": "<a href=\"https://www.example.org/\">Example</a>"})),
        ("edit.replace", json!({"text": "regards", "with": z, "all": true})),
        ("insert.symbol", json!({"char": "\u{3a9}"})),
        ("insert.dateTime", json!({"format": "iso"})),
        ("format.bold", json!({"from": 0, "to": 4})),
        ("format.italic", json!({"from": 0, "to": 4})),
        ("format.underline", json!({"from": 0, "to": 4})),
        ("format.size", json!({"size": 8, "from": 0, "to": 4})),
        ("format.color", json!({"value": "#336699", "from": 0, "to": 4})),
        ("format.highlight", json!({"value": "#ffff00", "from": 0, "to": 4})),
        ("format.font", json!({"value": "Georgia", "from": 0, "to": 4})),
        ("format.bullets", json!({"from": 0, "to": 4})),
        ("format.numbering", json!({"from": 0, "to": 4})),
        ("format.indent", json!({"from": 0, "to": 4})),
        ("format.outdent", json!({"from": 0, "to": 4})),
        ("format.clear", json!({"from": 0, "to": 4})),
        ("compose.link", json!({"url": "https://www.example.org/", "from": 0, "to": 4})),
        ("edit.formatPainter", json!({"apply": true, "from": 5, "to": 12})),
        ("compose.undo", json!({})),
        ("compose.redo", json!({})),
        ("signature.picture.add", json!({"name": SIG, "data": PNG, "file": "logo.png"})),
        ("signature.details", json!({"name": SIG, "details": {"name": "Ana Example", "website": "https://www.example.org/"}, "restart": true})),
        ("signature.html", json!({"html": "<p><a href=\"https://www.example.org/\">Example</a></p>"})),
        ("signature.layout", json!({"name": SIG, "value": "pictureAbove"})),
        ("signature.set", json!({"name": SIG, "text": "Best wishes,\nAna\u{200d}Example"})),
        ("signature.save", json!({})),
    ]
}

#[test]
fn the_owners_editing_of_an_open_signature_is_as_it_was() {
    let mut out = String::new();
    let mut changed = 0;
    for (id, params) in cases() {
        let mut s = opened();
        if matches!(id, "compose.undo" | "compose.redo") {
            s.run("compose.type", &json!({"text": " owner"})).unwrap();
            if id == "compose.redo" {
                s.run("compose.undo", &json!({})).unwrap();
            }
        }
        if id == "edit.formatPainter" {
            s.run("format.bold", &json!({"from": 0, "to": 4})).unwrap();
            s.run("compose.select", &json!({"from": 0, "to": 4})).unwrap();
            s.run("edit.formatPainter", &json!({})).unwrap();
        }
        if matches!(id, "signature.layout" | "signature.save") {
            s.run("signature.picture.add", &json!({"name": SIG, "data": PNG, "file": "logo.png"})).unwrap();
        }
        let before = format!("{:?}", s.signature_edit.as_ref().map(|e| (e.body.text(), &e.signature)));
        let r = s.run(id, &params);
        let e = s.signature_edit.as_ref();
        let after = format!("{:?}", e.map(|e| (e.body.text(), &e.signature)));
        changed += usize::from(after != before);
        out.push_str(&format!(
            "{id}\t{params}\t{}\t{:?}\t{}\t{after}\t{:?}\n",
            match &r {
                Ok(v) => format!("ok {v}"),
                Err(e) => format!("err {e}"),
            },
            e.map(|e| e.body.plain()),
            e.is_some_and(|e| e.dirty),
            s.mailbox.signatures,
        ));
        assert!(r.is_ok(), "{id} {params}: {r:?}");
    }
    if let Some(path) = std::env::var_os("SIG_GUARD_OWNER_OUT") {
        std::fs::write(path, &out).unwrap();
    }
    assert!(changed >= 25, "{changed} of {} changed the open signature\n{out}", cases().len());
}
