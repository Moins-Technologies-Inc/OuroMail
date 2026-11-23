// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Add Shared Mailbox's "Opened before" (Q4): the shared mailboxes opened under the account and
//! removed since ([`mailcraft_model::Account::shared_before`], newest first, at most 20), each with
//! Add again and Forget, and Forget all. Add again goes the way Add goes for a typed address (the
//! same command, the same check of the mailbox's Inbox before it is kept), with the name it had.
//! Forget and Forget all are the owner's own clicks: an agent or a script forgets with
//! `account.forgetSharedBefore`, which asks him. The list is kept in the mailbox file only and
//! never sent anywhere; nothing of it is logged, and `ui.inspect` has its rows as hit regions by
//! number, the rows in view only.

use egui::{Align, Layout, Rect, RichText, Sense, Ui, UiBuilder, pos2, vec2};
use mailcraft_model::Id;
use serde_json::json;

use crate::MailApp;
use crate::theme::Tokens as Theme;

/// The height of one row.
const ROW: f32 = 38.0;
/// How many rows show before the list scrolls.
const ROWS_SHOWN: f32 = 3.0;

/// The section for `account`, when anything was opened before under it: the address to add
/// again, when the owner said so. `enabled` is false while an address is being checked.
pub(crate) fn section(app: &mut MailApp, ui: &mut Ui, t: &Theme, account: Id, enabled: bool) -> Option<String> {
    let entries = app.session.mailbox.account(account).map(|a| a.shared_before.clone()).unwrap_or_default();
    if entries.is_empty() {
        return None;
    }
    let mut again = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new(tl!("Opened before")).strong());
        if entries.len() > 1 {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let all = ui.add_enabled(enabled, egui::Link::new(tl!("Forget all")));
                app.hit.add("sharedBeforeForgetAll", 0, all.rect);
                // The owner's own click: an agent or a script forgets through the command.
                if all.clicked() && !app.driven {
                    forget(app, account, None);
                }
            });
        }
    });
    egui::Frame::new().fill(t.input).stroke(egui::Stroke::new(1.0, t.input_border)).corner_radius(4.0).inner_margin(egui::Margin::same(2)).show(
        ui,
        |ui| {
            // A bar beside the list whenever it is longer than the box, not only under the pointer.
            let style = ui.style_mut();
            style.spacing.scroll = egui::style::ScrollStyle {
                bar_width: 8.0,
                dormant_handle_opacity: 1.0,
                active_handle_opacity: 1.0,
                interact_handle_opacity: 1.0,
                ..egui::style::ScrollStyle::solid()
            };
            style.visuals.widgets.inactive.bg_fill = crate::widgets::scroll_ink(t);
            egui::ScrollArea::vertical()
                .id_salt("sharedBefore")
                .max_height(ROWS_SHOWN * ROW)
                .auto_shrink([false, true])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (i, entry) in entries.iter().enumerate() {
                        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
                        // The buttons at the right, the address (over its name) in what is left.
                        let mut right =
                            ui.new_child(UiBuilder::new().max_rect(rect.shrink2(vec2(6.0, 4.0))).layout(Layout::right_to_left(Align::Center)));
                        let forget_it = right.add_enabled(enabled, egui::Button::new(tl!("Forget")).small());
                        let add = right.add_enabled(enabled, egui::Button::new(tl!("Add again")).small());
                        let used = right.min_rect().width();
                        let text = Rect::from_min_max(
                            pos2(rect.min.x + 8.0, rect.min.y + 3.0),
                            pos2((rect.max.x - used - 14.0).max(rect.min.x + 8.0), rect.max.y - 3.0),
                        );
                        let mut left = ui.new_child(UiBuilder::new().max_rect(text).layout(Layout::top_down(Align::Min)));
                        left.spacing_mut().item_spacing.y = 1.0;
                        left.add(egui::Label::new(RichText::new(entry.address.as_str()).color(t.text)).truncate().selectable(false));
                        if !entry.name.is_empty() {
                            left.add(egui::Label::new(RichText::new(entry.name.as_str()).color(t.text_dim).size(11.5)).truncate().selectable(false));
                        }
                        // Targets only for a row in view: one scrolled away can't be clicked.
                        let clip = ui.clip_rect();
                        if rect.intersect(clip).height() >= ROW / 2.0 {
                            let n = u64::try_from(i).unwrap_or(u64::MAX);
                            app.hit.add("sharedBeforeAdd", n, add.rect.intersect(clip));
                            app.hit.add("sharedBeforeForget", n, forget_it.rect.intersect(clip));
                        }
                        if add.clicked() {
                            again = Some(entry.address.clone());
                        } else if forget_it.clicked() && !app.driven {
                            forget(app, account, Some(&entry.address));
                        }
                    }
                });
        },
    );
    again
}

/// Forget one opened before under `account` (or every one), as the owner said.
fn forget(app: &mut MailApp, account: Id, address: Option<&str>) {
    let mut params = json!({"account": account});
    if let Some(address) = address {
        params["address"] = json!(address);
    }
    if let Err(e) = app.run("account.forgetSharedBefore", params) {
        app.status(e);
    }
}

#[cfg(test)]
#[path = "shared_before_tests.rs"]
mod tests;
