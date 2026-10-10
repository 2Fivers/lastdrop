//! The flyout's keyboard and layout behaviour (F§1.2, F§9), driven through Slint's headless
//! software platform with the demo data: no OS window, no Win32, no network.
//!
//! One test function: Slint's platform is set once per process and its components are not
//! `Send`, so everything runs on one thread here.

use std::cell::RefCell;
use std::rc::Rc;

use lastdrop_app::demo::{Demo, Scenario};
use lastdrop_app::{ui, wiring};
use lastdrop_core::grammar::English;
use slint::ComponentHandle;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, WindowAdapter, WindowEvent};

struct Headless;

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

/// Presses and releases one key.
fn press(window: &slint::Window, text: impl Into<slint::SharedString> + Clone) {
    let text: slint::SharedString = text.into();
    window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    window.dispatch_event(WindowEvent::KeyReleased { text });
}

/// A flyout showing `demo`'s model, sized to its content.
fn flyout_for(demo: &Demo) -> ui::Flyout {
    let flyout = ui::Flyout::new().expect("flyout");
    wiring::set_theme(&flyout.global::<ui::Theme>(), &demo.theme);
    wiring::set_flyout_labels(&flyout, &English);
    let model = demo.flyout();
    let menus: Vec<wiring::CardMenu> = model
        .cards
        .iter()
        .map(|c| wiring::card_menu(c, &English, &[], true))
        .collect();
    wiring::set_flyout(&flyout, &model, 1.0, &menus);
    flyout.show().expect("show");
    flyout
        .window()
        .set_size(slint::LogicalSize::new(360.0, 560.0));
    flyout
        .window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    flyout
}

#[test]
fn flyout_heights_and_keyboard() {
    slint::platform::set_platform(Box::new(Headless)).expect("platform");

    // F§1.2: 478 for the reference content; a trouble line adds 20; a two-line message adds 40.
    for (scenario, height) in [
        (Scenario::Normal, 478.0),
        (Scenario::Warning, 498.0),
        (Scenario::Stale, 502.0),
        (Scenario::SignIn, 518.0),
    ] {
        let flyout = flyout_for(&Demo::scenario(false, scenario));
        assert_eq!(flyout.get_content_height(), height, "{scenario:?}");
        flyout.hide().expect("hide");
    }

    let flyout = flyout_for(&Demo::new(false));
    let log = Rc::new(RefCell::new(Vec::<String>::new()));
    let record = |what: &'static str| {
        let log = log.clone();
        move || log.borrow_mut().push(what.to_owned())
    };
    flyout.on_dismiss(record("dismiss"));
    flyout.on_refresh(record("refresh"));
    flyout.on_open_settings(record("settings"));
    {
        let log = log.clone();
        flyout.on_open_service_dashboard(move |i| log.borrow_mut().push(format!("usage {i}")));
    }
    let window = flyout.window();

    // Esc hides, F5 refreshes (F§9), with the window itself focused (opened by mouse).
    flyout.invoke_reset_focus();
    press(window, Key::Escape);
    press(window, Key::F5);
    assert_eq!(log.borrow().as_slice(), ["dismiss", "refresh"]);
    log.borrow_mut().clear();

    // Opened by keyboard: the first card has focus; Down moves to the next; Enter opens the
    // service's usage page.
    flyout.invoke_focus_card_at(0, true);
    press(window, Key::Return);
    press(window, Key::DownArrow);
    press(window, Key::Return);
    press(window, Key::DownArrow);
    press(window, Key::DownArrow);
    press(window, Key::Return);
    press(window, Key::UpArrow);
    press(window, Key::Return);
    assert_eq!(
        log.borrow().as_slice(),
        ["usage 0", "usage 1", "usage 2", "usage 1"]
    );
    log.borrow_mut().clear();

    // Esc still hides from a focused card (the key bubbles up to the window's handler).
    press(window, Key::Escape);
    assert_eq!(log.borrow().as_slice(), ["dismiss"]);
    log.borrow_mut().clear();

    // Tab order (F§9): Refresh → card 1 → … → card n → Settings.
    flyout.invoke_reset_focus();
    press(window, Key::Tab);
    press(window, Key::Return);
    for _ in 0..3 {
        press(window, Key::Tab);
        press(window, Key::Return);
    }
    press(window, Key::Tab);
    press(window, Key::Return);
    assert_eq!(
        log.borrow().as_slice(),
        ["refresh", "usage 0", "usage 1", "usage 2", "settings"]
    );
    assert!(
        flyout.global::<ui::Theme>().get_keyboard_focus_visible(),
        "focus rectangles show after keyboard use"
    );
}
