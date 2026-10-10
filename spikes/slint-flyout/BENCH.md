# Lastdrop flyout spike: Rust + Slint 1.18.1 (keep-alive only)

Spike folder: `spikes\slint-flyout`
(below: `$SPIKE`). Every exe is in `$SPIKE\dist\`.

The flyout window is created once at startup (off-screen, not activated), hidden, then shown and hidden
again with Win32 `ShowWindow`. There are no destroy variants.

## Variants to measure

Use one exe per renderer. Each exe contains only that renderer's code and DLL imports, so RAM
numbers aren't skewed by unused renderers. The all-in-one exe (`lastdrop-slint-all.exe`) statically
imports `d3d12.dll` and `opengl32.dll` and loads both even when it runs the software renderer.
Don't use it for RAM numbers.

Always set `SLINT_BACKEND` explicitly. If it is unset, the exe falls back to its default renderer.

| variant name (`--variant`)      | exe                                   | environment                                                        | backdrop |
|---------------------------------|---------------------------------------|--------------------------------------------------------------------|----------|
| `slint-keepalive-software`      | `lastdrop-slint-software.exe`         | `SLINT_BACKEND=winit-software`                                     | acrylic works |
| `slint-keepalive-femtovg`       | `lastdrop-slint-femtovg.exe`          | `SLINT_BACKEND=winit-femtovg` (OpenGL/WGL)                         | acrylic works, but content intermittently missing (see below) |
| `slint-keepalive-femtovg-wgpu`  | `lastdrop-slint-femtovg-wgpu.exe`     | `SLINT_BACKEND=winit-femtovg-wgpu`, `WGPU_BACKEND=dx12`            | acrylic works |
| `slint-keepalive-skia-opengl`   | `lastdrop-slint-skia.exe`             | `SLINT_BACKEND=winit-skia-opengl`                                  | acrylic works, but content intermittently missing (see below) |
| `slint-keepalive-skia-software` | `lastdrop-slint-skia.exe`             | `SLINT_BACKEND=winit-skia-software`                                | acrylic works |
| `slint-keepalive-skia-wgpu` (only as a data point) | `lastdrop-slint-skia.exe` | `SLINT_BACKEND=winit-skia-wgpu` (wgpu → D3D12 + DirectComposition) | **broken: opaque black background** |

* In this build `SLINT_BACKEND=winit-skia` means Skia on OpenGL, the same as `winit-skia-opengl`.
  Compiling in `renderer-skia-opengl` makes OpenGL Skia's preferred surface. In a build with only
  `renderer-skia`, `winit-skia` is wgpu → D3D12.
* `winit-skia-wgpu` (Skia on wgpu → D3D12) renders, but the window background is opaque black: no
  backdrop, and the dark header and footer text is unreadable. This is a bug in Slint 1.18.1 + wgpu
  30. Slint first configures the swapchain with the default alpha mode, then
  switches it to premultiplied with a second `configure`. wgpu's DX12 backend applies that second
  call with `ResizeBuffers`, which cannot change the alpha mode, so the alpha mode never changes
  (`i-slint-renderer-skia-1.18.1/wgpu_30_surface.rs` `set_transparent`,
  `wgpu-hal-30.0.1/src/dx12/mod.rs` `configure`). FemtoVG-on-wgpu sets the alpha mode in its first
  `configure`, so it works.
* Without `WGPU_BACKEND=dx12`, `femtovg-wgpu` also starts a Vulkan instance (vulkan-1.dll and the
  NVIDIA ICD get loaded). It still works, but uses more memory for nothing. Pin it to dx12.
* The exe sets `WGPU_DX12_PRESENTATION_SYSTEM=Visual` itself unless the variable is already set. The
  wgpu renderers need a DirectComposition swapchain for per-pixel alpha.

## Commands

PowerShell. The exes are GUI-subsystem apps, so use `Start-Process -Wait` to block until exit:

```powershell
$SPIKE = "spikes\slint-flyout"
$OUT = "$SPIKE\bench"; New-Item -ItemType Directory -Force $OUT | Out-Null

# software
$env:SLINT_BACKEND = "winit-software"
$p = Start-Process "$SPIKE\dist\lastdrop-slint-software.exe" -ArgumentList "--bench `"$OUT\slint-software.json`" --variant slint-keepalive-software" -PassThru
# ... measure $p.Id ... ; $p.WaitForExit()

# femtovg (OpenGL)
$env:SLINT_BACKEND = "winit-femtovg"
$p = Start-Process "$SPIKE\dist\lastdrop-slint-femtovg.exe" -ArgumentList "--bench `"$OUT\slint-femtovg.json`" --variant slint-keepalive-femtovg" -PassThru

# femtovg on wgpu / D3D12
$env:SLINT_BACKEND = "winit-femtovg-wgpu"; $env:WGPU_BACKEND = "dx12"
$p = Start-Process "$SPIKE\dist\lastdrop-slint-femtovg-wgpu.exe" -ArgumentList "--bench `"$OUT\slint-femtovg-wgpu.json`" --variant slint-keepalive-femtovg-wgpu" -PassThru
Remove-Item Env:WGPU_BACKEND

# skia on OpenGL
$env:SLINT_BACKEND = "winit-skia-opengl"
$p = Start-Process "$SPIKE\dist\lastdrop-slint-skia.exe" -ArgumentList "--bench `"$OUT\slint-skia-opengl.json`" --variant slint-keepalive-skia-opengl" -PassThru

# skia, CPU raster
$env:SLINT_BACKEND = "winit-skia-software"
$p = Start-Process "$SPIKE\dist\lastdrop-slint-skia.exe" -ArgumentList "--bench `"$OUT\slint-skia-software.json`" --variant slint-keepalive-skia-software" -PassThru

Remove-Item Env:SLINT_BACKEND
```

Git Bash form: `SLINT_BACKEND=winit-software "$SPIKE/dist/lastdrop-slint-software.exe" --bench out.json --variant slint-keepalive-software`

Run variants one at a time. Two instances would put two tray icons and two flyouts in the same spot.

## Bench timeline (T0 = process creation time, from `GetProcessTimes`)

| time   | what happens |
|--------|--------------|
| T0     | tray icon created (≈ +20-200 ms); flyout created off-screen, first frame rendered, then hidden (`phases: windowPrecreatedHidden`) |
| +10 s  | positioned and shown with `SW_SHOWNOACTIVATE` (no focus steal), auto-hide-on-deactivate is off in bench mode |
| +12 s  | **partial json written** (`"partial": true`), including `flyoutRectPx`, so the screenshot can be taken now |
| +20 s  | hidden (`SW_HIDE`) |
| +30 s  | shown again (warm) |
| +32 s  | partial json updated (warm latency included) |
| +40 s  | hidden |
| +50 s  | tray icon removed (`NIM_DELETE`), event loop quits, final json (`"partial": false`), exit code 0 |

The json is written atomically (`out.json.tmp`, then rename). The renderer is ready this long after
process start (`windowPrecreatedHidden` phase):

* software: ≈ 0.08-0.3 s
* OpenGL (femtovg, skia-opengl): ≈ 0.4-0.5 s
* wgpu/D3D12 (femtovg-wgpu): ≈ 0.7-1.9 s

Indicative show latencies from compressed smoke runs on this PC (not measurements):

| renderer | cold | warm |
|----------|------|------|
| software | 3-4 ms | 3-4.5 ms |
| femtovg | 1.7-2 ms | 2-3 ms |
| skia-opengl | 2-3 ms | 2-3 ms |
| skia-software | 3-5 ms | 2.5-4 ms |
| femtovg-wgpu | 7-8 ms | 6.5-9 ms |

If the window isn't ready at +10 s, the show is retried every 20 ms.

## JSON fields

```
variant, pid, t0EpochMs, partial,
phases: [{name, epochMs}]  processStart, mainEntered, trayCreated, windowPrecreatedHidden,
                            show1, firstFramePresented1, hide1, show2, firstFramePresented2, hide2, exit
showLatencyMs: {cold, warm}
showLatencyAfterRenderingNotifierMs: {cold, warm}
flyoutRectPx: {x, y, w, h}       physical px, DWM extended frame bounds (= visible window)
trayIconRectPx: {x, y, w, h}     Shell_NotifyIconGetRect result (the overflow chevron if the icon is hidden)
backdrop: "acrylic" | "mica" | "none"   what was applied with DwmSetWindowAttribute (see caveats)
renderer: "<SLINT_BACKEND> [notifier: OpenGL|non-GL; modules: d3d12+dcomp+opengl32+...]"
debugCounts: {wmPaint, afterRenderingNotifier}
slintVersion, notes
```

* `showLatencyMs`: from the show call (just before positioning) until the first `WM_PAINT` after the
  show returns. Slint renders and presents inside that `WM_PAINT`: winit dispatches
  `RedrawRequested` synchronously, Slint draws, then software `BitBlt`s or GL/DXGI calls
  `SwapBuffers`/`Present`. This is the primary number, and it is the same method for every renderer.
* `showLatencyAfterRenderingNotifierMs`: Slint's `AfterRendering` notifier, which fires just before
  present. Only FemtoVG (GL and wgpu) and Skia-OpenGL call it. The software renderer doesn't
  support notifiers (`null`), and Skia-wgpu never called it in my tests.
* `modules`: the GPU DLLs loaded in the process at the first frame. It tells you which graphics API
  was actually used.

## Things the measurer should know

* **Transparency effects are OFF on this PC.** The registry value is
  `HKCU\...\Themes\Personalize\EnableTransparency=0`. DWM therefore draws the acrylic backdrop's
  solid fallback colour: about `#D2D2D2` in light mode and `#545454` in dark mode. Mica's fallback
  is `#F2F2F2`. With transparency effects off, the system flyouts are solid too. To see real blur,
  turn on Settings → Personalization → Colors → Transparency effects. I did not change it.
* Per-pixel alpha was proven separately: with `--backdrop none` the window is see-through to the
  desktop behind it, and the cards stay translucent.
* **The OpenGL variants (`winit-femtovg`, `winit-skia-opengl`) are unreliable on this PC.** In
  about 4 of 14 runs the flyout showed its full content. In the others it showed only the backdrop,
  with no content at all. FemtoVG/Skia still rendered every frame (the `AfterRendering` notifier
  fired) and `WM_PAINT` returned normally.
  * Both capture methods saw the empty window: a GDI screen copy and `PrintWindow(PW_RENDERFULLCONTENT)`.
  * It happened with the display on and off, with `SW_HIDE` and with DWM cloaking (`--hide-mode cloak`).
  * Good and bad runs used builds that differ only in unrelated lines.
  * My guess is NVIDIA's OpenGL present path on a DWM-backdrop window (layered on DXGI vs. GDI
    blit). Not resolved.
  * Software, Skia-software and FemtoVG-wgpu showed content in every run.
  * Reference captures where GL worked: `smoke\reference\femtovg-gl-display-on.png` and
    `smoke\reference\skia-opengl-display-on.png`.
  * Measure the GL variants if you want the numbers, but **check each screenshot for content**,
    and treat them as not shippable until this is understood.
* The window is 360 × 440 logical px, so 360 × 440 physical at this monitor's 100 % scale. On this
  machine it sits at x=4634, y=940. It is centred above the tray icon, or above the overflow chevron
  when Windows has hidden the icon there (the default for new icons). Its bottom edge is 12 px above
  the taskbar, and it is clamped at least 12 px inside the work area.
* The content is taller than 440 px. The cards sit in a scroll view (Fluent overlay scrollbar), so
  the Meshy card is below the fold, as it would be in the real fixed-height flyout.
* Bench mode never activates the window. `WM_NCACTIVATE` is answered as "active" so DWM draws the
  backdrop in its active state. Turn that off with `--no-active-look`. It made no visible difference
  with transparency effects off.
* If the process is killed instead of reaching T0+50 s, the tray icon stays as a ghost until the
  mouse passes over it.
* The first launch of a freshly built exe can spend about 2 s between process creation and `main`
  (`mainEntered` phase), probably the Defender first-run scan. Later launches take 30-300 ms. Do one
  throw-away launch of each exe before measuring.

## Optional flags

```
--bench <out.json>        bench mode (required for measurement)
--variant <name>          label written to json (default slint-keepalive-<renderer>)
--backdrop acrylic|mica|none   DWMSBT_TRANSIENTWINDOW (default) | DWMSBT_MAINWINDOW | DWMSBT_NONE
--theme light|dark        testing: override AppsUseLightTheme
--no-active-look          testing: let DWM draw the inactive backdrop
--timescale <f>           smoke tests only: compresses the timeline (invalidates the measurement)
--dump-icon <base>        writes <base>-light-taskbar.png / -dark-taskbar.png (the runtime tray icon) and exits
--redirection-bitmap off  experiment: WS_EX_NOREDIRECTIONBITMAP (DirectComposition-only window)
--single-position         testing: skip the second positioning pass
--height <logical px>     screenshots only: taller window so all three cards show (spec size is 440)
--hide-mode hide|cloak    hide (default): SW_HIDE / SW_SHOWNOACTIVATE
                          cloak: window stays shown, DWMWA_CLOAK toggled, parked off-screen when hidden
```

`smoke\tall-software.png` was taken with `--height 700` and shows all three cards and both sparklines.

Without `--bench` the exe runs as the real tray app:

* left-click the tray icon to toggle the flyout (it is activated, so focus loss hides it)
* Esc hides it
* right-click opens a menu with Quit

## Build

`tools/build-all.sh [all software femtovg skia femtovg-wgpu software-s femtovg-s]` builds each
variant and copies it to `dist/lastdrop-slint-<variant>.exe`. Sizes and times are appended to
`dist/build-times.txt`.

* `[profile.release]`: `opt-level=3`, `lto=true`, `codegen-units=1`, `strip=true`, `panic="abort"`.
* `[profile.release-s]`: the same with `opt-level="s"`, used only to report the size difference.
* `windows_subsystem="windows"`, plus an embedded manifest (PerMonitorV2, Windows 10/11
  supportedOS).

Release exe sizes (stripped):

| exe | opt-level 3 | opt-level "s" |
|-----|-------------|---------------|
| `lastdrop-slint-software.exe` | 10,432,512 B (9.9 MiB) | 9,020,928 B (8.6 MiB) |
| `lastdrop-slint-femtovg.exe` | 10,176,000 B (9.7 MiB) | 8,937,472 B (8.5 MiB) |
| `lastdrop-slint-femtovg-wgpu.exe` | 14,522,368 B (13.8 MiB) | - |
| `lastdrop-slint-skia.exe` | 18,211,840 B (17.4 MiB) | - |
| `lastdrop-slint-all.exe` | 19,907,584 B (19.0 MiB) | - |

Build times on the i9-14900KF:

* First build with all renderers: 167 s. Crates were already fetched; skia-bindings downloads
  prebuilt Skia binaries during the build.
* First build of each single-renderer variant: 82-107 s.
* After a `main.rs` / `.slint` change: 44-82 s, almost all of it the LTO link with
  `codegen-units=1`.

Smoke test helper: `tools\smoke.ps1 -Exe <exe> -Backend <SLINT_BACKEND> -Tag <name> -Timescale 0.2`
captures only the flyout's own rectangle and kills the process by PID if it overstays.
