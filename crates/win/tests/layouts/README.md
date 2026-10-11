# Recorded taskbar layouts

The readout's placement is a pure function, `lastdrop_win::placement::place(geometry, size,
prefs)`, unit-tested against this folder (ARCHITECTURE "Readout placement"; ROADMAP "Bug-free by
construction": bottom/top/left/right, centred/left, 100–200 % DPI, 1–3 monitors, auto-hide,
overflow open). `tests/placement_layouts.rs` runs every `*.json` file here.

One file per case:

```json
{
  "name": "…",                  // file name without .json
  "description": "what the case shows and why the expected result follows",
  "synthetic": true,            // false for a geometry recorded on a real machine
  "geometry": { … },            // lastdrop_win::placement::TaskbarGeometry, device px
  "layout": { … },              // lastdrop_core::readout::ReadoutLayout at the geometry's DPI
  "prefs": { "content": "TwoRows", "previous": "TwoRows" },
  "expected": { "Docked": { "rect": { … }, "form": "TwoRows" } }
}
```

`expected` is a `Placement`: `{"Docked": {"rect", "form"}}`, `{"Floating": {"rect", "form"}}`
or `{"Hidden": "NoRoom" | "Off" | "FullScreen" | "AutoHide" | "VerticalTaskbar"}`.

`synthetic-*` files are hand-made; their `layout` values were measured with DirectWrite (en
strings, Claude `5h`/`Wk`, with and without the glyph column) at 100/125/150/175/200 %, and their
`expected` placements follow the spec's formulas (R§4.2, R§6.1). The matrix covers: every DPI step
from 100 to 200 %; bottom, top, left and right taskbars; centred and left-aligned buttons; one to
three monitors, including negative and offset coordinates; auto-hide (hidden and shown);
full-screen apps; the readout turned off; app buttons near the tray (no room) and every step of
the form ladder with its "+ 8 px to go up" hysteresis; a short taskbar; a Windows 10 chevron
window; no app buttons; and unknown app-button extents (floating above or below the taskbar).

Real cases are recorded on real setups and named after the setup
(`5120x1440-100-bottom-centred.json`, `1440p-150-bottom-left-aligned-overflow-open.json`). The
readout's smoke tool writes the gathered geometry, with a fresh app-button measurement, and the
layout at that DPI:

```
cargo run -p lastdrop-win --example readout_smoke -- <output directory>
```

(`readout-smoke-geometry.json`, `readout-smoke-layout.json`; the app can expose the same through
`lastdrop_win::readout::taskbar_geometry`). A geometry holds only window and monitor rectangles,
DPI and flags — no window titles, app names or anything else from the user's session; check
before committing.
