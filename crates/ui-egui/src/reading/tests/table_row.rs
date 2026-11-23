// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A layout table's columns are judged by what their own cells want: a long paragraph in a row
//! the whole width of the table wraps inside it, and is no reason to stack a row of small
//! pictures that fits. A layout table that is stacked keeps its own width and its place.
//!
//! The people, addresses, words and pictures are invented.

use egui::Rect;
use mailcraft_html::{BlockKind, Length};

use super::{Dice, check, html_message, in_frame, lay_out, shown_pictures};
use crate::reading::layout::{Item, ItemKind, Laid};
use crate::reading::view::Doc;
use crate::theme::Tokens;

fn message(html: &str) -> mailcraft_model::Message {
    html_message(70, ("Robin Ashdown", "robin@example.org"), "Field notes", html)
}

/// A notice of the kind a signature closes with: one paragraph, far longer than any pane is wide.
const NOTICE: &str = "This message and anything attached to it are meant for the people it is addressed to and may \
    hold field notes that are not yet ready to be shared. If it reached you by mistake, please tell the sender and \
    delete it; do not copy it, pass it on or act on what it says. The Harbour Field Society keeps a record of the \
    mail it sends and receives, and reads it only to answer the questions people send. Nothing in this message is \
    an offer, a promise or advice unless it says so in so many words, and the society answers for nothing that a \
    message changed on its way says.";

/// One flat table, 504 pixels at most, as a signature is built: a banner, a name, a line of
/// contact under a rule, a row of seven small linked pictures each with its label, and the notice.
fn signature() -> String {
    let marks: String = ["Ash", "Elm", "Fir", "Oak", "Yew", "Bay", "Box"]
        .iter()
        .enumerate()
        .map(|(i, label)| {
            format!(
                "<td width='14%' align='center' style='padding:6px 0'><a href='https://example.org/mark/{i}'>\
                 <img src='cid:step-done@parcel' width='32' height='32' alt='{label} mark'></a><br>\
                 <span style='font-size:10px'>{label}</span></td>"
            )
        })
        .collect();
    format!(
        "<table width='100%' cellspacing='0' cellpadding='0' role='presentation' style='width:100%;max-width:504px'>\
         <tr><td colspan='7'><img src='cid:hero@listing' width='504' height='120' alt='Harbour Field Society'></td></tr>\
         <tr><td colspan='7' style='padding:6px 0;font-size:15px'><b>Robin Ashdown</b> · Field notes editor</td></tr>\
         <tr><td colspan='7' style='border-top:2px solid #ffdd00;padding:6px 0'>robin@example.org · +44 20 7946 0000</td></tr>\
         <tr>{marks}</tr>\
         <tr><td colspan='7' style='padding:8px 0;font-size:11px'>{NOTICE}</td></tr>\
         </table>"
    )
}

/// What a layout covers: the smallest rectangle round everything it laid out.
fn extent(laid: &Laid) -> Rect {
    laid.items.iter().fold(Rect::NOTHING, |r, i| r.union(i.rect))
}

/// The seven small pictures, left to right and top to bottom.
fn marks(laid: &Laid) -> Vec<&Item> {
    shown_pictures(laid).into_iter().filter(|p| (p.rect.width() - 32.0).abs() < 0.5).collect()
}

/// The rectangles a table and its cells fill: backgrounds and lines, not what is in them.
fn fills(laid: &Laid) -> Vec<Rect> {
    laid.items
        .iter()
        .filter(|i| matches!(i.kind, ItemKind::Fill { .. } | ItemKind::Dashes { .. } | ItemKind::Outline { .. }))
        .map(|i| i.rect)
        .collect()
}

#[test]
fn a_row_of_small_pictures_under_a_long_full_width_paragraph_stays_a_row() {
    let m = message(&signature());
    for dark in [false, true] {
        in_frame(dark, |ctx| {
            let t = Tokens::get(ctx);
            // Down to 504 points the table is its own 504 wide; under that, the room. The seven
            // pictures want 7 × 32 points, so they stand in one row at every one of these widths.
            for width in [400.0_f32, 520.0, 600.0, 769.0, 770.0, 900.0] {
                let laid = lay_out(ctx, &m, width);
                check(&laid, &t, &format!("signature at {width}"));
                assert_eq!(laid.stats.stacked, 0, "at {width}: {:?}", laid.stats);
                let marks = marks(&laid);
                assert_eq!(marks.len(), 7, "at {width}: {marks:?}");
                assert!(marks.iter().all(|p| (p.rect.min.y - marks[0].rect.min.y).abs() < 0.5), "one row at {width}: {marks:?}");
                assert!(marks.windows(2).all(|w| w[1].rect.min.x >= w[0].rect.max.x), "left to right at {width}: {marks:?}");
                // Never wider than its own width, nor than the room; its rule with it.
                let bound = 504.0_f32.min(width);
                let whole = extent(&laid);
                assert!(whole.min.x >= -0.5 && whole.max.x <= bound + 0.5, "at {width}: {whole:?}");
                let rules: Vec<Rect> = fills(&laid).into_iter().filter(|r| (r.height() - 2.0).abs() < 0.5).collect();
                assert_eq!(rules.len(), 1, "at {width}: {rules:?}");
                assert!(rules.iter().all(|r| r.width() > 100.0 && r.max.x <= bound + 0.5), "at {width}: {rules:?}");
                // The notice wraps inside the table.
                let notice: Vec<&Item> = laid.items.iter().filter(|i| super::text_of(i).contains("field notes that")).collect();
                assert!(!notice.is_empty() && notice.iter().all(|i| i.rect.max.x <= bound + 0.5), "at {width}: {notice:?}");
            }
            // With less room than the pictures want, it is stacked: one under the other, as wide
            // as the room.
            let laid = lay_out(ctx, &m, 200.0);
            check(&laid, &t, "signature at 200");
            assert_eq!(laid.stats.stacked, 1, "{:?}", laid.stats);
            let marks = marks(&laid);
            assert_eq!(marks.len(), 7);
            assert!(marks.windows(2).all(|w| w[1].rect.min.y >= w[0].rect.max.y), "{marks:?}");
            assert!(extent(&laid).max.x <= 200.5, "{:?}", extent(&laid));
        });
    }
}

/// A 40-pixel square written into the message (a `data:` PNG, invented).
const SQUARE: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAoCAIAAAADnC86AAAAMElEQVR42u3NQQkAAAgEsGtiTIsZ0BKCn8H+S/W8iFgsFovFYrFYLBaLxWKxWHxnAfgPDvn3C4zDAAAAAElFTkSuQmCC";

/// The smallest message of the kind: a name, a rule, seven pictures written into the message,
/// each over its label, and a paragraph spanning the seven columns, in a table 504 pixels at most.
fn small_signature() -> String {
    let marks: String = ["One", "Two", "Three", "Four", "Five", "Six", "Seven"]
        .iter()
        .map(|label| {
            format!(
                "<td width='14%' align='center' valign='top'><img src='{SQUARE}' width='40' height='40' alt='{label}' \
                 style='display:block;margin:0 auto'><span style='display:block;padding-top:4px;font-size:9px'>{label}</span></td>"
            )
        })
        .collect();
    format!(
        "<table width='100%' cellpadding='0' cellspacing='0' border='0' role='presentation' \
         style='width:100%;max-width:504px;border-collapse:collapse'>\
         <tr><td colspan='7' style='padding:12px 0;font-size:18px;font-weight:bold'>Example Studio</td></tr>\
         <tr><td colspan='7' style='border-top:2px solid #d4a017;font-size:0;line-height:2px;padding-top:8px'>&nbsp;</td></tr>\
         <tr>{marks}</tr>\
         <tr><td colspan='7' style='padding-top:8px;font-size:9px;line-height:13px'>{NOTICE}</td></tr></table>"
    )
}

#[test]
fn seven_written_pictures_over_a_spanning_paragraph_stay_a_row() {
    let m = message(&small_signature());
    in_frame(false, |ctx| {
        let t = Tokens::get(ctx);
        for width in [520.0_f32, 684.0, 769.0, 770.0, 900.0] {
            let laid = lay_out(ctx, &m, width);
            check(&laid, &t, &format!("small signature at {width}"));
            assert_eq!(laid.stats.stacked, 0, "at {width}: {:?}", laid.stats);
            let marks: Vec<&Item> = shown_pictures(&laid).into_iter().filter(|p| (p.rect.width() - 40.0).abs() < 0.5).collect();
            assert_eq!(marks.len(), 7, "at {width}: {marks:?}");
            assert!(marks.iter().all(|p| (p.rect.min.y - marks[0].rect.min.y).abs() < 0.5), "one row at {width}: {marks:?}");
            let bound = 504.0_f32.min(width);
            assert!(extent(&laid).max.x <= bound + 0.5, "at {width}: {:?}", extent(&laid));
        }
        // Narrower than what its seven cells can't do without (448 points in all): stacked, as
        // wide as the room, which is less than its own 504.
        let laid = lay_out(ctx, &m, 400.0);
        check(&laid, &t, "small signature at 400");
        assert_eq!(laid.stats.stacked, 1, "{:?}", laid.stats);
        let marks: Vec<&Item> = shown_pictures(&laid).into_iter().filter(|p| (p.rect.width() - 40.0).abs() < 0.5).collect();
        assert!(marks.len() == 7 && marks.windows(2).all(|w| w[1].rect.min.y >= w[0].rect.max.y), "{marks:?}");
        assert!(extent(&laid).max.x <= 400.5, "{:?}", extent(&laid));
    });
}

/// Eight columns of words that don't fit side by side in 800 points, each cell coloured, one
/// with a rule over it: a layout table that is stacked there however it is judged.
fn ledger(table: &str) -> String {
    let cells: String = (0..8)
        .map(|i| {
            let rule = if i == 3 { "border-top:2px solid #336699;" } else { "" };
            format!("<td style='{rule}background:#e8eef6'>Quarterly figures {i}</td>")
        })
        .collect();
    format!("{table}<tr>{cells}</tr></table>")
}

#[test]
fn a_stacked_table_keeps_its_width_and_its_place() {
    // (the table's opening tag, its expected left edge and width in a pane of 800 points)
    let cases = [
        ("<table role='presentation' width='300'>", 0.0, 300.0),
        ("<table role='presentation' width='300' style='margin:0 auto'>", 250.0, 300.0),
        ("<table role='presentation' width='300' align='center'>", 250.0, 300.0),
        ("<table role='presentation' width='300' align='right'>", 500.0, 300.0),
        ("<table role='presentation' style='width:100%;max-width:300px'>", 0.0, 300.0),
        ("<table role='presentation' width='50%'>", 0.0, 400.0),
        // No width of its own: the whole room, as before.
        ("<table role='presentation'>", 0.0, 800.0),
        // Wider than the room: the room.
        ("<table role='presentation' width='1200'>", 0.0, 800.0),
    ];
    for dark in [false, true] {
        in_frame(dark, |ctx| {
            let t = Tokens::get(ctx);
            for (open, left, width) in cases {
                let m = message(&ledger(open));
                let laid = lay_out(ctx, &m, 800.0);
                check(&laid, &t, open);
                assert_eq!(laid.stats.stacked, 1, "{open}: {:?}", laid.stats);
                let cells: Vec<Rect> = fills(&laid).into_iter().filter(|r| r.height() > 5.0).collect();
                assert_eq!(cells.len(), 8, "{open}: {cells:?}");
                for cell in &cells {
                    assert!((cell.min.x - left).abs() < 0.5 && (cell.width() - width).abs() < 0.5, "{open}: {cell:?}");
                }
                // One under the other.
                assert!(cells.windows(2).all(|w| w[1].min.y >= w[0].max.y - 0.5), "{open}: {cells:?}");
                // The rule is the table's width too.
                let rule: Vec<Rect> = fills(&laid).into_iter().filter(|r| (r.height() - 2.0).abs() < 0.5).collect();
                assert!(!rule.is_empty() && rule.iter().all(|r| r.min.x >= left - 0.5 && r.max.x <= left + width + 0.5), "{open}: {rule:?}");
                // And what is in the cells stays in them.
                let whole = extent(&laid);
                assert!(whole.min.x >= left - 0.5 && whole.max.x <= left + width + 0.5, "{open}: {whole:?}");
            }
        });
    }
}

#[test]
fn stacked_tables_floated_to_each_side_keep_their_widths_side_by_side() {
    // A width in pixels, and a share of the line: the box a stacked table takes on the line is
    // the width it is drawn at, so the two stand side by side.
    for (asked, width) in [("250", 250.0_f32), ("30%", 240.0)] {
        let one = |align: &str, word: &str| {
            let cells: String = (0..8).map(|i| format!("<td style='background:#e8eef6'>{word} figures {i}</td>")).collect();
            format!("<table role='presentation' align='{align}' width='{asked}'><tr>{cells}</tr></table>")
        };
        let m = message(&format!("<div>{}{}</div>", one("left", "Quarterly"), one("right", "Harbour")));
        in_frame(false, |ctx| {
            let laid = lay_out(ctx, &m, 800.0);
            check(&laid, &Tokens::get(ctx), "floated stacks");
            assert_eq!(laid.stats.stacked, 2, "{asked}: {:?}", laid.stats);
            let cells: Vec<Rect> = fills(&laid).into_iter().filter(|r| r.height() > 5.0).collect();
            let (left, right): (Vec<Rect>, Vec<Rect>) = cells.iter().partition(|r| r.center().x < 400.0);
            assert_eq!((left.len(), right.len()), (8, 8), "{asked}: {cells:?}");
            // Each is its own width, at its own side, and the two stand side by side.
            assert!(left.iter().all(|r| r.min.x.abs() < 0.5 && (r.width() - width).abs() < 0.5), "{asked}: {left:?}");
            assert!(right.iter().all(|r| (r.max.x - 800.0).abs() < 0.5 && (r.width() - width).abs() < 0.5), "{asked}: {right:?}");
            assert!((left[0].min.y - right[0].min.y).abs() < 0.5, "{asked}: {:?} {:?}", left[0], right[0]);
        });
    }
}

const WORDS: [&str; 12] =
    ["tide", "harbour", "lantern", "Quarterly", "figures", "north", "aftermath", "a", "ledger", "wrought-iron", "1234567890", "rope"];

fn words(dice: &mut Dice, n: usize) -> String {
    (0..n).map(|_| WORDS[dice.below(WORDS.len())]).collect::<Vec<_>>().join(" ")
}

/// What a generated cell holds; tables inside only when `nest` allows.
fn content(dice: &mut Dice, depth: usize, nest: bool) -> String {
    match dice.below(if nest && depth < 3 { 7 } else { 6 }) {
        0 => String::new(),
        1 => {
            let n = 1 + dice.below(3);
            words(dice, n)
        }
        2 => {
            let n = 20 + dice.below(90);
            words(dice, n)
        }
        3 => format!("<img src='cid:step-done@parcel' width='{}' height='32' alt='mark'>", [16, 32, 48, 120][dice.below(4)]),
        4 => "<img src='cid:hero@listing' width='504' height='120' alt='banner'>".into(),
        5 => format!("<img src='cid:step-done@parcel' width='32' height='32' alt='m'><br><span style='font-size:10px'>{}</span>", words(dice, 1)),
        _ => table(dice, depth + 1, nest),
    }
}

/// A generated table: 0 to 8 columns, rows the whole width, spans past the end, widths in pixels
/// and percent that are nothing, huge or ordinary, colours, rules and alignment.
fn table(dice: &mut Dice, depth: usize, nest: bool) -> String {
    let columns = dice.below(9);
    let width = match dice.below(8) {
        0 | 1 => String::new(),
        2 => format!(" width='{}'", [0, 1, 100, 300, 504, 900, 65535, 99999][dice.below(8)]),
        3 => format!(" width='{}%'", [0, 1, 14, 50, 100, 250][dice.below(6)]),
        4 => " style='width:100%;max-width:504px'".into(),
        5 => " width='300' align='center'".into(),
        6 => " width='320' style='margin:0 auto'".into(),
        _ => " width='300' align='right'".into(),
    };
    let spacing = [String::new(), " cellspacing='0'".into(), format!(" cellspacing='{}'", dice.below(40))][dice.below(3)].clone();
    let mut rows = String::new();
    for _ in 0..1 + dice.below(4) {
        rows.push_str("<tr>");
        if dice.below(3) == 0 {
            let span = [columns, columns + 5, 0, 1, 400][dice.below(5)];
            let rule = if dice.below(2) == 0 { "border-top:2px solid #c08000;" } else { "" };
            rows.push_str(&format!("<td colspan='{span}' style='{rule}'>{}</td>", content(dice, depth, nest)));
        } else {
            for _ in 0..columns {
                let w = [String::new(), String::new(), " width='14%'".into(), " width='72'".into(), format!(" width='{}'", dice.below(2000))]
                    [dice.below(5)]
                .clone();
                let span = if dice.below(6) == 0 { format!(" colspan='{}'", dice.below(4)) } else { String::new() };
                let rowspan = if dice.below(10) == 0 { " rowspan='9999'" } else { "" };
                let bg = if dice.below(2) == 0 { " style='background:#eef3fb'" } else { "" };
                rows.push_str(&format!("<td{w}{span}{rowspan}{bg}>{}</td>", content(dice, depth, nest)));
            }
        }
        rows.push_str("</tr>");
    }
    let role = if dice.below(5) == 0 { "" } else { " role='presentation'" };
    format!("<table{role}{spacing}{width}>{rows}</table>")
}

/// The width a stacked layout table may take in `room` points: its own, never more than the room.
fn stacked_bound(width: Option<Length>, room: f32) -> f32 {
    match width {
        Some(Length::Px(w)) => f32::from(w).min(room),
        Some(Length::Percent(p)) => room * f32::from(p.min(100)) / 100.0,
        None => room,
    }
}

#[test]
fn generated_and_hostile_tables_stay_inside_their_bounds() {
    let mut dice = Dice(0x7ab1_e5ee_d0c0_ffee);
    in_frame(false, |ctx| {
        let t = Tokens::get(ctx);
        let (mut stacked, mut rows) = (0usize, 0usize);
        // One table with nothing inside it: whether it is stacked is the table's own.
        for case in 0..160 {
            let html = table(&mut dice, 0, false);
            let m = message(&html);
            let Some(own) = Doc::of(&m).blocks.iter().find_map(|b| match &b.kind {
                BlockKind::Table(t) if t.layout && t.columns > 1 && !t.inline && !t.float => Some(t.width),
                _ => None,
            }) else {
                continue;
            };
            for width in [1.0_f32, 40.0, 150.0, 333.0, 504.0, 800.0, 1600.0] {
                let laid = lay_out(ctx, &m, width);
                check(&laid, &t, &format!("case {case} at {width}: {html}"));
                let painted = fills(&laid).into_iter().fold(Rect::NOTHING, |r, f| r.union(f));
                if !painted.is_positive() {
                    continue;
                }
                let room = width.max(40.0);
                if laid.stats.stacked > 0 {
                    stacked += 1;
                    let bound = stacked_bound(own, room).max(24.0_f32.min(room));
                    assert!(painted.width() <= bound + 0.5, "case {case} at {width}: {painted:?} > {bound}: {html}");
                } else {
                    rows += 1;
                    assert!(painted.max.x <= room + 0.5, "case {case} at {width}: {painted:?}: {html}");
                }
            }
        }
        assert!(stacked >= 40 && rows >= 40, "stacked {stacked}, rows {rows}");
        // Nested, hostile and empty tables: laid out to the end, every time, at every width.
        let mut hostile: Vec<String> = (0..80).map(|_| table(&mut dice, 0, true)).collect();
        hostile.extend(
            [
                "<table></table>",
                "<table role='presentation'><tr></tr></table>",
                "<table role='presentation'><tr><td colspan='0'></td><td colspan='65535'>x</td></tr></table>",
                "<table role='presentation' width='0'><tr><td width='0'>a</td><td width='99999'>b</td></tr></table>",
                "<table role='presentation' width='0%'><tr><td colspan='7'>wide</td></tr><tr><td>1</td><td>2</td></tr></table>",
            ]
            .map(String::from),
        );
        let deep = (0..40).fold(String::from("core"), |inner, _| {
            format!("<table role='presentation'><tr><td>{inner}</td><td colspan='9'>{NOTICE}</td></tr></table>")
        });
        hostile.push(deep);
        for (n, html) in hostile.iter().enumerate() {
            let m = message(html);
            for width in [1.0_f32, 40.0, 504.0, 4000.0] {
                let laid = lay_out(ctx, &m, width);
                check(&laid, &t, &format!("hostile {n} at {width}: {html}"));
            }
        }
    });
}
