# Lastdrop spike: Tauri 2 + WebView2 (bench notes for the measurer)

Stack: Tauri **2.12.2** (latest stable 2.x; 3.0 is alpha), tauri-build 2.7.1, wry 0.57.0, tao 0.37.1,
tray-icon 0.25.1, webview2-com 0.39.1, windows 0.62.2. Frontend: plain HTML + CSS + vanilla JS
(`dist/`, ~15 KB, no framework, no bundler), embedded in the exe (brotli) and served over Tauri's
custom protocol. WebView2 runtime on this machine: **154.0.4258.62** (Evergreen).

## Exe

```
spikes\tauri-flyout\target\release\lastdrop-tauri.exe
```

- Size: 3,155,456 bytes (3.0 MiB), single file. The WebView2 loader is statically linked and the
  VC runtime is static (imports only system DLLs and UCRT), so nothing else is needed on Windows 11.
- Release profile: `opt-level = "s"`, `lto = true`, `codegen-units = 1`, `strip = true`, `panic = "abort"`,
  `#![windows_subsystem = "windows"]`.
- Rebuild (Git Bash): `export PATH="$HOME/.cargo/bin:$PATH"; cd <spike>/tauri; cargo build --release`
  (no Tauri CLI needed: tauri-build in build.rs + the `custom-protocol` feature).
  Clean build 148 s; incremental (app crate + LTO) about 45 s.

## Commands

| variant | command |
|---|---|
| keepalive | `lastdrop-tauri.exe --bench C:\path\keepalive.json --mode keepalive` |
| destroy | `lastdrop-tauri.exe --bench C:\path\destroy.json --mode destroy` |
| *(optional extra)* keepalive + lowmem | `lastdrop-tauri.exe --bench C:\path\keepalive-lowmem.json --mode keepalive --lowmem` |

- `--mode keepalive` (the default): at T0 the window and webview are created **hidden** and the page
  loads. Hide = `SW_HIDE` + `ICoreWebView2Controller.IsVisible = false` (so WebView2 stops rendering
  and can trim memory). Show = `IsVisible = true` + `SW_SHOWNOACTIVATE`.
- `--mode destroy`: no window exists until T0+10s. Show builds the WebviewWindow and shows it when
  the page reports ready. Hide = `SW_HIDE` + `window.destroy()` (closes the controller and releases
  the environment). **The WebView2 browser process and all its children do exit after each destroy**:
  in smoke runs, 6 processes went to 0 within at most 2.4 s (see `webviewProcesses` in the JSON).
- `--lowmem` (keepalive only, extra): on hide it also sets `MemoryUsageTargetLevel = LOW`
  (ICoreWebView2_19) and calls `TrySuspend` (ICoreWebView2_3). On show it sets the level back to
  NORMAL; making the webview visible resumes it. Worth measuring because it is the knob a real
  keepalive tray app would use.
- `--theme light|dark` forces the theme (default: follows `AppsUseLightTheme`). Useful for screenshots.
- `--bench-scale <f>` compresses the timeline. **Smoke tests only; do not use for measurements.** Below about 0.3 the destroy
  variant can miss the warm show, because the hide arrives before the rebuilt webview has painted.
- Without `--bench` it is the interactive tray app: left-click toggles the flyout, it hides on
  deactivate or Esc, and right-click shows a menu with Quit.

## Timeline (T0 = process creation time from GetProcessTimes)

| T0+ | action |
|---|---|
| 0 | tray icon created (runtime-drawn 32x32 ring at 42%); keepalive: window precreated hidden |
| 10 s | show **without activation** (`SW_SHOWNOACTIVATE`; `focusStolen` in the JSON confirms it); auto-hide on deactivate is off in bench mode |
| 12 s | **partial JSON written** with `flyoutRectPx` (rewritten again at 28 s and 32 s) |
| 20 s | hide (keepalive) / destroy (destroy) |
| 30 s | show again ("warm") |
| 40 s | hide / destroy |
| 50 s | tray icon removed, final JSON (`"complete": true`), `app.exit(0)` (exit code 0) |

The app samples its own process tree with a Toolhelp snapshot (under 5 ms) at T0+0.5, 9.5, 18, 28,
38 and 48 s. Avoid taking memory samples at exactly those instants.

### First launch after a build is about 11 s late, so do a warm-up run

Each time the exe is rebuilt or copied, the **first** launch spends about 11 s between process
creation and `main()` (consistent with an AV/Defender first-run scan); WebView2 startup is also
slower on that run. When `main()` starts more than 2 s after process creation, the app **rebases T0
to `main()` entry** so the timeline still works. The JSON then has `"t0Rebased": true`, `t0EpochMs`
holds the rebased value, and `processStartEpochMs` holds the real creation time. **Before measuring,
do one untimed launch** (for example `--bench x.json --bench-scale 0.1`, about 5 s plus the scan), then check
`t0Rebased == false` in the real runs.

## Output JSON

Required fields: `variant` (`tauri-keepalive`, `tauri-destroy`, `tauri-keepalive-lowmem`), `pid`,
`t0EpochMs`, `phases[{name, epochMs}]`, `showLatencyMs{cold, warm}`, `flyoutRectPx{x,y,w,h}`,
`backdrop` ("acrylic"), `renderer`, `notes`.
Extras: `complete`, `t0Rebased`, `processStartEpochMs`, `showDetail[]` (`windowBuildMs`, `pageReadyMs`,
JS-side timings), `webviewProcesses[]` (own msedgewebview2 descendants at each sample),
`clientRectPx`, `windowRectPx`, `viewport` (CSS px, DPR, overflow), `positioning`, `dpi`,
`windowExStyle`, `focusStolen`, `theme`, `accent`, `accentSource`, `webview2Version`,
`userDataFolder`, `optLevel`.
The file is written to `<out>.json.tmp` and then renamed over `<out>.json`, so a reader never sees a half-written file.

- **Latency** = Rust `Instant` at the show call → Rust receipt of the `frame_presented` IPC. The page
  sends it from the 2nd `requestAnimationFrame` callback after `document.visibilityState` becomes
  `visible`, so the time includes one IPC hop of about 1 ms. In destroy mode the clock starts before
  `WebviewWindowBuilder::build()`, so it includes WebView2 browser-process startup, page load and the
  first frame.
- **`flyoutRectPx`** = `DWMWA_EXTENDED_FRAME_BOUNDS`, the visible flyout including DWM's 1 px border:
  **362x442** at 100% scaling. The web content (client area) is exactly 360x440 logical px, and
  `clientRectPx` is that area, inset 1 px. `windowRectPx` is larger (376x449) because tao keeps
  invisible resize borders; ignore it. Screenshot `flyoutRectPx`.
- Placement: `Shell_NotifyIconGetRect` (via `TrayIcon::rect()`) → `CalculatePopupWindowPosition`
  (TPM_CENTERALIGN | TPM_BOTTOMALIGN | TPM_VERTICAL | TPM_WORKAREA), with the icon rect as the
  exclusion rect and a 12 px (scaled) gap above the taskbar. Falls back to the cursor position when
  the icon is hidden in the overflow area. On this machine the icon is at (4798,1392) on a 96-DPI
  monitor and the flyout lands at client (4634,940) 360x440.

## Processes belonging to the app (sum all of them)

`lastdrop-tauri.exe` **plus every `msedgewebview2.exe` that descends from its PID**. Other apps on
this machine also run msedgewebview2.exe, so never match by name alone. The tree is:

```
lastdrop-tauri.exe (pid P)
└─ msedgewebview2.exe            browser          (ppid = P)
   ├─ msedgewebview2.exe         crashpad-handler
   ├─ msedgewebview2.exe         gpu-process
   ├─ msedgewebview2.exe         utility  network.mojom.NetworkService
   ├─ msedgewebview2.exe         utility  storage.mojom.StorageService
   └─ msedgewebview2.exe         renderer
```

Walk ParentProcessId from P (Win32_Process / Toolhelp). In destroy mode the whole subtree is gone
while hidden, and only the exe remains. The browser exits about 1–2 s after the destroy, not
instantly, so sample a few seconds after T0+20/T0+40.

**WebView2 user data folder:** `%LOCALAPPDATA%\se.twofivers.lastdrop.spike\EBWebView`
(`%USERPROFILE%\AppData\Local\se.twofivers.lastdrop.spike\EBWebView`, about 8.5 MB after the smoke runs).
Deleting it simulates a first-ever launch (cold profile creation). Keep it to measure steady state.

## Smoke-test reference numbers (time-compressed runs, NOT the measurement)

| run | cold | warm | notes |
|---|---|---|---|
| keepalive (scale 0.25) | 43.0 ms | 13.4 ms | page ready hidden at T0+0.35 s |
| keepalive --lowmem (0.2) | 20.6 ms | 11.9 ms | resume after TrySuspend OK |
| destroy (0.3, rebased) | 398 ms | 355 ms | window build 264 / 244 ms, page ready +35 ms, first frame +60–80 ms |

All runs: `backdrop = acrylic`, `focusStolen = false`, `viewport = 360x440@1`, `overflowPx = 0`,
exit code 0, no processes left behind.
