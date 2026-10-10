//! The flyout as a model (flyout-spec). All text and geometry inputs are produced here by the
//! same functions as the readout; Slint only lays out what it is given (F§0: "Nothing in `.slint`
//! formats a number or a time"). The Slint structs in crates/app/ui/flyout.slint mirror these
//! types (F§11); the app's wiring converts them, running [`crate::readout::bar_geometry`] and
//! [`spark_geometry`] at the window's scale.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

use crate::consts::SPARK_DAYS;
use crate::glyph::{ProviderGlyph, SeverityGlyph};
use crate::model::{MeterKey, SourceKey};
use crate::readout::bar::{BarInput, Seg};
use crate::readout::build::BuildContext;
use crate::stats::{Evaluation, HistoryView};

/// The colour family of a meter row, bar and say-glyph (F§11 `Tone`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tone {
    /// Calm: AccentFill bar, no glyph.
    Calm,
    /// Caution: WARNING.
    Caution,
    /// Critical: CRITICAL and EXHAUSTED.
    Critical,
    /// Stale: dashed, stale colour.
    Stale,
}

/// The header verdict (F§3): worst first (EXHAUSTED > CRITICAL > WARNING > STALE > OK), with
/// ` · N more` when more than one item is not OK.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    /// In trouble: severity glyph 12 + 4 + text.
    pub glyph: Option<SeverityGlyph>,
    /// `All limits last until their reset`, `Claude 5-hour runs out at 15:20`, … (F§3 table).
    pub text: String,
}

/// What a message-line hyperlink does (F§4.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CardAction {
    /// `Open sign-in…`: the vendor's own login in a terminal with this account's config folder
    /// (`claude` / `codex login`), ROADMAP "Add an account". Hides the flyout.
    OpenSignIn,
    /// `Resume`: end the pause.
    Resume,
    /// `Retry`: refresh this service now (respecting `min_interval` and back-off).
    Retry,
    /// `Enter key…`: open the settings window at this service's API key (services without a
    /// vendor sign-in, `ProviderId::client_name` is `None`: Meshy). Hides the flyout.
    OpenSettings,
}

/// Why a card shows a message line (F§4.3). Slint `CardData.msg-kind` is this enum's position
/// + 1 (0 = no message): 1 Offline, 2 NoData, 3 SignIn, 4 Paused, 5 FirstFetchFailed, 6 ApiKey.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MessageKind {
    /// OFFLINE: **Offline** · showing 12:42 · next try 13:30.
    Offline,
    /// NO_DATA: **Not answering** · showing 12:42 · next try 13:30.
    NoData,
    /// SIGN_IN: **Sign-in expired.** Lastdrop never signs in for you. + `Open sign-in…`.
    SignIn,
    /// PAUSED: **Paused** · showing 12:42 + `Resume`.
    Paused,
    /// The first fetch failed: **Couldn't read Claude** · {short detail} + `Retry`.
    FirstFetchFailed,
    /// SIGN_IN of a service signed in with an API key (Meshy: key missing or rejected):
    /// **API key missing or rejected.** + `Enter key…` (informational glyph). Not in F§4.3,
    /// whose sign-in action opens a vendor login that such services do not have.
    ApiKey,
}

/// A hyperlink in a message line or the footer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinkModel {
    /// Its text (AccentText); also its accessible name.
    pub label: String,
    /// What it does.
    pub action: CardAction,
}

/// The optional message line of a card (F§4.3): margin-top 8; glyph 12 + 6 + text (Small 12/16
/// TextSecondary, first phrase semibold TextPrimary); wraps to at most 2 lines.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessageLine {
    /// Why.
    pub kind: MessageKind,
    /// Stale (U+F140), informational (sign-in) or critical (first fetch failed).
    pub glyph: SeverityGlyph,
    /// The semibold first phrase.
    pub lead: String,
    /// The rest, starting with its separator (` · showing 12:42 · next try 13:30`).
    pub text: String,
    /// Optional hyperlink.
    pub action: Option<LinkModel>,
}

/// What sits right of the meter name (F§4.4 "Right cluster"; F§11 `right-kind` 0, 1, 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RightKind {
    /// U+E777 12 px + 4 + text, TextSecondary (`Resets 14:56`).
    Reset,
    /// Severity glyph + 4 + semibold TextPrimary text (`Out 15:20`, `12 min left`, `Back 16:30`).
    Say,
    /// No glyph, text in TextSecondary (`Starts with your next use`, `No reset time given`,
    /// stale `Resets 14:56`); LOADING: empty text. F§11's `right-kind` 2 ("none").
    Plain,
}

/// The right cluster of a meter row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RightCluster {
    /// Layout.
    pub kind: RightKind,
    /// The severity glyph for [`RightKind::Say`].
    pub glyph: Option<SeverityGlyph>,
    /// The text.
    pub text: String,
}

/// A window meter row (F§4.4): name row 20 (name, right cluster, 12, value min 36), 4, bar
/// 310 × 9, optional trouble line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeterRowModel {
    /// The meter.
    pub key: MeterKey,
    /// Meter `name`, Text 14/20, ellipsis.
    pub name: String,
    /// `42%` (Text 14/20 semibold; `100%` allowed here); LOADING `–`.
    pub value: String,
    /// STALE: the value in TextSecondary.
    pub value_dim: bool,
    /// Right of the name.
    pub right: RightCluster,
    /// Trouble line (margin-top 4, 12/16 TextSecondary, one line, ellipsis) in JUST_RESET,
    /// WARNING, CRITICAL and EXHAUSTED (F§4.4 table).
    pub line: Option<String>,
    /// Colour family.
    pub tone: Tone,
    /// Bar inputs (calm fill AccentFill).
    pub bar: BarInput,
    /// Accessible label: the row sentence (R§11).
    pub a11y: String,
    /// Accessible value `pct` (min 0, max 100); `None` while LOADING.
    pub pct: Option<u8>,
}

/// The balance body (F§4.5): row 28 (amount 20/28, 6, unit 14/20, forecast right), 2, row 16
/// (spent · burn left with ellipsis, 8, until right). No bar: no window, no pace.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BalanceBodyModel {
    /// The meter.
    pub key: MeterKey,
    /// Locale-grouped amount (`1,240`), Display 20/28 semibold.
    pub amount: String,
    /// Provider unit (`credits`), Text 14/20 TextSecondary.
    pub unit: String,
    /// `≈ 9 days left`, …; EXHAUSTED `Empty`.
    pub forecast: String,
    /// WARNING/CRITICAL/EXHAUSTED: severity glyph 12 + 4 + semibold.
    pub forecast_glyph: Option<SeverityGlyph>,
    /// `62% of last top-up used · 136 a day` (` · 136 a day` omitted without a forecast).
    pub spent: String,
    /// `until ≈ Mon 19 Oct`, never cut; `None` without a forecast.
    pub until: Option<String>,
    /// Accessible text (F§10).
    pub a11y: String,
}

/// The body of a card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CardBody {
    /// Window meters in provider order (hidden meters omitted, F§2.2).
    Meters(Vec<MeterRowModel>),
    /// A balance service.
    Balance(BalanceBodyModel),
}

/// What a sparkline counts (F§6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SparkKind {
    /// Window services: the history meter (longest all-models window: Claude weekly all models,
    /// Codex weekly); `v_k` = sum of positive increases of `u` between consecutive readings of
    /// the same window instance within local day k, in percentage points; a drop at a reset is
    /// ignored. Floor 10 points.
    Window,
    /// Balance services: sum of balance decreases within local day k (top-ups ignored), in the
    /// provider unit. Floor 5 % of `ref`.
    Balance,
}

/// A 14-day sparkline (F§6), DPI-independent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SparkModel {
    /// `v_k` per local day, oldest first, today last; `None` = no data (empty slot).
    pub values: [Option<f64>; SPARK_DAYS],
    /// The scale floor: `max = max(max_k v_k, floor)`.
    pub floor: f64,
    /// A stale service: all columns TextTertiary (no accent for today).
    pub dim: bool,
    /// Accessible label (`Weekly limit used per day, last 14 days: today 6 percent, …`).
    pub a11y: String,
}

/// One sparkline column, device px, relative to the sparkline box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SparkColumn {
    /// x of the column.
    pub x: i32,
    /// Height, from the bottom edge of the box (it covers the baseline there).
    pub h: i32,
    /// Today's column (AccentFill unless dim).
    pub today: bool,
}

/// The sparkline in device px (F§6).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SparkGeometry {
    /// `round(55 s)`.
    pub box_w: i32,
    /// `round(16 s)`.
    pub box_h: i32,
    /// `round(3 s)`: column width.
    pub col_w: i32,
    /// `hair = max(1, floor(s))`: gap and baseline height.
    pub hair: i32,
    /// The baseline across all 14 slots, `hair` high at the bottom, TextDisabled.
    pub baseline: Seg,
    /// Columns with `v > 0`.
    pub columns: Vec<SparkColumn>,
}

/// F§6 geometry: 14 columns `round(3 s)` wide with `hair` gaps, right-aligned in the
/// `round(55 s)` box (oldest left, today right); column height
/// `round(v / max × (16 s − hair))`, at least one `hair` when `v > 0`; `v = 0` or no data → no
/// column.
pub fn spark_geometry(spark: &SparkModel, scale: f64) -> SparkGeometry {
    todo!("M1: F§6 geometry")
}

/// `v_k` for the 14 local days ending today (F§6), from the history of `key`.
pub fn spark_values(
    history: &dyn HistoryView,
    key: &MeterKey,
    kind: SparkKind,
    now: Timestamp,
    tz: &TimeZone,
) -> [Option<f64>; SPARK_DAYS] {
    todo!("M1: F§6 values")
}

/// A service card (F§4): border 1, radius 8, padding 12 → inner 310; header 20
/// (`[glyph 16] 8 [name] 8 [plan badge] [flex] [age] 8 [sparkline 55 × 16]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CardModel {
    /// The service and account.
    pub source: SourceKey,
    /// Provider glyph, 16 px.
    pub glyph: ProviderGlyph,
    /// `Claude` (M2: `Claude · Work`), Text 14/20 semibold.
    pub name: String,
    /// Plan badge text; `None` = badge omitted.
    pub plan: Option<String>,
    /// `Just now`, `1 min ago`, …; LOADING `Reading…` (F§4.2).
    pub age: String,
    /// Message line (F§4.3).
    pub message: Option<MessageLine>,
    /// Meters or balance.
    pub body: CardBody,
    /// Sparkline.
    pub spark: SparkModel,
    /// Accessible label: `Claude, Max plan, updated 1 minute ago` (+ `, offline` /
    /// `, sign-in expired`), role groupbox (F§10).
    pub a11y: String,
}

/// The footer's left part (F§5): `Updates paused · Resume` (or `Update available · Install`,
/// M5), 12/16 TextSecondary with an accent hyperlink.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FooterNote {
    /// `Updates paused`.
    pub text: String,
    /// `Resume`.
    pub link: LinkModel,
}

/// Everything the flyout draws (F§1–F§10).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FlyoutModel {
    /// `Lastdrop` (Text 14/20 semibold).
    pub title: String,
    /// Header verdict; also the window's accessible description (F§10).
    pub verdict: Verdict,
    /// A refresh is running: the refresh glyph becomes a 16 px indeterminate progress ring and
    /// the button is disabled (F§3).
    pub refreshing: bool,
    /// `Refresh all services` (accessible name) and `Refresh (F5)` (tooltip).
    pub refresh_name: String,
    /// The refresh button's tooltip.
    pub refresh_tooltip: String,
    /// Cards in the user's service order (F§2.2).
    pub cards: Vec<CardModel>,
    /// `Settings` (footer button label).
    pub settings_label: String,
    /// Footer left part.
    pub footer_note: Option<FooterNote>,
    /// Opened by a click while a service needs sign-in (R§10): that card is scrolled into view
    /// and focused, without a focus rect (F§9).
    pub focus_card: Option<usize>,
    /// Announce once (a meter's severity rose while open, F§10).
    pub announce: Option<String>,
}

/// Builds the flyout model (F§3–F§10) from an evaluation; `history` feeds the sparklines and
/// `refreshing` the refresh button.
pub fn build_flyout(
    eval: &Evaluation,
    history: &dyn HistoryView,
    refreshing: bool,
    cx: &BuildContext<'_>,
) -> FlyoutModel {
    todo!("M1: flyout-spec")
}
