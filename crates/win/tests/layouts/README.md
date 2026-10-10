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

The three `synthetic-*` files are hand-made starting points. Real cases are recorded on real
setups (the readout implementer adds a debug dump of the gathered `TaskbarGeometry`, e.g.
`lastdrop.exe --dump-taskbar-geometry`) and named after the setup:
`1440p-150-bottom-left-aligned-overflow-open.json`. A geometry holds only window and monitor
rectangles, DPI and flags — no window titles, app names or anything else from the user's
session; check before committing.
