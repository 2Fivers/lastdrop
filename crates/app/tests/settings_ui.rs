//! The settings window's pages, instant apply, service expanders and deep links (settings-spec
//! §4, §5, §7), driven through Slint's headless software platform with the demo's settings,
//! invented accounts and the described services Lastdrop ships: no OS window, no files, no
//! registry, no Credential Manager — commits and API keys go to an in-memory `settings.json` and
//! key store (`MemoryOutside`), and the platform's clock is the test's, so the 250 ms and 800 ms
//! timers fire when the test says.
//!
//! One test function: Slint's platform is set once per process and its components are not
//! `Send`, so everything runs on one thread here.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lastdrop_app::demo::Demo;
use lastdrop_app::settings_accounts::folder_id;
use lastdrop_app::settings_edits::MemoryOutside;
use lastdrop_app::settings_window::{self as sw, Focus, Inputs};
use lastdrop_app::{custom_services, preview, ui};
use lastdrop_core::model::{AccountKey, ProviderId};
use lastdrop_core::settings::{AccountSettings, ReadoutContent, RefreshInterval, SourceSettings};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, Model};

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

/// Types `text`, one key at a time.
fn type_text(window: &ui::SettingsWindow, text: &str) {
    for c in text.chars() {
        press(window, c.to_string());
    }
}

/// The English demo's settings window inputs.
fn demo_inputs() -> Inputs {
    preview::demo_settings_inputs(&Demo::new(false), false)
}

/// The settings window for the English demo, committing to `outside`, at its preferred size.
fn open(outside: &Rc<RefCell<MemoryOutside>>) -> ui::SettingsWindow {
    open_with(outside, demo_inputs())
}

/// The settings window for `inputs`, committing to `outside`, at its preferred size.
fn open_with(outside: &Rc<RefCell<MemoryOutside>>, inputs: Inputs) -> ui::SettingsWindow {
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

/// The service list's rows.
fn services(window: &ui::SettingsWindow) -> Vec<ui::ProviderRow> {
    window.get_providers().iter().collect()
}

/// The row of the service `id` in the service list.
fn service_row(window: &ui::SettingsWindow, id: &str) -> i32 {
    let row = services(window)
        .iter()
        .position(|r| r.id == id)
        .expect("listed");
    i32::try_from(row).expect("a few rows")
}

/// The account rows of accounts block `block`.
fn accounts(window: &ui::SettingsWindow, block: usize) -> Vec<ui::AccountRow> {
    let blocks = window.get_account_blocks();
    blocks
        .row_data(block)
        .map(|b| b.accounts.iter().collect())
        .unwrap_or_default()
}

#[test]
fn settings_pages_instant_apply_and_deep_links() {
    slint::platform::set_platform(Box::new(Headless)).expect("platform");
    // The described services Lastdrop ships, listed after the built-in ones (turned off).
    custom_services::load_bundled();
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
    // pause. Claude's row expands to its accounts.
    window.invoke_select_page(sw::PAGE_SERVICES);
    let claude = service_row(&window, "claude");
    window.invoke_service_toggled(claude, true);
    frame(&window);
    assert!(services(&window)[0].expanded);
    assert_eq!(services(&window)[0].kind, ui::ServiceKind::Accounts);
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

    // ---- A described service read with an API key expands to its key row (settings-spec §4):
    // from the pane, Tab to its chevron, Space opens it, Tab reaches the key box; Enter saves
    // the key (Credential Manager here is `MemoryOutside`'s) and turns the service on; Remove
    // takes it back. The rows are all closed, so each has its switch and, when it has
    // something to manage, its chevron.
    window.invoke_service_toggled(claude, false);
    window.invoke_select_page(sw::PAGE_GENERAL);
    frame(&window);
    let deepseek = service_row(&window, "deepseek");
    let at = usize::try_from(deepseek).expect("a row");
    {
        let row = &services(&window)[at];
        assert_eq!(row.kind, ui::ServiceKind::Key);
        assert_eq!(
            row.description,
            "Described in a JSON file · Needs an API key"
        );
        assert_eq!(row.toggle_label, "DeepSeek API key");
        assert!(!row.enabled && !row.expanded);
    }
    ctrl(&window, Key::Tab);
    assert_eq!(
        window.get_page(),
        sw::PAGE_SERVICES,
        "the pane has the focus"
    );
    let stops: usize = services(&window)[..at]
        .iter()
        .map(|r| {
            if r.kind == ui::ServiceKind::None {
                1
            } else {
                2
            }
        })
        .sum();
    for _ in 0..stops + 2 {
        press(&window, Key::Tab);
    }
    press(&window, " ");
    assert!(
        services(&window)[at].expanded,
        "Space on its chevron opens it"
    );
    press(&window, Key::Tab);
    let writes = outside.borrow().writes;
    type_text(&window, "sk-invented-key");
    press(&window, Key::Return);
    let target = "Lastdrop:deepseek:default:api_key";
    assert!(outside.borrow().keys.contains(target), "stored");
    assert_eq!(outside.borrow().writes, writes + 1, "turned on at once");
    assert!(
        outside
            .borrow()
            .disk
            .provider(ProviderId::from_id("deepseek").expect("loaded"))
            .is_some_and(|p| p.enabled)
    );
    let row = &services(&window)[at];
    assert!(row.enabled);
    assert_eq!(row.description, "Described in a JSON file · API key stored");
    let key_row = usize::try_from(row.key_row).expect("a key row");
    let key = window.get_api_keys().row_data(key_row).expect("key row");
    assert!(key.stored);
    assert_eq!(key.status, "Key saved. Lastdrop reads DeepSeek with it.");
    assert_eq!(key.save_label, "Save the DeepSeek API key");
    assert_eq!(key.remove_label, "Remove the DeepSeek API key");
    // Save key is off with the box cleared, so Tab reaches Remove; Remove goes with the key and
    // the focus goes back to the box: typing there saves again.
    press(&window, Key::Tab);
    press(&window, " ");
    assert!(outside.borrow().keys.is_empty(), "removed");
    let key = window.get_api_keys().row_data(key_row).expect("key row");
    assert!(!key.stored);
    assert_eq!(key.status, "Key removed.");
    type_text(&window, "sk-another-key");
    press(&window, Key::Return);
    assert!(outside.borrow().keys.contains(target), "stored again");
    // The expander stays open on another page and back.
    window.invoke_select_page(sw::PAGE_DISPLAY);
    window.invoke_select_page(sw::PAGE_SERVICES);
    frame(&window);
    assert!(services(&window)[at].expanded);

    // ---- Deep link to the accounts: Services, the service with found folders expanded, its
    // first found folder's Add focused; Space adds it, committed at once.
    window.invoke_service_toggled(claude, false);
    frame(&window);
    assert!(!services(&window)[0].expanded);
    window.invoke_select_page(sw::PAGE_GENERAL);
    sw::reveal(Focus::Accounts);
    frame(&window);
    assert_eq!(window.get_page(), sw::PAGE_SERVICES);
    let rows = services(&window);
    assert!(rows[0].expanded, "Claude has a found folder");
    assert!(!rows[1].expanded, "Codex has none");
    let writes = outside.borrow().writes;
    press(&window, " ");
    assert_eq!(outside.borrow().writes, writes + 1, "added at once");
    assert_eq!(claude_aliases(&outside.borrow()).len(), 3);
    let status: Vec<slint::SharedString> = window.get_block_status().iter().collect();
    assert_eq!(status[0], "Account added.");
    // Its row went with the focus; the focus goes on to Add folder… (no other found folder).
    assert_eq!(window.get_focus_target(), "add-folder");

    // Removing it commits at once too; the focus goes to the next account (there is none: Add
    // folder…).
    window.invoke_account_removed(0, 2);
    frame(&window);
    assert_eq!(claude_aliases(&outside.borrow()).len(), 2);
    assert_eq!(window.get_focus_target(), "add-folder");
    window.invoke_account_removed(0, 0);
    frame(&window);
    assert_eq!(
        window.get_focus_target(),
        format!("account:{}", accounts(&window, 0)[0].id),
        "the account after it"
    );
    assert_eq!(claude_aliases(&outside.borrow()), [Some("Work".to_owned())]);
    assert_eq!(
        services(&window)[0].description,
        "Claude Code's sign-in on this PC · 1 account"
    );

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

    // ---- Every configured folder has its row, a duplicate too (accounts-spec §3, §7): a third
    // Claude folder signed in to the same account as the default one says so and its meters
    // are left out of the meter list.
    let mut inputs = demo_inputs();
    let home = std::path::PathBuf::from(r"C:\Users\tester");
    let copy = home.join(".claude-copy");
    let same = inputs.accounts.identity(&home.join(".claude")).clone();
    inputs.accounts.identities.insert(folder_id(&copy), same);
    if let Some(p) = inputs.settings.provider_mut(ProviderId::Claude) {
        let taken: Vec<AccountKey> = p.accounts.iter().map(|a| a.key.clone()).collect();
        p.accounts.push(AccountSettings::for_folder(
            SourceSettings::ClaudeConfigDir { path: copy },
            Some("Copy"),
            &taken,
        ));
    }
    let window = open_with(&outside, inputs);
    window.invoke_service_toggled(service_row(&window, "claude"), true);
    frame(&window);
    let rows = accounts(&window, 0);
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["tester@example.invalid", "Work", "Copy"]);
    assert_eq!(
        rows[2].status,
        "Same account as tester@example.invalid, shown once"
    );
    assert!(rows[2].folder.ends_with(".claude-copy"));
    assert!(rows[2].removable);
    assert_eq!(
        services(&window)[0].description,
        "Claude Code's sign-in on this PC · 3 accounts"
    );
    let meters: Vec<String> = window
        .get_meters()
        .iter()
        .map(|m| m.label.to_string())
        .collect();
    assert!(meters.iter().any(|m| m.contains("Work")), "{meters:?}");
    assert!(!meters.iter().any(|m| m.contains("Copy")), "{meters:?}");
    window.invoke_dismiss();
    assert!(!sw::is_open());
}
