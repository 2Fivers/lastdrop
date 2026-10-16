//! Every string is a resource (R§5.9, R Appendix B, flyout-spec texts, R§10 menu, R§11 UIA).
//!
//! A [`Msg`] names a template and a [`PluralMsg`] a template with plural forms; a [`Strings`]
//! table gives the templates of one language; [`fill`] substitutes `{name}` placeholders. Column
//! widths are computed from the localized strings (R§6.3), so a translation can never be cut off.
//!
//! The texts live in `crates/core/locales`, one TOML file per language, with `en.toml` as the
//! source (ROADMAP M4 "Language"). The build script checks every file (each key of `en.toml`, the
//! English placeholders, the forms of the language's [`PluralRule`]) and turns them into the
//! [`Language`] enum, a table struct per language ([`English`], [`Swedish`]) and arrays of
//! `&'static str`. Each message finds its place in those arrays at compile time: a key the files
//! lack does not compile, and nothing is parsed or allocated at run time. Adding a language is
//! adding a file (`crates/core/locales/README.md`).
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
//! model's display name (`meter.*`); `{language}` a language's own name ([`Language::native_name`])
//! and `{version}` the app's version (`settings.*`, `about.*`); `{email}` the e-mail address an
//! account is signed in with and `{name}` an account's name (accounts-spec §4, `settings.accounts.*`);
//! `{group}` a model pool as the service names it and `{initial}` its first letter (`meter.group.*`).
//!
//! Titles, verdicts, notification titles, "Also tracked" lines and the settings window's meter
//! rows are capitalised as a whole after filling ([`capitalise`]; Appendix B: `{meter}`
//! "capitalised at the start of a sentence"), so a template may start with `{meter}`, `{label}`
//! or `{short_name}` in any language.

use std::cmp::Ordering;

pub use super::plural::{Plural, PluralRule};

/// Declares a catalogue of string resources: the enum, each one's key in the locale files and its
/// place in the generated tables (`$keys`, every key of that kind, sorted by bytes).
macro_rules! catalogue {
    ($(#[$meta:meta])* $enum:ident in $keys:ident { $( $name:ident = $key:literal, )* }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $enum {
            $(
                #[doc = concat!("`", $key, "`")]
                $name,
            )*
        }

        impl $enum {
            /// Every message, in declaration order (for completeness tests of a language table).
            pub const ALL: &'static [$enum] = &[$($enum::$name,)*];

            /// The resource key (`reason.offline`), as the locale files name it.
            pub const fn key(self) -> &'static str {
                match self { $($enum::$name => $key,)* }
            }

            /// The message's place in every language's table, found at compile time.
            const fn index(self) -> usize {
                match self { $($enum::$name => const { index_of(&$keys, $key) },)* }
            }
        }

        // Every key of that kind in the files belongs to exactly one message.
        const _: () = {
            let mut seen = [false; $keys.len()];
            let mut i = 0;
            while i < $enum::ALL.len() {
                let at = $enum::ALL[i].index();
                assert!(!seen[at], concat!("two ", stringify!($enum), "s share a key"));
                seen[at] = true;
                i += 1;
            }
            assert!(
                $enum::ALL.len() == $keys.len(),
                concat!(
                    "crates/core/locales/en.toml has a key that no ",
                    stringify!($enum),
                    " declares (grammar/strings.rs)"
                )
            );
        };
    };
}

/// `a` against `b` by bytes, the order `build.rs` sorts the keys in.
const fn compare(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut i = 0;
    while i < a.len() && i < b.len() {
        if a[i] != b[i] {
            return if a[i] < b[i] {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        i += 1;
    }
    if a.len() < b.len() {
        Ordering::Less
    } else if a.len() > b.len() {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

/// The place of `key` in `keys` (sorted by bytes). Evaluated at compile time, so a key that
/// `crates/core/locales/en.toml` lacks stops the build with the key as the message.
const fn index_of(keys: &[&str], key: &str) -> usize {
    let (mut low, mut high) = (0, keys.len());
    while low < high {
        let mid = low + (high - low) / 2;
        match compare(keys[mid], key) {
            Ordering::Less => low = mid + 1,
            Ordering::Greater => high = mid,
            Ordering::Equal => return mid,
        }
    }
    panic!("{}", key)
}

catalogue! {
    /// A string resource. The doc of each variant is its key (Appendix B style); the texts are in
    /// `crates/core/locales`.
    Msg in KEYS {
        // ---- Readout tokens (R§5.1–R§5.5, Appendix B)
        ReasonOffline = "reason.offline",
        ReasonSignIn = "reason.signin",
        ReasonPaused = "reason.paused",
        ReasonNoData = "reason.nodata",
        ReasonOldReading = "reason.oldreading",
        Loading = "loading",
        Today = "today",
        Out = "out",
        Back = "back",
        Empty = "empty",
        CountdownMin = "countdown.min",
        CountdownHours = "countdown.h",
        CountdownDays = "countdown.d",
        CountdownCompactMin = "countdown.compact.min",
        CountdownCompactHours = "countdown.compact.h",
        CountdownCompactDays = "countdown.compact.d",
        NoReset = "noreset",
        AgeMin = "age.m",
        AgeHours = "age.h",
        AgeDays = "age.d",
        TokenDays = "token.days",
        TokenRelMin = "token.rel.m",
        TokenRelHours = "token.rel.h",
        TokenRelDays = "token.rel.d",
        BalanceApprox = "balance.approx",
        BalanceHours = "balance.h",
        BalanceDays = "balance.d",
        BalanceWeeks = "balance.w",
        BalanceMax = "balance.max",

        // ---- Long durations (R§5.0)
        DurationMin = "duration.min",
        DurationHours = "duration.h",
        DurationHoursMin = "duration.hmin",
        DurationDays = "duration.d",
        DurationDaysHours = "duration.dh",

        // ---- Units (the counted ones are `PluralMsg`s)
        UnitCredits = "unit.credits",

        // ---- Hover card (R§8.2, Appendix B)
        CardTitleOkOne = "card.title.ok.1",
        CardTitleOkTwo = "card.title.ok.2",
        CardTitleOkMany = "card.title.ok.n",
        CardTitleWarning = "card.title.warning",
        CardTitleWarningService = "card.title.warning.service",
        CardTitleWarningCourse = "card.title.warning.course",
        CardTitleWarningCourseService = "card.title.warning.course.service",
        CardTitleCriticalRunway = "card.title.critical.runway",
        CardTitleCriticalLevel = "card.title.critical.level",
        CardTitleExhausted = "card.title.exhausted",
        CardTitleStale = "card.title.stale",
        CardTitleSignIn = "card.title.signin",
        CardTitleApiKey = "card.title.apikey",
        CardSentenceApiKey = "card.sentence.apikey",
        CardSentenceOkOne = "card.sentence.ok.1",
        CardSentenceOkMany = "card.sentence.ok.n",
        CardSentenceOkOneAbout = "card.sentence.ok.1.about",
        CardSentenceOkOneAhead = "card.sentence.ok.1.ahead",
        CardSentenceOkManyMixed = "card.sentence.ok.n.mixed",
        CardSentenceOkOnePaceUnder = "card.sentence.ok.1.pace.under",
        CardSentenceOkOnePaceAbout = "card.sentence.ok.1.pace.about",
        CardSentenceOkOnePaceAhead = "card.sentence.ok.1.pace.ahead",
        CardSentenceOkManyPace = "card.sentence.ok.n.pace",
        CardSentenceOkOneLeft = "card.sentence.ok.1.left",
        CardTitleOkNoReset = "card.title.ok.noreset",
        CardWindowSession = "card.window.session",
        CardWindowWeek = "card.window.week",
        CardWindowMonth = "card.window.month",
        CardSentenceWarning = "card.sentence.warning",
        CardSentenceWarningCourse = "card.sentence.warning.course",
        CardSentenceOtherFine = "card.sentence.other",
        CardSentenceCritical = "card.sentence.critical",
        CardSentenceLevel = "card.sentence.level",
        CardSentenceExhausted = "card.sentence.exhausted",
        CardSentenceExhaustedNoSince = "card.sentence.exhausted.nosince",
        CardSentenceOffline = "card.sentence.offline",
        CardSentenceNoData = "card.sentence.nodata",
        CardSentenceOldReading = "card.sentence.oldreading",
        CardSentenceSignIn = "card.sentence.signin",
        CardSentencePaused = "card.sentence.paused",
        CardSentencePausedOpen = "card.sentence.paused.open",
        CardLineElapsed = "card.line.elapsed",
        CardLineElapsedStale = "card.line.elapsed.stale",
        CardLineElapsedUnknown = "card.line.elapsed.unknown",
        CardForecastCourse = "card.line.forecast.course",
        CardForecastLasts = "card.line.forecast.lasts",
        CardForecastNone = "card.line.forecast.none",
        CardForecastRunsOut = "card.line.forecast.runsout",
        CardForecastUsedUp = "card.line.forecast.usedup",
        CardLineReset = "card.line.reset",
        CardLineIn = "card.line.in",
        CardLineIdle = "card.line.idle",
        CardLineNotStarted = "card.line.notstarted",
        CardLineNoReset = "card.line.noreset",
        CardLineNoLength = "card.line.nolength",
        CardLineJustReset = "card.line.justreset",
        CardLineEnded = "card.line.ended",
        CardValue = "card.value",
        CardBalanceValue = "card.balance.value",
        CardBalanceLeft = "card.balance.left",
        CardBalanceRate = "card.balance.rate",
        CardBalanceLasts = "card.balance.forecast.lasts",
        CardBalanceRunsOut = "card.balance.forecast.runsout",
        CardBalanceEmpty = "card.balance.forecast.empty",
        CardBalanceNoUse = "card.balance.forecast.nouse",
        CardBalanceNoHistory = "card.balance.forecast.nohistory",
        CardBalanceUntil = "card.balance.until",
        CardAge = "card.age",
        CardAgeStale = "card.age.stale",
        CardAlso = "card.also",
        CardAlsoMore = "card.also.more",
        CardAlsoLine = "card.also.line",

        // ---- Flyout (F§3–F§6, F§10)
        FlyoutTitle = "flyout.title",
        FlyoutVerdictOk = "flyout.verdict.ok",
        FlyoutVerdictOkNoReset = "flyout.verdict.ok.noreset",
        FlyoutVerdictWarning = "flyout.verdict.warning",
        FlyoutVerdictWarningCourse = "flyout.verdict.warning.course",
        FlyoutVerdictRunway = "flyout.verdict.critical.runway",
        FlyoutVerdictLevel = "flyout.verdict.critical.level",
        FlyoutVerdictExhausted = "flyout.verdict.exhausted",
        FlyoutVerdictStale = "flyout.verdict.stale",
        FlyoutVerdictSignIn = "flyout.verdict.signin",
        FlyoutVerdictPaused = "flyout.verdict.paused",
        FlyoutVerdictLoading = "flyout.verdict.loading",
        FlyoutVerdictMore = "flyout.verdict.more",
        FlyoutRefreshName = "flyout.refresh.a11y",
        FlyoutRefreshTooltip = "flyout.refresh.tooltip",
        FlyoutSettings = "flyout.settings",
        FlyoutAgeNow = "flyout.age.now",
        FlyoutAgeMin = "flyout.age.min",
        FlyoutAgeHours = "flyout.age.h",
        FlyoutAgeDays = "flyout.age.d",
        FlyoutMsgSep = "flyout.msg.sep",
        FlyoutMsgOffline = "flyout.msg.offline",
        FlyoutMsgNoData = "flyout.msg.nodata",
        FlyoutMsgOldReading = "flyout.msg.oldreading",
        FlyoutMsgShowing = "flyout.msg.showing",
        FlyoutMsgNextTry = "flyout.msg.nexttry",
        FlyoutMsgSignInLead = "flyout.msg.signin.lead",
        FlyoutMsgSignInText = "flyout.msg.signin.text",
        FlyoutMsgSignInAction = "flyout.msg.signin.action",
        FlyoutMsgPaused = "flyout.msg.paused",
        FlyoutMsgResume = "flyout.msg.resume",
        FlyoutMsgFailedLead = "flyout.msg.failed.lead",
        FlyoutMsgRetry = "flyout.msg.retry",
        FlyoutMsgApiKeyLead = "flyout.msg.apikey.lead",
        FlyoutMsgApiKeyAction = "flyout.msg.apikey.action",
        FlyoutLineWarning = "flyout.line.warning",
        FlyoutLineWarningCourse = "flyout.line.warning.course",
        FlyoutLineCritical = "flyout.line.critical",
        FlyoutLineLevel = "flyout.line.level",
        FlyoutLineExhausted = "flyout.line.exhausted",
        FlyoutLineExhaustedNoSince = "flyout.line.exhausted.nosince",
        FlyoutBalanceLeft = "flyout.balance.left",
        FlyoutBalanceMax = "flyout.balance.max",
        FlyoutBalanceNoUse = "flyout.balance.nouse",
        FlyoutBalanceNoHistory = "flyout.balance.nohistory",
        FlyoutBalanceSpent = "flyout.balance.spent",
        FlyoutBalanceBurn = "flyout.balance.burn",
        FlyoutBalanceUntil = "flyout.balance.until",
        FlyoutFooterPaused = "flyout.footer.paused",
        FlyoutFooterResume = "flyout.footer.resume",
        FlyoutCardName = "flyout.card.a11y",
        FlyoutCardNameNoPlan = "flyout.card.a11y.noplan",
        FlyoutCardOffline = "flyout.card.a11y.offline",
        FlyoutCardSignIn = "flyout.card.a11y.signin",
        FlyoutBalanceName = "flyout.balance.a11y",
        FlyoutSparkWindow = "flyout.spark.a11y.window",
        FlyoutSparkBalance = "flyout.spark.a11y.balance",

        // ---- UI Automation (R§11)
        A11yRoot = "a11y.root",
        A11yHelp = "a11y.help",
        A11yAt = "a11y.at",
        A11yAgo = "a11y.ago",
        A11yRowOk = "a11y.row.ok",
        A11yRowWarning = "a11y.row.warning",
        A11yRowWarningCourse = "a11y.row.warning.course",
        A11yRowRunway = "a11y.row.critical.runway",
        A11yRowLevel = "a11y.row.critical.level",
        A11yRowExhausted = "a11y.row.exhausted",
        A11yRowExhaustedNoReset = "a11y.row.exhausted.noreset",
        A11yRowIdle = "a11y.row.idle",
        A11yRowNoReset = "a11y.row.noreset",
        A11yRowLoading = "a11y.row.loading",
        A11yRowEnded = "a11y.row.ended",
        A11yServiceOffline = "a11y.service.offline",
        A11yServiceNoData = "a11y.service.nodata",
        A11yServiceOldReading = "a11y.service.oldreading",
        A11yServicePaused = "a11y.service.paused",
        A11yServiceSignIn = "a11y.service.signin",
        A11yServiceApiKey = "a11y.service.apikey",
        A11yStaleRow = "a11y.stale.row",
        A11yBalance = "a11y.balance",
        A11yBalanceNoForecast = "a11y.balance.noforecast",
        A11yListSep = "a11y.list.sep",

        // ---- Tray tooltip (R§9.3)
        TrayOk = "tray.ok",
        TrayNoReset = "tray.noreset",
        TrayWarning = "tray.warning",
        TrayRunway = "tray.runway",
        TrayLevel = "tray.level",
        TrayExhausted = "tray.exhausted",
        TrayOffline = "tray.offline",
        TrayNoData = "tray.nodata",
        TrayOldReading = "tray.oldreading",
        TraySignIn = "tray.signin",
        TrayApiKey = "tray.apikey",
        TrayPaused = "tray.paused",
        TrayLoading = "tray.loading",
        TrayNothing = "tray.nothing",

        // ---- Notifications (N§3)
        NoticeLevelTitle = "notice.level.title",
        NoticeUsedUpTitle = "notice.usedup.title",
        NoticeResetTitle = "notice.reset.title",
        NoticeBalanceLowTitle = "notice.balance.low.title",
        NoticeBalanceEmptyTitle = "notice.balance.empty.title",
        NoticeBodyRunsOut = "notice.body.runsout",
        NoticeBodyLasts = "notice.body.lasts",
        NoticeBodyResets = "notice.body.resets",
        NoticeBodyBack = "notice.body.back",
        NoticeBodyNoReset = "notice.body.noreset",
        NoticeBodyReset = "notice.body.reset",
        NoticeBodyResetUsedUp = "notice.body.reset.usedup",
        NoticeBodyBalance = "notice.body.balance",
        NoticeBodyBalanceLeft = "notice.body.balance.left",
        NoticeBodyBalanceEmpty = "notice.body.balance.empty",
        NoticeMore = "notice.more",

        // ---- Meter names (R§2 table: `short_label` / `short_name` / `name`; [`super::meters`])
        MeterFiveHourLabel = "meter.five_hour.label",
        MeterFiveHourShort = "meter.five_hour.short",
        MeterFiveHourName = "meter.five_hour.name",
        MeterWeeklyLabel = "meter.weekly.label",
        MeterWeeklyShort = "meter.weekly.short",
        MeterWeeklyName = "meter.weekly.name",
        MeterWeeklyAllName = "meter.weekly.all.name",
        MeterWeeklyModelShort = "meter.weekly.model.short",
        MeterWeeklyModelName = "meter.weekly.model.name",
        MeterHoursLabel = "meter.hours.label",
        MeterHoursShort = "meter.hours.short",
        MeterHoursName = "meter.hours.name",
        MeterDaysLabel = "meter.days.label",
        MeterDaysShort = "meter.days.short",
        MeterDaysName = "meter.days.name",
        MeterCreditsLabel = "meter.credits.label",
        MeterCreditsShort = "meter.credits.short",
        MeterCreditsName = "meter.credits.name",
        MeterPremiumLabel = "meter.premium.label",
        MeterPremiumShort = "meter.premium.short",
        MeterPremiumName = "meter.premium.name",
        MeterChatLabel = "meter.chat.label",
        MeterChatShort = "meter.chat.short",
        MeterChatName = "meter.chat.name",
        MeterCompletionsLabel = "meter.completions.label",
        MeterCompletionsShort = "meter.completions.short",
        MeterCompletionsName = "meter.completions.name",
        MeterMonthlyLabel = "meter.monthly.label",
        MeterMonthlyShort = "meter.monthly.short",
        MeterMonthlyName = "meter.monthly.name",
        MeterAutoLabel = "meter.auto.label",
        MeterAutoShort = "meter.auto.short",
        MeterAutoName = "meter.auto.name",
        MeterApiLabel = "meter.api.label",
        MeterApiShort = "meter.api.short",
        MeterApiName = "meter.api.name",
        MeterIncludedLabel = "meter.included.label",
        MeterIncludedShort = "meter.included.short",
        MeterIncludedName = "meter.included.name",
        MeterOnDemandLabel = "meter.on_demand.label",
        MeterOnDemandShort = "meter.on_demand.short",
        MeterOnDemandName = "meter.on_demand.name",
        MeterPrepaidLabel = "meter.prepaid.label",
        MeterPrepaidShort = "meter.prepaid.short",
        MeterPrepaidName = "meter.prepaid.name",
        MeterGroupFiveHourLabel = "meter.group.five_hour.label",
        MeterGroupFiveHourShort = "meter.group.five_hour.short",
        MeterGroupFiveHourName = "meter.group.five_hour.name",
        MeterGroupWeeklyLabel = "meter.group.weekly.label",
        MeterGroupWeeklyShort = "meter.group.weekly.short",
        MeterGroupWeeklyName = "meter.group.weekly.name",

        // ---- Context menus (R§10, F§9)
        MenuRefresh = "menu.refresh",
        MenuPause = "menu.pause",
        MenuPauseHour = "menu.pause.hour",
        MenuPauseUntilResume = "menu.pause.resume",
        MenuResume = "menu.resume",
        MenuReadout = "menu.readout",
        MenuReadoutAuto = "menu.readout.auto",
        MenuReadoutPinned = "menu.readout.pinned",
        MenuReadoutOff = "menu.readout.off",
        MenuSettings = "menu.settings",
        MenuAbout = "menu.about",
        MenuQuit = "menu.quit",
        MenuCardUsage = "menu.card.usage",
        MenuCardRefresh = "menu.card.refresh",
        MenuCardReadout = "menu.card.readout",

        // ---- Settings window (ROADMAP M1 "settings basics"; out of both specs, F§12)
        SettingsTitle = "settings.title",
        SettingsHeading = "settings.heading",
        SettingsServices = "settings.services",
        SettingsServicesHint = "settings.services.hint",
        SettingsKey = "settings.key",
        SettingsKeyPaste = "settings.key.paste",
        SettingsKeyReplace = "settings.key.replace",
        SettingsKeySave = "settings.key.save",
        SettingsKeyRemove = "settings.key.remove",
        SettingsKeyHint = "settings.key.hint",
        SettingsKeySaved = "settings.key.saved",
        SettingsKeyNotSaved = "settings.key.notsaved",
        SettingsKeyRemoved = "settings.key.removed",
        SettingsKeyNotRemoved = "settings.key.notremoved",
        SettingsRefresh = "settings.refresh",
        SettingsRefreshCheck = "settings.refresh.check",
        SettingsRefreshManual = "settings.refresh.manual",
        SettingsReadout = "settings.readout",
        SettingsReadoutShow = "settings.readout.show",
        SettingsReadoutRows = "settings.readout.rows",
        SettingsReadoutAuto = "settings.readout.auto",
        SettingsReadoutPinned = "settings.readout.pinned",
        SettingsReadoutTimes = "settings.readout.times",
        SettingsReadoutClock = "settings.readout.clock",
        SettingsReadoutRelative = "settings.readout.relative",
        SettingsMetersHint = "settings.meters.hint",
        SettingsMeter = "settings.meter",
        SettingsMeterShow = "settings.meter.show",
        SettingsMeterPin = "settings.meter.pin",
        SettingsNotify = "settings.notify",
        SettingsNotifyOn = "settings.notify.on",
        SettingsNotifyLevels = "settings.notify.levels",
        SettingsNotifyAnd = "settings.notify.and",
        SettingsNotifyOff = "settings.notify.off",
        SettingsNotifyUsedUp = "settings.notify.usedup",
        SettingsNotifyResets = "settings.notify.resets",
        SettingsNotifyLow = "settings.notify.low",
        SettingsNotifyHint = "settings.notify.hint",
        SettingsShortcut = "settings.shortcut",
        SettingsShortcutOn = "settings.shortcut.on",
        SettingsShortcutCtrl = "settings.shortcut.ctrl",
        SettingsShortcutAlt = "settings.shortcut.alt",
        SettingsShortcutShift = "settings.shortcut.shift",
        SettingsShortcutWin = "settings.shortcut.win",
        SettingsShortcutKey = "settings.shortcut.key",
        SettingsShortcutInvalid = "settings.shortcut.invalid",
        SettingsShortcutInUse = "settings.shortcut.inuse",
        SettingsShortcutFailed = "settings.shortcut.failed",
        SettingsLanguage = "settings.language",
        SettingsLanguageWindows = "settings.language.windows",
        SettingsLanguageHint = "settings.language.hint",
        SettingsStartup = "settings.startup",
        SettingsStartupOn = "settings.startup.on",
        SettingsApply = "settings.apply",
        SettingsClose = "settings.close",
        SettingsSaved = "settings.saved",
        SettingsSavedNoShortcut = "settings.saved.noshortcut",
        SettingsSavedNoStartup = "settings.saved.nostartup",
        SettingsSavedNeither = "settings.saved.neither",
        SettingsNotSaved = "settings.notsaved",

        // ---- Accounts (accounts-spec §4, §7)
        AccountN = "account.n",
        SettingsAccounts = "settings.accounts",
        SettingsAccountsHint = "settings.accounts.hint",
        SettingsAccountsSignedIn = "settings.accounts.signedin",
        SettingsAccountsSignedInHidden = "settings.accounts.signedin.hidden",
        SettingsAccountsSignedOut = "settings.accounts.signedout",
        SettingsAccountsUnknown = "settings.accounts.unknown",
        SettingsAccountsSame = "settings.accounts.same",
        SettingsAccountsAlias = "settings.accounts.alias",
        SettingsAccountsAliasNone = "settings.accounts.alias.none",
        SettingsAccountsRemove = "settings.accounts.remove",
        SettingsAccountsFound = "settings.accounts.found",
        SettingsAccountsAdd = "settings.accounts.add",
        SettingsAccountsAddFolder = "settings.accounts.addfolder",
        SettingsAccountsPicker = "settings.accounts.picker",
        SettingsAccountsNew = "settings.accounts.new",
        SettingsAccountsNewName = "settings.accounts.new.name",
        SettingsAccountsNewCreate = "settings.accounts.new.create",
        SettingsAccountsAdded = "settings.accounts.added",
        SettingsAccountsAlready = "settings.accounts.already",
        SettingsAccountsCreated = "settings.accounts.created",
        SettingsAccountsNotCreated = "settings.accounts.notcreated",
        SettingsAccountsNoSignIn = "settings.accounts.nosignin",
        SettingsPrivacyHide = "settings.privacy.hide",

        // ---- About window (R§10 "About Lastdrop")
        AboutTitle = "about.title",
        AboutHeading = "about.heading",
        AboutTagline = "about.tagline",
        AboutCopyright = "about.copyright",
        AboutCredits = "about.credits",
        AboutClose = "about.close",
    }
}

catalogue! {
    /// A string resource with plural forms (`unit.minute`: `{n} minute`, `{n} minutes`), each
    /// language with the forms of its [`PluralRule`]. [`count`] picks the form and fills `{n}`.
    PluralMsg in PLURAL_KEYS {
        // ---- Spoken units (R§11 sentences, F§4.5 balance forecast)
        UnitMinute = "unit.minute",
        UnitHour = "unit.hour",
        UnitDay = "unit.day",
        UnitWeek = "unit.week",
    }
}

/// One language's string table.
pub trait Strings: Send + Sync {
    /// BCP 47 language tag of the table (`en`).
    fn language(&self) -> &str;

    /// The template of `msg`, with `{name}` placeholders.
    fn template(&self, msg: Msg) -> &str;

    /// The plural form the count `n` takes in this language.
    fn plural(&self, n: i64) -> Plural;

    /// The template of `msg` in the plural form `form`; empty for a form the language's rule
    /// never picks. [`count`] picks the form.
    fn plural_template(&self, msg: PluralMsg, form: Plural) -> &str;

    /// Meter names start lower-case inside a sentence (`weekly, all models`); off for languages
    /// that capitalise nouns.
    fn lowercase_names(&self) -> bool;
}

/// Implements [`Strings`] for a language's table struct from its generated [`Language`] data.
macro_rules! table {
    ($language:ident) => {
        impl Strings for $language {
            fn language(&self) -> &str {
                Language::$language.tag()
            }

            fn template(&self, msg: Msg) -> &str {
                Language::$language.texts()[msg.index()]
            }

            fn plural(&self, n: i64) -> Plural {
                Language::$language.plural_rule().category(n)
            }

            fn plural_template(&self, msg: PluralMsg, form: Plural) -> &str {
                Language::$language.plural_texts()[msg.index()][form as usize]
            }

            fn lowercase_names(&self) -> bool {
                Language::$language.lowercase_names()
            }
        }
    };
}

// `KEYS`, `PLURAL_KEYS`, `Language` with its data, and a table struct per language.
include!(concat!(env!("OUT_DIR"), "/locales.rs"));

impl Language {
    /// The language of a BCP 47 tag, in any case and with `-` or `_`: the language of the whole
    /// tag (a file named `pt-BR.toml`), else of its primary subtag (`sv`, `sv-SE` and `sv-FI` are
    /// Swedish); `None` for a language without a table.
    pub fn from_tag(tag: &str) -> Option<Language> {
        let same = |a: &str, b: &str| {
            let norm = |c: u8| {
                if c == b'_' {
                    b'-'
                } else {
                    c.to_ascii_lowercase()
                }
            };
            a.len() == b.len() && a.bytes().zip(b.bytes()).all(|(x, y)| norm(x) == norm(y))
        };
        let primary = tag.split(['-', '_']).next().unwrap_or_default();
        let find = |t: &str| Language::ALL.into_iter().find(|l| same(l.tag(), t));
        find(tag).or_else(|| find(primary))
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

/// `n` in the plural form the language gives it (`1 minute`, `5 minutes`), `{n}` filled in.
pub fn count(strings: &dyn Strings, msg: PluralMsg, n: i64) -> String {
    let template = strings.plural_template(msg, strings.plural(n));
    fill(template, &[("n", &n.to_string())])
}

/// `s` with its first letter upper-cased: titles, verdicts, "Also tracked" lines and meter rows,
/// whose templates may start with `{meter}`, `{label}` or `{short_name}` (Appendix B: `{meter}`
/// "capitalised at the start of a sentence").
pub fn capitalise(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
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
    fn every_message_has_a_unique_key_and_a_text_in_every_language() {
        let mut keys: Vec<&str> = Msg::ALL.iter().map(|m| m.key()).collect();
        keys.extend(PluralMsg::ALL.iter().map(|m| m.key()));
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate message keys");
        for l in Language::ALL {
            let strings = l.strings();
            for m in Msg::ALL {
                assert!(!strings.template(*m).is_empty(), "{}: {}", l.tag(), m.key());
            }
            // Exactly the forms the language's rule picks.
            for m in PluralMsg::ALL {
                for form in Plural::ALL {
                    let picked = l.plural_rule().forms().contains(&form);
                    assert_eq!(
                        !strings.plural_template(*m, form).is_empty(),
                        picked,
                        "{}: {}.{}",
                        l.tag(),
                        m.key(),
                        form.name()
                    );
                }
            }
        }
    }

    #[test]
    fn messages_find_their_own_texts() {
        // The compile-time index lands on the message's own key in every table.
        assert_eq!(English.template(Msg::ReasonOffline), "Offline");
        assert_eq!(English.template(Msg::AboutClose), "Close");
        assert_eq!(Swedish.template(Msg::MenuQuit), "Avsluta Lastdrop");
        assert_eq!(Swedish.template(Msg::Today), "idag");
        for m in Msg::ALL {
            assert_eq!(KEYS[m.index()], m.key());
        }
        for m in PluralMsg::ALL {
            assert_eq!(PLURAL_KEYS[m.index()], m.key());
        }
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

    #[test]
    fn every_language_keeps_the_shape_of_each_english_template() {
        // Fragments joined to other text keep their leading space or separator, sentences
        // their full stop, so a joined line reads in every language as the English one does.
        for l in Language::ALL {
            let strings = l.strings();
            for m in Msg::ALL {
                let (en, t) = (English.template(*m), strings.template(*m));
                let what = format!("{}: {}: {t:?}", l.tag(), m.key());
                assert_eq!(en.starts_with(' '), t.starts_with(' '), "{what}");
                assert_eq!(en.starts_with(", "), t.starts_with(", "), "{what}");
                assert_eq!(en.ends_with('.'), t.ends_with('.'), "{what}");
                assert_eq!(en.ends_with('…'), t.ends_with('…'), "{what}");
            }
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
        assert_eq!(Language::from_tag("sv_se"), Some(Language::Swedish));
        assert_eq!(Language::from_tag("en_GB"), Some(Language::English));
        // `qaa` is a private-use language: no locale file will ever be named after it.
        assert_eq!(Language::from_tag("qaa-DE"), None);
        assert_eq!(Language::from_tag("s"), None);
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
            Language::resolve(None, &ui(&["qaa-DE", "sv-SE"])),
            Language::Swedish
        );
        assert_eq!(Language::resolve(None, &ui(&["qaa-DE"])), Language::English);
        assert_eq!(Language::resolve(None, &[]), Language::English);
        // A newer build's language follows Windows.
        assert_eq!(
            Language::resolve(Some("qaa"), &ui(&["sv-SE"])),
            Language::Swedish
        );

        assert_eq!(Language::ALL[0], Language::English);
        assert_eq!(Language::default(), Language::English);
        for l in Language::ALL {
            assert_eq!(l.strings().language(), l.tag());
            assert_eq!(Language::from_tag(l.tag()), Some(l));
        }
    }

    #[test]
    fn counts_take_the_language_plural_form() {
        assert_eq!(Swedish.plural(1), Plural::One);
        assert_eq!(Swedish.plural(0), Plural::Other);
        assert_eq!(Swedish.plural(2), Plural::Other);
        assert_eq!(count(&Swedish, PluralMsg::UnitMinute, 1), "1 minut");
        assert_eq!(count(&Swedish, PluralMsg::UnitMinute, 5), "5 minuter");
        assert_eq!(count(&English, PluralMsg::UnitWeek, 1), "1 week");
        assert_eq!(count(&English, PluralMsg::UnitWeek, 0), "0 weeks");
    }
}
