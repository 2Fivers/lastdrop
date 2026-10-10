//! Builds the readout model from an evaluation (R§4.3, R§5.7, R§6.3, R§6.4, R§11).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use super::ReadoutModel;
use crate::grammar::TextContext;
use crate::settings::Settings;
use crate::stats::Evaluation;

/// What every surface builder reads besides the evaluation.
#[derive(Clone, Copy)]
pub struct BuildContext<'a> {
    /// Clock, time style, locale and strings.
    pub text: TextContext<'a>,
    /// Settings: content mode and pins (R§4.3), enabled services and hidden meters (which meters
    /// *can* appear, for widths and the glyph column, R§6.3–R§6.4).
    pub settings: &'a Settings,
}

/// The readout model (R§4–R§6, R§11): rows for `eval.readout_rows` in display order, each with
/// label, bar input (calm fill TextPrimary), right cell (R§5.7) and compact cell (R§5.8); the
/// compact row (the winner among the rows); the width inputs from what *can* appear (R§6.3);
/// UIA names; `ContentForm::Hidden` when the readout is off or there is nothing to show
/// (R§3.10 "No meter at all").
///
/// "What can appear" (R§6.3, R§6.4) is: pinned mode — the pins; auto mode — every non-hidden
/// meter of every enabled service, i.e. the catalogue meters ([`crate::model::catalog`]) plus
/// the meters the evaluation holds that are not in the catalogue (Claude's per-model weeklies,
/// so `Fable` widens the label column once it has been reported). Balance strings are included
/// only when a balance meter can appear. The width inputs therefore change with settings, the
/// locale or a newly reported meter, never with values.
pub fn build_readout(eval: &Evaluation, cx: &BuildContext<'_>) -> ReadoutModel {
    todo!("M1: R§4–R§6 readout model")
}
