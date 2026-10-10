//! Every string is a resource (R§5.9, R Appendix B, flyout-spec texts, R§10 menu, R§11 UIA).
//!
//! A [`Msg`] names a template; a [`Strings`] table gives the template for one language; [`fill`]
//! substitutes `{name}` placeholders. Column widths are computed from the localized strings
//! (R§6.3), so a translation can never be cut off. English is [`English`]; Swedish is a second
//! table later (ROADMAP "Language: English and Swedish to start").
//!
//! Placeholders: `{time}`, `{reset}`, `{since}`, `{next}`, `{until}` are formatted time points
//! (R§5.2 or R§8.2; in `a11y.*` the spoken form of [`super::time::spoken_time`], `at 2:56 PM`);
//! `{date}` a card date (`Mon 19 Oct`); `{duration}` and `{span}` long durations (R§5.0; spoken
//! in `a11y.*`), `{countdown}` a COUNTDOWN (R§5.3); `{age}` an age; `{n}`, `{h}`, `{m}`, `{d}`
//! bare integers — **spoken percentages are `{n} percent`**, never `{pct}`; `{pct}`, `{e}`,
//! `{p}`, `{left}` formatted percents (R§5.6, sign included: `42%`, sv-SE `42 %`); `{meter}` a
//! meter's `short_name` (capitalised at the start of a sentence, Appendix B) except in `a11y.*`
//! and `flyout.spark.*`, where it is the spoken name ([`super::a11y`]); `{service}` the service
//! name with alias (R§6.4); `{client}` [`crate::model::ProviderId::client_name`]; `{label}` =
//! `card.also.line`; `{short_name}` the meter's `short_name`; `{amount}`, `{ref}`, `{burn}`
//! grouped numbers; `{unit}` a unit word; `{window}` `card.window.*`; `{other}` the other
//! meter's `short_name`; `{detail}` a secret-free failure detail; `{rows}` a joined list;
//! `{today}`, `{max}`, `{avg}` bare numbers and `{day}` a full weekday (sparkline).

macro_rules! messages {
    ($( $name:ident = $key:literal => $en:literal, )*) => {
        /// A string resource. The doc of each variant is its key (Appendix B style) and its
        /// English template.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Msg {
            $(
                #[doc = concat!("`", $key, "`: \"", $en, "\"")]
                $name,
            )*
        }

        impl Msg {
            /// Every message, in declaration order (for completeness tests of a language table).
            pub const ALL: &'static [Msg] = &[$(Msg::$name,)*];

            /// The resource key (`reason.offline`).
            pub const fn key(self) -> &'static str {
                match self { $(Msg::$name => $key,)* }
            }
        }

        const fn english(msg: Msg) -> &'static str {
            match msg { $(Msg::$name => $en,)* }
        }
    };
}

messages! {
    // ---- Readout tokens (R§5.1–R§5.5, Appendix B)
    ReasonOffline = "reason.offline" => "Offline",
    ReasonSignIn = "reason.signin" => "Sign in",
    ReasonPaused = "reason.paused" => "Paused",
    ReasonNoData = "reason.nodata" => "No data",
    Loading = "loading" => "Reading…",
    Today = "today" => "Today",
    Out = "out" => "Out {time}",
    Back = "back" => "Back {time}",
    Empty = "empty" => "Empty",
    CountdownMin = "countdown.min" => "{n} min left",
    CountdownHours = "countdown.h" => "{n} h left",
    CountdownDays = "countdown.d" => "{n} d left",
    CountdownCompactMin = "countdown.compact.min" => "{n} min",
    CountdownCompactHours = "countdown.compact.h" => "{n} h",
    CountdownCompactDays = "countdown.compact.d" => "{n} d",
    NoReset = "noreset" => "–",
    AgeMin = "age.m" => "{n}m ago",
    AgeHours = "age.h" => "{n}h ago",
    AgeDays = "age.d" => "{n}d ago",
    TokenDays = "token.days" => "{n}d",
    TokenRelMin = "token.rel.m" => "{n}m",
    TokenRelHours = "token.rel.h" => "{n}h",
    TokenRelDays = "token.rel.d" => "{n}d",
    BalanceApprox = "balance.approx" => "≈",
    BalanceHours = "balance.h" => "{n}h",
    BalanceDays = "balance.d" => "{n}d",
    BalanceWeeks = "balance.w" => "{n}w",
    BalanceMax = "balance.max" => "9w+",

    // ---- Long durations (R§5.0)
    DurationMin = "duration.min" => "{n} min",
    DurationHours = "duration.h" => "{h} h",
    DurationHoursMin = "duration.hmin" => "{h} h {m} min",
    DurationDays = "duration.d" => "{d} d",
    DurationDaysHours = "duration.dh" => "{d} d {h} h",

    // ---- Spoken units with plurals (R§11 sentences, F§4.5 balance forecast)
    UnitMinuteOne = "unit.minute.one" => "{n} minute",
    UnitMinuteOther = "unit.minute.other" => "{n} minutes",
    UnitHourOne = "unit.hour.one" => "{n} hour",
    UnitHourOther = "unit.hour.other" => "{n} hours",
    UnitDayOne = "unit.day.one" => "{n} day",
    UnitDayOther = "unit.day.other" => "{n} days",
    UnitWeekOne = "unit.week.one" => "{n} week",
    UnitWeekOther = "unit.week.other" => "{n} weeks",
    UnitCredits = "unit.credits" => "credits",

    // ---- Hover card (R§8.2, Appendix B)
    CardTitleOkOne = "card.title.ok.1" => "You'll make it to the reset",
    CardTitleOkTwo = "card.title.ok.2" => "You'll make it to both resets",
    CardTitleOkMany = "card.title.ok.n" => "You'll make it to every reset",
    CardTitleWarning = "card.title.warning" => "{meter} limit runs out at {time}",
    CardTitleWarningService = "card.title.warning.service" => "{service} {meter} limit runs out at {time}",
    CardTitleCriticalRunway = "card.title.critical.runway" => "{meter} limit runs out in {duration}",
    CardTitleCriticalLevel = "card.title.critical.level" => "{meter} limit is {pct} used",
    CardTitleExhausted = "card.title.exhausted" => "{meter} limit is used up until {time}",
    CardTitleStale = "card.title.stale" => "No new reading from {service} for {age}",
    CardTitleSignIn = "card.title.signin" => "{service} needs you to sign in again",
    CardTitleApiKey = "card.title.apikey" => "{service} needs an API key",
    CardSentenceApiKey = "card.sentence.apikey" => "Lastdrop has no working {service} API key. Select the readout and choose Enter key… to add one.",
    CardSentenceOkOne = "card.sentence.ok.1" => "It is being used more slowly than time passes. At your recent pace about {left} of the {window} will be left when it resets.",
    CardSentenceOkMany = "card.sentence.ok.n" => "All limits are being used more slowly than time passes. At your recent pace about {left} will be left when they reset.",
    CardWindowSession = "card.window.session" => "session",
    CardWindowWeek = "card.window.week" => "week",
    CardWindowMonth = "card.window.month" => "month",
    CardSentenceWarning = "card.sentence.warning" => "You're using it faster than time passes: {pct} used with {e} of the window gone. At the pace of the last {span} it runs out {duration} before the {time} reset.",
    CardSentenceOtherFine = "card.sentence.other" => " The {other} limit is fine.",
    CardSentenceCritical = "card.sentence.critical" => "{pct} used. At the pace of the last {span} it is empty at about {time}, {duration} before the reset at {reset}.",
    CardSentenceLevel = "card.sentence.level" => "Nearly used up. At your recent pace it lasts until the reset at {time}.",
    CardSentenceExhausted = "card.sentence.exhausted" => "Used up since {since}. It is back at {time}, in {duration}.",
    CardSentenceExhaustedNoSince = "card.sentence.exhausted.nosince" => "Used up. It is back at {time}, in {duration}.",
    CardSentenceOffline = "card.sentence.offline" => "Lastdrop is offline, so these are the numbers from {time}. Bars are dashed and frozen at that moment until a new reading arrives. Next try at {next}.",
    CardSentenceNoData = "card.sentence.nodata" => "{service} isn't answering ({detail}), so these are the numbers from {time}. Next try at {next}.",
    CardSentenceSignIn = "card.sentence.signin" => "{service}'s sign-in has expired. Lastdrop never signs in for you: open {client} in this account to renew it. Select the readout for the steps.",
    CardSentencePaused = "card.sentence.paused" => "Updates are paused until {until}. These are the numbers from {time}.",
    CardSentencePausedOpen = "card.sentence.paused.open" => "Updates are paused. These are the numbers from {time}.",
    CardLineElapsed = "card.line.elapsed" => "{e} of time gone",
    CardLineElapsedStale = "card.line.elapsed.stale" => "{e} of time gone at {time}",
    CardForecastCourse = "card.line.forecast.course" => "on course for {p} at reset",
    CardForecastLasts = "card.line.forecast.lasts" => "lasts to the reset",
    CardForecastNone = "card.line.forecast.none" => "no forecast yet",
    CardForecastRunsOut = "card.line.forecast.runsout" => "runs out ≈ {time}",
    CardForecastUsedUp = "card.line.forecast.usedup" => "used up",
    CardLineReset = "card.line.reset" => "Resets {time}",
    CardLineIn = "card.line.in" => "in {duration}",
    CardLineIdle = "card.line.idle" => "Starts with your next use",
    CardLineNotStarted = "card.line.notstarted" => "window not started",
    CardLineNoReset = "card.line.noreset" => "No reset time given",
    CardLineNoLength = "card.line.nolength" => "window length unknown",
    CardLineJustReset = "card.line.justreset" => "Reset at {time} · waiting for the first reading",
    CardValue = "card.value" => "{pct} used",
    CardBalanceValue = "card.balance.value" => "{pct} of top-up used",
    CardBalanceLeft = "card.balance.left" => "{amount} of {ref} {unit} left",
    CardBalanceRate = "card.balance.rate" => "{burn} a day, last 7 days",
    CardBalanceLasts = "card.balance.forecast.lasts" => "lasts ≈ {duration}",
    CardBalanceRunsOut = "card.balance.forecast.runsout" => "runs out ≈ {time}",
    CardBalanceEmpty = "card.balance.forecast.empty" => "empty",
    CardBalanceNoUse = "card.balance.forecast.nouse" => "no use in 7 days",
    CardBalanceNoHistory = "card.balance.forecast.nohistory" => "not enough history yet",
    CardBalanceUntil = "card.balance.until" => "until ≈ {date}",
    CardAge = "card.age" => "Updated {age}",
    CardAgeStale = "card.age.stale" => "Last reading {time}",
    CardAlso = "card.also" => "Also tracked",
    CardAlsoMore = "card.also.more" => "+{n} more",
    CardAlsoLine = "card.also.line" => "{service} {short_name}",

    // ---- Flyout (F§3–F§6, F§10)
    FlyoutTitle = "flyout.title" => "Lastdrop",
    FlyoutVerdictOk = "flyout.verdict.ok" => "All limits last until their reset",
    FlyoutVerdictWarning = "flyout.verdict.warning" => "{label} runs out at {time}",
    FlyoutVerdictRunway = "flyout.verdict.critical.runway" => "{label} runs out in {countdown}",
    FlyoutVerdictLevel = "flyout.verdict.critical.level" => "{label} is {pct} used",
    FlyoutVerdictExhausted = "flyout.verdict.exhausted" => "{label} is used up until {time}",
    FlyoutVerdictStale = "flyout.verdict.stale" => "{service}: no reading for {age}",
    FlyoutVerdictSignIn = "flyout.verdict.signin" => "{service}: sign in again",
    FlyoutVerdictPaused = "flyout.verdict.paused" => "Updates paused",
    FlyoutVerdictLoading = "flyout.verdict.loading" => "Reading usage…",
    FlyoutVerdictMore = "flyout.verdict.more" => " · {n} more",
    FlyoutRefreshName = "flyout.refresh.a11y" => "Refresh all services",
    FlyoutRefreshTooltip = "flyout.refresh.tooltip" => "Refresh (F5)",
    FlyoutSettings = "flyout.settings" => "Settings",
    FlyoutAgeNow = "flyout.age.now" => "Just now",
    FlyoutAgeMin = "flyout.age.min" => "{n} min ago",
    FlyoutAgeHours = "flyout.age.h" => "{n} h ago",
    FlyoutAgeDays = "flyout.age.d" => "{n} d ago",
    FlyoutMsgSep = "flyout.msg.sep" => " · ",
    FlyoutMsgOffline = "flyout.msg.offline" => "Offline",
    FlyoutMsgNoData = "flyout.msg.nodata" => "Not answering",
    FlyoutMsgShowing = "flyout.msg.showing" => "showing {time}",
    FlyoutMsgNextTry = "flyout.msg.nexttry" => "next try {time}",
    FlyoutMsgSignInLead = "flyout.msg.signin.lead" => "Sign-in expired.",
    FlyoutMsgSignInText = "flyout.msg.signin.text" => "Lastdrop never signs in for you.",
    FlyoutMsgSignInAction = "flyout.msg.signin.action" => "Open sign-in…",
    FlyoutMsgPaused = "flyout.msg.paused" => "Paused",
    FlyoutMsgResume = "flyout.msg.resume" => "Resume",
    FlyoutMsgFailedLead = "flyout.msg.failed.lead" => "Couldn't read {service}",
    FlyoutMsgRetry = "flyout.msg.retry" => "Retry",
    FlyoutMsgApiKeyLead = "flyout.msg.apikey.lead" => "API key missing or rejected.",
    FlyoutMsgApiKeyAction = "flyout.msg.apikey.action" => "Enter key…",
    FlyoutLineWarning = "flyout.line.warning" => "Runs out {duration} before the reset at {time}.",
    FlyoutLineCritical = "flyout.line.critical" => "Empty ≈ {time}, {duration} before the {reset} reset.",
    FlyoutLineLevel = "flyout.line.level" => "Nearly used up. Lasts until the reset at your recent pace.",
    FlyoutLineExhausted = "flyout.line.exhausted" => "Used up since {since}. Back in {duration}.",
    FlyoutLineExhaustedNoSince = "flyout.line.exhausted.nosince" => "Back in {duration}.",
    FlyoutBalanceLeft = "flyout.balance.left" => "≈ {duration} left",
    FlyoutBalanceMax = "flyout.balance.max" => "More than 8 weeks left",
    FlyoutBalanceNoUse = "flyout.balance.nouse" => "No use in 7 days",
    FlyoutBalanceNoHistory = "flyout.balance.nohistory" => "Not enough history yet",
    FlyoutBalanceSpent = "flyout.balance.spent" => "{pct} of last top-up used",
    FlyoutBalanceBurn = "flyout.balance.burn" => " · {burn} a day",
    FlyoutBalanceUntil = "flyout.balance.until" => "until ≈ {date}",
    FlyoutFooterPaused = "flyout.footer.paused" => "Updates paused",
    FlyoutFooterResume = "flyout.footer.resume" => "Resume",
    FlyoutCardName = "flyout.card.a11y" => "{service}, {plan} plan, updated {age}",
    FlyoutCardNameNoPlan = "flyout.card.a11y.noplan" => "{service}, updated {age}",
    FlyoutCardOffline = "flyout.card.a11y.offline" => ", offline",
    FlyoutCardSignIn = "flyout.card.a11y.signin" => ", sign-in expired",
    FlyoutBalanceName = "flyout.balance.a11y" => "{amount} {unit} left, {n} percent of the last top-up used, about {duration} at {burn} a day",
    FlyoutSparkWindow = "flyout.spark.a11y.window" => "{meter} limit used per day, last 14 days: today {today} percent, highest {max} percent on {day}, average {avg} percent.",
    FlyoutSparkBalance = "flyout.spark.a11y.balance" => "{unit} used per day, last 14 days: today {today}, highest {max} on {day}, average {avg}.",

    // ---- UI Automation (R§11)
    A11yRoot = "a11y.root" => "Lastdrop. {rows}",
    A11yHelp = "a11y.help" => "Select for details, right-click for options.",
    A11yAt = "a11y.at" => "at {time}",
    A11yRowOk = "a11y.row.ok" => "{service} {meter}: on track, {n} percent used, resets {reset}.",
    A11yRowWarning = "a11y.row.warning" => "{service} {meter}: warning, runs out at about {time}, before the reset {reset}; {n} percent used.",
    A11yRowRunway = "a11y.row.critical.runway" => "{service} {meter}: critical, runs out in about {duration}; {n} percent used; resets {reset}.",
    A11yRowLevel = "a11y.row.critical.level" => "{service} {meter}: critical, {n} percent used, lasts until the reset {reset}.",
    A11yRowExhausted = "a11y.row.exhausted" => "{service} {meter}: empty, back {time}.",
    A11yRowIdle = "a11y.row.idle" => "{service} {meter}: not started, {n} percent used, starts with your next message.",
    A11yRowNoReset = "a11y.row.noreset" => "{service} {meter}: {n} percent used, no reset time given.",
    A11yRowLoading = "a11y.row.loading" => "{service} {meter}: reading.",
    A11yServiceOffline = "a11y.service.offline" => "{service}: offline, last reading {age} ago. Then: {rows}.",
    A11yServiceNoData = "a11y.service.nodata" => "{service}: not answering, last reading {age} ago. Then: {rows}.",
    A11yServicePaused = "a11y.service.paused" => "{service}: updates paused, last reading {age} ago. Then: {rows}.",
    A11yServiceSignIn = "a11y.service.signin" => "{service}: sign-in expired, last reading {age} ago. Select to open Lastdrop and see how to sign in.",
    A11yServiceApiKey = "a11y.service.apikey" => "{service}: API key missing or rejected. Select to open Lastdrop and add one.",
    A11yStaleRow = "a11y.stale.row" => "{meter} {n} percent used",
    A11yBalance = "a11y.balance" => "{service}: {amount} {unit} left, about {duration} at the current rate.",
    A11yBalanceNoForecast = "a11y.balance.noforecast" => "{service}: {amount} {unit} left.",
    A11yListSep = "a11y.list.sep" => ", ",

    // ---- Tray tooltip (R§9.3)
    TrayOk = "tray.ok" => "Lastdrop: {service} {meter} {pct} used, resets {time}",
    TrayNoReset = "tray.noreset" => "Lastdrop: {service} {meter} {pct} used",
    TrayWarning = "tray.warning" => "Lastdrop: {service} {meter} runs out at {time}",
    TrayRunway = "tray.runway" => "Lastdrop: {service} {meter} runs out in {duration}",
    TrayLevel = "tray.level" => "Lastdrop: {service} {meter} {pct} used",
    TrayExhausted = "tray.exhausted" => "Lastdrop: {service} {meter} used up until {time}",
    TrayOffline = "tray.offline" => "Lastdrop: {service} offline, last reading {age} ago",
    TrayNoData = "tray.nodata" => "Lastdrop: {service} not answering, last reading {age} ago",
    TraySignIn = "tray.signin" => "Lastdrop: {service} needs you to sign in again",
    TrayApiKey = "tray.apikey" => "Lastdrop: {service} needs an API key",
    TrayPaused = "tray.paused" => "Lastdrop: updates paused",
    TrayLoading = "tray.loading" => "Lastdrop: reading usage…",
    TrayNothing = "tray.nothing" => "Lastdrop",

    // ---- Context menus (R§10, F§9)
    MenuRefresh = "menu.refresh" => "Refresh now",
    MenuPause = "menu.pause" => "Pause updates",
    MenuPauseHour = "menu.pause.hour" => "For 1 hour",
    MenuPauseUntilResume = "menu.pause.resume" => "Until I resume",
    MenuResume = "menu.resume" => "Resume updates",
    MenuReadout = "menu.readout" => "Taskbar readout",
    MenuReadoutAuto = "menu.readout.auto" => "Auto",
    MenuReadoutPinned = "menu.readout.pinned" => "Pinned meters…",
    MenuReadoutOff = "menu.readout.off" => "Off",
    MenuSettings = "menu.settings" => "Settings…",
    MenuAbout = "menu.about" => "About Lastdrop",
    MenuQuit = "menu.quit" => "Quit Lastdrop",
    MenuCardUsage = "menu.card.usage" => "Open usage page",
    MenuCardRefresh = "menu.card.refresh" => "Refresh {service}",
    MenuCardReadout = "menu.card.readout" => "Show in taskbar readout",
}

/// CLDR-style plural category; English uses `One` and `Other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Plural {
    /// Singular (`1 minute`).
    One,
    /// Everything else.
    Other,
}

/// One language's string table.
pub trait Strings: Send + Sync {
    /// BCP 47 language tag of the table (`en`).
    fn language(&self) -> &str;

    /// The template of `msg`, with `{name}` placeholders.
    fn template(&self, msg: Msg) -> &str;

    /// The plural category of `n` in this language.
    fn plural(&self, n: i64) -> Plural;
}

/// The English table (R Appendix B; the en-US/en-GB texts of both specs).
#[derive(Clone, Copy, Debug, Default)]
pub struct English;

impl Strings for English {
    fn language(&self) -> &str {
        "en"
    }

    fn template(&self, msg: Msg) -> &str {
        english(msg)
    }

    fn plural(&self, n: i64) -> Plural {
        if n == 1 { Plural::One } else { Plural::Other }
    }
}

/// Replaces every `{name}` in `template` with its value from `args`. A placeholder without a value
/// is left as is (visible in tests, never silently dropped). `{{` and `}}` are not special.
pub fn fill(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let name = &after[..close];
                match args.iter().find(|(k, _)| *k == name) {
                    Some((_, v)) => out.push_str(v),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// `fill(strings.template(msg), args)`.
pub fn text(strings: &dyn Strings, msg: Msg, args: &[(&str, &str)]) -> String {
    fill(strings.template(msg), args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_substitutes_and_keeps_unknown_placeholders() {
        assert_eq!(fill("Out {time}", &[("time", "15:20")]), "Out 15:20");
        assert_eq!(fill("{n} min left", &[("n", "12")]), "12 min left");
        assert_eq!(fill("{a} and {b}", &[("a", "x")]), "x and {b}");
        assert_eq!(fill("no braces", &[]), "no braces");
        assert_eq!(fill("open { only", &[]), "open { only");
    }

    #[test]
    fn every_message_has_a_unique_key_and_an_english_template() {
        let mut keys: Vec<&str> = Msg::ALL.iter().map(|m| m.key()).collect();
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate message keys");
        assert!(Msg::ALL.iter().all(|m| !English.template(*m).is_empty()));
    }

    #[test]
    fn spoken_percentages_use_a_bare_number() {
        // `{pct}` carries the percent sign (`42%`), so `{pct} percent` would read "42% percent".
        for m in Msg::ALL {
            assert!(
                !English.template(*m).contains("{pct} percent"),
                "{}",
                m.key()
            );
        }
    }
}
