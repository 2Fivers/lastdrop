//! Every string is a resource (R§5.9, R Appendix B, flyout-spec texts, R§10 menu, R§11 UIA).
//!
//! A [`Msg`] names a template; a [`Strings`] table gives the template for one language; [`fill`]
//! substitutes `{name}` placeholders. Column widths are computed from the localized strings
//! (R§6.3), so a translation can never be cut off. The tables are [`English`] and [`Swedish`]
//! (ROADMAP "Language: English and Swedish to start"); [`Language`] picks one.
//!
//! Placeholders: `{time}`, `{reset}`, `{since}`, `{next}`, `{until}` are formatted time points
//! (R§5.2 or R§8.2; in `a11y.*` the spoken form of [`super::time::spoken_time`], `at 2:56 PM`);
//! `{date}` a card date (`Mon 19 Oct`); `{duration}` and `{span}` long durations (R§5.0; spoken
//! in `a11y.*`), `{countdown}` a COUNTDOWN (R§5.3); `{age}` an age; `{n}`, `{h}`, `{m}`, `{d}`,
//! `{course}` bare integers — **spoken percentages are `{n} percent`** (`{course} percent`),
//! never `{pct}`; `{pct}`, `{e}`,
//! `{p}`, `{left}` formatted percents (R§5.6, sign included: `42%`, sv-SE `42 %`); `{meter}` a
//! meter's `short_name` as it is, except in `a11y.*`, where it is the spoken name
//! ([`super::a11y`]); `{service}` the service
//! name with alias (R§6.4); `{client}` [`crate::model::ProviderId::client_name`]; `{label}` =
//! `card.also.line`; `{short_name}` the meter's `short_name`; `{amount}`, `{ref}`, `{burn}`
//! grouped numbers; `{unit}` a unit word; `{window}` `card.window.*`; `{other}` the other
//! meter's `short_name`; `{detail}` a secret-free failure detail; `{rows}` a joined list;
//! `{today}`, `{max}`, `{avg}` bare numbers and `{day}` a full weekday (sparkline); `{model}` a
//! model's display name (`meter.*`).
//!
//! Titles, verdicts, notification titles and "Also tracked" lines are capitalised as a whole
//! after filling (Appendix B: `{meter}` "capitalised at the start of a sentence"), so a
//! template may start with `{meter}` or `{label}` in any language.

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
    // Never drawn in the readout (an old reading shows AGE on every row); for completeness.
    ReasonOldReading = "reason.oldreading" => "No new data",
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
    // A WARNING that hysteresis holds although the run-out moved past the reset (R§5.3): the
    // projection instead of a run-out time.
    CardTitleWarningCourse = "card.title.warning.course" => "{meter} limit is on course for {p} at the reset",
    CardTitleWarningCourseService = "card.title.warning.course.service" => "{service} {meter} limit is on course for {p} at the reset",
    CardTitleCriticalRunway = "card.title.critical.runway" => "{meter} limit runs out in {duration}",
    CardTitleCriticalLevel = "card.title.critical.level" => "{meter} limit is {pct} used",
    CardTitleExhausted = "card.title.exhausted" => "{meter} limit is used up until {time}",
    CardTitleStale = "card.title.stale" => "No new reading from {service} for {age}",
    CardTitleSignIn = "card.title.signin" => "{service} needs you to sign in again",
    CardTitleApiKey = "card.title.apikey" => "{service} needs an API key",
    CardSentenceApiKey = "card.sentence.apikey" => "Lastdrop has no working {service} API key. Select the readout and choose Enter key… to add one.",
    CardSentenceOkOne = "card.sentence.ok.1" => "It is being used more slowly than time passes. At your recent pace about {left} of the {window} will be left when it resets.",
    CardSentenceOkMany = "card.sentence.ok.n" => "All limits are being used more slowly than time passes. At your recent pace about {left} will be left when they reset.",
    // R§3.4's other two pace wordings for an OK card ("about as fast as" / "faster than time
    // passes"); Appendix B only lists the "more slowly" sentence.
    CardSentenceOkOneAbout = "card.sentence.ok.1.about" => "It is being used about as fast as time passes. At your recent pace about {left} of the {window} will be left when it resets.",
    CardSentenceOkOneAhead = "card.sentence.ok.1.ahead" => "It is being used faster than time passes, but at your recent pace about {left} of the {window} will be left when it resets.",
    CardSentenceOkManyMixed = "card.sentence.ok.n.mixed" => "At your recent pace about {left} will be left when they reset.",
    // The OK sentence in parts, for windows without `e` (no pace, R§3.4) or without a valid
    // forecast (no "at your recent pace", R§3.5): only what is known is said.
    CardSentenceOkOnePaceUnder = "card.sentence.ok.1.pace.under" => "It is being used more slowly than time passes.",
    CardSentenceOkOnePaceAbout = "card.sentence.ok.1.pace.about" => "It is being used about as fast as time passes.",
    CardSentenceOkOnePaceAhead = "card.sentence.ok.1.pace.ahead" => "It is being used faster than time passes.",
    CardSentenceOkManyPace = "card.sentence.ok.n.pace" => "All limits are being used more slowly than time passes.",
    CardSentenceOkOneLeft = "card.sentence.ok.1.left" => "At your recent pace about {left} of the {window} will be left when it resets.",
    // OK while no shown window has a reset time (Claude Desktop's history alone): no claim about
    // making it to a reset.
    CardTitleOkNoReset = "card.title.ok.noreset" => "Within your limits",
    CardWindowSession = "card.window.session" => "session",
    CardWindowWeek = "card.window.week" => "week",
    CardWindowMonth = "card.window.month" => "month",
    CardSentenceWarning = "card.sentence.warning" => "You're using it faster than time passes: {pct} used with {e} of the window gone. At the pace of the last {span} it runs out {duration} before the {time} reset.",
    CardSentenceWarningCourse = "card.sentence.warning.course" => "You're using it faster than time passes: {pct} used with {e} of the window gone. At the pace of the last {span} it reaches about {p} by the {time} reset.",
    CardSentenceOtherFine = "card.sentence.other" => " The {other} limit is fine.",
    CardSentenceCritical = "card.sentence.critical" => "{pct} used. At the pace of the last {span} it is empty at about {time}, {duration} before the reset at {reset}.",
    CardSentenceLevel = "card.sentence.level" => "Nearly used up. At your recent pace it lasts until the reset at {time}.",
    CardSentenceExhausted = "card.sentence.exhausted" => "Used up since {since}. It is back at {time}, in {duration}.",
    CardSentenceExhaustedNoSince = "card.sentence.exhausted.nosince" => "Used up. It is back at {time}, in {duration}.",
    CardSentenceOffline = "card.sentence.offline" => "Lastdrop is offline, so these are the numbers from {time}. Bars are dashed and frozen at that moment until a new reading arrives. Next try at {next}.",
    CardSentenceNoData = "card.sentence.nodata" => "{service} isn't answering ({detail}), so these are the numbers from {time}. Next try at {next}.",
    CardSentenceOldReading = "card.sentence.oldreading" => "{service} answers, but its newest numbers are from {time}. Next try at {next}.",
    CardSentenceSignIn = "card.sentence.signin" => "{service}'s sign-in has expired. Lastdrop never signs in for you: open {client} in this account to renew it. Select the readout for the steps.",
    CardSentencePaused = "card.sentence.paused" => "Updates are paused until {until}. These are the numbers from {time}.",
    CardSentencePausedOpen = "card.sentence.paused.open" => "Updates are paused. These are the numbers from {time}.",
    CardLineElapsed = "card.line.elapsed" => "{e} of time gone",
    CardLineElapsedStale = "card.line.elapsed.stale" => "{e} of time gone at {time}",
    // The tick line of a window without a reset time that is not IDLE (R§8.2 names only its
    // reset line).
    CardLineElapsedUnknown = "card.line.elapsed.unknown" => "time gone unknown",
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
    // R§3.9 ENDED: the hover card's reset line and the flyout row's right cluster.
    CardLineEnded = "card.line.ended" => "Ended by {time}",
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
    FlyoutVerdictOkNoReset = "flyout.verdict.ok.noreset" => "Within your limits",
    FlyoutVerdictWarning = "flyout.verdict.warning" => "{label} runs out at {time}",
    FlyoutVerdictWarningCourse = "flyout.verdict.warning.course" => "{label} is on course for {p} at the reset",
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
    FlyoutMsgOldReading = "flyout.msg.oldreading" => "No new reading",
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
    FlyoutLineWarningCourse = "flyout.line.warning.course" => "On course for {p} at the {time} reset.",
    FlyoutLineCritical = "flyout.line.critical" => "Empty ≈ {time}, {duration} before the {reset} reset.",
    FlyoutLineLevel = "flyout.line.level" => "Nearly used up. Lasts until the reset at your recent pace.",
    FlyoutLineExhausted = "flyout.line.exhausted" => "Used up since {since}. Back in {duration}.",
    FlyoutLineExhaustedNoSince = "flyout.line.exhausted.nosince" => "Used up. Back in {duration}.",
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
    A11yAgo = "a11y.ago" => "{duration} ago",
    A11yRowOk = "a11y.row.ok" => "{service} {meter}: on track, {n} percent used, resets {reset}.",
    A11yRowWarning = "a11y.row.warning" => "{service} {meter}: warning, runs out at about {time}, before the reset {reset}; {n} percent used.",
    A11yRowWarningCourse = "a11y.row.warning.course" => "{service} {meter}: warning, on course for {course} percent at the reset {reset}; {n} percent used.",
    A11yRowRunway = "a11y.row.critical.runway" => "{service} {meter}: critical, runs out in about {duration}; {n} percent used; resets {reset}.",
    A11yRowLevel = "a11y.row.critical.level" => "{service} {meter}: critical, {n} percent used, lasts until the reset {reset}.",
    A11yRowExhausted = "a11y.row.exhausted" => "{service} {meter}: empty, back {time}.",
    A11yRowExhaustedNoReset = "a11y.row.exhausted.noreset" => "{service} {meter}: empty, {n} percent used, no reset time given.",
    A11yRowIdle = "a11y.row.idle" => "{service} {meter}: not started, {n} percent used, starts with your next message.",
    A11yRowNoReset = "a11y.row.noreset" => "{service} {meter}: {n} percent used, no reset time given.",
    A11yRowLoading = "a11y.row.loading" => "{service} {meter}: reading.",
    A11yRowEnded = "a11y.row.ended" => "{service} {meter}: ended by {time}, no newer reading.",
    A11yServiceOffline = "a11y.service.offline" => "{service}: offline, last reading {age} ago. Then: {rows}.",
    A11yServiceNoData = "a11y.service.nodata" => "{service}: not answering, last reading {age} ago. Then: {rows}.",
    A11yServiceOldReading = "a11y.service.oldreading" => "{service}: no new reading, last reading {age} ago. Then: {rows}.",
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
    TrayOldReading = "tray.oldreading" => "Lastdrop: {service} has no new reading, last reading {age} ago",
    TraySignIn = "tray.signin" => "Lastdrop: {service} needs you to sign in again",
    TrayApiKey = "tray.apikey" => "Lastdrop: {service} needs an API key",
    TrayPaused = "tray.paused" => "Lastdrop: updates paused",
    TrayLoading = "tray.loading" => "Lastdrop: reading usage…",
    TrayNothing = "tray.nothing" => "Lastdrop",

    // ---- Notifications (N§3)
    NoticeLevelTitle = "notice.level.title" => "{service} {meter} limit is {pct} used",
    NoticeUsedUpTitle = "notice.usedup.title" => "{service} {meter} limit is used up",
    NoticeResetTitle = "notice.reset.title" => "{service} {meter} limit has reset",
    NoticeBalanceLowTitle = "notice.balance.low.title" => "{service} {meter} running low",
    NoticeBalanceEmptyTitle = "notice.balance.empty.title" => "{service} {meter} used up",
    NoticeBodyRunsOut = "notice.body.runsout" => "At your recent pace it runs out at {time}, {duration} before the reset.",
    NoticeBodyLasts = "notice.body.lasts" => "At your recent pace it lasts until the reset at {time}.",
    NoticeBodyResets = "notice.body.resets" => "It resets {time}.",
    NoticeBodyBack = "notice.body.back" => "It is back at {time}, in {duration}.",
    NoticeBodyNoReset = "notice.body.noreset" => "Select to see the details in Lastdrop.",
    NoticeBodyReset = "notice.body.reset" => "Back to {pct} used.",
    NoticeBodyResetUsedUp = "notice.body.reset.usedup" => "Back to {pct} used. You can use it again.",
    NoticeBodyBalance = "notice.body.balance" => "{amount} {unit} left, about {duration} at your use of the last 7 days.",
    NoticeBodyBalanceLeft = "notice.body.balance.left" => "{amount} {unit} left.",
    NoticeBodyBalanceEmpty = "notice.body.balance.empty" => "Top it up to keep using {service}.",
    NoticeMore = "notice.more" => "+{n} more",

    // ---- Meter names (R§2 table: `short_label` / `short_name` / `name`; [`super::meters`])
    MeterFiveHourLabel = "meter.five_hour.label" => "5h",
    MeterFiveHourShort = "meter.five_hour.short" => "5-hour",
    MeterFiveHourName = "meter.five_hour.name" => "5-hour session",
    MeterWeeklyLabel = "meter.weekly.label" => "Wk",
    MeterWeeklyShort = "meter.weekly.short" => "weekly",
    MeterWeeklyName = "meter.weekly.name" => "Weekly",
    // Claude's weekly window counts every model; its per-model windows name the model.
    MeterWeeklyAllName = "meter.weekly.all.name" => "Weekly · all models",
    MeterWeeklyModelShort = "meter.weekly.model.short" => "{model} weekly",
    MeterWeeklyModelName = "meter.weekly.model.name" => "Weekly · {model}",
    // A window outside the catalogue, named by its length (Codex).
    MeterHoursLabel = "meter.hours.label" => "{n}h",
    MeterHoursShort = "meter.hours.short" => "{n}-hour",
    MeterHoursName = "meter.hours.name" => "{n}-hour window",
    MeterDaysLabel = "meter.days.label" => "{n}d",
    MeterDaysShort = "meter.days.short" => "{n}-day",
    MeterDaysName = "meter.days.name" => "{n}-day window",
    MeterCreditsLabel = "meter.credits.label" => "Cr",
    MeterCreditsShort = "meter.credits.short" => "credits",
    MeterCreditsName = "meter.credits.name" => "Credits",
    MeterPremiumLabel = "meter.premium.label" => "Prem",
    MeterPremiumShort = "meter.premium.short" => "premium",
    MeterPremiumName = "meter.premium.name" => "Premium requests",
    MeterChatLabel = "meter.chat.label" => "Chat",
    MeterChatShort = "meter.chat.short" => "chat",
    MeterChatName = "meter.chat.name" => "Chat messages",
    MeterCompletionsLabel = "meter.completions.label" => "Code",
    MeterCompletionsShort = "meter.completions.short" => "completions",
    MeterCompletionsName = "meter.completions.name" => "Code completions",
    // A Copilot quota this build does not know; its `name` stays the id the service gave.
    MeterMonthlyLabel = "meter.monthly.label" => "Mo",
    MeterMonthlyShort = "meter.monthly.short" => "monthly",

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

/// The Swedish template of every message ([`Swedish`]). The match lists every [`Msg`] with no
/// fallback arm, so a message without a Swedish text does not compile.
const fn swedish(msg: Msg) -> &'static str {
    match msg {
        // ---- Readout tokens (R§5.1–R§5.5)
        Msg::ReasonOffline => "Offline",
        Msg::ReasonSignIn => "Logga in",
        Msg::ReasonPaused => "Pausad",
        Msg::ReasonNoData => "Ingen data",
        Msg::ReasonOldReading => "Ingen ny data",
        Msg::Loading => "Hämtar…",
        // Lower-case like the weekday abbreviations it stands with (`↺idag`, `↺mån`, R§5.9).
        Msg::Today => "idag",
        Msg::Out => "Slut {time}",
        Msg::Back => "Åter {time}",
        Msg::Empty => "Tomt",
        Msg::CountdownMin => "{n} min kvar",
        Msg::CountdownHours => "{n} h kvar",
        Msg::CountdownDays => "{n} d kvar",
        Msg::CountdownCompactMin => "{n} min",
        Msg::CountdownCompactHours => "{n} h",
        Msg::CountdownCompactDays => "{n} d",
        Msg::NoReset => "–",
        Msg::AgeMin => "{n}m sedan",
        Msg::AgeHours => "{n}h sedan",
        Msg::AgeDays => "{n}d sedan",
        Msg::TokenDays => "{n}d",
        Msg::TokenRelMin => "{n}m",
        Msg::TokenRelHours => "{n}h",
        Msg::TokenRelDays => "{n}d",
        Msg::BalanceApprox => "≈",
        Msg::BalanceHours => "{n}h",
        Msg::BalanceDays => "{n}d",
        Msg::BalanceWeeks => "{n}v",
        Msg::BalanceMax => "9v+",

        // ---- Long durations (R§5.0)
        Msg::DurationMin => "{n} min",
        Msg::DurationHours => "{h} h",
        Msg::DurationHoursMin => "{h} h {m} min",
        Msg::DurationDays => "{d} d",
        Msg::DurationDaysHours => "{d} d {h} h",

        // ---- Spoken units with plurals
        Msg::UnitMinuteOne => "{n} minut",
        Msg::UnitMinuteOther => "{n} minuter",
        Msg::UnitHourOne => "{n} timme",
        Msg::UnitHourOther => "{n} timmar",
        Msg::UnitDayOne => "{n} dag",
        Msg::UnitDayOther => "{n} dagar",
        Msg::UnitWeekOne => "{n} vecka",
        Msg::UnitWeekOther => "{n} veckor",
        Msg::UnitCredits => "krediter",

        // ---- Hover card (R§8.2)
        Msg::CardTitleOkOne => "Det räcker till återställningen",
        Msg::CardTitleOkTwo => "Det räcker till båda återställningarna",
        Msg::CardTitleOkMany => "Det räcker till alla återställningar",
        Msg::CardTitleWarning => "{meter} tar slut {time}",
        Msg::CardTitleWarningService => "{meter} för {service} tar slut {time}",
        Msg::CardTitleWarningCourse => "{meter} är på väg mot {p} vid återställningen",
        Msg::CardTitleWarningCourseService => {
            "{meter} för {service} är på väg mot {p} vid återställningen"
        }
        Msg::CardTitleCriticalRunway => "{meter} tar slut om {duration}",
        // Also a balance's title (`krediterna`), so no adjective that agrees with the noun.
        Msg::CardTitleCriticalLevel => "{pct} av {meter} har använts",
        Msg::CardTitleExhausted => "{meter} är slut till {time}",
        Msg::CardTitleStale => "Ingen ny avläsning från {service} på {age}",
        Msg::CardTitleSignIn => "{service} behöver att du loggar in igen",
        Msg::CardTitleApiKey => "{service} behöver en API-nyckel",
        Msg::CardSentenceApiKey => {
            "Lastdrop har ingen fungerande API-nyckel för {service}. Välj visningen i \
             aktivitetsfältet och sedan Ange nyckel… för att lägga till en."
        }
        Msg::CardSentenceOkOne => {
            "Den används långsammare än tiden går. I din senaste takt finns ungefär {left} av \
             {window} kvar när den återställs."
        }
        Msg::CardSentenceOkMany => {
            "Alla gränser används långsammare än tiden går. I din senaste takt finns ungefär \
             {left} kvar när de återställs."
        }
        Msg::CardSentenceOkOneAbout => {
            "Den används ungefär lika snabbt som tiden går. I din senaste takt finns ungefär \
             {left} av {window} kvar när den återställs."
        }
        Msg::CardSentenceOkOneAhead => {
            "Den används snabbare än tiden går, men i din senaste takt finns ungefär {left} av \
             {window} kvar när den återställs."
        }
        Msg::CardSentenceOkManyMixed => {
            "I din senaste takt finns ungefär {left} kvar när de återställs."
        }
        Msg::CardSentenceOkOnePaceUnder => "Den används långsammare än tiden går.",
        Msg::CardSentenceOkOnePaceAbout => "Den används ungefär lika snabbt som tiden går.",
        Msg::CardSentenceOkOnePaceAhead => "Den används snabbare än tiden går.",
        Msg::CardSentenceOkManyPace => "Alla gränser används långsammare än tiden går.",
        Msg::CardSentenceOkOneLeft => {
            "I din senaste takt finns ungefär {left} av {window} kvar när den återställs."
        }
        Msg::CardTitleOkNoReset => "Inom dina gränser",
        // Definite: the templates say `av {window}` ("of the session").
        Msg::CardWindowSession => "sessionen",
        Msg::CardWindowWeek => "veckan",
        Msg::CardWindowMonth => "månaden",
        Msg::CardSentenceWarning => {
            "Du använder den snabbare än tiden går: {pct} använt när {e} av perioden har gått. \
             I takten från de senaste {span} tar den slut {duration} före återställningen \
             {time}."
        }
        Msg::CardSentenceWarningCourse => {
            "Du använder den snabbare än tiden går: {pct} använt när {e} av perioden har gått. \
             I takten från de senaste {span} når den ungefär {p} vid återställningen {time}."
        }
        Msg::CardSentenceOtherFine => " Det är ingen fara med {other}.",
        Msg::CardSentenceCritical => {
            "{pct} använt. I takten från de senaste {span} är den slut ungefär {time}, \
             {duration} före återställningen {reset}."
        }
        Msg::CardSentenceLevel => {
            "Nästan slut. I din senaste takt räcker den till återställningen {time}."
        }
        Msg::CardSentenceExhausted => "Slut sedan {since}. Den är tillbaka {time}, om {duration}.",
        Msg::CardSentenceExhaustedNoSince => "Slut. Den är tillbaka {time}, om {duration}.",
        Msg::CardSentenceOffline => {
            "Lastdrop är offline, så det här är siffrorna från {time}. Staplarna är streckade \
             och frysta vid den tidpunkten tills en ny avläsning kommer. Nästa försök {next}."
        }
        Msg::CardSentenceNoData => {
            "{service} svarar inte ({detail}), så det här är siffrorna från {time}. Nästa \
             försök {next}."
        }
        Msg::CardSentenceOldReading => {
            "{service} svarar, men de senaste siffrorna är från {time}. Nästa försök {next}."
        }
        Msg::CardSentenceSignIn => {
            "Inloggningen till {service} har gått ut. Lastdrop loggar aldrig in åt dig: öppna \
             {client} med det här kontot för att förnya den. Välj visningen i aktivitetsfältet \
             för att se stegen."
        }
        Msg::CardSentencePaused => {
            "Uppdateringarna är pausade till {until}. Det här är siffrorna från {time}."
        }
        Msg::CardSentencePausedOpen => {
            "Uppdateringarna är pausade. Det här är siffrorna från {time}."
        }
        // The card's tick line shares its row with the forecast: short forms.
        Msg::CardLineElapsed => "{e} av tiden gått",
        Msg::CardLineElapsedStale => "{e} av tiden hade gått {time}",
        Msg::CardLineElapsedUnknown => "förfluten tid okänd",
        Msg::CardForecastCourse => "prognos {p} vid återställning",
        Msg::CardForecastLasts => "räcker till återställningen",
        Msg::CardForecastNone => "ingen prognos än",
        Msg::CardForecastRunsOut => "tar slut ≈ {time}",
        Msg::CardForecastUsedUp => "slut",
        Msg::CardLineReset => "Återställs {time}",
        Msg::CardLineIn => "om {duration}",
        Msg::CardLineIdle => "Börjar vid nästa användning",
        Msg::CardLineNotStarted => "perioden har inte börjat",
        Msg::CardLineNoReset => "Ingen återställningstid angiven",
        Msg::CardLineNoLength => "periodens längd okänd",
        Msg::CardLineJustReset => "Återställd {time} · väntar på första avläsningen",
        Msg::CardLineEnded => "Avslutad senast {time}",
        Msg::CardValue => "{pct} använt",
        Msg::CardBalanceValue => "{pct} av påfyllningen använt",
        Msg::CardBalanceLeft => "{amount} av {ref} {unit} kvar",
        Msg::CardBalanceRate => "{burn} per dag, senaste 7 dagarna",
        Msg::CardBalanceLasts => "räcker ≈ {duration}",
        Msg::CardBalanceRunsOut => "tar slut ≈ {time}",
        Msg::CardBalanceEmpty => "tomt",
        Msg::CardBalanceNoUse => "ingen användning på 7 dagar",
        Msg::CardBalanceNoHistory => "inte tillräckligt med historik än",
        Msg::CardBalanceUntil => "till ≈ {date}",
        Msg::CardAge => "Uppdaterad {age}",
        Msg::CardAgeStale => "Senaste avläsning {time}",
        Msg::CardAlso => "Bevakas också",
        Msg::CardAlsoMore => "+{n} till",
        Msg::CardAlsoLine => "{short_name} för {service}",

        // ---- Flyout (F§3–F§6, F§10)
        Msg::FlyoutTitle => "Lastdrop",
        Msg::FlyoutVerdictOk => "Alla gränser räcker till återställningen",
        Msg::FlyoutVerdictOkNoReset => "Inom dina gränser",
        Msg::FlyoutVerdictWarning => "{label} tar slut {time}",
        Msg::FlyoutVerdictWarningCourse => "{label} är på väg mot {p} vid återställningen",
        Msg::FlyoutVerdictRunway => "{label} tar slut om {countdown}",
        // Also a balance's verdict, as `card.title.critical.level`.
        Msg::FlyoutVerdictLevel => "{pct} av {label} har använts",
        Msg::FlyoutVerdictExhausted => "{label} är slut till {time}",
        Msg::FlyoutVerdictStale => "{service}: ingen avläsning på {age}",
        Msg::FlyoutVerdictSignIn => "{service}: logga in igen",
        Msg::FlyoutVerdictPaused => "Uppdateringar pausade",
        Msg::FlyoutVerdictLoading => "Hämtar användning…",
        Msg::FlyoutVerdictMore => " · {n} till",
        Msg::FlyoutRefreshName => "Uppdatera alla tjänster",
        Msg::FlyoutRefreshTooltip => "Uppdatera (F5)",
        Msg::FlyoutSettings => "Inställningar",
        Msg::FlyoutAgeNow => "Nyss",
        Msg::FlyoutAgeMin => "för {n} min sedan",
        Msg::FlyoutAgeHours => "för {n} h sedan",
        Msg::FlyoutAgeDays => "för {n} d sedan",
        Msg::FlyoutMsgSep => " · ",
        Msg::FlyoutMsgOffline => "Offline",
        Msg::FlyoutMsgNoData => "Svarar inte",
        Msg::FlyoutMsgOldReading => "Ingen ny avläsning",
        Msg::FlyoutMsgShowing => "visar {time}",
        Msg::FlyoutMsgNextTry => "nästa försök {time}",
        Msg::FlyoutMsgSignInLead => "Inloggningen har gått ut.",
        Msg::FlyoutMsgSignInText => "Lastdrop loggar aldrig in åt dig.",
        Msg::FlyoutMsgSignInAction => "Öppna inloggning…",
        Msg::FlyoutMsgPaused => "Pausad",
        Msg::FlyoutMsgResume => "Återuppta",
        Msg::FlyoutMsgFailedLead => "Kunde inte läsa {service}",
        Msg::FlyoutMsgRetry => "Försök igen",
        Msg::FlyoutMsgApiKeyLead => "API-nyckeln saknas eller avvisades.",
        Msg::FlyoutMsgApiKeyAction => "Ange nyckel…",
        Msg::FlyoutLineWarning => "Tar slut {duration} före återställningen {time}.",
        Msg::FlyoutLineWarningCourse => "På väg mot {p} vid återställningen {time}.",
        Msg::FlyoutLineCritical => "Slut ≈ {time}, {duration} före återställningen {reset}.",
        Msg::FlyoutLineLevel => "Nästan slut. Räcker till återställningen i din senaste takt.",
        Msg::FlyoutLineExhausted => "Slut sedan {since}. Tillbaka om {duration}.",
        Msg::FlyoutLineExhaustedNoSince => "Slut. Tillbaka om {duration}.",
        Msg::FlyoutBalanceLeft => "≈ {duration} kvar",
        Msg::FlyoutBalanceMax => "Mer än 8 veckor kvar",
        Msg::FlyoutBalanceNoUse => "Ingen användning på 7 dagar",
        Msg::FlyoutBalanceNoHistory => "Inte tillräckligt med historik än",
        // Shares its row with `flyout.balance.until`: short forms.
        Msg::FlyoutBalanceSpent => "{pct} av påfyllningen använt",
        Msg::FlyoutBalanceBurn => " · {burn}/dag",
        Msg::FlyoutBalanceUntil => "till ≈ {date}",
        Msg::FlyoutFooterPaused => "Uppdateringar pausade",
        Msg::FlyoutFooterResume => "Återuppta",
        Msg::FlyoutCardName => "{service}, abonnemang {plan}, uppdaterad {age}",
        Msg::FlyoutCardNameNoPlan => "{service}, uppdaterad {age}",
        Msg::FlyoutCardOffline => ", offline",
        Msg::FlyoutCardSignIn => ", inloggningen har gått ut",
        Msg::FlyoutBalanceName => {
            "{amount} {unit} kvar, {n} procent av senaste påfyllningen använt, ungefär \
             {duration} med {burn} per dag"
        }
        Msg::FlyoutSparkWindow => {
            "Användning av {meter} per dag de senaste 14 dagarna: idag {today} procent, högst \
             {max} procent ({day}), i genomsnitt {avg} procent."
        }
        Msg::FlyoutSparkBalance => {
            "{unit} använda per dag de senaste 14 dagarna: idag {today}, högst {max} ({day}), \
             i genomsnitt {avg}."
        }

        // ---- UI Automation (R§11)
        Msg::A11yRoot => "Lastdrop. {rows}",
        Msg::A11yHelp => "Välj för detaljer, högerklicka för alternativ.",
        Msg::A11yAt => "kl. {time}",
        Msg::A11yAgo => "för {duration} sedan",
        Msg::A11yRowOk => "{service} {meter}: i fas, {n} procent använt, återställs {reset}.",
        Msg::A11yRowWarning => {
            "{service} {meter}: varning, tar slut ungefär {time}, före återställningen {reset}; \
             {n} procent använt."
        }
        Msg::A11yRowWarningCourse => {
            "{service} {meter}: varning, på väg mot {course} procent vid återställningen \
             {reset}; {n} procent använt."
        }
        Msg::A11yRowRunway => {
            "{service} {meter}: kritisk, tar slut om ungefär {duration}; {n} procent använt; \
             återställs {reset}."
        }
        Msg::A11yRowLevel => {
            "{service} {meter}: kritisk, {n} procent använt, räcker till återställningen \
             {reset}."
        }
        Msg::A11yRowExhausted => "{service} {meter}: slut, tillbaka {time}.",
        Msg::A11yRowExhaustedNoReset => {
            "{service} {meter}: slut, {n} procent använt, ingen återställningstid angiven."
        }
        Msg::A11yRowIdle => {
            "{service} {meter}: inte påbörjad, {n} procent använt, börjar med ditt nästa \
             meddelande."
        }
        Msg::A11yRowNoReset => {
            "{service} {meter}: {n} procent använt, ingen återställningstid angiven."
        }
        Msg::A11yRowLoading => "{service} {meter}: hämtar.",
        Msg::A11yRowEnded => "{service} {meter}: avslutad senast {time}, ingen nyare avläsning.",
        Msg::A11yServiceOffline => {
            "{service}: offline, senaste avläsning för {age} sedan. Då: {rows}."
        }
        Msg::A11yServiceNoData => {
            "{service}: svarar inte, senaste avläsning för {age} sedan. Då: {rows}."
        }
        Msg::A11yServiceOldReading => {
            "{service}: ingen ny avläsning, senaste avläsning för {age} sedan. Då: {rows}."
        }
        Msg::A11yServicePaused => {
            "{service}: uppdateringar pausade, senaste avläsning för {age} sedan. Då: {rows}."
        }
        Msg::A11yServiceSignIn => {
            "{service}: inloggningen har gått ut, senaste avläsning för {age} sedan. Välj för \
             att öppna Lastdrop och se hur du loggar in."
        }
        Msg::A11yServiceApiKey => {
            "{service}: API-nyckeln saknas eller avvisades. Välj för att öppna Lastdrop och \
             lägga till en."
        }
        Msg::A11yStaleRow => "{meter} {n} procent använt",
        Msg::A11yBalance => "{service}: {amount} {unit} kvar, ungefär {duration} i nuvarande takt.",
        Msg::A11yBalanceNoForecast => "{service}: {amount} {unit} kvar.",
        Msg::A11yListSep => ", ",

        // ---- Tray tooltip (R§9.3)
        Msg::TrayOk => "Lastdrop: {pct} av {meter} för {service} använt, återställs {time}",
        Msg::TrayNoReset => "Lastdrop: {pct} av {meter} för {service} använt",
        Msg::TrayWarning => "Lastdrop: {meter} för {service} tar slut {time}",
        Msg::TrayRunway => "Lastdrop: {meter} för {service} tar slut om {duration}",
        Msg::TrayLevel => "Lastdrop: {pct} av {meter} för {service} använt",
        Msg::TrayExhausted => "Lastdrop: {meter} för {service} är slut till {time}",
        Msg::TrayOffline => "Lastdrop: {service} offline, senaste avläsning för {age} sedan",
        Msg::TrayNoData => "Lastdrop: {service} svarar inte, senaste avläsning för {age} sedan",
        Msg::TrayOldReading => {
            "Lastdrop: {service} har ingen ny avläsning, senaste avläsning för {age} sedan"
        }
        Msg::TraySignIn => "Lastdrop: {service} behöver att du loggar in igen",
        Msg::TrayApiKey => "Lastdrop: {service} behöver en API-nyckel",
        Msg::TrayPaused => "Lastdrop: uppdateringar pausade",
        Msg::TrayLoading => "Lastdrop: hämtar användning…",
        Msg::TrayNothing => "Lastdrop",

        // ---- Notifications (N§3)
        Msg::NoticeLevelTitle => "{meter} för {service} är till {pct} använd",
        Msg::NoticeUsedUpTitle => "{meter} för {service} är slut",
        Msg::NoticeResetTitle => "{meter} för {service} har återställts",
        Msg::NoticeBalanceLowTitle => "{meter} för {service} börjar ta slut",
        Msg::NoticeBalanceEmptyTitle => "{meter} för {service} är slut",
        Msg::NoticeBodyRunsOut => {
            "I din senaste takt tar den slut {time}, {duration} före återställningen."
        }
        Msg::NoticeBodyLasts => "I din senaste takt räcker den till återställningen {time}.",
        Msg::NoticeBodyResets => "Den återställs {time}.",
        Msg::NoticeBodyBack => "Den är tillbaka {time}, om {duration}.",
        Msg::NoticeBodyNoReset => "Välj för att se detaljerna i Lastdrop.",
        Msg::NoticeBodyReset => "Tillbaka på {pct} använt.",
        Msg::NoticeBodyResetUsedUp => "Tillbaka på {pct} använt. Du kan använda den igen.",
        Msg::NoticeBodyBalance => {
            "{amount} {unit} kvar, ungefär {duration} med din användning de senaste 7 dagarna."
        }
        Msg::NoticeBodyBalanceLeft => "{amount} {unit} kvar.",
        Msg::NoticeBodyBalanceEmpty => "Fyll på för att fortsätta använda {service}.",
        Msg::NoticeMore => "+{n} till",

        // ---- Meter names (R§2): `short_name` is the definite noun with "limit" in it
        Msg::MeterFiveHourLabel => "5h",
        Msg::MeterFiveHourShort => "5-timmarsgränsen",
        Msg::MeterFiveHourName => "5-timmarssession",
        Msg::MeterWeeklyLabel => "Vk",
        Msg::MeterWeeklyShort => "veckogränsen",
        Msg::MeterWeeklyName => "Vecka",
        Msg::MeterWeeklyAllName => "Vecka · alla modeller",
        Msg::MeterWeeklyModelShort => "{model}-veckogränsen",
        Msg::MeterWeeklyModelName => "Vecka · {model}",
        Msg::MeterHoursLabel => "{n}h",
        Msg::MeterHoursShort => "{n}-timmarsgränsen",
        Msg::MeterHoursName => "{n}-timmarsfönster",
        Msg::MeterDaysLabel => "{n}d",
        Msg::MeterDaysShort => "{n}-dagarsgränsen",
        Msg::MeterDaysName => "{n}-dagarsfönster",
        // Not `Kr`, which reads as kronor.
        Msg::MeterCreditsLabel => "Kred",
        Msg::MeterCreditsShort => "krediterna",
        Msg::MeterCreditsName => "Krediter",
        Msg::MeterPremiumLabel => "Prem",
        Msg::MeterPremiumShort => "premiumgränsen",
        Msg::MeterPremiumName => "Premiumförfrågningar",
        Msg::MeterChatLabel => "Chatt",
        Msg::MeterChatShort => "chattgränsen",
        Msg::MeterChatName => "Chattmeddelanden",
        Msg::MeterCompletionsLabel => "Kod",
        Msg::MeterCompletionsShort => "kompletteringsgränsen",
        Msg::MeterCompletionsName => "Kodkompletteringar",
        // Not `Mån`, which reads as Monday next to the weekday tokens.
        Msg::MeterMonthlyLabel => "Mnd",
        Msg::MeterMonthlyShort => "månadsgränsen",

        // ---- Context menus (R§10, F§9)
        Msg::MenuRefresh => "Uppdatera nu",
        Msg::MenuPause => "Pausa uppdateringar",
        Msg::MenuPauseHour => "I 1 timme",
        Msg::MenuPauseUntilResume => "Tills jag återupptar",
        Msg::MenuResume => "Återuppta uppdateringar",
        Msg::MenuReadout => "Visning i aktivitetsfältet",
        Msg::MenuReadoutAuto => "Automatiskt",
        Msg::MenuReadoutPinned => "Fästa mätare…",
        Msg::MenuReadoutOff => "Av",
        Msg::MenuSettings => "Inställningar…",
        Msg::MenuAbout => "Om Lastdrop",
        Msg::MenuQuit => "Avsluta Lastdrop",
        Msg::MenuCardUsage => "Öppna användningssidan",
        Msg::MenuCardRefresh => "Uppdatera {service}",
        Msg::MenuCardReadout => "Visa i aktivitetsfältet",
    }
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
        Language::English.tag()
    }

    fn template(&self, msg: Msg) -> &str {
        english(msg)
    }

    fn plural(&self, n: i64) -> Plural {
        if n == 1 { Plural::One } else { Plural::Other }
    }
}

/// The Swedish table. Numbers, times and weekday names come from the regional format
/// ([`super::LocaleFormat`], `sv-SE`: `42 %`, `mån`), so only the words are here.
///
/// `short_name` is a definite noun that already says "limit" (`veckogränsen`, `krediterna`),
/// so the templates do not add it, and a meter with its service reads
/// `veckogränsen för Claude` (`card.also.line`). Percent is always `använt`/`har använts`, never
/// `kvar`, except the share left at the reset of an OK card, as in English.
#[derive(Clone, Copy, Debug, Default)]
pub struct Swedish;

impl Strings for Swedish {
    fn language(&self) -> &str {
        Language::Swedish.tag()
    }

    fn template(&self, msg: Msg) -> &str {
        swedish(msg)
    }

    fn plural(&self, n: i64) -> Plural {
        // Swedish has the same two forms as English (`1 minut`, `2 minuter`).
        if n == 1 { Plural::One } else { Plural::Other }
    }
}

/// A language with a string table (ROADMAP M4 "Language: English and Swedish to start").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Language {
    /// [`English`], and the fallback for every language without a table.
    #[default]
    English,
    /// [`Swedish`].
    Swedish,
}

impl Language {
    /// Every language with a table, in the order a language picker lists them.
    pub const ALL: [Language; 2] = [Language::English, Language::Swedish];

    /// The BCP 47 language tag (`en`, `sv`): [`Strings::language`] and the value of the
    /// language setting.
    pub const fn tag(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::Swedish => "sv",
        }
    }

    /// The language's name in itself (`English`, `Svenska`), as language pickers show it.
    pub const fn native_name(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Swedish => "Svenska",
        }
    }

    /// The string table.
    pub fn strings(self) -> &'static dyn Strings {
        match self {
            Language::English => &English,
            Language::Swedish => &Swedish,
        }
    }

    /// The language of a BCP 47 tag by its primary subtag, in any case (`sv`, `sv-SE` and
    /// `sv-FI` are Swedish); `None` for a language without a table.
    pub fn from_tag(tag: &str) -> Option<Language> {
        let primary = tag.split(['-', '_']).next().unwrap_or_default();
        Language::ALL
            .into_iter()
            .find(|l| l.tag().eq_ignore_ascii_case(primary))
    }

    /// The language to show: `chosen` (the setting) when it has a table; otherwise the first of
    /// Windows' preferred display languages (`ui_languages`, most preferred first) that has one;
    /// otherwise English. A tag without a table (a newer build's setting) follows Windows too.
    pub fn resolve(chosen: Option<&str>, ui_languages: &[String]) -> Language {
        chosen
            .and_then(Language::from_tag)
            .or_else(|| ui_languages.iter().find_map(|tag| Language::from_tag(tag)))
            .unwrap_or_default()
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
            assert!(
                !Swedish.template(*m).contains("{pct} procent"),
                "{}",
                m.key()
            );
        }
    }

    /// The `{name}` placeholders of a template, sorted, each once.
    fn placeholders(template: &str) -> Vec<&str> {
        let mut names: Vec<&str> = template
            .split('{')
            .skip(1)
            .filter_map(|part| part.split_once('}').map(|(name, _)| name))
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    #[test]
    fn every_message_has_a_swedish_template_with_the_english_placeholders() {
        for m in Msg::ALL {
            let sv = Swedish.template(*m);
            assert!(!sv.is_empty(), "{}", m.key());
            assert_eq!(
                placeholders(sv),
                placeholders(English.template(*m)),
                "{}: {sv}",
                m.key()
            );
        }
    }

    #[test]
    fn swedish_keeps_the_shape_of_each_english_template() {
        // Fragments joined to other text keep their leading space or separator, sentences
        // their full stop, so a joined Swedish line reads as the English one does.
        for m in Msg::ALL {
            let (en, sv) = (English.template(*m), Swedish.template(*m));
            assert_eq!(
                en.starts_with(' '),
                sv.starts_with(' '),
                "{}: {sv:?}",
                m.key()
            );
            assert_eq!(en.starts_with(", "), sv.starts_with(", "), "{}", m.key());
            assert_eq!(en.ends_with('.'), sv.ends_with('.'), "{}: {sv:?}", m.key());
            assert_eq!(en.ends_with('…'), sv.ends_with('…'), "{}: {sv:?}", m.key());
        }
    }

    #[test]
    fn swedish_never_says_percent_left() {
        // R§5: percent is always used. The only share left is the OK card's `{left}` at the
        // reset, as in English.
        for m in Msg::ALL {
            let sv = Swedish.template(*m);
            for pct in ["{pct}", "{p}", "{e}", "{n} procent"] {
                assert!(!sv.contains(&format!("{pct} kvar")), "{}: {sv}", m.key());
            }
        }
    }

    #[test]
    fn languages_resolve_from_the_setting_then_windows_then_english() {
        let ui = |tags: &[&str]| tags.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
        assert_eq!(Language::from_tag("sv-SE"), Some(Language::Swedish));
        assert_eq!(Language::from_tag("SV"), Some(Language::Swedish));
        assert_eq!(Language::from_tag("sv-FI"), Some(Language::Swedish));
        assert_eq!(Language::from_tag("en_GB"), Some(Language::English));
        assert_eq!(Language::from_tag("de-DE"), None);
        assert_eq!(Language::from_tag(""), None);

        assert_eq!(
            Language::resolve(None, &ui(&["sv-SE", "en-US"])),
            Language::Swedish
        );
        assert_eq!(
            Language::resolve(Some("en"), &ui(&["sv-SE"])),
            Language::English
        );
        assert_eq!(
            Language::resolve(Some("sv"), &ui(&["en-US"])),
            Language::Swedish
        );
        // A display language without a table falls through to the next one, then to English.
        assert_eq!(
            Language::resolve(None, &ui(&["de-DE", "sv-SE"])),
            Language::Swedish
        );
        assert_eq!(Language::resolve(None, &ui(&["de-DE"])), Language::English);
        assert_eq!(Language::resolve(None, &[]), Language::English);
        // A newer build's language follows Windows.
        assert_eq!(
            Language::resolve(Some("de"), &ui(&["sv-SE"])),
            Language::Swedish
        );

        for l in Language::ALL {
            assert_eq!(l.strings().language(), l.tag());
            assert_eq!(Language::from_tag(l.tag()), Some(l));
        }
    }

    #[test]
    fn swedish_plurals_are_one_and_other() {
        assert_eq!(Swedish.plural(1), Plural::One);
        assert_eq!(Swedish.plural(0), Plural::Other);
        assert_eq!(Swedish.plural(2), Plural::Other);
        assert_eq!(
            text(&Swedish, Msg::UnitMinuteOther, &[("n", "5")]),
            "5 minuter"
        );
    }
}
