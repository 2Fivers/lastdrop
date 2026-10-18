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
use lastdrop_providers::discover::Candidate;
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
fn ctrl(window: &ui::SettingsWindow, key: impl Into<slint::SharedString> + Clone) {
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

    // ---- The navigation pane: it has the focus when the window opens, so the arrow keys, Home
    // and End move the selection at once; one Tab stop.
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

    // Removing it commits at once too; the focus goes to the next account, here none: Add
    // folder…, so Tab reaches New account…, which Space opens (its name box takes the focus);
    // Tab passes Create and sign in (off while the box is empty) to Cancel.
    let request = window.get_focus_request();
    window.invoke_account_removed(0, 2);
    frame(&window);
    assert_eq!(claude_aliases(&outside.borrow()).len(), 2);
    assert!(window.get_focus_request() > request);
    assert_eq!(window.get_focus_target(), "add-folder");
    press(&window, Key::Tab);
    press(&window, " ");
    assert_eq!(window.get_new_account_block(), 0, "New account… opened");
    press(&window, Key::Tab);
    press(&window, " ");
    assert_eq!(window.get_new_account_block(), -1, "Cancel closed it");
    // Removing the first account moves the focus to the next one's alias box: typed there.
    window.invoke_account_removed(0, 0);
    frame(&window);
    assert_eq!(
        window.get_focus_target(),
        format!("account:{}", accounts(&window, 0)[0].id),
        "the account after it"
    );
    assert_eq!(claude_aliases(&outside.borrow()), [Some("Work".to_owned())]);
    ctrl(&window, "a");
    type_text(&window, "Office");
    press(&window, Key::Return);
    wait(&window, 300);
    assert_eq!(
        claude_aliases(&outside.borrow()),
        [Some("Office".to_owned())]
    );
    assert_eq!(
        services(&window)[0].description,
        "Claude Code's sign-in on this PC · 1 account"
    );

    // ---- Deep link to the pins: Display, the readout on and "Meters I choose" (choosing: no
    // pin yet, still automatic), the content's card outlined, the first slot focused; Down
    // picks its first meter.
    window.invoke_select_page(sw::PAGE_GENERAL);
    sw::reveal(Focus::Pins);
    frame(&window);
    assert_eq!(window.get_page(), sw::PAGE_DISPLAY);
    assert!(window.get_highlight_pins());
    assert_eq!(window.get_readout_content_index(), 1);
    assert!(window.get_choosing_pins());
    assert!(!window.get_pin_second_enabled(), "no first meter yet");
    assert_eq!(window.get_pin_first_index(), 0, "None");
    assert_eq!(
        outside.borrow().disk.readout.content,
        ReadoutContent::Auto,
        "choosing changes nothing yet"
    );
    press(&window, Key::DownArrow);
    wait(&window, 300);
    assert!(!window.get_choosing_pins());
    assert!(window.get_pin_second_enabled());
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
    // are left out of the meter list. A file that can be written again.
    let outside = Rc::new(RefCell::new(MemoryOutside::default()));
    let mut inputs = demo_inputs();
    let home = std::path::PathBuf::from(r"C:\Users\tester");
    // A second folder found on this PC besides `.claude-old`.
    let spare = home.join(".claude-spare");
    if let Some(found) = inputs.accounts.found.get_mut(&ProviderId::Claude) {
        found.push(Candidate {
            folder: spare.clone(),
            source: SourceSettings::ClaudeConfigDir { path: spare },
        });
    }
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
    let meters = slot_choices(&window, 0);
    assert!(meters.iter().any(|m| m.contains("Work")), "{meters:?}");
    assert!(!meters.iter().any(|m| m.contains("Copy")), "{meters:?}");

    // ---- Two found folders: the deep link focuses the first one's Add; Space adds it and the
    // focus moves on to the next one's Add, so Space adds that too.
    sw::reveal(Focus::Accounts);
    frame(&window);
    press(&window, " ");
    press(&window, " ");
    let folders: Vec<String> = accounts(&window, 0)
        .iter()
        .map(|r| r.folder.to_string())
        .collect();
    assert_eq!(folders.len(), 5, "{folders:?}");
    assert!(folders[3].ends_with(".claude-old") && folders[4].ends_with(".claude-spare"));
    assert_eq!(window.get_focus_target(), "add-folder");
    assert_eq!(
        outside
            .borrow()
            .disk
            .provider(ProviderId::Claude)
            .map(|p| p.accounts.len()),
        Some(5),
        "each saved at once"
    );
    window.invoke_dismiss();
    assert!(!sw::is_open());

    // ---- The accounts deep link with no found folder: Services, and the pane takes the focus,
    // which was on the page just hidden (where no key would reach the window): Down moves the
    // selection.
    let mut inputs = demo_inputs();
    inputs.accounts.found.clear();
    let window = open_with(&outside, inputs);
    press(&window, Key::End);
    press(&window, Key::Tab);
    assert_eq!(window.get_page(), sw::PAGE_GENERAL);
    sw::reveal(Focus::Accounts);
    frame(&window);
    assert_eq!(window.get_page(), sw::PAGE_SERVICES);
    press(&window, Key::DownArrow);
    assert_eq!(window.get_page(), sw::PAGE_DISPLAY);
    window.invoke_dismiss();
    assert!(!sw::is_open());

    pin_slots();
    meter_visibility();
}

/// The labels pin slot `slot` lists.
fn slot_choices(window: &ui::SettingsWindow, slot: i32) -> Vec<String> {
    let model = if slot == 0 {
        window.get_pin_first_choices()
    } else {
        window.get_pin_second_choices()
    };
    model.iter().map(|c| c.to_string()).collect()
}

/// The label pin slot `slot` shows.
fn slot_shows(window: &ui::SettingsWindow, slot: i32) -> String {
    let index = if slot == 0 {
        window.get_pin_first_index()
    } else {
        window.get_pin_second_index()
    };
    let at = usize::try_from(index).expect("a choice");
    slot_choices(window, slot)[at].clone()
}

/// Picks `label` in pin slot `slot`, as the combo box does.
fn pick(window: &ui::SettingsWindow, slot: i32, label: &str) {
    let choices = slot_choices(window, slot);
    let at = choices
        .iter()
        .position(|c| c == label)
        .unwrap_or_else(|| panic!("{label:?} in {choices:?}"));
    let index = i32::try_from(at).expect("a few choices");
    if slot == 0 {
        window.set_pin_first_index(index);
    } else {
        window.set_pin_second_index(index);
    }
    window.invoke_pin_slot_selected(slot, index);
    frame(window);
}

/// The pins as saved, each as its slot lists it (by its service, account and meter id).
fn saved_pins(outside: &Rc<RefCell<MemoryOutside>>) -> Vec<(ProviderId, String, String)> {
    match &outside.borrow().disk.readout.content {
        ReadoutContent::Pinned(pins) => pins
            .iter()
            .map(|k| (k.provider, k.account.0.clone(), k.meter.0.clone()))
            .collect(),
        ReadoutContent::Auto => Vec::new(),
    }
}

/// The Meters list's card of `service`: its index and its meter types.
fn meter_card(window: &ui::SettingsWindow, service: &str) -> Option<(i32, Vec<ui::MeterType>)> {
    let groups: Vec<ui::MeterGroup> = window.get_meter_groups().iter().collect();
    let at = groups.iter().position(|g| g.name == service)?;
    let types = groups[at].types.iter().collect();
    Some((i32::try_from(at).expect("a few cards"), types))
}

/// The pin slots (settings-spec §4): the choosing state survives an unrelated commit; no meter
/// in both slots; emptying the first promotes the second; a pin whose service is off is kept
/// and listed "(not available)"; hiding a pinned meter takes it off with a note; removing an
/// account clears its pins and promotes the second.
fn pin_slots() {
    let outside = Rc::new(RefCell::new(MemoryOutside::default()));
    let window = open(&outside);
    window.invoke_select_page(sw::PAGE_DISPLAY);
    window.set_readout_content_index(1);
    window.invoke_readout_mode_selected(1);
    frame(&window);
    assert!(window.get_choosing_pins());
    // An unrelated commit keeps the choosing state; the file stays automatic.
    window.set_readout_every(true);
    window.invoke_readout_every_toggled(true);
    wait(&window, 300);
    assert_eq!(outside.borrow().writes, 1);
    assert!(window.get_choosing_pins());
    assert_eq!(window.get_readout_content_index(), 1);
    assert_eq!(saved_pins(&outside), []);

    // The choices: the shown meters with their account where the service has several.
    let me = "tester@example.invalid";
    let work_weekly = "Claude · Work · Weekly limit";
    let codex_five = "Codex · 5-hour limit";
    let first = slot_choices(&window, 0);
    assert_eq!(first[0], "None");
    assert_eq!(first[1], format!("Claude · {me} · 5-hour limit"));
    assert!(first.iter().any(|c| c == work_weekly), "{first:?}");
    assert!(first.iter().any(|c| c == "Meshy · Credits"), "{first:?}");

    // No duplicates: the second slot does not list the first's meter.
    pick(&window, 0, work_weekly);
    assert!(!window.get_choosing_pins());
    assert!(window.get_pin_second_enabled());
    assert!(!slot_choices(&window, 1).iter().any(|c| c == work_weekly));
    pick(&window, 1, codex_five);
    wait(&window, 300);
    let work = |meter: &str| {
        let account = outside
            .borrow()
            .disk
            .provider(ProviderId::Claude)
            .map(|p| p.accounts[1].key.0.clone())
            .expect("Work");
        (ProviderId::Claude, account, meter.to_owned())
    };
    let codex = |meter: &str| (ProviderId::Codex, "default".to_owned(), meter.to_owned());
    assert_eq!(saved_pins(&outside), [work("weekly"), codex("five_hour")]);
    // The second slot's meter picked in the first empties the second.
    pick(&window, 0, codex_five);
    assert_eq!(slot_shows(&window, 1), "None");
    // Emptying the first promotes the second.
    pick(&window, 1, work_weekly);
    pick(&window, 0, "None");
    wait(&window, 300);
    assert_eq!(saved_pins(&outside), [work("weekly")]);
    assert_eq!(slot_shows(&window, 0), work_weekly);
    assert_eq!(slot_shows(&window, 1), "None");

    // A pin whose service is off is kept, listed as not available.
    pick(&window, 1, codex_five);
    window.invoke_provider_toggled(service_row(&window, "codex"), false);
    wait(&window, 300);
    assert_eq!(saved_pins(&outside), [work("weekly"), codex("five_hour")]);
    assert_eq!(
        slot_shows(&window, 1),
        "Codex · 5-hour limit (not available)"
    );
    assert!(
        !slot_choices(&window, 0)
            .iter()
            .any(|c| c.starts_with("Codex"))
    );
    assert!(meter_card(&window, "Codex").is_none(), "off: not listed");
    // An unrelated pick keeps it.
    pick(&window, 0, &format!("Claude · {me} · 5-hour limit"));
    wait(&window, 300);
    assert_eq!(saved_pins(&outside)[1], codex("five_hour"));
    window.invoke_provider_toggled(service_row(&window, "codex"), true);
    wait(&window, 300);
    assert_eq!(slot_shows(&window, 1), codex_five, "available again");

    // Hiding a pinned meter takes it off its slot, with a note; the second moves up.
    pick(&window, 0, work_weekly);
    let (claude, types) = meter_card(&window, "Claude").expect("Claude's card");
    let weekly = types
        .iter()
        .position(|t| t.label == "Weekly limit")
        .and_then(|r| i32::try_from(r).ok())
        .expect("weekly");
    window.invoke_meter_shown_toggled(claude, weekly, false);
    wait(&window, 300);
    assert_eq!(saved_pins(&outside), [codex("five_hour")]);
    assert_eq!(slot_shows(&window, 0), codex_five);
    assert_eq!(
        window.get_pin_note(),
        "Claude weekly limit is hidden, so it was taken off the taskbar."
    );
    // Shown again, the note goes; the meter is not pinned again.
    window.invoke_meter_shown_toggled(claude, weekly, true);
    wait(&window, 300);
    assert_eq!(window.get_pin_note(), "");
    assert_eq!(saved_pins(&outside), [codex("five_hour")]);

    // Removing an account clears its pins (committed at once) and promotes the second.
    pick(&window, 0, work_weekly);
    pick(&window, 1, codex_five);
    wait(&window, 300);
    window.invoke_account_removed(0, 1);
    frame(&window);
    assert_eq!(saved_pins(&outside), [codex("five_hour")]);
    assert_eq!(slot_shows(&window, 0), codex_five);
    assert!(!slot_choices(&window, 0).iter().any(|c| c.contains("Work")));
    // Removing the last pin is choosing again (saved: automatic).
    pick(&window, 0, "None");
    wait(&window, 300);
    assert!(window.get_choosing_pins());
    assert_eq!(saved_pins(&outside), []);

    // Each arrow key in a list is a pick; browsing the first slot loses nothing on the way. Its
    // pin is not available (Codex is off) and the second is Meshy's credits.
    pick(&window, 0, codex_five);
    pick(&window, 1, "Meshy · Credits");
    window.invoke_provider_toggled(service_row(&window, "codex"), false);
    wait(&window, 300);
    let meshy = (
        ProviderId::Meshy,
        "default".to_owned(),
        "credits".to_owned(),
    );
    assert_eq!(saved_pins(&outside), [codex("five_hour"), meshy.clone()]);
    let unavailable = "Codex · 5-hour limit (not available)";
    assert_eq!(slot_shows(&window, 0), unavailable);
    // The deep link focuses the first slot; Down to the last choice, Meshy's credits, empties
    // the second; Up brings it back.
    sw::reveal(Focus::Pins);
    frame(&window);
    let last = slot_choices(&window, 0).len() - 1;
    for _ in 1..last {
        press(&window, Key::DownArrow);
    }
    assert_eq!(slot_shows(&window, 0), "Meshy · Credits");
    assert_eq!(
        saved_pins(&outside),
        [codex("five_hour"), meshy.clone()],
        "a burst of picks commits once, after the last"
    );
    wait(&window, 300);
    assert_eq!(saved_pins(&outside), std::slice::from_ref(&meshy));
    press(&window, Key::UpArrow);
    wait(&window, 300);
    assert_eq!(saved_pins(&outside)[1], meshy, "the second is back");
    // The pin that is not available stays listed while the slot has the focus: Up to it.
    assert!(slot_choices(&window, 0).iter().any(|c| c == unavailable));
    while slot_shows(&window, 0) != unavailable {
        press(&window, Key::UpArrow);
    }
    wait(&window, 300);
    assert_eq!(saved_pins(&outside), [codex("five_hour"), meshy.clone()]);
    // Another meter, then the focus leaves: the pin that is not available leaves the list.
    press(&window, Key::DownArrow);
    press(&window, Key::Tab);
    wait(&window, 300);
    assert!(!slot_choices(&window, 0).iter().any(|c| c == unavailable));
    assert_eq!(saved_pins(&outside).len(), 2);
    window.invoke_dismiss();
    assert!(!sw::is_open());
}

/// The Meters list (settings-spec §4): a card per service that is on, a check box per meter
/// type with its short name; hiding one hides it for the whole service, every account; with no
/// service on, the empty state's link goes to Services.
fn meter_visibility() {
    let outside = Rc::new(RefCell::new(MemoryOutside::default()));
    let window = open(&outside);
    let (claude, types) = meter_card(&window, "Claude").expect("Claude's card");
    let labels: Vec<&str> = types.iter().map(|t| t.label.as_str()).collect();
    // One row per type, though Claude has two accounts.
    assert_eq!(
        labels,
        ["5-hour limit", "Weekly limit", "Fable weekly limit"]
    );
    assert_eq!(types[1].show_label, "Show Claude weekly limit");
    assert!(types.iter().all(|t| t.shown));
    let (_, meshy) = meter_card(&window, "Meshy").expect("Meshy's card");
    assert_eq!(meshy[0].label, "Credits");

    // Hiding the weekly limit hides it for the service.
    window.invoke_meter_shown_toggled(claude, 1, false);
    wait(&window, 300);
    let hidden = outside
        .borrow()
        .disk
        .provider(ProviderId::Claude)
        .map(|p| p.hidden_meters.clone())
        .expect("Claude");
    assert_eq!(hidden.len(), 1, "once for the service: {hidden:?}");
    assert_eq!(hidden[0].0, "weekly");
    let (_, types) = meter_card(&window, "Claude").expect("Claude's card");
    assert!(!types[1].shown);
    let choices = slot_choices(&window, 0);
    assert!(
        !choices
            .iter()
            .any(|c| c.starts_with("Claude") && c.ends_with("· Weekly limit")),
        "neither account's weekly limit can be pinned: {choices:?}"
    );

    // Every service off: the empty state, whose link goes to Services. The pane has the focus
    // (the window opened with it), Down shows Display; Tab passes the theme list, Theme
    // Studio…, Open themes folder, the times, the readout's two switches and its content
    // (automatic: no slots) to the link; Space follows it.
    for row in services(&window) {
        if row.enabled {
            window.invoke_provider_toggled(service_row(&window, &row.id), false);
        }
    }
    wait(&window, 300);
    assert_eq!(window.get_meter_groups().row_count(), 0);
    press(&window, Key::DownArrow);
    assert_eq!(window.get_page(), sw::PAGE_DISPLAY);
    for _ in 0..8 {
        press(&window, Key::Tab);
    }
    press(&window, " ");
    assert_eq!(window.get_page(), sw::PAGE_SERVICES);
    window.invoke_dismiss();
    assert!(!sw::is_open());
}
