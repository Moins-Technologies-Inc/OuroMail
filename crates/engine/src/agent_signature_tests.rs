// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An agent never changes the signature the owner has open in File › Signatures
//! (ouromail-signature-guard). While one is open, the editing commands work on it rather than on
//! a message being written, so every command that writes in the editor in front, and every
//! signature command that changes the open one in place, is refused for an agent, whatever it
//! was allowed ([`agent::open_signature_refusal`]). Behind the list, [`Session::run`] undoes
//! whatever an agent's command left in the open signature, so that a command the list forgot
//! changes nothing either. An agent proposes a whole signature with `signature.set`, which asks
//! the owner, and whose text may not hold characters that draw nothing.
//!
//! The people and the text are invented (`example.org`, `.example`); the signature text is data.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use mailcraft_model::rich::RichText;

use crate::agent::{self, Class, Verdict};
use crate::{Session, sample};

const SIG: &str = "Studio";
const SIG_TEXT: &str = "Kind regards\nAna Example, Studio Example";
const CANARY: &str = "CANARY-7Q";
const ZWSP: char = '\u{200b}';
/// A 1×1 PNG.
const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

fn session() -> Session {
    Session::at(sample::mailbox(1_750_000_000, 0), 1_750_000_000, 0)
}

/// The owner's signature open in the editor, as File › Signatures opens it (run as the window
/// runs it: not as an agent).
fn opened() -> Session {
    let mut s = session();
    s.mailbox.signatures.clear();
    s.run("signature.set", &json!({"name": SIG, "text": SIG_TEXT, "default": true})).unwrap();
    s.run("signature.open", &json!({"name": SIG})).unwrap();
    s
}

/// What the open signature holds (its text with its formatting, the signature as opened: its
/// name, pictures, layout, details, HTML) and every signature kept.
fn holds(s: &Session) -> String {
    format!("{:?}|{:?}|{:?}", s.signature_edit.as_ref().map(|e| e.body.text()), s.signature_edit.as_ref().map(|e| &e.signature), s.mailbox.signatures)
}

/// What the open signature holds alone.
fn editor(s: &Session) -> Option<String> {
    s.signature_edit.as_ref().map(|e| format!("{:?}|{:?}", e.body.text(), e.signature))
}

/// No grant, each grant alone, all of them.
fn grants() -> Vec<Vec<Class>> {
    let mut g = vec![Vec::new()];
    g.extend(Class::ALL.iter().map(|c| vec![*c]));
    g.push(Class::ALL.to_vec());
    g
}

/// The probe's 22 silent writes into the open signature (SIG-PROBE, no grant), the Format
/// Painter with a look copied, then the signature commands that change the open one in place.
fn cases() -> Vec<(&'static str, Value)> {
    let z = format!("{CANARY}{ZWSP}");
    vec![
        ("compose.type", json!({"text": z})),
        ("compose.replaceText", json!({"from": 0, "to": 4, "expected": "Kind", "text": z})),
        ("compose.delete", json!({"from": 0, "to": 5})),
        ("edit.cut", json!({"from": 0, "to": 5})),
        ("edit.paste", json!({"text": z})),
        ("edit.paste", json!({"html": format!("<a href=\"https://attacker.example/\">{CANARY}</a>{ZWSP}")})),
        ("edit.replace", json!({"text": "regards", "with": z, "all": true})),
        ("insert.symbol", json!({"char": "\u{3a9}"})),
        ("insert.dateTime", json!({"format": "iso"})),
        ("format.bold", json!({"from": 0, "to": 4})),
        ("format.italic", json!({"from": 0, "to": 4})),
        ("format.underline", json!({"from": 0, "to": 4})),
        ("format.size", json!({"size": 8, "from": 0, "to": 4})),
        ("format.color", json!({"value": "#ffffff", "from": 0, "to": 4})),
        ("format.highlight", json!({"value": "#ffff00", "from": 0, "to": 4})),
        ("format.font", json!({"value": "Georgia", "from": 0, "to": 4})),
        ("format.bullets", json!({"from": 0, "to": 4})),
        ("format.numbering", json!({"from": 0, "to": 4})),
        ("format.indent", json!({"from": 0, "to": 4})),
        ("compose.link", json!({"url": "https://attacker.example/", "from": 0, "to": 4})),
        ("compose.undo", json!({})),
        ("compose.redo", json!({})),
        ("edit.formatPainter", json!({"apply": true, "from": 5, "to": 12})),
        ("signature.picture.add", json!({"name": SIG, "data": PNG, "file": "logo.png"})),
        ("signature.details", json!({"name": SIG, "details": {"name": "Ana Example", "website": "https://attacker.example/"}, "restart": true})),
        ("signature.html", json!({"html": format!("<p><a href=\"https://attacker.example/\">{CANARY}</a></p>")})),
        ("signature.layout", json!({"name": SIG, "value": "pictureAbove"})),
    ]
}

/// The probe's 22 rows (the first 22 cases).
const PROBE_ROWS: usize = 22;

/// `opened()` with what `id` acts on: the owner's own typing for undo and redo, a look copied
/// for the Format Painter, a picture for the layout.
fn prepared(id: &str) -> Session {
    let mut s = opened();
    prepare(&mut s, id);
    s
}

/// What `id` acts on, given by the owner in the open signature.
fn prepare(s: &mut Session, id: &str) {
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
    if id == "format.clear" {
        s.run("format.bold", &json!({"from": 0, "to": 4})).unwrap();
    }
    if id == "format.outdent" {
        s.run("format.indent", &json!({"from": 0, "to": 4})).unwrap();
    }
    if id == "signature.layout" {
        s.run("signature.picture.add", &json!({"name": SIG, "data": PNG, "file": "logo.png"})).unwrap();
    }
}

/// An agent's call as the window and the MCP server make it: the gate, then, when it says run,
/// the command with [`Session::agent`] set to what was allowed.
fn through_gate(s: &mut Session, id: &str, params: &Value, allow: &[Class]) -> (Verdict, Option<Result<Value, String>>) {
    let v = agent::check(s, id, params, allow);
    let ran = matches!(v, Verdict::Run).then(|| {
        s.agent = Some(allow.to_vec());
        let r = s.run(id, params).map_err(|e| e.to_string());
        s.agent = None;
        r
    });
    (v, ran)
}

/// The command run with [`Session::agent`] set, whatever the gate would say: what a command the
/// list forgot, or one the owner said yes to, would do.
fn as_agent(s: &mut Session, id: &str, params: &Value, allow: &[Class]) -> Result<Value, String> {
    s.agent = Some(allow.to_vec());
    let r = s.run(id, params).map_err(|e| e.to_string());
    s.agent = None;
    r
}

#[test]
fn each_case_changes_the_open_signature_when_the_owner_runs_it() {
    // The cases are real writes: run by the owner, each changes what the signature holds.
    for (id, params) in cases() {
        let mut s = prepared(id);
        let before = holds(&s);
        s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"));
        assert_ne!(holds(&s), before, "{id} {params} changes the open signature when the owner runs it");
    }
}

#[test]
fn every_write_into_the_open_signature_is_refused_for_an_agent_under_every_grant() {
    let mut rows = 0;
    for allow in grants() {
        for (id, params) in cases() {
            let mut s = prepared(id);
            let before = holds(&s);
            let (v, ran) = through_gate(&mut s, id, &params, &allow);
            match &v {
                Verdict::Refuse(why) => {
                    assert!(
                        why.contains(id) && why.contains("signature.set") && why.contains("File › Signatures"),
                        "{id}: the refusal names the command, the page and the way that works: {why}"
                    );
                }
                other => panic!("{id} {params} under {allow:?}: refused, not {other:?}"),
            }
            assert!(ran.is_none());
            assert_eq!(holds(&s), before, "{id} under {allow:?}: nothing changed");
            rows += 1;
        }
    }
    assert_eq!(rows, cases().len() * grants().len());
    assert!(cases().len() > PROBE_ROWS);
}

/// The backstop: whatever the gate says (a command the list forgot, a gate that lets it
/// through), an agent's command leaves the open signature as it was.
#[test]
fn whatever_the_gate_says_an_agents_command_leaves_the_open_signature_as_it_was() {
    for allow in grants() {
        for (id, params) in cases() {
            let mut s = prepared(id);
            let before = holds(&s);
            let guarded = s.signature_guarded();
            let r = as_agent(&mut s, id, &params, &allow);
            let why = r.expect_err(id);
            assert!(why.contains(id) && why.contains("signature.set") && why.contains("nothing was changed"), "{id}: {why}");
            assert_eq!(holds(&s), before, "{id} {params} under {allow:?}: undone");
            assert_eq!(s.signature_guarded(), guarded.wrapping_add(1), "{id}: counted for the window's status line");
            // And the owner's own editing goes on from there as before.
            assert!(
                s.signature_edit.as_ref().is_some_and(|e| !e.dirty)
                    || matches!(id, "compose.undo" | "compose.redo" | "edit.formatPainter" | "signature.layout")
            );
        }
    }
}

/// The same through the gate, as a mutant that drops an id from the refused list leaves it:
/// whatever the verdict, nothing of the agent's stays.
#[test]
fn a_forgotten_command_still_changes_nothing() {
    for allow in grants() {
        for (id, params) in cases() {
            let mut s = prepared(id);
            let before = holds(&s);
            let (v, ran) = through_gate(&mut s, id, &params, &allow);
            if let Some(r) = ran {
                assert!(r.is_err(), "{id} under {allow:?}: ran ({v:?}) and kept {r:?}");
            }
            assert_eq!(holds(&s), before, "{id} under {allow:?}");
        }
    }
}

/// Every command of the registry: an agent that names it with any of these shapes, under any
/// grant and without the gate, leaves the open signature holding nothing of its own: as it was,
/// closed, or given a signature anew exactly as it is kept, with nothing unsaved in it.
#[test]
fn no_command_of_the_registry_leaves_an_agents_change_in_the_open_signature() {
    let ids: Vec<&'static str> = session().registry.all().iter().map(|c| c.id).collect();
    let mut runs = 0usize;
    for id in &ids {
        for shape in shapes() {
            for allow in [Vec::new(), Class::ALL.to_vec()] {
                let mut s = with_a_draft_too();
                let before = editor(&s);
                let _ = as_agent(&mut s, id, &shape, &allow);
                runs += 1;
                let Some(e) = s.signature_edit.as_ref() else { continue };
                if editor(&s) == before {
                    continue;
                }
                let kept = s.mailbox.signature(&e.signature.name);
                let anew = !e.dirty && kept.is_some_and(|k| k.body() == e.body.text() && k.html() == e.signature.html() && *k == e.signature);
                assert!(anew, "{id} {shape} under {allow:?} left the open signature holding {:?}", editor(&s));
            }
        }
    }
    assert!(runs > 1000, "{runs}");
}

/// Shapes of params for every command: the editing ones' and the signature ones', with the canary.
fn shapes() -> Vec<Value> {
    let z = format!("{CANARY}{ZWSP}");
    vec![
        json!({}),
        json!({"from": 0, "to": 4}),
        json!({"text": z, "html": format!("<b>{CANARY}</b>")}),
        json!({
            "from": 0, "to": 4, "text": CANARY, "expected": "Kind", "with": CANARY, "all": true, "value": "#ffffff", "size": 8,
            "url": "https://attacker.example/", "char": "\u{3a9}", "format": "iso", "apply": true, "name": SIG
        }),
        json!({"name": SIG, "html": format!("<p>{CANARY}</p>"), "text": CANARY, "data": PNG, "file": "logo.png", "value": "pictureLeft", "details": {"name": CANARY}, "restart": true}),
        json!({"name": SIG, "to": "Renamed", "id": 1, "alt": CANARY, "width": 20}),
        json!({"text": "regards", "with": "wishes", "all": true}),
        json!({"apply": true, "from": 5, "to": 12}),
        json!({"value": "Georgia", "from": 0, "to": 4}),
    ]
}

/// `opened()` with a message being written too, so that every writing command is enabled; the
/// signature is still the editor in front.
fn with_a_draft_too() -> Session {
    let mut s = session();
    s.mailbox.signatures.clear();
    s.run("signature.set", &json!({"name": SIG, "text": SIG_TEXT})).unwrap();
    s.run("mail.new", &json!({})).unwrap();
    s.run("signature.open", &json!({"name": SIG})).unwrap();
    s
}

/// The commands whose target can be the editor in front (`writing`, or enabled only while there
/// is something to edit) and that the gate doesn't refuse, each with why it can't change an open
/// signature. A new command of either kind fails here until it is classified.
const CANT_CHANGE_THE_EDITOR: &[(&str, &str)] = &[
    ("compose.select", "moves the caret or the selection; the text and its formatting stay"),
    ("edit.copy", "copies; changes nothing"),
    ("edit.find", "finds; changes nothing"),
    ("review.wordCount", "counts; changes nothing"),
    ("edit.undo", "puts back the mailbox, never the editor; a signature it puts back is settings, asked"),
    ("signature.save", "keeps what the editor holds; settings, asked"),
    ("signature.close", "closes the editor, holding nothing of anybody's; settings, asked"),
    // The message being written itself: its recipients, attachments, flags, format and the
    // signature inserted in it (`compose_mut`, never `doc_mut`); disabled without one.
    ("compose.set", MESSAGE),
    ("compose.attach", MESSAGE),
    ("compose.attachItem", MESSAGE),
    ("insert.businessCard", MESSAGE),
    ("compose.removeAttachment", MESSAGE),
    ("compose.signature", MESSAGE),
    ("compose.signatureAsText", MESSAGE),
    ("compose.checkNames", MESSAGE),
    ("compose.replyTo", MESSAGE),
    ("compose.readReceipt", MESSAGE),
    ("compose.followUp", MESSAGE),
    ("compose.highImportance", MESSAGE),
    ("compose.lowImportance", MESSAGE),
    ("compose.send", MESSAGE),
    ("compose.saveDraft", MESSAGE),
    ("compose.close", MESSAGE),
    ("compose.discard", MESSAGE),
    ("compose.popOut", MESSAGE),
    ("compose.formatHtml", MESSAGE),
    ("compose.formatPlain", MESSAGE),
    ("compose.delay", MESSAGE),
    ("compose.sendLater", MESSAGE),
];

/// Why a command for the message being written can't change an open signature.
const MESSAGE: &str = "acts on the message being written itself, never on the editor in front";

#[test]
fn every_command_that_can_target_the_editor_in_front_is_classified() {
    let reg = session().registry.clone();
    for id in agent::EDITS_THE_EDITOR.iter().chain(agent::CHANGES_THE_OPEN_SIGNATURE) {
        assert!(reg.get(id).is_some(), "{id} is a command");
    }
    let nothing = session();
    let sig_only = opened();
    let mut unclassified = Vec::new();
    for spec in reg.all() {
        let flips = (spec.enabled)(&nothing).is_some() && (spec.enabled)(&sig_only).is_none();
        if (spec.writing || flips) && !agent::EDITS_THE_EDITOR.contains(&spec.id) && !CANT_CHANGE_THE_EDITOR.iter().any(|(id, _)| *id == spec.id) {
            unclassified.push(spec.id);
        }
    }
    assert!(unclassified.is_empty(), "classify these in agent::EDITS_THE_EDITOR, or in CANT_CHANGE_THE_EDITOR with why: {unclassified:?}");
    // Run by the owner with every shape, the commands that change what the open signature holds
    // in place: each must be refused for agents.
    let mut changes = BTreeSet::new();
    for spec in reg.all() {
        for shape in shapes() {
            let mut s = with_a_draft_too();
            prepare(&mut s, spec.id);
            let generation = s.signature_edit.as_ref().map(|e| e.generation);
            let before = editor(&s);
            let _ = s.run(spec.id, &shape);
            let same_editor = s.signature_edit.as_ref().map(|e| e.generation) == generation;
            if same_editor && editor(&s) != before {
                changes.insert(spec.id);
            }
        }
    }
    let unrefused: Vec<&&str> = changes
        .iter()
        .filter(|id| !agent::EDITS_THE_EDITOR.contains(id) && !agent::CHANGES_THE_OPEN_SIGNATURE.contains(id) && !REWRITES_WHAT_IS_KEPT.contains(id))
        .collect();
    assert!(unrefused.is_empty(), "these change the open signature in place and an agent isn't refused them: {unrefused:?}");
    for (id, why) in CANT_CHANGE_THE_EDITOR {
        assert!(reg.get(id).is_some() && !changes.contains(id), "{id}: {why}");
    }
    // Enough was seen for this to mean something: every listed writer showed itself.
    for id in agent::EDITS_THE_EDITOR.iter().filter(|id| **id != "insert.picture") {
        assert!(changes.contains(id), "{id} was seen changing the open signature");
    }
}

/// Signature commands that change the open one only as the owner asked for (saving it keeps what
/// it holds; `set` gives it the signature anew, as kept): the gate asks about them, as settings.
const REWRITES_WHAT_IS_KEPT: &[&str] = &["signature.save"];

#[test]
fn the_owners_own_editing_is_untouched() {
    for (id, params) in cases() {
        let mut s = prepared(id);
        let guarded = s.signature_guarded();
        assert!(s.run(id, &params).is_ok(), "{id}");
        assert_eq!(s.signature_guarded(), guarded, "{id}: the owner's edit isn't counted as an agent's");
    }
}

#[test]
fn the_owners_save_after_an_agents_attempt_keeps_only_what_the_owner_wrote() {
    let mut s = opened();
    let p = json!({"text": format!(" {CANARY}{ZWSP}")});
    let (v, _) = through_gate(&mut s, "compose.type", &p, &[]);
    assert!(matches!(v, Verdict::Refuse(_)), "{v:?}");
    let _ = as_agent(&mut s, "compose.type", &p, &[]);
    s.run("compose.type", &json!({"text": "Best "})).unwrap();
    s.run("signature.save", &json!({})).unwrap();
    let kept = s.mailbox.signature(SIG).map(|x| x.plain()).unwrap_or_default();
    assert_eq!(kept, format!("Best {SIG_TEXT}"));
    s.run("signature.close", &json!({})).unwrap();
    s.run("mail.new", &json!({})).unwrap();
    let body = s.compose.as_ref().map(|c| c.body.plain()).unwrap_or_default();
    assert!(!body.contains(CANARY) && !body.contains(ZWSP), "{body:?}");
}

#[test]
fn an_agents_typing_meant_for_its_own_message_is_refused_never_rerouted() {
    let mut s = session();
    s.mailbox.signatures.clear();
    s.run("signature.set", &json!({"name": SIG, "text": SIG_TEXT})).unwrap();
    // The agent's own message.
    as_agent(&mut s, "mail.new", &json!({}), &[]).unwrap();
    let compose = s.compose_id().unwrap();
    // No signature open: the agent writes in its own message, as before.
    let p = json!({"text": "Hello"});
    let (v, ran) = through_gate(&mut s, "compose.type", &p, &[]);
    assert_eq!(v, Verdict::Run);
    assert!(ran.is_some_and(|r| r.is_ok()));
    assert_eq!(s.compose.as_ref().map(|c| c.body.plain()).unwrap_or_default(), "Hello");
    // The owner opens a signature in File › Signatures.
    s.run("signature.open", &json!({"name": SIG})).unwrap();
    let before = (holds(&s), s.compose.as_ref().map(|c| c.body.plain()));
    for params in [json!({"text": " there"}), json!({"text": " there", "compose": compose})] {
        let (v, _) = through_gate(&mut s, "compose.type", &params, &[]);
        match v {
            Verdict::Refuse(why) => assert!(why.contains("wait until the owner has left File › Signatures"), "{why}"),
            other => panic!("refused, not {other:?}"),
        }
        assert!(as_agent(&mut s, "compose.type", &params, &Class::ALL).is_err());
        assert_eq!((holds(&s), s.compose.as_ref().map(|c| c.body.plain())), before, "{params}: neither changed");
    }
    // The remedy the refusal names: once the owner has left the page, an agent writes in a
    // message again; the one open meanwhile may have become the owner's (opening a signature
    // counts as the owner's work on an unsaved message, as before), so it starts one of its own.
    s.run("signature.close", &json!({})).unwrap();
    // (It opens beside the one there, which stays current: it is named by the id it answers.)
    let own = as_agent(&mut s, "mail.new", &json!({}), &[]).unwrap()["compose"].as_u64().unwrap();
    let (v, ran) = through_gate(&mut s, "compose.type", &json!({"text": "Hi", "compose": own}), &[]);
    assert_eq!(v, Verdict::Run);
    assert!(matches!(ran, Some(Ok(_))), "{ran:?}");
    assert_eq!(s.all_composing().find(|c| c.id == own).map(|c| c.body.plain()).unwrap_or_default(), "Hi");
    assert!(as_agent(&mut s, "signature.open", &json!({"name": SIG}), &[Class::Settings]).is_ok() && holds(&s).contains("Kind regards"));
}

#[test]
fn signature_set_from_an_agent_asks_and_a_yes_writes_what_was_shown() {
    let mut s = opened();
    let p = json!({"name": SIG, "text": "Best wishes,\nAna Example"});
    match agent::check(&s, "signature.set", &p, &[]) {
        Verdict::Ask(Class::Settings, what) => assert!(what.contains("Best wishes,") && what.contains("Ana Example"), "{what}"),
        other => panic!("asked, not {other:?}"),
    }
    // The owner's yes: run as the window runs it, with settings added to what was allowed.
    assert_eq!(agent::answered(&s, "signature.set", &p, &[], Class::Settings), Verdict::Run);
    let guarded = s.signature_guarded();
    as_agent(&mut s, "signature.set", &p, &[Class::Settings]).unwrap();
    assert_eq!(s.signature_guarded(), guarded);
    assert_eq!(s.mailbox.signature(SIG).map(|x| x.plain()).unwrap_or_default(), "Best wishes,\nAna Example");
    let e = s.signature_edit.as_ref().unwrap();
    assert!(!e.dirty && e.body.plain() == "Best wishes,\nAna Example", "the editor shows it, as kept");
    // Another signature than the one open isn't the open one: in-place commands ask as settings.
    s.run("signature.set", &json!({"name": "Other", "text": "Other signature"})).unwrap();
    assert!(matches!(agent::check(&s, "signature.html", &json!({"name": "Other", "html": "<p>Hi</p>"}), &[]), Verdict::Ask(Class::Settings, _)));
    // The remedy for the in-place commands: once the owner has left the page, they are asked.
    s.run("signature.close", &json!({"discard": true})).unwrap();
    for (id, params) in cases().into_iter().filter(|(id, _)| id.starts_with("signature.")) {
        assert!(matches!(agent::check(&s, id, &params, &[]), Verdict::Ask(Class::Settings, _)), "{id}");
    }
}

#[test]
fn a_signature_from_an_agent_that_holds_characters_which_draw_nothing_is_refused() {
    let hidden = ['\u{200b}', '\u{200d}', '\u{202e}', '\u{2066}', '\u{feff}', '\u{e0041}', '\u{00ad}', '\u{115f}', '\u{3164}', '\u{0007}'];
    for allow in grants() {
        for c in hidden {
            for (id, params) in [
                ("signature.set", json!({"name": SIG, "text": format!("Best{c} wishes")})),
                ("signature.set", json!({"name": "Other", "html": format!("<p>Best{c}</p>")})),
                ("signature.html", json!({"name": SIG, "html": format!("<p>Best{c}</p>")})),
                ("signature.details", json!({"name": SIG, "details": {"name": format!("Ana{c}")}})),
                ("signature.rename", json!({"name": SIG, "to": format!("Stu{c}dio")})),
            ] {
                let s = session_with_sig();
                match agent::check(&s, id, &params, &allow) {
                    Verdict::Refuse(why) => {
                        assert!(why.contains("draw nothing") && why.contains(&format!("U+{:04X}", u32::from(c))), "{id}: {why}")
                    }
                    other => panic!("{id} with U+{:04X} under {allow:?}: refused, not {other:?}", u32::from(c)),
                }
            }
        }
    }
    // Written as a character reference, it is only found once read: the run undoes it.
    for (id, params) in [
        ("signature.set", json!({"name": SIG, "html": "<p>Best&#8203; wishes</p>"})),
        ("signature.html", json!({"name": SIG, "html": "<p>Best&#x200B; wishes</p>"})),
    ] {
        let mut s = session_with_sig();
        let before = holds(&s);
        assert_eq!(agent::check(&s, id, &params, &[Class::Settings]), Verdict::Run, "{id}");
        let why = as_agent(&mut s, id, &params, &[Class::Settings]).expect_err(id);
        assert!(why.contains("draw nothing"), "{why}");
        assert_eq!(holds(&s), before);
    }
    // Ordinary text, accents and a Persian name with its joiner-free spelling go through.
    let mut s = session_with_sig();
    let p = json!({"name": SIG, "text": "Grüße,\nZoë Exämple — Café Example\nمریم"});
    assert_eq!(agent::check(&s, "signature.set", &p, &[Class::Settings]), Verdict::Run);
    as_agent(&mut s, "signature.set", &p, &[Class::Settings]).unwrap();
    // The owner's own text is the owner's: nothing is refused.
    let mut s = session_with_sig();
    s.run("signature.set", &json!({"name": SIG, "text": "Ana\u{200d}Example"})).unwrap();
    assert!(s.mailbox.signature(SIG).is_some_and(|x| x.plain().contains('\u{200d}')));
    // And an agent's rename of the owner's signature that holds one isn't refused for it.
    assert_eq!(agent::check(&s, "signature.rename", &json!({"name": SIG, "to": "Work"}), &[Class::Settings]), Verdict::Run);
    as_agent(&mut s, "signature.rename", &json!({"name": SIG, "to": "Work"}), &[Class::Settings]).unwrap();
}

fn session_with_sig() -> Session {
    let mut s = session();
    s.mailbox.signatures.clear();
    s.run("signature.set", &json!({"name": SIG, "text": SIG_TEXT})).unwrap();
    s
}

/// Closing, saving and opening, which the gate asks about as settings, still work for an agent
/// with the owner's yes, and leave nothing of the agent's in the editor.
#[test]
fn an_agent_with_the_owners_yes_still_opens_saves_and_closes() {
    let mut s = opened();
    s.run("compose.type", &json!({"text": "Owner "})).unwrap();
    as_agent(&mut s, "signature.save", &json!({}), &[Class::Settings]).unwrap();
    assert_eq!(s.mailbox.signature(SIG).map(|x| x.plain()).unwrap_or_default(), format!("Owner {SIG_TEXT}"));
    as_agent(&mut s, "signature.rename", &json!({"name": SIG, "to": "Work"}), &[Class::Settings]).unwrap();
    as_agent(&mut s, "signature.close", &json!({}), &[Class::Settings]).unwrap();
    as_agent(&mut s, "signature.open", &json!({"name": "Work"}), &[Class::Settings]).unwrap();
    assert!(s.signature_edit.as_ref().is_some_and(|e| e.signature.name == "Work" && !e.dirty));
}

/// Arrays inside arrays, `n` deep, round a hidden character.
fn deep(n: usize) -> Value {
    (0..n).fold(json!("x\u{200b}"), |v, _| json!([v]))
}

/// Hostile params never make the gate or the guard panic.
#[test]
fn hostile_params_to_the_signature_guard_never_panic() {
    let junk = [
        json!(null),
        json!([]),
        json!("text"),
        json!({"name": null}),
        json!({"name": 5, "html": [1]}),
        json!({"name": "\u{0}", "text": "\u{202e}".repeat(10_000)}),
        json!({"details": {"name": deep(200)}, "text": deep(40)}),
    ];
    for allow in grants() {
        for spec in session().registry.all().iter().filter(|c| c.id.starts_with("signature.") || agent::EDITS_THE_EDITOR.contains(&c.id)) {
            for j in &junk {
                let mut s = opened();
                let before = holds(&s);
                let _ = through_gate(&mut s, spec.id, j, &allow);
                let _ = as_agent(&mut s, spec.id, j, &allow);
                let _ = agent::open_signature_refusal(&s, spec.id, j);
                if agent::EDITS_THE_EDITOR.contains(&spec.id) {
                    assert_eq!(holds(&s), before, "{}", spec.id);
                }
            }
        }
    }
}

/// The check after each command, alone: what counts as the agent's change to the open signature.
#[test]
fn the_check_after_each_command_judges_what_the_editor_holds() {
    use crate::cmd::signature::agent_changed_editor as changed;
    let s = opened();
    let mb = &s.mailbox;
    let e = s.signature_edit.clone().unwrap();
    assert!(!changed(None, None, mb));
    assert!(!changed(Some(&e), None, mb), "closed: nothing is left in it");
    let mut caret = e.clone();
    caret.body.select(2, 4).unwrap();
    assert!(!changed(Some(&e), Some(&caret), mb), "the caret moved: nothing was written");
    let mut typed = e.clone();
    typed.body.replace(RichText::from_plain(CANARY)).unwrap();
    assert!(changed(Some(&e), Some(&typed), mb), "the same opening with other text");
    let mut bold = e.clone();
    bold.body.select(0, 4).unwrap();
    bold.body.format(mailcraft_model::rich::Format::Bold(true)).unwrap();
    assert!(changed(Some(&e), Some(&bold), mb), "the same opening with other formatting");
    let mut anew = e.clone();
    anew.generation = e.generation.wrapping_add(1);
    assert!(!changed(Some(&e), Some(&anew), mb), "given anew, as it was");
    assert!(!changed(None, Some(&anew), mb), "opened as it is kept");
    let mut other = typed.clone();
    other.generation = e.generation.wrapping_add(1);
    other.dirty = false;
    assert!(changed(Some(&e), Some(&other), mb), "given anew holding what isn't kept");
    assert!(changed(None, Some(&other), mb), "opened holding what isn't kept");
    let mut unsaved = anew.clone();
    unsaved.dirty = true;
    assert!(changed(None, Some(&unsaved), mb), "opened with something unsaved in it");
    let mut renamed = e.clone();
    renamed.signature.name = "Work".into();
    renamed.generation = e.generation.wrapping_add(1);
    assert!(!changed(Some(&e), Some(&renamed), mb), "a rename follows the open signature");
}

/// Arrays inside arrays, `n` deep, round ordinary text with no hidden characters.
fn deep_clean(n: usize) -> Value {
    (0..n).fold(json!("clean"), |v, _| json!([v]))
}

/// SEC-B2: HTML character references decoded into control or zero-width/bidi characters are refused
/// across `signature.html` and `signature.import`, and every field of `Signature` is walked by
/// `agent_hid_text`. Normal visible entities still succeed.
#[test]
fn sec_b2_html_entities_and_all_fields_are_refused() {
    let scope = mailcraft_html::AuthoredScope::new("mc-secb2").unwrap();
    let payloads: [(&str, Value, &str); 5] = [
        ("signature.html", json!({"name": SIG, "html": "<p>Best&#7;wishes</p>"}), "\u{7}"),
        ("signature.html", json!({"name": SIG, "html": "<style>.h{display:none}</style><p>Best</p><span class=\"h\">&#8203;</span>"}), "\u{200b}"),
        ("signature.html", json!({"name": SIG, "html": "<p>Best</p><img src=\"https://example.org/a.png\" alt=\"&#8203;\">"}), "\u{200b}"),
        (
            "signature.import",
            json!({"name": "ImportedBidi", "html": "<style>.h{display:none}</style><p>Best</p><span class=\"h\">&#x202E;</span>", "assets": []}),
            "\u{202e}",
        ),
        ("signature.import", json!({"name": "ImportedEsc", "html": "<p>Best&#27;wishes</p>", "assets": []}), "\u{1b}"),
    ];
    for (id, params, leaked_char) in payloads {
        let mut s = session_with_sig();
        let before = holds(&s);
        let err = as_agent(&mut s, id, &params, &[Class::Settings]).expect_err(&format!("{id} with payload {params} must be refused"));
        assert!(err.contains("draw nothing"), "{id}: {err}");
        assert_eq!(holds(&s), before, "{id}: mailbox must stay unchanged");
        // Confirm no signature in the mailbox emits the decoded hidden/control character.
        for sig in &s.mailbox.signatures {
            if let Some(own) = sig.html() {
                let frag =
                    mailcraft_mime_model::rich::authored_signature_html(sig, own, &scope, &|p| format!("sig-{}@example.invalid", p.id)).unwrap();
                assert!(!frag.body.contains(leaked_char) && !frag.head.contains(leaked_char), "{id}: leaked in authored HTML");
            }
        }
    }

    // Normal visible HTML entities still succeed and emit no hidden characters.
    let mut s = session_with_sig();
    let html_ok = "<p>R&amp;D &lt;team&gt; &copy; 2026</p>";
    as_agent(&mut s, "signature.html", &json!({"name": SIG, "html": html_ok}), &[Class::Settings]).unwrap();
    as_agent(&mut s, "signature.import", &json!({"name": "VisibleImport", "html": html_ok, "assets": []}), &[Class::Settings]).unwrap();
    for name in [SIG, "VisibleImport"] {
        let sig = s.mailbox.signature(name).unwrap();
        let own = sig.html().unwrap();
        let frag = mailcraft_mime_model::rich::authored_signature_html(sig, own, &scope, &|p| format!("sig-{}@example.invalid", p.id)).unwrap();
        assert!(!frag.body.chars().any(agent::draws_nothing), "{name}: {}", frag.body);
    }

    // `agent_hid_text` walks every Signature field: name, body runs (text, link, table), pictures (name, alt), details.
    use crate::cmd::signature::agent_hid_text;
    use mailcraft_model::rich::{CellAlignment, CellText, Paragraph, PastedTable, Run, Style, TableCell};
    use mailcraft_model::signature::{Details, Signature};
    use std::sync::Arc;

    let base = session_with_sig().mailbox;
    // 1. sig.name
    let mut after = base.clone();
    after.signatures[0].name = format!("Studio{ZWSP}");
    assert!(agent_hid_text(&base, &after), "sig.name must be checked");

    // 2. sig.body() run.style.link (including a `tel:` link omitted from `sig.plain()` because digits match)
    let mut after = base.clone();
    let link_body = RichText::from_paragraphs(vec![Paragraph::new(
        vec![Run::new("+1234567", Style { link: Some(format!("tel:+1{ZWSP}234567")), ..Style::default() })],
        Default::default(),
    )]);
    after.signatures[0].set_body(link_body).unwrap();
    assert!(!after.signatures[0].plain().chars().any(agent::draws_nothing), "plain() omits matching tel: link");
    assert!(agent_hid_text(&base, &after), "run.style.link must be checked even when omitted from plain()");

    // 3. sig.body() run.table cell text and cell link
    for (cell_text, cell_link) in [(format!("Cell{ZWSP}"), None), ("Cell".to_string(), Some(format!("https://example.org/{ZWSP}")))] {
        let mut after = base.clone();
        let table = PastedTable::new(
            1,
            vec![vec![TableCell {
                lines: vec![vec![CellText { text: cell_text, style: Style { link: cell_link, ..Style::default() } }]],
                header: false,
                colspan: 1,
                rowspan: 1,
                background: None,
                alignment: CellAlignment::Left,
            }]],
        )
        .unwrap();
        let mut run = Run::new("\u{fffc}", Style::default());
        run.table = Some(Arc::new(table));
        let body = RichText::from_paragraphs(vec![Paragraph::new(vec![run], Default::default())]);
        after.signatures[0].set_body(body).unwrap();
        assert!(agent_hid_text(&base, &after), "run.table cell must be checked");
    }

    // 4. sig.pictures (name and alt)
    let png_bytes = crate::cmd::base64_decode(PNG).unwrap();
    let mut after_name = base.clone();
    let mut s_pic = Signature::plain_text("PicName", "Clean");
    s_pic.add_picture(&format!("logo{ZWSP}.png"), png_bytes.clone(), Some("Logo"), Some(0)).unwrap();
    after_name.signatures.push(s_pic);
    assert!(agent_hid_text(&base, &after_name), "picture.name must be checked");

    let mut after_alt = base.clone();
    let mut s_alt = Signature::plain_text("PicAlt", "Clean");
    s_alt.add_picture("logo.png", png_bytes, Some(&format!("Alt{ZWSP}")), None).unwrap();
    after_alt.signatures.push(s_alt);
    assert!(agent_hid_text(&base, &after_alt), "picture.alt must be checked");

    // 5. sig.details
    let mut after_det = base.clone();
    after_det.signatures[0].details = Some(Details { organisation: format!("Studio{ZWSP}"), ..Details::default() });
    assert!(agent_hid_text(&base, &after_det), "sig.details must be checked");
}

/// SEC-M1: `agent_changed_editor` compares `(signature, body.text(), dirty, body.painter())` directly,
/// catching `> MAX_TEXT` signature mutations, `dirty` flips, and `painter()` changes.
#[test]
fn sec_m1_backstop_catches_over_max_text_dirty_and_painter() {
    use crate::cmd::signature::agent_changed_editor as changed;
    use mailcraft_model::signature::{Layout, MAX_TEXT};

    let mut s = opened();
    // Grow open signature body past MAX_TEXT so `edited(e)` returns Err.
    let big = "a".repeat(MAX_TEXT + 50);
    s.run("compose.type", &json!({"text": big})).unwrap();
    let mb = &s.mailbox;
    let over = s.signature_edit.clone().unwrap();
    assert!(over.body.text().len() > MAX_TEXT);

    let mut mutated_layout = over.clone();
    mutated_layout.signature.layout = Layout::PictureAbove;
    assert!(changed(Some(&over), Some(&mutated_layout), mb), "> MAX_TEXT signature.layout mutation must be caught");

    // `dirty` flip (false -> true) on same generation.
    let clean = opened().signature_edit.clone().unwrap();
    let mut flipped_dirty = clean.clone();
    flipped_dirty.dirty = true;
    assert!(changed(Some(&clean), Some(&flipped_dirty), mb), "dirty false -> true flip must be caught");

    // `dirty` flip (true -> false) without actually saving to mailbox.
    let mut dirty_ed = clean.clone();
    dirty_ed.dirty = true;
    dirty_ed.signature.layout = Layout::PictureAbove;
    let mut cleared_without_save = dirty_ed.clone();
    cleared_without_save.dirty = false;
    assert!(changed(Some(&dirty_ed), Some(&cleared_without_save), mb), "dirty true -> false without saving must be caught");

    // `body.painter()` armed on same generation.
    let mut armed = clean.clone();
    armed.body.copy_look();
    assert!(armed.body.painter().is_some());
    assert!(changed(Some(&clean), Some(&armed), mb), "arming format painter must be caught");

    // End-to-end backstop: bypassed `edit.formatPainter {}` on open signature is undone.
    let mut s2 = opened();
    let guarded = s2.signature_guarded();
    let err = as_agent(&mut s2, "edit.formatPainter", &json!({}), &Class::ALL).expect_err("painter arm must be undone");
    assert!(err.contains("edit.formatPainter"), "{err}");
    assert_eq!(s2.signature_guarded(), guarded.wrapping_add(1));
    assert!(s2.signature_edit.as_ref().is_some_and(|e| e.body.painter().is_none()));
}

/// SEC-R2-m1: `agent_changed_editor` also compares `RichBody::pending()`, catching collapsed-cursor
/// `format.bold {}` / `format.color` mutations on same-generation edits and fresh opens.
#[test]
fn sec_r2_m1_backstop_catches_pending_style_changes() {
    use crate::cmd::signature::agent_changed_editor as changed;
    use mailcraft_model::rich::Format;

    let s = opened();
    let mb = &s.mailbox;
    let clean = s.signature_edit.clone().unwrap();
    assert!(clean.body.pending().is_none());

    // Direct check: collapsed cursor + format sets `body.pending()` without changing `body.text()` or `dirty`.
    let mut pending_bold = clean.clone();
    pending_bold.body.select(0, 0).unwrap();
    pending_bold.body.format(Format::Bold(true)).unwrap();
    assert!(pending_bold.body.pending().is_some());
    assert_eq!(clean.body.text(), pending_bold.body.text());
    assert_eq!(clean.dirty, pending_bold.dirty);
    assert!(changed(Some(&clean), Some(&pending_bold), mb), "same-gen pending() mutation must be caught");
    assert!(changed(None, Some(&pending_bold), mb), "fresh open with pending() set must be caught");

    // End-to-end backstop: bypassed `format.bold {}` and `format.color` with collapsed cursor are undone.
    for (id, params) in [("format.bold", json!({})), ("format.color", json!({"value": "#ffffff"}))] {
        let mut s2 = opened();
        let guarded = s2.signature_guarded();
        assert!(s2.signature_edit.as_ref().is_some_and(|e| e.body.pending().is_none()));
        let err = as_agent(&mut s2, id, &params, &Class::ALL).expect_err(&format!("{id} pending style must be undone"));
        assert!(err.contains(id), "{id}: {err}");
        assert_eq!(s2.signature_guarded(), guarded.wrapping_add(1), "{id}: guarded counter must increment");
        assert!(s2.signature_edit.as_ref().is_some_and(|e| e.body.pending().is_none()), "{id}: pending() must be restored to None");
    }
}

/// SM14: `hidden_text_refusal` enforces the `MAX_TEXT_DEPTH = 32` depth cap even on clean leaves.
#[test]
fn sm14_hidden_text_refusal_enforces_depth_cap() {
    let why = agent::hidden_text_refusal("signature.set", &json!({"name": SIG, "text": deep(40)})).expect("depth 40 must be refused");
    assert!(why.contains("more than 32 deep"), "expected depth-cap refusal, got: {why}");
    let why_clean = agent::hidden_text_refusal("signature.set", &json!({"name": SIG, "text": deep_clean(40)}))
        .expect("clean leaf at depth 40 must still hit depth cap");
    assert!(why_clean.contains("more than 32 deep"), "expected depth-cap refusal on clean leaf, got: {why_clean}");
    assert!(
        agent::hidden_text_refusal("signature.set", &json!({"name": SIG, "text": deep_clean(30)})).is_none(),
        "depth 30 clean value stays allowed"
    );
}

/// CORR-04: `signature.set` on the open signature asks & refreshes when clean (`!dirty`),
/// and is refused even under `--allow settings` when dirty (`dirty`), preserving unsaved edits.
#[test]
fn corr_04_dirty_open_signature_refuses_signature_set() {
    // 1. Clean open signature: asks Settings, and with Settings refreshes the open editor cleanly.
    let mut s = opened();
    assert!(s.signature_edit.as_ref().is_some_and(|e| !e.dirty));
    let p = json!({"name": SIG, "text": "Fresh text"});
    match agent::check(&s, "signature.set", &p, &[]) {
        Verdict::Ask(Class::Settings, what) => assert!(what.contains("Fresh text"), "{what}"),
        other => panic!("clean open signature should Ask(Settings), got {other:?}"),
    }
    assert_eq!(agent::check(&s, "signature.set", &p, &[Class::Settings]), Verdict::Run);
    as_agent(&mut s, "signature.set", &p, &[Class::Settings]).unwrap();
    assert_eq!(s.mailbox.signature(SIG).map(|x| x.plain()).as_deref(), Some("Fresh text"));
    assert!(s.signature_edit.as_ref().is_some_and(|e| !e.dirty && e.body.plain() == "Fresh text"));

    // 2. Dirty open signature: refused under every grant (including Settings and ALL).
    s.run("compose.type", &json!({"text": "Unsaved "})).unwrap();
    assert!(s.signature_edit.as_ref().is_some_and(|e| e.dirty));
    let before = holds(&s);
    for name in [SIG, "  studio  ", "Stu\ndio"] {
        let p = json!({"name": name, "text": "Overwritten"});
        for allow in [vec![], vec![Class::Settings], Class::ALL.to_vec()] {
            match agent::check(&s, "signature.set", &p, &allow) {
                Verdict::Refuse(why) => {
                    assert!(why.contains("save") && why.contains("discard"), "refusal must tell owner to save or discard first: {why}");
                }
                other => panic!("dirty signature.set ({name:?}) under {allow:?} must Refuse, got {other:?}"),
            }
            // Even bypassing the gate, the backstop preserves the dirty open signature.
            assert!(as_agent(&mut s, "signature.set", &p, &allow).is_err(), "backstop must protect dirty signature.set ({name:?})");
            assert_eq!(holds(&s), before, "unsaved edits must be preserved");
        }
    }
    assert!(s.signature_edit.as_ref().is_some_and(|e| e.dirty && e.body.plain().starts_with("Unsaved ")));

    // 3. Another signature (not the open dirty one) still asks Settings as usual.
    let other = json!({"name": "Other", "text": "Other text"});
    assert!(matches!(agent::check(&s, "signature.set", &other, &[]), Verdict::Ask(Class::Settings, _)));
}
