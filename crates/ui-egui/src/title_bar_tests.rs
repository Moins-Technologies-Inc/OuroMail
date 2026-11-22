// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The title row is compact (#1158): the search box, the account's picture and the buttons fit a
//! row of 36 points, and the tabs and the ribbon start right under it, so the message list and
//! the reading pane keep the height.

use serde_json::json;

use crate::people_view::testing;

#[test]
fn the_title_row_is_compact_and_the_tabs_follow_it() {
    for theme in ["light", "dark"] {
        let mut h = testing::window("view.mail");
        let _ = h.state_mut().execute("ui.dismissReminder", json!({"all": true}));
        h.state_mut().execute("ui.theme", json!({"value": theme})).unwrap();
        h.run_steps(4);
        if let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") {
            h.render().unwrap().save(std::path::Path::new(&dir).join(format!("title-row-{theme}.png"))).unwrap();
        }
        assert!(h.state().search_box_shown, "the search box is in the row");
        // The numbers are the ones the row is meant to have (36 points, a box of 28), not the
        // constants, so that a row made taller again fails here.
        let search = h.state().hit.rect("searchBox", 0).expect("the search box");
        assert!((search.height() - 28.0).abs() < 0.5, "{theme}: the box is 28 points high, {search:?}");
        let row_bottom = search.center().y + 18.0;
        // Under the row come the tabs (30 points) and then the ribbon's card, 13 points further
        // down: 43 in all. A taller row (48 points, as before) would put New Email 49 down.
        let new_mail = h.state().hit.rect("ribbon", crate::ribbon::hit_id("mail.new")).expect("New Email in the ribbon");
        let below = new_mail.min.y - row_bottom;
        assert!((40.0..46.0).contains(&below), "{theme}: New Email {below} points under the row");
    }
}

fn mean_opaque_luminance(png: &[u8]) -> f64 {
    let rgba = image::load_from_memory_with_format(png, image::ImageFormat::Png).unwrap().into_rgba8();
    let (sum, count) = rgba.pixels().fold((0.0f64, 0u64), |(sum, count), p| {
        if p[3] >= 128 {
            let lum = 0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]);
            (sum + lum, count + 1)
        } else {
            (sum, count)
        }
    });
    assert!(count > 0, "icon must have opaque pixels");
    sum / count as f64
}

#[test]
fn title_icon_switches_with_theme_and_uses_light_artwork_in_dark_mode() {
    let ctx = egui::Context::default();
    let light_mode = super::title_icon(&ctx, false).expect("light-mode title icon is built in");
    let dark_mode = super::title_icon(&ctx, true).expect("dark-mode title icon is built in");
    assert_eq!(light_mode.size(), [64, 64]);
    assert_eq!(dark_mode.size(), [64, 64]);
    assert_ne!(light_mode.id(), dark_mode.id(), "dark and light modes must cache distinct title icons");
    assert_eq!(super::title_icon(&ctx, false).unwrap().id(), light_mode.id());
    assert_eq!(super::title_icon(&ctx, true).unwrap().id(), dark_mode.id());
    let lum_dark_mode = mean_opaque_luminance(super::title_icon_png(true));
    let lum_light_mode = mean_opaque_luminance(super::title_icon_png(false));
    assert!(
        lum_dark_mode > lum_light_mode,
        "dark mode must use the lighter title icon (dark_mode={lum_dark_mode:.2} vs light_mode={lum_light_mode:.2})"
    );
}
