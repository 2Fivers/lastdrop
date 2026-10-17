//! The settings window's pages, instant apply and deep links (settings-spec §5, §7), driven
//! through Slint's headless software platform with the demo's settings and invented accounts: no
//! OS window, no files, no registry, no Credential Manager — commits go to an in-memory
//! `settings.json` (`MemoryOutside`), and the platform's clock is the test's, so the 250 ms and
//! 800 ms timers fire when the test says.
//!
//! One test function: Slint's platform is set once per process and its components are not
//! `Send`, so everything runs on one thread here.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lastdrop_app::demo::Demo;
use lastdrop_app::settings_edits::MemoryOutside;
use lastdrop_app::settings_window::{self as sw, Focus};
use lastdrop_app::{preview, ui};
use lastdrop_core::model::ProviderId;
use lastdrop_core::settings::{ReadoutContent, RefreshInterval};
use slint::ComponentHandle;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, WindowAdapter, WindowEvent};

/// The platform's clock, ms since the test started.
static NOW_MS: AtomicU64 = AtomicU64::new(0);

struct Headless;

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }

    fn duration_since_start(&self) -> Duration {
        Duration::from_millis(NOW_MS.load(Ordering::SeqCst))
    }
}

/// Lays the window out and draws it (repeaters are instantiated, change handlers run).
fn frame(window: &ui::SettingsWindow) {
    slint::platform::update_timers_and_animations();
    let _ = window.window().take_snapshot();
    slint::platform::update_timers_and_animations();
}

/// Moves the clock on by `ms` and runs what is due.
fn wait(window: &ui::SettingsWindow, ms: u64) {
    NOW_MS.fetch_add(ms, Ordering::SeqCst);
    frame(window);
}

/// Presses and releases one key.
fn press(window: &ui::SettingsWindow, text: impl Into<slint::SharedString> + Clone) {
    let text: slint::SharedString = text.into();
    window
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    window
        .window()
        .dispatch_event(WindowEvent::KeyReleased { text });
    frame(window);
}

/// Presses `key` with Ctrl held.
fn ctrl(window: &ui::SettingsWindow, key: Key) {
    let w = window.window();
    w.dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    press(window, key);
    w.dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
}

/// The settings window for the English demo, committing to `outside`, at its preferred size.
fn open(outside: &Rc<RefCell<MemoryOutside>>) -> ui::SettingsWindow {
    let inputs = preview::demo_settings_inputs(&Demo::new(false), false);
    outside.borrow_mut().disk = inputs.settings.clone();
    let window = sw::start(inputs, Box::new(outside.clone()), false).expect("window");
    window.show().expect("show");
    window
        .window()
        .set_size(slint::LogicalSize::new(880.0, 680.0));
    window
        .window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    frame(&window);
    window
}

#[test]
fn settings_pages_instant_apply_and_deep_links() {
    slint::platform::set_platform(Box::new(Headless)).expect("platform");
    let outside = Rc::new(RefCell::new(MemoryOutside::default()));
    let window = open(&outside);
    assert_eq!(window.get_page(), sw::PAGE_SERVICES, "Services first");

    // ---- The navigation pane: one Tab stop; the arrow keys, Home and End move the selection.
    press(&window, Key::Tab);
    press(&window, Key::DownArrow);
    assert_eq!(window.get_page(), sw::PAGE_DISPLAY);
    press(&window, Key::End);
    assert_eq!(window.get_page(), sw::PAGE_GENERAL);
    press(&window, Key::Home);
    assert_eq!(window.get_page(), sw::PAGE_SERVICES);
    ctrl(&window, Key::Tab);
    assert_eq!(window.get_page(), sw::PAGE_DISPLAY, "Ctrl+Tab cycles");
    press(&window, Key::End);
    assert_eq!(window.get_page(), sw::PAGE_GENERAL);

    // Tab goes from the pane into the page shown, not into a hidden one: the first control of
    // General is "Start Lastdrop when I sign in"; Space turns it on (Services' first control
    // would turn a service off).
    press(&window, Key::Tab);
    press(&window, " ");
    assert!(window.get_start_with_windows());
    assert_eq!(outside.borrow().writes, 0, "not before the 250 ms");

    // ---- Reconcile: a change the tray wrote meanwhile shows after the next commit, also in a
    // combo box that has the keyboard focus but no edit (the language box, then the interval).
    press(&window, Key::Tab);
    press(&window, Key::Tab);
    outside.borrow_mut().disk.refresh.interval = RefreshInterval::Minutes(30);
    window.set_updates_check(false);
    window.invoke_updates_toggled(false);
    wait(&window, 100);
    assert_eq!(
        outside.borrow().writes,
        0,
        "a burst commits 250 ms after its last edit"
    );
    wait(&window, 200);
    {
        let out = outside.borrow();
        assert_eq!(out.writes, 1, "one commit for the burst");
        assert_eq!(out.autostart, [true], "the registry before the file");
        assert!(out.disk.start_with_windows && !out.disk.updates.check);
        assert_eq!(out.disk.refresh.interval, RefreshInterval::Minutes(30));
    }
    assert_eq!(window.get_interval_index(), 6, "30 min, from the file");
    // It kept the focus: Up picks 15 min.
    press(&window, Key::UpArrow);
    assert_eq!(window.get_interval_index(), 5);

    // ---- Edits on several pages commit together; the pages keep their values.
    ctrl(&window, Key::Tab);
    assert_eq!(window.get_page(), sw::PAGE_SERVICES, "Ctrl+Tab wraps");
    window.invoke_select_page(sw::PAGE_NOTIFICATIONS);
    window.set_notify_enabled(false);
    window.invoke_notify_enabled_toggled(false);
    wait(&window, 300);
    {
        let out = outside.borrow();
        assert_eq!(out.writes, 2);
        assert_eq!(out.disk.refresh.interval, RefreshInterval::Minutes(15));
        assert!(!out.disk.notifications.enabled);
    }
    window.invoke_select_page(sw::PAGE_GENERAL);
    frame(&window);
    assert_eq!(window.get_interval_index(), 5);
    assert!(window.get_start_with_windows());
    assert!(!window.get_notify_enabled());

    // ---- An alias being typed is not committed by an account action, and commits after its
    // pause.
    window.invoke_select_page(sw::PAGE_SERVICES);
    window.invoke_block_toggled(0, true);
    frame(&window);
    window.invoke_account_alias_edited(0, 0, "Home".into());
    wait(&window, 300);
    let claude_aliases = |out: &MemoryOutside| -> Vec<Option<String>> {
        out.disk
            .provider(ProviderId::Claude)
            .map(|p| p.accounts.iter().map(|a| a.alias.clone()).collect())
            .unwrap_or_default()
    };
    assert_eq!(claude_aliases(&outside.borrow())[0], None, "still typing");
    wait(&window, 600);
    assert_eq!(
        claude_aliases(&outside.borrow())[0].as_deref(),
        None,
        "pending after its pause"
    );
    wait(&window, 300);
    assert_eq!(
        claude_aliases(&outside.borrow())[0].as_deref(),
        Some("Home")
    );

    // ---- Deep link to the accounts: Services, the block with found folders expanded, its first
    // found folder's Add focused; Space adds it, committed at once.
    window.invoke_block_toggled(0, false);
    window.invoke_select_page(sw::PAGE_GENERAL);
    sw::reveal(Focus::Accounts);
    frame(&window);
    assert_eq!(window.get_page(), sw::PAGE_SERVICES);
    let writes = outside.borrow().writes;
    press(&window, " ");
    assert_eq!(outside.borrow().writes, writes + 1, "added at once");
    assert_eq!(claude_aliases(&outside.borrow()).len(), 3);
    let status: Vec<slint::SharedString> = slint::Model::iter(&window.get_block_status()).collect();
    assert_eq!(status[0], "Account added.");

    // Removing it commits at once too.
    window.invoke_account_removed(0, 2);
    frame(&window);
    assert_eq!(claude_aliases(&outside.borrow()).len(), 2);

    // ---- Deep link to the pins: Display, the readout on and "Meters I pin" (choosing: no pin
    // yet, still automatic), outlined, the first Pin box focused; Space pins it.
    window.invoke_select_page(sw::PAGE_GENERAL);
    sw::reveal(Focus::Pins);
    frame(&window);
    assert_eq!(window.get_page(), sw::PAGE_DISPLAY);
    assert!(window.get_highlight_pins());
    assert_eq!(window.get_readout_content_index(), 1);
    assert!(window.get_choosing_pins());
    assert_eq!(
        outside.borrow().disk.readout.content,
        ReadoutContent::Auto,
        "choosing changes nothing yet"
    );
    press(&window, " ");
    wait(&window, 300);
    assert!(!window.get_choosing_pins());
    assert!(matches!(
        outside.borrow().disk.readout.content,
        ReadoutContent::Pinned(ref p) if p.len() == 1
    ));
    // A repeated request keeps everything and only reveals again.
    window.set_readout_every(true);
    window.invoke_readout_every_toggled(true);
    sw::reveal(Focus::Pins);
    frame(&window);
    assert!(window.get_readout_every());
    wait(&window, 300);
    assert!(outside.borrow().disk.readout.every_taskbar);
    assert!(matches!(
        outside.borrow().disk.readout.content,
        ReadoutContent::Pinned(_)
    ));

    // ---- Closing when the file cannot be written keeps the window open with the InfoBar's
    // Try again and Close without saving.
    outside.borrow_mut().fail_write = Some("The disk is full.".to_owned());
    window.set_hide_personal_info(true);
    window.invoke_privacy_toggled(true);
    window.window().dispatch_event(WindowEvent::CloseRequested);
    frame(&window);
    assert!(sw::is_open(), "kept open");
    assert!(window.get_info_text().contains("The disk is full."));
    assert!(window.get_info_closing());
    // Try again still fails; then it is written and the window closes.
    window.invoke_info_retry();
    assert!(sw::is_open());
    outside.borrow_mut().fail_write = None;
    window.invoke_info_retry();
    assert!(!sw::is_open(), "closed once saved");
    assert!(outside.borrow().disk.hide_personal_info);

    // Close without saving: nothing more is written.
    let window = open(&outside);
    outside.borrow_mut().fail_write = Some("The disk is full.".to_owned());
    window.set_readout_enabled(false);
    window.invoke_readout_enabled_toggled(false);
    window.invoke_dismiss();
    assert!(sw::is_open(), "Escape keeps it open too");
    window.invoke_info_discard();
    assert!(!sw::is_open());
    assert!(outside.borrow().disk.readout.enabled, "not saved");
}
