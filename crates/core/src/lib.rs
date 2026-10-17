//! `lastdrop-core`: everything that decides **what** Lastdrop shows, as pure Rust.
//!
//! No Windows dependencies, no I/O and no clock reads: every function takes `now`, the data and
//! the state it needs, and returns values. The Windows layers (`lastdrop-win`, the Slint UI in
//! the `lastdrop` binary) only draw the models built here (docs/ARCHITECTURE.md, goal 4).
//!
//! Spec references in the docs of this crate:
//! - `R§n` — `docs/design/readout-spec.md`, section n (meter model, statistics, grammar, readout,
//!   hover card, tray icon). `R§13` holds the test vectors (`tests/readout_spec_13.rs`).
//! - `F§n` — `docs/design/flyout-spec.md`, section n.
//! - `N§n` — `docs/design/notifications-spec.md`, section n.
//! - `ARCHITECTURE` — `docs/ARCHITECTURE.md`.
//!
//! Pipeline (one pass per new reading and per minute tick, R§12):
//!
//! ```text
//! Snapshots + ServiceStatus + Settings + HistoryView + now
//!     └─ stats::evaluate ──▶ Evaluation (assessments, ranking, readout rows, winner)
//!            ├─ readout::build_readout     ──▶ ReadoutModel   (readout thread draws it)
//!            ├─ readout::build_hover_card  ──▶ HoverCardModel (Slint hover card)
//!            ├─ readout::build_tray        ──▶ TrayModel      (tray icon image + tooltip)
//!            ├─ flyout::build_flyout       ──▶ FlyoutModel    (Slint flyout)
//!            └─ notify::notices + NotifyMemory
//!                  └─ notify::build_notification ──▶ NotificationModel (Windows notification)
//! ```
//!
//! Models are DPI-independent: they carry the *inputs* of the pixel algorithms
//! ([`readout::bar::BarInput`], [`readout::layout::WidthInputs`]) and each surface runs the
//! normative algorithm ([`readout::bar::bar_geometry`], [`readout::layout::compute_layout`]) at
//! its own scale, so a DPI change never needs a new model.
#![forbid(unsafe_code)]

pub mod charts;
pub mod color;
pub mod consts;
pub mod custom;
pub mod flyout;
pub mod glyph;
pub mod grammar;
pub mod model;
pub mod notify;
pub mod readout;
pub mod refresh;
pub mod secret;
pub mod settings;
pub mod stats;
pub mod theme;
pub mod tokens;
pub mod update;
mod util;

/// The time crate every Lastdrop crate uses, re-exported so versions cannot drift.
pub use jiff;
