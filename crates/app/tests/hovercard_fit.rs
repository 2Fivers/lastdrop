//! The hover card's fit steps (R§8.1): what the card drops to fit the space above the taskbar
//! leaves its measured height, measured through Slint's headless software platform with the demo
//! data: no OS window, no Win32, no network.
//!
//! One test function: Slint's platform is set once per process and its components are not
//! `Send`, so everything runs on one thread here.

use std::rc::Rc;

use lastdrop_app::demo::{Demo, Scenario};
use lastdrop_app::{ui, wiring};
use lastdrop_core::readout::hovercard::HoverCardModel;
use slint::ComponentHandle;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};

struct Headless;

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

/// A hover card showing `model`. Never rendered: the app shows the card with Win32, behind
/// Slint's back, and measures it before Slint draws it.
fn card_for(demo: &Demo, model: &HoverCardModel) -> ui::HoverCard {
    let card = ui::HoverCard::new().expect("hover card");
    wiring::set_theme(&card.global::<ui::Theme>(), &demo.theme);
    wiring::set_hover_card(&card, model, 1.0);
    card.show().expect("show");
    card
}

#[test]
fn fit_steps_shrink_the_measured_height() {
    slint::platform::set_platform(Box::new(Headless)).expect("platform");
    let demo = Demo::scenario(false, Scenario::Normal);
    let model = demo.hover_card();
    let lines = model.also.as_ref().expect("also tracked").lines.len();
    assert!(lines > 0 && !model.sentence.is_empty());

    // What the card measures for a model without "Also tracked", and without the sentence too.
    let without_also = HoverCardModel {
        also: None,
        ..model.clone()
    };
    let bare = HoverCardModel {
        sentence: String::new(),
        ..without_also.clone()
    };
    let without_also = wiring::hover_card_content_height(&card_for(&demo, &without_also));
    let bare = wiring::hover_card_content_height(&card_for(&demo, &bare));

    // The fit loop's steps on one card, measured after each as the loop does: every dropped line
    // takes its 20 px; the last one takes the whole block with it (margin, divider, header);
    // then the sentence goes.
    let card = card_for(&demo, &model);
    let full = wiring::hover_card_content_height(&card);
    let mut height = full;
    for visible in (1..lines).rev() {
        card.set_also_visible(i32::try_from(visible).expect("lines"));
        let now = wiring::hover_card_content_height(&card);
        assert_eq!(now, height - 20.0, "{visible} lines");
        height = now;
    }
    card.set_also_visible(0);
    assert_eq!(wiring::hover_card_content_height(&card), without_also);
    card.set_show_sentence(false);
    assert_eq!(wiring::hover_card_content_height(&card), bare);

    // Shown again with everything (hide_hover_card, set_hover_card), it measures in full again.
    card.set_show_sentence(true);
    card.set_also_visible(i32::try_from(lines).expect("lines"));
    assert_eq!(wiring::hover_card_content_height(&card), full);
}
