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
//! ([`super::a11y`]), and in `settings.meter.*.a11y`, the settings window's meter row label;
//! `{service}` the service
//! name with alias (R§6.4); `{client}` [`crate::model::ProviderId::client_name`]; `{label}` =
//! `card.also.line`; `{short_name}` the meter's `short_name`; `{amount}`, `{ref}`, `{burn}`
//! grouped numbers; `{unit}` a balance's unit word in the plural form of the number before it
//! (`{ref}` in `card.balance.left`, else `{amount}`; [`super::numbers::unit_word`]); `{window}`
//! `card.window.*`; `{other}` the other meter's `short_name`; `{detail}` a secret-free failure
//! detail; `{rows}` a joined list; `{today}`, `{max}`, `{avg}` bare numbers in a window's
//! sparkline and amounts with their unit in a balance's, and `{day}` a full weekday; `{model}` a
//! model's display name (`meter.*`); `{language}` a language's own name ([`Language::native_name`])
//! and `{version}` the app's version (`settings.*`, `about.*`); `{email}` the e-mail address an
//! account is signed in with and `{name}` an account's name (accounts-spec §4, `settings.accounts.*`);
//! `{group}` a model pool as the service names it and `{initial}` its first letter (`meter.group.*`);
//! `{tokens}`, `{input}`, `{output}`, `{cache}` compact token counts and `{cost}` an amount of money
//! with its currency code (`history.tokens.*`, tokens-spec §3); `{file}` a theme file's name
//! (`studio.*`, themes-spec §4); `{folder}` a folder's path (`install.*`, updates-spec §2).
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
        CardTitleBalanceWarning = "card.title.balance.warning",
        CardTitleBalanceRunway = "card.title.balance.runway",
        CardTitleBalanceEmpty = "card.title.balance.empty",
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
        FlyoutVerdictBalanceWarning = "flyout.verdict.balance.warning",
        FlyoutVerdictBalanceRunway = "flyout.verdict.balance.runway",
        FlyoutVerdictBalanceEmpty = "flyout.verdict.balance.empty",
        FlyoutVerdictStale = "flyout.verdict.stale",
        FlyoutVerdictSignIn = "flyout.verdict.signin",
        FlyoutVerdictApiKey = "flyout.verdict.apikey",
        FlyoutVerdictPaused = "flyout.verdict.paused",
        FlyoutVerdictLoading = "flyout.verdict.loading",
        FlyoutVerdictMore = "flyout.verdict.more",
        FlyoutRefreshName = "flyout.refresh.a11y",
        FlyoutRefreshTooltip = "flyout.refresh.tooltip",
        FlyoutHistoryName = "flyout.history.a11y",
        FlyoutHistoryTooltip = "flyout.history.tooltip",
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
        TrayBalanceWarning = "tray.balance.warning",
        TrayBalanceRunway = "tray.balance.runway",
        TrayBalanceEmpty = "tray.balance.empty",
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
        MenuHistory = "menu.history",
        MenuSettings = "menu.settings",
        MenuAbout = "menu.about",
        MenuQuit = "menu.quit",
        MenuCardUsage = "menu.card.usage",
        MenuCardRefresh = "menu.card.refresh",
        MenuCardHistory = "menu.card.history",
        MenuCardReadout = "menu.card.readout",

        // ---- Settings window (ROADMAP M1 "settings basics"; out of both specs, F§12)
        SettingsTitle = "settings.title",
        SettingsPageServices = "settings.page.services",
        SettingsPageServicesDescription = "settings.page.services.description",
        SettingsPageDisplay = "settings.page.display",
        SettingsPageDisplayDescription = "settings.page.display.description",
        SettingsPageNotifications = "settings.page.notifications",
        SettingsPageGeneral = "settings.page.general",
        SettingsNotSavedTitle = "settings.notsaved.title",
        SettingsRetry = "settings.retry",
        SettingsDiscard = "settings.discard",
        SettingsServiceClaude = "settings.service.claude",
        SettingsServiceCodex = "settings.service.codex",
        SettingsServiceCopilot = "settings.service.copilot",
        SettingsServiceCursor = "settings.service.cursor",
        SettingsServiceGrok = "settings.service.grok",
        SettingsServiceAntigravity = "settings.service.antigravity",
        SettingsServiceCustom = "settings.service.custom",
        SettingsServiceKeyStored = "settings.service.key.stored",
        SettingsServiceKeyNeeded = "settings.service.key.needed",
        SettingsServiceOpenCodeOwn = "settings.service.opencode.own",
        SettingsServiceOpenCodeShared = "settings.service.opencode.shared",
        SettingsServiceAccountsA11y = "settings.service.accounts.a11y",
        SettingsKeyPaste = "settings.key.paste",
        SettingsKeyReplace = "settings.key.replace",
        SettingsKeySave = "settings.key.save",
        SettingsKeyRemove = "settings.key.remove",
        SettingsKeyLabel = "settings.key.label",
        SettingsKeyA11y = "settings.key.a11y",
        SettingsKeySaveA11y = "settings.key.save.a11y",
        SettingsKeyRemoveA11y = "settings.key.remove.a11y",
        SettingsKeyHint = "settings.key.hint",
        SettingsKeyOpenCode = "settings.key.opencode",
        SettingsKeySaved = "settings.key.saved",
        SettingsKeyNotSaved = "settings.key.notsaved",
        SettingsKeyRemoved = "settings.key.removed",
        SettingsKeyNotRemoved = "settings.key.notremoved",
        SettingsRefresh = "settings.refresh",
        SettingsRefreshCheck = "settings.refresh.check",
        SettingsRefreshManual = "settings.refresh.manual",
        ThemeWindows = "theme.windows",
        ThemeSegmented = "theme.segmented",
        ThemeCompact = "theme.compact",
        ThemeEmber = "theme.ember",
        StudioTitle = "studio.title",
        StudioStartFrom = "studio.start_from",
        StudioName = "studio.name",
        StudioColours = "studio.colours",
        StudioMode = "studio.mode",
        StudioModeSystem = "studio.mode.system",
        StudioModeLight = "studio.mode.light",
        StudioModeDark = "studio.mode.dark",
        StudioAccentOwn = "studio.accent_own",
        StudioRolesHint = "studio.roles_hint",
        StudioLight = "studio.light",
        StudioDark = "studio.dark",
        StudioRoleAccentFill = "studio.role.accent_fill",
        StudioRoleTextPrimary = "studio.role.text_primary",
        StudioRoleTextSecondary = "studio.role.text_secondary",
        StudioRoleTrack = "studio.role.track",
        StudioRoleCaution = "studio.role.caution",
        StudioRoleCritical = "studio.role.critical",
        StudioRoleSuccess = "studio.role.success",
        StudioRoleCardBackground = "studio.role.card_background",
        StudioRoleCardBorder = "studio.role.card_border",
        StudioRoleDivider = "studio.role.divider",
        StudioRoleBadge = "studio.role.badge",
        StudioLayout = "studio.layout",
        StudioDensity = "studio.density",
        StudioDensityRegular = "studio.density.regular",
        StudioDensityCompact = "studio.density.compact",
        StudioBar = "studio.bar",
        StudioBarContinuous = "studio.bar.continuous",
        StudioBarSegmented = "studio.bar.segmented",
        StudioWidgets = "studio.widgets",
        StudioWidgetsHint = "studio.widgets_hint",
        StudioWidgetPlan = "studio.widget.plan",
        StudioWidgetAge = "studio.widget.age",
        StudioWidgetSparkline = "studio.widget.sparkline",
        StudioWidgetResets = "studio.widget.resets",
        StudioWidgetBars = "studio.widget.bars",
        StudioWidgetTrouble = "studio.widget.trouble",
        StudioUp = "studio.up",
        StudioDown = "studio.down",
        StudioPreview = "studio.preview",
        StudioSave = "studio.save",
        StudioSaveUse = "studio.save_use",
        StudioFolder = "studio.folder",
        StudioClose = "studio.close",
        StudioSaved = "studio.saved",
        StudioUsed = "studio.used",
        StudioNotSaved = "studio.not_saved",
        SettingsTheme = "settings.theme",
        SettingsThemeStudio = "settings.theme.studio",
        SettingsThemeHint = "settings.theme.hint",
        SettingsThemeFolder = "settings.theme.folder",
        SettingsAppearance = "settings.appearance",
        InstallAsk = "install.ask",
        InstallDone = "install.done",
        InstallFailed = "install.failed",
        InstallStillRunning = "install.still_running",
        InstallShortcut = "install.shortcut",
        UninstallAsk = "uninstall.ask",
        UninstallDone = "uninstall.done",
        UninstallFailed = "uninstall.failed",
        UpdateAvailable = "update.available",
        UpdateAvailableBody = "update.available_body",
        UpdateInstall = "update.install",
        UpdateDownloading = "update.downloading",
        UpdateCurrent = "update.current",
        UpdateCheckFailed = "update.check_failed",
        UpdateFailed = "update.failed",
        SettingsUpdates = "settings.updates",
        SettingsUpdatesCheck = "settings.updates.check",
        SettingsUpdatesNow = "settings.updates.now",
        SettingsUpdatesHint = "settings.updates.hint",
        SettingsUpdatesChecking = "settings.updates.checking",
        SettingsUpdatesCurrent = "settings.updates.current",
        SettingsUpdatesAvailable = "settings.updates.available",
        SettingsUpdatesFailed = "settings.updates.failed",
        SettingsUpdatesNotRunning = "settings.updates.not_running",
        SettingsUpdatesAsked = "settings.updates.asked",
        SettingsVersion = "settings.version",
        SettingsReadout = "settings.readout",
        SettingsReadoutShow = "settings.readout.show",
        SettingsReadoutEvery = "settings.readout.every",
        SettingsReadoutRows = "settings.readout.rows",
        SettingsReadoutAuto = "settings.readout.auto",
        SettingsReadoutPinned = "settings.readout.pinned",
        SettingsReadoutChoosing = "settings.readout.choosing",
        SettingsPinsFirst = "settings.pins.first",
        SettingsPinsSecond = "settings.pins.second",
        SettingsPinsNone = "settings.pins.none",
        SettingsPinsUnavailable = "settings.pins.unavailable",
        SettingsPinsHidden = "settings.pins.hidden",
        SettingsReadoutTimes = "settings.readout.times",
        SettingsReadoutClock = "settings.readout.clock",
        SettingsReadoutRelative = "settings.readout.relative",
        SettingsMetersHint = "settings.meters.hint",
        SettingsMeters = "settings.meters",
        SettingsMetersEmpty = "settings.meters.empty",
        SettingsMetersEmptyLink = "settings.meters.empty.link",
        SettingsMeterLimit = "settings.meter.limit",
        SettingsMeter = "settings.meter",
        SettingsMeterShowA11y = "settings.meter.show.a11y",
        SettingsNotifyOn = "settings.notify.on",
        SettingsNotifyLevels = "settings.notify.levels",
        SettingsNotifyAnd = "settings.notify.and",
        SettingsNotifyAlso = "settings.notify.also",
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
        SettingsStartupOn = "settings.startup.on",
        SettingsStartupNotChanged = "settings.startup.notchanged",
        SettingsStartupStuckOn = "settings.startup.stuck.on",
        SettingsStartupStuckOff = "settings.startup.stuck.off",
        SettingsNotSaved = "settings.notsaved",

        // ---- Accounts (accounts-spec §4, §7)
        AccountN = "account.n",
        SettingsAccountsSignedIn = "settings.accounts.signedin",
        SettingsAccountsSignedInHidden = "settings.accounts.signedin.hidden",
        SettingsAccountsSignedOut = "settings.accounts.signedout",
        SettingsAccountsUnknown = "settings.accounts.unknown",
        SettingsAccountsSame = "settings.accounts.same",
        SettingsAccountsAlias = "settings.accounts.alias",
        SettingsAccountsAliasNone = "settings.accounts.alias.none",
        SettingsAccountsAliasA11y = "settings.accounts.alias.a11y",
        SettingsAccountsRemoveA11y = "settings.accounts.remove.a11y",
        SettingsAccountsRemove = "settings.accounts.remove",
        SettingsAccountsFound = "settings.accounts.found",
        SettingsAccountsAdd = "settings.accounts.add",
        SettingsAccountsAddFolder = "settings.accounts.addfolder",
        SettingsAccountsPicker = "settings.accounts.picker",
        SettingsAccountsNew = "settings.accounts.new",
        SettingsAccountsNewOpen = "settings.accounts.new.open",
        SettingsAccountsNewName = "settings.accounts.new.name",
        SettingsAccountsNewCreate = "settings.accounts.new.create",
        SettingsAccountsNewCancel = "settings.accounts.new.cancel",
        SettingsAccountsNewNote = "settings.accounts.new.note",
        SettingsAccountsAdded = "settings.accounts.added",
        SettingsAccountsAlready = "settings.accounts.already",
        SettingsAccountsCreated = "settings.accounts.created",
        SettingsAccountsNotCreated = "settings.accounts.notcreated",
        SettingsAccountsNoSignIn = "settings.accounts.nosignin",
        SettingsPrivacyHide = "settings.privacy.hide",
        SettingsPrivacy = "settings.privacy",
        SettingsPrivacyHideDescription = "settings.privacy.hide.description",
        SettingsCustom = "settings.custom",
        SettingsCustomAdd = "settings.custom.add",
        SettingsCustomAddDescription = "settings.custom.add.description",
        SettingsCustomOpen = "settings.custom.open",
        SettingsCustomOpenA11y = "settings.custom.open.a11y",

        // ---- Usage history window (charts-spec)
        HistoryTitle = "history.title",
        HistoryService = "history.service",
        HistoryRange = "history.range",
        HistoryRangeDays = "history.range.days",
        HistoryOverTime = "history.over_time",
        HistoryPerDay = "history.per_day",
        HistoryPerDaySummary = "history.per_day.summary",
        HistoryHours = "history.hours",
        HistoryHoursSummary = "history.hours.summary",
        HistoryEmpty = "history.empty",
        HistoryEmptyRange = "history.empty.range",
        HistoryTokens = "history.tokens",
        HistoryTokensReading = "history.tokens.reading",
        HistoryTokensNone = "history.tokens.none",
        HistoryTokensEmpty = "history.tokens.empty",
        HistoryTokensSummary = "history.tokens.summary",
        HistoryTokensSummaryNoCost = "history.tokens.summary.nocost",
        HistoryTokensSplit = "history.tokens.split",
        HistoryTokensCost = "history.tokens.cost",
        HistoryTokensTotal = "history.tokens.total",
        HistoryTokensTotalPartial = "history.tokens.total.partial",
        HistoryTokensNote = "history.tokens.note",
        HistoryTokensCountK = "history.tokens.count.k",
        HistoryTokensCountM = "history.tokens.count.m",
        HistoryTokensCountB = "history.tokens.count.b",
        HistoryClose = "history.close",
        A11yHistorySpan = "a11y.history.span",
        A11yHistoryPercent = "a11y.history.percent",
        A11yHistoryWindow = "a11y.history.window",
        A11yHistoryBalance = "a11y.history.balance",
        A11yHistoryPerDay = "a11y.history.per_day",
        A11yHistoryHours = "a11y.history.hours",
        A11yHistoryTokens = "a11y.history.tokens",

        // ---- About window (R§10 "About Lastdrop")
        AboutTitle = "about.title",
        AboutHeading = "about.heading",
        AboutTagline = "about.tagline",
        AboutCopyright = "about.copyright",
        AboutCredits = "about.credits",
        AboutSupport = "about.support",
        AboutClose = "about.close",
    }
}

catalogue! {
    /// A string resource with plural forms (`unit.minute`: `{n} minute`, `{n} minutes`), each
    /// language with the forms of its [`PluralRule`]. [`count`] picks the form and fills `{n}`;
    /// [`plural_form`] picks it for a word shown apart from its number (`unit.credits`).
    PluralMsg in PLURAL_KEYS {
        // ---- Spoken units (R§11 sentences, F§4.5 balance forecast)
        UnitMinute = "unit.minute",
        UnitHour = "unit.hour",
        UnitDay = "unit.day",
        // ---- A balance's unit after its amount (F§4.5; [`super::numbers::unit_word`])
        UnitCredits = "unit.credits",
        // ---- Usage history window (charts-spec §5)
        A11yHistoryUsedUp = "a11y.history.used_up",
        UnitWeek = "unit.week",
        // ---- Settings window (settings-spec §4)
        SettingsAccountsCount = "settings.accounts.count",
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

/// The template of `msg` in the plural form the count `n` takes (`unit.credits`: `credit` for 1,
/// `credits` for 0 and 5).
pub fn plural_form(strings: &dyn Strings, msg: PluralMsg, n: i64) -> &str {
    strings.plural_template(msg, strings.plural(n))
}

/// `n` in the plural form the language gives it (`1 minute`, `5 minutes`), `{n}` filled in.
pub fn count(strings: &dyn Strings, msg: PluralMsg, n: i64) -> String {
    fill(plural_form(strings, msg, n), &[("n", &n.to_string())])
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

    /// Each language's word for a spoken percentage (`{n} percent`).
    fn percent_word(language: Language) -> &'static str {
        match language {
            Language::Swedish => "procent",
            Language::German => "Prozent",
            Language::French => "pour cent",
            Language::Spanish => "por ciento",
            _ => "percent",
        }
    }

    #[test]
    fn spoken_percentages_use_a_bare_number() {
        // `{pct}` carries the percent sign (`42%`), so `{pct} percent` would read "42% percent".
        for l in Language::ALL {
            let word = percent_word(l);
            for m in Msg::ALL {
                let t = l.strings().template(*m);
                for pct in ["{pct}", "{p}", "{e}", "{left}"] {
                    assert!(!t.contains(&format!("{pct} {word}")), "{}: {t}", m.key());
                }
            }
        }
    }

    /// `t` ends in an abbreviation's full stop: one to four letters before it (`auj.`, `Tsd.`).
    fn ends_in_abbreviation(t: &str) -> bool {
        t.strip_suffix('.')
            .and_then(|s| s.split(|c: char| !c.is_alphabetic()).next_back())
            .is_some_and(|word| (1..=4).contains(&word.chars().count()))
    }

    #[test]
    fn every_language_keeps_the_shape_of_each_english_template() {
        // Fragments joined to other text keep their leading space or separator, sentences
        // their full stop, so a joined line reads in every language as the English one does.
        // A text English ends without one may end in an abbreviation's (fr `auj.`, de `Tsd.`).
        for l in Language::ALL {
            let strings = l.strings();
            for m in Msg::ALL {
                let (en, t) = (English.template(*m), strings.template(*m));
                let what = format!("{}: {}: {t:?}", l.tag(), m.key());
                assert_eq!(en.starts_with(' '), t.starts_with(' '), "{what}");
                assert_eq!(en.starts_with(", "), t.starts_with(", "), "{what}");
                assert!(
                    en.ends_with('.') == t.ends_with('.')
                        || (!en.ends_with('.') && ends_in_abbreviation(t)),
                    "{what}"
                );
                assert_eq!(en.ends_with('…'), t.ends_with('…'), "{what}");
            }
        }
        assert!(ends_in_abbreviation("{n} Tsd."));
        assert!(ends_in_abbreviation("auj."));
        assert!(!ends_in_abbreviation("Connexion expirée."));
        assert!(!ends_in_abbreviation("Hors ligne."));
    }

    #[test]
    fn no_language_says_percent_left() {
        // R§5: percent is always used. The only share left is the OK card's `{left}` at the
        // reset, as in English. `%` stands for a percentage in each language's ways of saying
        // that something is left.
        let left = |l: Language| -> &'static [&'static str] {
            match l {
                Language::Swedish => &["% kvar"],
                Language::German => &["% übrig", "noch %", "Noch %"],
                Language::French => &["% restant", "reste %", "Reste %"],
                Language::Spanish => &["% restante", "queda %", "Queda %", "quedan %", "Quedan %"],
                _ => &["% left"],
            }
        };
        for l in Language::ALL {
            let spoken = format!("{{n}} {}", percent_word(l));
            for m in Msg::ALL {
                let t = l.strings().template(*m);
                for pct in ["{pct}", "{p}", "{e}", spoken.as_str()] {
                    for pattern in left(l) {
                        let said = pattern.replace('%', pct);
                        assert!(!t.contains(&said), "{}: {}: {t}", l.tag(), m.key());
                    }
                }
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
    fn german_french_and_spanish_serve_every_region() {
        let ui = |tags: &[&str]| tags.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
        for (tags, language) in [
            (["de-DE", "de-AT", "de-CH"], Language::German),
            (["fr-FR", "fr-CA", "fr-BE"], Language::French),
            (["es-ES", "es-MX", "es-419"], Language::Spanish),
        ] {
            for tag in tags {
                assert_eq!(Language::from_tag(tag), Some(language), "{tag}");
                // As Windows' display language, alone or before another one with a table.
                assert_eq!(Language::resolve(None, &ui(&[tag])), language, "{tag}");
                assert_eq!(
                    Language::resolve(None, &ui(&[tag, "en-US"])),
                    language,
                    "{tag}"
                );
                assert_eq!(
                    Language::resolve(None, &ui(&["qaa-DE", tag, "sv-SE"])),
                    language,
                    "{tag}"
                );
                // The setting wins over Windows.
                assert_eq!(
                    Language::resolve(Some("en"), &ui(&[tag])),
                    Language::English
                );
            }
        }
        assert_eq!(
            Language::resolve(Some("fr"), &ui(&["de-DE"])),
            Language::French
        );
        assert_eq!(
            Language::resolve(None, &ui(&["en-US", "de-DE"])),
            Language::English
        );
        assert_eq!(Language::German.native_name(), "Deutsch");
        assert_eq!(Language::French.native_name(), "Français");
        assert_eq!(Language::Spanish.native_name(), "Español");
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
        // French counts 0 with the singular (CLDR `zero-one`); German and Spanish only 1.
        assert_eq!(French.plural(0), Plural::One);
        assert_eq!(
            count(&French, PluralMsg::SettingsAccountsCount, 0),
            "0\u{A0}compte"
        );
        assert_eq!(
            count(&French, PluralMsg::SettingsAccountsCount, 2),
            "2\u{A0}comptes"
        );
        assert_eq!(count(&German, PluralMsg::UnitDay, 0), "0 Tage");
        assert_eq!(count(&German, PluralMsg::UnitDay, 1), "1 Tag");
        assert_eq!(count(&Spanish, PluralMsg::UnitDay, 1), "1 día");
        assert_eq!(count(&Spanish, PluralMsg::UnitDay, 0), "0 días");
    }
}
