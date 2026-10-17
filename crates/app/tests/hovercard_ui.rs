//! The hover card's height (R§8.1, R§8.2), measured through Slint's headless software platform
//! with the demo data: no OS window, no Win32, no network.

use std::rc::Rc;

use lastdrop_app::demo::{Demo, Scenario};
use lastdrop_app::{ui, wiring};
use slint::ComponentHandle;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};

struct Headless;

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

#[test]
fn content_height_counts_every_line_of_the_sentence() {
    slint::platform::set_platform(Box::new(Headless)).expect("platform");
    let demo = Demo::scenario(false, Scenario::Normal);
    let card = ui::HoverCard::new().expect("hover card");
    wiring::set_theme(&card.global::<ui::Theme>(), &demo.theme);
    wiring::set_hover_card(&card, &demo.hover_card(), 1.0);
    card.show().expect("show");

    // Padding 12, header 20, the sentence 4 + 3 lines × 16, the Claude section 12 + 1 + 10 + 16,
    // two meter blocks × 71, "Also tracked" 12 + 1 + 10 + 16 + 3 lines × 20, padding 12.
    assert_eq!(card.get_content_height(), 376.0);
    // A one-line sentence is two lines shorter.
    card.set_sentence("All limits last until their reset.".into());
    assert_eq!(card.get_content_height(), 344.0);
}
