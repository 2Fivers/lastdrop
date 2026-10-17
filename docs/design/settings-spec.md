# Lastdrop — the settings window

Implementation spec (`S§`). Companions: [accounts-spec.md](accounts-spec.md) (accounts-spec §7,
the accounts in the window), [readout-spec.md](readout-spec.md) (`R§`; R§4.3 pinned meters,
R§10 the tray menu's deep link), [notifications-spec.md](notifications-spec.md) (`N§2`),
[themes-spec.md](themes-spec.md) (Theme Studio), [custom-services-spec.md](custom-services-spec.md)
(described services). Code: `crates/app/ui/settings.slint` and
`settings_controls.slint` (§2–§4), `lastdrop_app::settings_window` (§2–§5, §7),
`lastdrop_app::settings_edits` (§5), `lastdrop_app::settings_sync` and
`lastdrop_win::named_lock` (§6).

The settings window runs in the dialog process (ARCHITECTURE "Processes"), created when it
opens and released when it closes.

## 1. Scope

The window was one 2700 px scroll of 13 flat sections in implementation order, with an Apply
button. This spec replaces it with a left navigation pane, four pages of settings cards and
instant apply. It is built in phases; this spec describes what is built:

- **Built** (phases 1–2): the write coordination (§6), the change model (§5), the shell (§2),
  the building blocks and colours (§3), today's controls moved onto the four pages (§4), instant
  apply, the deep links (§7), the previews and the tests (§9).
- **Built** (phase 3): the service list's rows with their glyph, a line on how each service is
  read and capability-driven expanders holding the accounts and the API keys; "Add your own
  service" (§4).
- **Later** (§10): pin slots and per-service meter visibility; the shortcut editor with a
  preview, the version card's result, open-folder links.

## 2. Shell

- **Window**: title "Lastdrop settings" (the system title bar; no heading inside the page),
  preferred 880 × 680, smallest 680 × 520 (logical px). It opens at the preferred size, made
  smaller to fit the work area of the monitor under the cursor with a 24 px margin, never below
  the smallest size unless the work area is smaller (`opening_size`).
- **Navigation pane**: 220 px, four labelled items (no icon-only collapse): **Services**,
  **Display**, **Notifications**, **General** (sv: **Tjänster**, **Visning**, **Aviseringar**,
  **Allmänt**). Items are 36 px with a 16 px Segoe Fluent icon and a 14 px label, radius 4;
  hover: a subtle fill; selected: a subtle fill and a 3 × 16 px accent pill at the left edge. The
  pane is a tab list (items: tabs); its keyboard focus rectangle is drawn on the selected item
  only after keyboard use.
- **Content pane**: the page's title (28 px semibold, Segoe UI Variable Display) and, for
  Services and Display, a one-line description (14 px, secondary); side padding 32 px (24 px
  when the pane is narrower than 560 px); content at most 720 px wide; one scroll view per page.
  Every page is a tab panel.
- **InfoBar** at the top of the content pane, above the page (it does not scroll with it):
  only for errors not tied to one visible control — "Couldn't save the settings: {detail}" with
  **Try again** (and **Close without saving** while closing, §5.5). A live region (assertive), so
  a screen reader says it.
- **No footer**: no Apply, no Close button, no status line (§5).
- **Pages stay instantiated**: all four pages exist while the window is open; the inactive ones
  are invisible, which takes them out of the Tab order (Slint skips invisible items in focus
  traversal) and out of the accessibility tree. Each keeps its scroll position and open
  expanders. Every editable value lives in Rust (`Edits`) or in a property of the window, never
  only in a page.

## 3. Building blocks and colours

### 3.1 Cards (`settings_controls.slint`)

- **SettingsCard**: radius 4, the card fill and a 1 px card stroke around its rows; `highlighted`
  draws a 2 px accent outline (a deep link's target, §7). One row, or several separated by 1 px
  dividers (a **grouped card**: the service list, the meters, "Also notify me"). A card does not
  clip its rows: Slint's Tab skips an item that a clipping element other than the page's scroll
  view hides, so a clipping card taller than the page would lose the Tab stops scrolled out of
  view. A row with a fill of its own (the expander's hover) rounds its corners itself.
- **CardRow**: [20 px icon] header (14 px) and description (12 px, secondary, wraps) · the
  trailing controls, centred. At least 48 px high, 64 px with a description; text wraps, never
  clips. A dependent row (`enabled: false`) dims its texts while its controls are disabled.
- **StackedRow**: the same look with the controls under the header, for controls too wide to
  share its line (the theme list and Theme Studio…, the taskbar content list, the language list,
  the level boxes, the shortcut's keys, an API key box, an account).
- **Group headings**: 14 px semibold, 24 px above and 8 px below, with an optional 12 px
  description; 4 px between separate cards.

### 3.2 Expander

**ExpanderHeader** is a row whose expansion is its own control: [a 20 px provider glyph, drawn in
the window's text colour (`ProviderGlyph`'s `foreground`)] header and description · the trailing
controls (a service's switch) · a 32 px chevron button (accessible role button, named after
what it shows — "Claude accounts", "Meshy API key" —, expandable, expanded state; Enter or
Space) and a click on the row's free area. It is never coupled to the switch. A row with nothing
to expand keeps the chevron's place empty, so the switches of a grouped card line up, and does
nothing on a click. The expanded content follows in the same card and is instantiated only while
expanded (`if expanded`), so collapsed content is in neither the Tab order nor the accessibility
tree; its rows are indented 40 px (16 px in a narrow column, §4).

- **Opening** scrolls the page as little as it takes to show the header and its content (the
  header at the top when they do not fit), as WinUI brings an expanded expander into view. Tab
  passes over a text box scrolled out of view (Slint 1.18: a LineEdit clips itself, and Tab only
  reaches what the scroll view alone hides), so the boxes an expander opens must be in view.
- **Closing** with the pointer gives the chevron the keyboard focus, which the content may have
  held.

### 3.3 Colours

The window follows Windows' app mode (light or dark) through Slint's Fluent palette, and its
accent (Slint tints the Fluent accent with Windows' accent colour, so the switches, check boxes
and the selected item's pill match). It never uses the flyout theme's colours. The settings
tokens (`SettingsTheme`): page fill `#F3F3F3` / `#202020`, card fill `#FBFBFB` / `#2B2B2B`, card
stroke `#E5E5E5` / `#1D1D1D`, dividers, primary, secondary and disabled text, subtle hover and
pressed fills, the selected item's fill, the InfoBar's critical fill and mark.

**Contrast themes**: Rust passes Windows' contrast colours (`COLOR_WINDOW`, `COLOR_WINDOWTEXT`,
`COLOR_HIGHLIGHT`, `COLOR_HIGHLIGHTTEXT`, `COLOR_GRAYTEXT`, `COLOR_HOTLIGHT`). Pages and cards
are the window colour, strokes and dividers and every text the window text, disabled text gray
text, the selected item Highlight with Highlight text, the accent Highlight. The standard controls
take the Fluent scheme that matches the contrast theme's window colour (dark for a dark one), so
their text stays readable; their own fills stay Fluent's.

## 4. Pages

Defaults: the window opens on **Services**. Reopening the window while it is open keeps its
page, scroll positions, open expanders and edits; a deep link overrides (§7).

### Services / Tjänster

"Choose services and manage their accounts or API keys."

- **The service list**: one grouped card of the services in the settings' order. A row: the
  service's glyph · its name and one line on how it is read or its state · its on/off switch
  (named after the service) · the chevron when it has something to manage.

  | Service | Line |
  |---|---|
  | Claude, Codex | "Claude Code's sign-in on this PC · {n} accounts", "Codex's sign-in on this PC · {n} accounts" (every configured folder) |
  | Copilot | "The GitHub CLI's sign-in (gh auth login)" |
  | Cursor | "The Cursor app's sign-in" |
  | Grok | "The Grok CLI's sign-in (grok login)" |
  | Antigravity | "Read through its CLI (agy)" |
  | Meshy | "API key stored" / "Needs an API key" |
  | OpenCode Go | "Uses your API key" / "Uses the key OpenCode keeps" (without a key here it reads with OpenCode's own: no key is not "needs a key") |
  | Described services | "Described in a JSON file · API key stored" / "· Needs an API key" |

  What a row expands to (`ProviderRow.kind`) comes from the service's capabilities, not a list of
  names (`settings_window::service_kind`): **accounts** when its accounts are folders
  (`settings_accounts::has_folders`: Claude, Codex), **key** for every service `key_services()`
  returns (read with an API key stored here: Meshy, OpenCode Go and the described services), else
  **none**. Every expander is closed when the window opens; deep links (§7) open some. Its state
  is kept across page changes while the window is open, and a service that is off can still be
  expanded and configured.
- **Accounts** (Claude, Codex; accounts-spec §7): a row per configured folder, duplicates included
  ("Same account as {name}, shown once"): the name (semibold), its state and its folder (shortened
  with `…`; the whole path in the accessible description) · **Alias** (a labelled box, about
  160 px) and **Remove** (not on the last account). Then **Found on this PC** with each unused
  folder's **Add**; **Add folder…** and **New account…**, which shows a labelled name box with
  "Opens {client}'s sign-in in a terminal." under it, **Create and sign in** and **Cancel**; and a
  line saying what the last action did ("Account added.", "Opened {client}'s sign-in in a
  terminal.").
- **API key** (Meshy, OpenCode Go, described services): a password box labelled **API key** (its
  accessible name "Meshy API key"; its placeholder "Paste your Meshy API key", or "A key is
  stored. Paste a new one to replace it"), **Save key** ("Save the Meshy API key") and, while a key
  is stored, **Remove** ("Remove the Meshy API key"; after it the focus goes back to the box), what
  the last Save or Remove did ("Key saved. Lastdrop reads {service} with it.") and the note
  "Stored in Windows Credential Manager, never in a file. Lastdrop only reads your usage." (for
  OpenCode Go also "Without a key here, Lastdrop uses the one OpenCode itself keeps."). The box is
  cleared after Save, when its expander closes and when the page changes (§5.6).
- **A narrow column** (under 520 px, from the content pane's width so a scroll bar never changes
  it: the window's smallest size has 412 px): an account's alias row goes under its name, state
  and folder (the same box, placed by hand, so a resize keeps what it holds), a box's buttons go
  under it, and expanded rows are indented 16 px instead of 40.
- **Privacy**: "Hide e-mail addresses" — "Show Account 1, Account 2 instead" — switch.
- **More services**: "Add your own service" — "Services with a simple usage API can be described
  in a JSON file, read when Lastdrop starts." — **Open folder** (accessible name "Open the folder
  of your own services"), which opens `%APPDATA%\Lastdrop\providers` in File Explorer, made first
  when it is missing (custom-services-spec §6).

### Display / Visning

"The taskbar readout, the flyout and the meters they show."

- **Appearance**: Theme (description, list, **Theme Studio…**); "Show times as" (Clock times /
  Time remaining).
- **Taskbar readout**: "Show usage in the taskbar" (switch); "Show it on every taskbar" (switch,
  dimmed while the readout is off); "Taskbar content" (The most constrained meters (auto) /
  Meters I pin). Picking "Meters I pin" with no pin yet is the **choosing state**: nothing is
  saved (no pins = auto), the card says "The taskbar stays automatic until you pin a meter
  below.", and the choice stays shown through commits and reconciling until a meter is pinned or
  auto is picked again; unpinning or hiding every pinned meter, or removing their account,
  returns to it.
- **Meters**: "Show them in the flyout, and pin up to two to the taskbar." A grouped card of the
  meters of the services that are on (the duplicates' left out), each with **Show** and **Pin**
  (Pin enabled while the meter is shown, the readout is on and "Meters I pin" is chosen; at most
  two, in the order picked). Hiding a meter hides it for its whole service and unpins it for
  every account. Accessible names name the meter ("Show Claude weekly", "Pin Claude weekly to the
  taskbar"). With no service on: "No meters yet. Turn on a service."

### Notifications / Aviseringar

Everything visible, no expander: "Notify me about my limits" — the existing hint — switch; "When
a limit reaches" [80 %] "and" [95 %] (the regional percent format; dimmed while off); "Also
notify me": used up, resets after a notification, a balance runs low (check boxes, dimmed while
off).

### General / Allmänt

"Start Lastdrop when I sign in to Windows" (switch; what went wrong with it shows under it, §5.3);
"Language" — "Numbers and times follow the regional format in Windows." — list; "Refresh
usage" / "Hämta användningsdata" — "How often Lastdrop reads your services" — list (ends the
"Uppdatering"/"Uppdateringar" clash); "Keyboard shortcut" — "Open the flyout from anywhere" —
switch, and while on a row of Ctrl, Alt, Shift, Win and the key box, with the problem and **Try
again** under it (§5.2); **Updates**: "Check for updates once a day" (switch), "Lastdrop
{version}" with **Check now** (the status stays "Asked. A notification says what was found.").

## 5. Apply model

Windows-11-style **instant apply**: no Apply, no footer; a switch means immediate effect. The
model is `lastdrop_app::settings_edits` (`Edits`); the window only feeds it and shows its draft.

### 5.1 Three states, field by field

1. **Raw editor values**: what an editor shows while the user is still typing (an alias, a
   shortcut being composed). The window owns them; an editor holding such an edit is *dirty*
   until its commit point.
2. **Pending edits** (`Edit`): edits whose commit point has arrived and that are valid, keyed by
   a stable field id (`Field`: `ProviderEnabled(service)`, `HiddenMeter(service, meter)`, `Pins`,
   `ReadoutEnabled`, `Interval`, `Language`, `Levels`, `Hotkey` (the whole shortcut is one
   field), `Alias(SourceKey)`, `AccountAdded(service, folder)`, `AccountRemoved(SourceKey)`, …).
   A later edit of a field replaces an earlier one.
3. **Base**: the settings as last written (as read when the window opened, then as each commit
   wrote them).

The window shows the **draft**: the base with the pending edits applied.

- **Normalised when pending**: a value a combo box cannot show — a language this build has no
  table for (shown as "Same as Windows"), levels the level boxes cannot show (a hand-edited 83),
  an interval between the choices (4 min shows as 5 min) — stays as stored until the user picks
  another choice. The shortcut is parsed and validated (a letter, a digit or F1–F24, trimmed and
  upper case; at least one modifier).
- **Per account**: pending aliases, alias editors and their timers are keyed by `SourceKey`
  (service + account key), never by the account key alone: Claude's and Codex's default accounts
  are both `default`.

### 5.2 Commit points

- Switches, check boxes, combo boxes: pending at once; a 250 ms single-shot timer coalesces a
  burst into one commit.
- Account add and remove, found-folder **Add**, **New account**: pending at once, committed at
  once.
- Alias box: pending on Enter, on focus loss, after 800 ms without a keystroke, when its
  service's row closes and when the page changes; never earlier. An alias commit never touches
  the shortcut editor, and the shortcut never an alias editor.
- Shortcut: pending only when the composition is complete and valid. Incomplete or invalid: an
  inline hint in its card, nothing pending, the registered shortcut stays. A shortcut refused at
  registration (in use) drops out of the commit with the error shown; the editor records the
  refused revision (a counter every shortcut edit bumps). It is tried again only by a newer
  shortcut edit or the card's **Try again** — never by unrelated commits, never by closing.
- API keys: only by an explicit **Save key** (§5.6).

### 5.3 Commit sequence and side effects

Under the settings lock (§6): read the file → start with Windows in the registry when that field
is pending and differs from the file → the hotkey test when that field is pending and differs →
apply **only the pending edits** onto the file as read (nothing else is collected from the
window) → atomic write → base = written → unlock → tell the tray → reconcile (§5.4).

- **Start with Windows** (registry before file): the registry refusing drops the field out of the
  commit (the switch shows the registry's state, the rest is written). When the registry changed
  and the write then fails, the registry change is undone; when the undo fails too, the card says
  so plainly ("Windows will start Lastdrop when you sign in, but the setting couldn't be saved")
  and the switch shows the registry's real state.
- **Hotkey**: refused → drops out (above).
- **Write failure**: the pending edits stay pending (not lost); the InfoBar shows the detail with
  **Try again**; controls keep showing the user's values. A refused shortcut keeps its refusal.
  A **New account** whose write failed is dropped with the folder it just made (it would be
  saved later without its folder); **Remove** on an account whose adding is still pending takes
  the adding back.
- Independent actions are reported apart from configuration, in their own rows: "Key saved"
  (Credential Manager), "Opened {client}'s sign-in in a terminal"; "Account added" only after its
  commit succeeded.

### 5.4 Reconcile after a commit

The merge may bring in changes the tray made meanwhile. After each successful commit every
control whose value differs from the base is set to it **unless it holds an active uncommitted
edit** (a dirty raw editor, or a field pending again). Keyboard focus is not the criterion: a
focused combo box without an edit is updated and keeps the focus. Protected editors reconcile when
their own edit commits; the user's newer edit then wins (it merges as a change from the base).

Account rows are updated in place by `SourceKey` (`set_row_data` of the changed rows); the
account model is never replaced while an alias editor holds a raw edit, so caret, selection and
IME composition survive.

### 5.5 Closing

Close (✕, Alt+F4, Escape) flushes: dirty alias editors become pending; a complete valid
shortcut whose revision was never tried becomes pending (a refused revision is not tried again:
the shortcut that works stays); the 250 ms queue runs; the commit runs at once.

- Success, or nothing pending: the window closes.
- Write failure: the window stays open with the InfoBar ("Couldn't save the settings: …") and two
  actions, **Try again** (which flushes the editors again, then closes once the write works) and
  **Close without saving**.
- An incomplete or invalid shortcut never blocks closing; it is dropped, as its hint said.

### 5.6 Secrets

No API key is ever in a property, the draft, the pending set, a model or the log. **Save key**
stores it in Credential Manager at once and tells the tray (`Outside::store_key`,
`Changes::key_of`; **Remove** is `Outside::remove_key`, so tests and previews use an in-memory
store that keeps the credential names only); the password box is cleared after Save, when its
expander closes and when the page changes.

### 5.7 Tests

`settings_edits` tests: the normalisations (unknown language, odd levels, interval, shortcut
parse); a commit writes only the pending fields onto the file as it is; an alias committed by its
pause while a shortcut is being composed (alias saved, shortcut not); an account action does not
commit an alias being typed; a refused shortcut is not tried again by an unrelated commit or by
closing, only by Try again or a newer edit, and not on closing after a commit whose write failed;
an account whose adding is still pending is taken back by Remove or dropped; aliases of Claude's and Codex's `default` accounts
persist apart; reconciling shows the file's values but leaves a dirty alias editor alone; a failed
write keeps the edits pending; start with Windows is undone when the write fails, and an undo that
fails leaves the switch at the registry's state; pins are chosen, promoted, unpinned by hiding a
meter or removing its account.

## 6. Settings write coordination

Three writers change `settings.json`: the **tray process** (pause and resume, a meter pinned from
a card menu, readout auto or off from the tray menu, the first run's save), the **settings
window** and **Theme Studio** ("Save and use"). The settings window writes often (every commit,
§5), so a write that replaces the whole file with an older picture of it would lose another
writer's change.

### 6.1 The lock and the merge

- **Lock**: the named mutex `Local\Lastdrop.Settings` (`lastdrop_win::named_lock`), held for one
  read-merge-write and nothing else: nobody waits for another thread or process while holding it.
  A writer that cannot have it within 5 s writes without it (logged): losing the write would be
  worse than the race. An abandoned mutex (its process ended while holding it) is taken by the
  next writer.
- **Read-merge-write** (`SettingsFile::update`): under the lock, read the file as it is now
  (missing or unreadable: the writer's own starting point; a broken file is set aside first, so
  the write never overwrites it), apply the writer's change onto it, write atomically.
- **Merge** (`apply_onto(base, new, disk)`): what `new` changed from `base` (the settings the
  change started from), applied onto `disk`; every other setting keeps the file's value. When two
  writers changed the same setting, the later write wins. The merge works on the JSON form:
  objects merge key by key while their keys match, arrays and scalars are single values.

### 6.2 The tray process

- **Rebasing jobs**: every tray save is queued for the tray's writer thread as (the settings as of
  its previous save or reload, its settings now), never as bytes. The writer thread rebases it
  when the job runs: under the lock, onto the file as it is then (`TraySync::save`). So a tray
  save queued before a settings window commit but run after it keeps the commit.
- **Re-read after the dialog's write**: on `Lastdrop.SettingsChanged` (settings flag) the tray
  queues a **barrier** job (`TraySync::barrier`) and returns: nothing on the UI thread waits for
  the writer (`WriterHandle::flush` is for tests and shutdown only). When the writer thread
  reaches the barrier, every tray save queued before the message is on disk; the thread reads the
  file and posts the read, with the barrier's token, to the tray's event loop.
- **Reload as a rebase** (`TraySync::reload`): the tray applies the file it read with its own
  changes made since the barrier was queued rebased onto it (they are queued after the barrier, so
  the file may lack them; they survive). Only the latest barrier reloads: an earlier one that a
  later message superseded does nothing. The tray keeps its own pause (the tray menu owns it).
  A missing or unreadable file is ignored.

### 6.3 The dialog process

- The settings window's commit (§5) and Theme Studio's "Save and use" (only the theme changes) do
  their read-merge-write on the dialog process's main thread, then post
  `Lastdrop.SettingsChanged`.

### 6.4 Tests

- The blocked-writer race (`settings_sync` tests): the tray's writer is blocked (the writer's
  test-only `Block` job); the tray pauses (queued); the settings window commits a different
  change; the tray turns the readout off while the barrier is pending; the writer is released →
  the pause, the window's change and the readout change are all on disk and in the tray's state,
  and the tray's next save keeps them.
- The barrier fires only after the earlier writes and reads what they wrote; a superseded barrier
  does not reload.
- Theme Studio waits for the lock and keeps what the lock's holder wrote meanwhile.

## 7. State, navigation and deep links

- One session-wide set of edits (`Edits`) for the window; per-page scroll positions and the
  service rows' expansion (Rust's, in `ProviderRow.expanded`) are kept while the window is open.
- **Navigation**: one Tab stop into the pane; Up, Down, Home and End move the selection (the
  page changes with it and the focus stays in the pane); Ctrl+Tab and Ctrl+Shift+Tab cycle the
  pages from anywhere in the window and put the focus in the pane (the focused control may be on
  the page just hidden). A combo box's open list keeps these keys to itself. Escape closes the
  window (§5.5). A page change is the commit point of every alias box (§5.2), and it clears a key
  typed but not saved.
- **Deep links** = select the page → expand → scroll → keyboard focus → highlight
  (`settings_window::reveal`). Focusing an item scrolls it into view (Slint).
  - `Focus::Pins` (tray menu "Taskbar readout ▸ Pinned meters…", `--settings-pins`): the readout
    on and "Meters I pin" (the choosing state when no pin is saved), committed at once — the menu
    choice is the user's intent; the Display page scrolled to the taskbar readout; the first
    shown meter's **Pin** focused; the taskbar content card and the meters outlined in the accent
    until the page changes.
  - `Focus::Accounts` (the first run found more folders, `--settings-accounts`): the Services
    page; the rows of Claude and Codex that have found folders expanded; the first of them
    scrolled to the top and its first found folder's **Add** focused.
  - A repeated request to the open window keeps every edit and only reveals again.

## 8. Keyboard and accessibility

- Tab order: the pane, then the page shown, top to bottom; nothing on a hidden page or in a
  closed expander.
- Every switch, list and box has an accessible name from its card ("Show usage in the taskbar",
  "Alias of Work", "Remove account Work", "Pin Claude weekly to the taskbar", "Save the Meshy API
  key"); a found folder's **Add** names its folder; an expander's chevron what it shows ("Claude
  accounts", "Meshy API key").
- When the row holding the focus goes, the focus moves on in its block: after **Remove** to the
  next account's alias box, after a found folder's **Add** to the next found folder's Add, else to
  **Add folder…** (also when the write failed: the adding stays pending and its row is gone);
  after an API key's **Remove** back to its box. The target is named by the row's data (the
  account key, the folder), not its index, which the rows after it keep until Slint's repeater
  catches up; the found folders, like the accounts, are updated in place (by folder), so the
  other rows, and the handlers that move the focus, stay.
- Focus rectangles on the pane and the expander's chevron only after keyboard use.
- The InfoBar is a live region; the expander's chevron reports expanded or collapsed.
- Contrast themes: §3.3.

## 9. Verification

- **Previews**: `lastdrop.exe --demo --preview-settings <png>` renders each page's viewport —
  `services`, `display`, `notifications`, `general` — and four states — `accounts` (the
  accounts deep link), `keys` (Meshy's and OpenCode Go's API keys expanded), `pins` (the pins
  deep link, choosing), `errors` (General with a refused shortcut and the InfoBar of a write that
  failed on closing) — at 880 × 680 and 680 × 520 into `<stem>-<view>-<w>x<h>.png` next to
  `<png>`. With `--language sv`, `--dark`, `--contrast` (Windows' "Night sky" contrast colours)
  and `--scale`. No files are read or written: the demo's invented accounts, the described
  services Lastdrop ships (turned off) and an in-memory `settings.json` and key store.
- **Interaction test** (`crates/app/tests/settings_ui.rs`, headless, the test's own clock): the
  pane's keys and Ctrl+Tab; Tab from the pane lands on the page shown; a burst of edits on two
  pages commits once, 250 ms after its last edit, the registry before the file; a change the
  tray wrote meanwhile shows after the next commit in a focused combo box without an edit, which
  keeps the focus; an alias commits after its 800 ms pause, not before; both deep links (page,
  expansion, focus — Space then acts on the focused control —, highlight, choosing; a repeated
  request keeps an edit); an account added and removed at once, "Account added.", the focus
  moving on (after Remove to Add folder…, where Tab and Space open New account… and Cancel closes
  it, or into the next alias box, where typing saves an alias; with two found folders Space on the
  first Add, then Space again, adds both); a described service's row (key) expands to its key
  row from the keyboard (Tab to its chevron, Space, Tab into the box), Enter saves the key, turns
  the service on and changes its line, Remove takes the key back; the expansion survives a page
  change; every configured folder listed, a duplicate marked "Same account as …" and its meters
  left out of the meter list; a failed write on closing keeps the window open with the InfoBar,
  Try again closes it once the write works, Close without saving discards.
- Unit tests: §5.7 and §6.4.
- **Manual**: keyboard only, Narrator, 100/150/200 % scaling, Windows text size 150 %, a
  contrast theme.

## 10. Later phases

- **Display**: two pin slots ("First meter", "Second meter (optional)") instead of the Pin boxes,
  with the choosing state, no duplicates, promotion of the second slot and unavailable pins kept;
  meter visibility per service with short names (`5-hour limit`); appearance cards with **Open
  themes folder**; readout-spec R§10's wording.
- **Polish**: the shortcut editor with a live preview ("Ctrl + Alt + L"), the version card's
  result, a Narrator and DPI pass, the preview CLI's documentation.
