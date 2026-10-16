# The readout on every taskbar

ROADMAP M4 "Readout on every taskbar" (readout-spec §14 left it for M4; ROADMAP "Multiple
monitors": "the readout can show on the primary taskbar only or on every taskbar").

References: readout-spec (R§), ARCHITECTURE "Threads", "Windows layer".

## 1. The setting

Settings ▸ Taskbar readout ▸ **Show it on every taskbar** (`readout.every_taskbar`, default off),
under "Show the readout in the taskbar" and greyed out while that is off. Off: the readout is on
the primary taskbar only, as before. On: also on every secondary taskbar Windows shows ("Show my
taskbar on all displays"). Every readout draws the same rows (R§4.3); a monitor's own DPI and
taskbar decide its form and size (R§4.2), so one can be compact while another shows two rows.

## 2. One readout thread per taskbar

- The primary readout thread is unchanged (`TaskbarTarget::Primary`, `Shell_TrayWnd`). It alone
  forwards the system broadcasts (theme, locale, time, resume), and it also says when the
  taskbars may have changed: `SystemEvent::Taskbars` on `WM_DISPLAYCHANGE`, `TaskbarCreated` and
  `WM_SETTINGCHANGE` "TraySettings".
- On that event, at start-up and after a settings change, the main thread lists the monitors
  that have a `Shell_SecondaryTrayWnd` (`readout::secondary_monitors`: the top-level windows of
  that class, each by its monitor's rectangle) and starts or stops one more readout thread per
  monitor (`TaskbarTarget::Secondary { monitor }`, `crate::secondary` in the app). It does not
  poll.
- A secondary readout thread is the same code as the primary one (its own message window,
  layered readout window, WinEvent hooks, occupancy thread, UIA provider): it finds the
  `Shell_SecondaryTrayWnd` on its monitor, follows it the way the primary one follows
  `Shell_TrayWnd` (R§6), and sends no system broadcasts. When its taskbar is gone (the monitor
  was unplugged) it hides and looks for it again every 2 s until the main thread stops it.
- Its events: hover shows the hover card above it (R§8.1, on that monitor), a click toggles the
  flyout anchored at it, UIA Invoke opens the flyout as a keyboard open, a right click shows the
  tray menu at the pointer (R§10). Its placement is kept for the flyout's anchor only: the tray
  icon's size, the hover card's rows and the flyout's floating-readout rule (R§9.1, R§8.2, F§1.3)
  follow the primary readout.

## 3. Placement on a secondary taskbar

A secondary taskbar has no notification area window (`TrayNotifyWnd`): on Windows 11 its right end
holds the clock and Show desktop, as XAML without windows of their own.

- **Measuring** (occupancy thread, one UI Automation transaction as for the primary taskbar):
  the buttons at the right end are those right of the rightmost task-list button
  (`Taskbar.TaskListButton*`), or, with no app open on that monitor, those in the right half of
  the taskbar that are not task-list buttons, plus every `SystemTray.*` button. Their leftmost
  left edge is `tray_left`; the others are app buttons as before. This does not depend on the
  clock's class names, which differ between Windows builds.
- **Placing** (`placement::place`, pure): the anchor is `tray_left`, else the taskbar's right edge
  (no clock shown); `room`, the form ladder and the docked and floating rectangles are R§4.2 and
  R§6.1 unchanged. Unknown extents float, right-aligned to the anchor, as on the primary taskbar.
- Tests: `tests/layouts/synthetic-2x1080p-100-secondary-*.json` (clock, no clock, unknown
  extents) and the occupancy split in `occupancy.rs`. A secondary taskbar recorded on a real
  setup belongs in the folder with the manual multi-monitor matrix (ARCHITECTURE "Quality bar").

## 4. Memory

Each secondary readout costs what the primary one does: about 2.5 MB of private working set
(ARCHITECTURE "Processes", the readout's share of the budget, of which 0.7 MB is its occupancy
thread's UI Automation). The 10 MB budget is for the default, primary-only readout; with the
setting on, the budget grows by that much per extra taskbar. Not measured on a multi-monitor
setup yet: a release check, with the manual matrix.
