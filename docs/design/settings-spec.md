# Lastdrop — the settings window

Implementation spec (`S§`). Companions: [accounts-spec.md](accounts-spec.md) (accounts-spec §7,
the accounts in the window), [readout-spec.md](readout-spec.md) (`R§`; R§4.3 pinned meters,
R§10 the tray menu's deep link), [notifications-spec.md](notifications-spec.md) (`N§2`),
[themes-spec.md](themes-spec.md) (Theme Studio). Code: `lastdrop_app::settings_edits` (§5),
`lastdrop_app::settings_sync` and `lastdrop_win::named_lock` (§6).

The settings window runs in the dialog process (ARCHITECTURE "Processes"). This spec grows with
the window's redesign: §5 is its change model, §6 the write coordination instant apply needs.

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
- Alias box: pending on Enter, on focus loss, after 800 ms without a keystroke, when its section
  collapses and when the page changes; never earlier. An alias commit never touches the shortcut
  editor, and the shortcut never an alias editor.
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
  **Try again**; controls keep showing the user's values.
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
  actions, **Try again** and **Close without saving**.
- An incomplete or invalid shortcut never blocks closing; it is dropped, as its hint said.

### 5.6 Secrets

No API key is ever in a property, the draft, the pending set, a model or the log. **Save key**
stores it in Credential Manager at once and tells the tray (`Changes::key_of`); the password box
is cleared after Save.

### 5.7 Tests

`settings_edits` tests: the normalisations (unknown language, odd levels, interval, shortcut
parse); a commit writes only the pending fields onto the file as it is; an alias committed by its
pause while a shortcut is being composed (alias saved, shortcut not); an account action does not
commit an alias being typed; a refused shortcut is not tried again by an unrelated commit or by
closing, only by Try again or a newer edit; aliases of Claude's and Codex's `default` accounts
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
