# Lastdrop — the settings window

Implementation spec (`S§`). Companions: [accounts-spec.md](accounts-spec.md) (accounts-spec §7,
the accounts in the window), [readout-spec.md](readout-spec.md) (`R§`; R§4.3 pinned meters,
R§10 the tray menu's deep link), [notifications-spec.md](notifications-spec.md) (`N§2`),
[themes-spec.md](themes-spec.md) (Theme Studio). Code: `lastdrop_app::settings_sync` (§6),
`lastdrop_win::named_lock` (§6).

The settings window runs in the dialog process (ARCHITECTURE "Processes"). This spec grows with
the window's redesign; §6 is the write coordination its instant apply needs.

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
