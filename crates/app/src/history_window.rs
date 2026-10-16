//! The usage history window (docs/design/charts-spec.md) in the dialog process: the services
//! with history, a range, and the charts `lastdrop_core::charts` builds from the history files of
//! the selected service, read when the window opens and when the service or range changes.
//!
//! For Claude and Codex the window also shows tokens and cost per day and model
//! (docs/design/tokens-spec.md) from the tools' own session logs.
//!
//! Threading: the main thread runs the window; the history files are read on it (a few megabytes
//! at most, charts-spec §8). The session logs can be hundreds of megabytes: they are read on a
//! `tokens` thread, one source at a time, and the result comes back through
//! `slint::invoke_from_event_loop` (tokens-spec §4).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::charts::{self, ChartMeter, HistoryInput, HistoryModel, Period, Range};
use lastdrop_core::flyout::SparkKind;
use lastdrop_core::grammar::strings::text;
use lastdrop_core::grammar::{LocaleFormat, Msg, Strings};
use lastdrop_core::model::{MeterKey, MeterKind, ProviderId, SourceKey, catalog, fnv1a64};
use lastdrop_core::settings::Settings;
use lastdrop_core::stats::Sample;
use lastdrop_core::tokens::{self, ModelRow, PriceTable, TokenLog, TokensInput, TokensModel};
use lastdrop_providers::AccountSource;
use lastdrop_providers::real::{RealEnv, RealFs};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::persist::{self, Paths, SavedState};
use crate::settings_window::Wording;
use crate::{AppError, dialog, history, ui, wiring};

/// One service of the window: an account of a service with its meters.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    /// The source.
    pub key: SourceKey,
    /// Its name as the flyout gives it (`Claude`, `Claude · Work`, `Codex · Account 2`).
    pub label: String,
    /// Its meters, in the order they are charted.
    pub meters: Vec<ChartMeter>,
}

/// The tag of a source (charts-spec §1): 24 bits of the FNV-1a hash of its service id and
/// account key, so the tray process can name it on the dialog process's command line.
pub fn source_tag(key: &SourceKey) -> u32 {
    u32::try_from(fnv1a64(&[key.provider.as_str(), &key.account.0]) & 0xFF_FFFF).unwrap_or(0)
}

/// Every account of every service in `settings`, in settings order, with its meters: its
/// catalogue's, then the others its readings reported (`saved`, with the units of its
/// balances). Named as the flyout names them: the alias, else `Account n` when the service has
/// several accounts (the dialog process does not know who is signed in).
pub fn sources(settings: &Settings, saved: &SavedState, strings: &dyn Strings) -> Vec<Source> {
    let units = saved.units_map();
    let mut out = Vec::new();
    for p in settings.providers.iter().filter_map(|e| e.known()) {
        let name = p.id.display_name();
        let several = p.accounts.len() >= 2;
        for (i, account) in p.accounts.iter().enumerate() {
            let key = SourceKey {
                provider: p.id,
                account: account.key.clone(),
            };
            let alias = account.alias.as_deref().filter(|a| !a.trim().is_empty());
            let label = match alias {
                Some(alias) => format!("{name} · {alias}"),
                None if several => {
                    let n = text(strings, Msg::AccountN, &[("n", &(i + 1).to_string())]);
                    format!("{name} · {n}")
                }
                None => name.to_owned(),
            };
            let meter_key = |id: &str| MeterKey {
                provider: key.provider,
                account: key.account.clone(),
                meter: lastdrop_core::model::MeterId::new(id),
            };
            let mut defs: Vec<_> = catalog(key.provider).iter().collect();
            defs.sort_by_key(|d| d.order);
            let mut meters: Vec<ChartMeter> = defs
                .iter()
                .map(|d| {
                    let key = meter_key(d.id);
                    let (kind, window) = match d.kind {
                        MeterKind::Window { window, .. } => (SparkKind::Window, Some(window)),
                        MeterKind::Balance => (SparkKind::Balance, None),
                    };
                    let names = saved
                        .seen
                        .iter()
                        .find(|m| m.key == key)
                        .map_or_else(|| d.names(), |m| m.names.clone());
                    let unit = units.get(&key).cloned();
                    ChartMeter {
                        key,
                        names,
                        kind,
                        window,
                        unit,
                    }
                })
                .collect();
            for seen in saved.seen.iter().filter(|m| {
                m.key.provider == key.provider
                    && m.key.account == key.account
                    && !defs.iter().any(|d| d.id == m.key.meter.0)
            }) {
                meters.push(ChartMeter {
                    key: seen.key.clone(),
                    names: seen.names.clone(),
                    kind: if seen.balance {
                        SparkKind::Balance
                    } else {
                        SparkKind::Window
                    },
                    window: None,
                    unit: units.get(&seen.key).cloned(),
                });
            }
            out.push(Source { key, label, meters });
        }
    }
    out
}

/// `sources` with only the meters that have a history file in `dir`, without the sources left
/// with none.
pub fn with_history(mut sources: Vec<Source>, dir: &Path) -> Vec<Source> {
    for source in &mut sources {
        source.meters.retain(|m| history::has_file(dir, &m.key));
    }
    sources.retain(|s| !s.meters.is_empty());
    sources
}

/// The window's fixed texts.
fn texts(strings: &dyn Strings) -> ui::HistoryTexts {
    let t = |msg: Msg| SharedString::from(strings.template(msg));
    ui::HistoryTexts {
        title: t(Msg::HistoryTitle),
        service: t(Msg::HistoryService),
        range: t(Msg::HistoryRange),
        over_time: t(Msg::HistoryOverTime),
        per_day: t(Msg::HistoryPerDay),
        hours: t(Msg::HistoryHours),
        tokens: t(Msg::HistoryTokens),
        close: t(Msg::HistoryClose),
    }
}

/// A `Vec` as a Slint model.
fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(items)))
}

/// Sets every property of the window but the texts and the choices from `model`.
pub fn fill_model(window: &ui::HistoryWindow, model: &HistoryModel) {
    window.set_empty(model.empty.as_deref().unwrap_or_default().into());
    window.set_charts(self::model(
        model
            .charts
            .iter()
            .map(|c| ui::ChartData {
                title: c.title.as_str().into(),
                value: c.value.as_str().into(),
                top: c.top.as_str().into(),
                bars: self::model(
                    c.bars
                        .iter()
                        .map(|b| ui::BarData {
                            x0: b.x0,
                            x1: b.x1,
                            height: b.height,
                            used_up: b.used_up,
                        })
                        .collect(),
                ),
                risers: self::model(
                    c.risers
                        .iter()
                        .map(|r| ui::RiserData {
                            x: r.x,
                            low: r.low,
                            high: r.high,
                        })
                        .collect(),
                ),
                filled: c.filled,
                a11y: c.a11y.as_str().into(),
            })
            .collect(),
    ));
    window.set_ticks(self::model(
        model
            .ticks
            .iter()
            .map(|t| ui::TickData {
                at: t.at,
                label: t.label.as_str().into(),
            })
            .collect(),
    ));
    match &model.per_day {
        Some(d) => {
            window.set_has_per_day(true);
            window.set_days(self::model(
                d.columns
                    .iter()
                    .map(|c| ui::DayData {
                        height: c.height,
                        today: c.today,
                    })
                    .collect(),
            ));
            window.set_per_day_top(d.top.as_str().into());
            window.set_per_day_summary(d.summary.as_str().into());
            window.set_per_day_a11y(d.a11y.as_str().into());
        }
        None => {
            window.set_has_per_day(false);
            window.set_days(self::model(Vec::new()));
        }
    }
    match &model.hours {
        Some(h) => {
            window.set_has_hours(true);
            let rows = h
                .rows
                .iter()
                .enumerate()
                .map(|(r, label)| ui::HourRow {
                    label: label.as_str().into(),
                    levels: self::model(
                        h.levels
                            .iter()
                            .skip(r * 24)
                            .take(24)
                            .map(|l| i32::from(*l))
                            .collect(),
                    ),
                })
                .collect();
            window.set_hour_rows(self::model(rows));
            window.set_hour_labels(self::model(
                h.columns
                    .iter()
                    .map(|(hour, label)| ui::HourLabel {
                        hour: i32::from(*hour),
                        label: label.as_str().into(),
                    })
                    .collect(),
            ));
            window.set_hours_summary(h.summary.as_str().into());
            window.set_hours_a11y(h.a11y.as_str().into());
        }
        None => {
            window.set_has_hours(false);
            window.set_hour_rows(self::model(Vec::new()));
        }
    }
}

/// How much earlier than the period's start the session logs are read: a file changed then can
/// hold the period's first requests (tokens-spec §1).
const TOKENS_MARGIN: SignedDuration = SignedDuration::from_hours(24);

/// The largest `prices.json` the user's file may be.
const MAX_PRICES_FILE: u64 = 1 << 20;

/// Sets the tokens section from `section`; `None` hides it (a service without session logs).
pub fn fill_tokens(window: &ui::HistoryWindow, section: Option<&TokensModel>) {
    let Some(section) = section else {
        window.set_has_tokens(false);
        return;
    };
    let row = |r: &ModelRow| ui::TokenRow {
        name: r.name.as_str().into(),
        split: r.split.as_str().into(),
        tokens: r.tokens.as_str().into(),
        cost: r.cost.as_str().into(),
    };
    window.set_has_tokens(true);
    window.set_tokens_message(section.empty.as_deref().unwrap_or_default().into());
    window.set_token_days(model(
        section
            .columns
            .iter()
            .map(|c| ui::DayData {
                height: c.height,
                today: c.today,
            })
            .collect(),
    ));
    window.set_tokens_top(section.top.as_str().into());
    window.set_tokens_summary(section.summary.as_str().into());
    window.set_tokens_a11y(section.a11y.as_str().into());
    window.set_token_rows(model(section.rows.iter().map(row).collect()));
    window.set_tokens_total(section.total.as_ref().map(row).unwrap_or_default());
    window.set_tokens_note(section.note.as_str().into());
}

/// The session-log folder of every Claude and Codex account in `settings` (tokens-spec §1).
pub(crate) fn log_folders(settings: &Settings) -> HashMap<SourceKey, PathBuf> {
    let mut out = HashMap::new();
    for p in settings.providers.iter().filter_map(|e| e.known()) {
        if !matches!(p.id, ProviderId::Claude | ProviderId::Codex) {
            continue;
        }
        for account in &p.accounts {
            let folder = AccountSource::resolve(p.id, account, &RealEnv)
                .ok()
                .and_then(|s| s.folder().map(Path::to_path_buf));
            if let Some(folder) = folder {
                let key = SourceKey {
                    provider: p.id,
                    account: account.key.clone(),
                };
                out.insert(key, folder);
            }
        }
    }
    out
}

/// The bundled prices with the user's `prices.json` over them, when it is there and readable
/// (tokens-spec §2).
pub fn load_prices(path: &Path) -> PriceTable {
    let bundled = PriceTable::bundled();
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_PRICES_FILE) {
        log::warn!("history window: prices.json is larger than 1 MB; ignored");
        return bundled;
    }
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => bundled,
        Err(e) => {
            log::warn!("history window: prices.json not read: {e}");
            bundled
        }
        Ok(json) => match PriceTable::parse(&json) {
            Ok(user) => {
                log::info!(
                    "history window: prices.json adds {} models",
                    user.models.len()
                );
                bundled.with(user)
            }
            Err(e) => {
                log::warn!("history window: prices.json ignored: {e}");
                bundled
            }
        },
    }
}

/// Sets the texts and the choices: the services' names and the ranges.
pub fn fill_choices(window: &ui::HistoryWindow, sources: &[Source], strings: &dyn Strings) {
    window.set_texts(texts(strings));
    window.set_services(model(
        sources
            .iter()
            .map(|s| SharedString::from(s.label.as_str()))
            .collect(),
    ));
    window.set_ranges(model(
        Range::ALL
            .iter()
            .map(|r| SharedString::from(r.label(strings)))
            .collect(),
    ));
}

/// The charts of `source` over `range` from `history` at `now`; with no source, the no-history
/// message.
pub fn build(
    source: Option<&Source>,
    history: &BTreeMap<MeterKey, Vec<Sample>>,
    range: Range,
    now: Timestamp,
    locale: &LocaleFormat,
    strings: &dyn Strings,
) -> HistoryModel {
    let meters = source.map_or(&[][..], |s| s.meters.as_slice());
    charts::build_history(&HistoryInput {
        meters,
        history,
        range,
        now,
        locale,
        strings,
    })
}

/// The window while it is open (main thread).
struct Session {
    window: ui::HistoryWindow,
    sources: Vec<Source>,
    selected: RefCell<usize>,
    range: RefCell<Range>,
    history_dir: PathBuf,
    wording: Wording,
    /// The session-log folders of the Claude and Codex sources.
    folders: HashMap<SourceKey, PathBuf>,
    /// The session logs read so far, with the time they were read from, kept while the window
    /// is open.
    logs: RefCell<HashMap<SourceKey, (Timestamp, Rc<TokenLog>)>>,
    /// The sources whose logs are being read, with the time they are read from.
    reading: RefCell<HashMap<SourceKey, Timestamp>>,
    prices: PriceTable,
}

impl Session {
    /// Reads the selected service's files and shows its charts.
    fn show_selected(&self) {
        let selected = *self.selected.borrow();
        let range = *self.range.borrow();
        let source = self.sources.get(selected);
        let now = Timestamp::now();
        // A sample written moments ago with a clock slightly ahead still counts.
        let until = now.checked_add(SignedDuration::from_mins(1)).unwrap_or(now);
        let history: BTreeMap<MeterKey, Vec<Sample>> = source
            .map(|s| {
                s.meters
                    .iter()
                    .map(|m| {
                        (
                            m.key.clone(),
                            history::read_meter(&self.history_dir, &m.key, until),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let strings = self.wording.strings();
        let model = build(source, &history, range, now, &self.wording.locale, strings);
        fill_model(&self.window, &model);
        self.show_tokens();
    }

    /// Shows the selected source's tokens and cost, starting to read its logs if they are not
    /// read back far enough for the period yet; hides the section for a source without session
    /// logs.
    fn show_tokens(&self) {
        let selected = *self.selected.borrow();
        let source = self
            .sources
            .get(selected)
            .filter(|s| self.folders.contains_key(&s.key));
        let Some(source) = source else {
            fill_tokens(&self.window, None);
            return;
        };
        let strings = self.wording.strings();
        let range = *self.range.borrow();
        let now = Timestamp::now();
        let since = Period::of(range, now, &self.wording.locale.tz)
            .from
            .checked_sub(TOKENS_MARGIN)
            .unwrap_or(Timestamp::MIN);
        let log = self
            .logs
            .borrow()
            .get(&source.key)
            .filter(|(from, _)| *from <= since)
            .map(|(_, log)| Rc::clone(log));
        let section = match log {
            Some(log) => tokens::build(&TokensInput {
                log: &log,
                prices: &self.prices,
                client: source.key.provider.client_name().unwrap_or_default(),
                range,
                now,
                locale: &self.wording.locale,
                strings,
            }),
            None => {
                self.start_reading(&source.key, since);
                TokensModel {
                    empty: Some(strings.template(Msg::HistoryTokensReading).to_owned()),
                    ..TokensModel::default()
                }
            }
        };
        fill_tokens(&self.window, Some(&section));
    }

    /// Reads the session logs of `key` from `since` on a thread of their own, unless a read from
    /// that far back is under way.
    fn start_reading(&self, key: &SourceKey, since: Timestamp) {
        let Some(folder) = self.folders.get(key).cloned() else {
            return;
        };
        if self
            .reading
            .borrow()
            .get(key)
            .is_some_and(|from| *from <= since)
        {
            return;
        }
        self.reading.borrow_mut().insert(key.clone(), since);
        let thread_key = key.clone();
        let spawned = std::thread::Builder::new()
            .name("tokens".to_owned())
            .spawn(move || {
                let started = std::time::Instant::now();
                let log = match thread_key.provider {
                    ProviderId::Claude => {
                        lastdrop_providers::tokens::read_claude(&RealFs, &folder, since)
                    }
                    _ => lastdrop_providers::tokens::read_codex(&RealFs, &folder, since),
                };
                log::info!(
                    "history window: {} {} session logs read in {} ms: {} requests, {} models",
                    log.files,
                    thread_key.provider.display_name(),
                    started.elapsed().as_millis(),
                    log.requests.len(),
                    log.models.len()
                );
                let _ = slint::invoke_from_event_loop(move || {
                    with_session(|s| s.logs_read(thread_key, since, log));
                });
            });
        if let Err(e) = spawned {
            log::warn!("history window: no thread to read session logs: {e}");
            self.reading.borrow_mut().remove(key);
        }
    }

    /// Keeps the logs of `key` read from `since`, unless logs from further back are kept already,
    /// and shows the selected source's again.
    fn logs_read(&self, key: SourceKey, since: Timestamp, log: TokenLog) {
        {
            let mut reading = self.reading.borrow_mut();
            if reading.get(&key).is_some_and(|from| *from >= since) {
                reading.remove(&key);
            }
        }
        {
            let mut logs = self.logs.borrow_mut();
            let older = logs.get(&key).is_some_and(|(from, _)| *from <= since);
            if !older {
                logs.insert(key, (since, Rc::new(log)));
            }
        }
        self.show_tokens();
    }

    /// Selects the source of `tag`, if one has it.
    fn select_tag(&self, tag: Option<u32>) {
        let Some(tag) = tag else {
            return;
        };
        if let Some(i) = self.sources.iter().position(|s| source_tag(&s.key) == tag) {
            *self.selected.borrow_mut() = i;
            self.window.set_service_index(i32::try_from(i).unwrap_or(0));
        }
    }
}

thread_local! {
    static SESSION: RefCell<Option<Rc<Session>>> = const { RefCell::new(None) };
}

/// Opens the usage history window at the service of `tag` (or the first one), or brings the
/// open one to the front at that service.
pub fn open(paths: &Paths, tag: Option<u32>) -> Result<(), AppError> {
    let open = SESSION.with(|s| s.borrow().clone());
    if let Some(session) = open {
        if tag.is_some() {
            session.select_tag(tag);
            session.show_selected();
        }
        session.window.show()?;
        dialog::bring_to_front(session.window.as_weak());
        return Ok(());
    }

    let (settings, outcome) = persist::load_settings(&paths.settings);
    log::info!("history window: settings {outcome:?}");
    let wording = Wording::read(&settings);
    let strings = wording.strings();
    let all = sources(&settings, &persist::load_state(&paths.state), strings);
    let sources = with_history(all, &paths.history);
    log::info!("history window: {} services with history", sources.len());
    let folders = log_folders(&settings);
    let prices = load_prices(&paths.prices);
    let window = wiring::create_window(false, ui::HistoryWindow::new)?;
    fill_choices(&window, &sources, strings);
    let range = Range::default();
    let range_index = Range::ALL.iter().position(|r| *r == range).unwrap_or(0);
    window.set_range_index(i32::try_from(range_index).unwrap_or(0));
    window.set_service_index(0);
    window.on_service_changed(|i| {
        with_session(|s| {
            *s.selected.borrow_mut() = usize::try_from(i).unwrap_or(0);
            s.show_selected();
        });
    });
    window.on_range_changed(|i| {
        with_session(|s| {
            let range = usize::try_from(i)
                .ok()
                .and_then(|i| Range::ALL.get(i).copied())
                .unwrap_or_default();
            *s.range.borrow_mut() = range;
            s.show_selected();
        });
    });
    window.on_dismiss(close);
    window.window().on_close_requested(|| {
        close();
        slint::CloseRequestResponse::HideWindow
    });
    let session = Rc::new(Session {
        window: window.clone_strong(),
        sources,
        selected: RefCell::new(0),
        range: RefCell::new(range),
        history_dir: paths.history.clone(),
        wording,
        folders,
        logs: RefCell::new(HashMap::new()),
        reading: RefCell::new(HashMap::new()),
        prices,
    });
    session.select_tag(tag);
    session.show_selected();
    SESSION.with(|s| *s.borrow_mut() = Some(session));
    if let Err(e) = window.show() {
        release();
        return Err(e.into());
    }
    dialog::bring_to_front(window.as_weak());
    Ok(())
}

/// Runs `f` with the open window's session.
fn with_session(f: impl FnOnce(&Session)) {
    let session = SESSION.with(|s| s.borrow().clone());
    if let Some(session) = session {
        f(&session);
    }
}

/// Closes the window and releases it ([`dialog::release_later`]). The last window closing ends
/// the dialog process's event loop.
fn close() {
    let session = SESSION.with(|s| s.borrow_mut().take());
    if let Some(session) = session {
        let _ = session.window.hide();
        dialog::release_later(session);
    }
}

/// Drops the window (after the event loop, or when it could not be shown).
pub fn release() {
    drop(SESSION.with(|s| s.borrow_mut().take()));
}

#[cfg(test)]
mod tests {
    use lastdrop_core::grammar::English;
    use lastdrop_core::model::{AccountKey, MeterId, MeterNames, ProviderId, Unit};
    use lastdrop_core::settings::{AccountSettings, ProviderEntry, ProviderSettings};

    use super::*;
    use crate::persist::SeenMeter;

    fn claude_settings(accounts: Vec<AccountSettings>) -> Settings {
        let mut claude = ProviderSettings::new(ProviderId::Claude, true);
        claude.accounts = accounts;
        Settings {
            providers: vec![
                ProviderEntry::Known(claude),
                ProviderEntry::Known(ProviderSettings::new(ProviderId::Meshy, false)),
            ],
            ..Settings::default()
        }
    }

    fn account(key: &str, alias: Option<&str>) -> AccountSettings {
        AccountSettings {
            key: AccountKey(key.to_owned()),
            alias: alias.map(str::to_owned),
            ..AccountSettings::primary()
        }
    }

    #[test]
    fn sources_name_accounts_and_list_catalogue_then_reported_meters() {
        let settings = claude_settings(vec![
            account("default", None),
            account("work", Some("Work")),
        ]);
        let fable = MeterKey {
            provider: ProviderId::Claude,
            account: AccountKey::primary(),
            meter: MeterId::weekly_model("fable"),
        };
        let credits = MeterKey {
            provider: ProviderId::Meshy,
            account: AccountKey::primary(),
            meter: MeterId::new(MeterId::CREDITS),
        };
        let saved = SavedState {
            seen: vec![
                SeenMeter {
                    key: fable.clone(),
                    names: MeterNames::weekly_model("Fable"),
                    balance: false,
                    unit: None,
                },
                SeenMeter {
                    key: credits.clone(),
                    names: MeterNames::new("Cr", "credits", "Credits"),
                    balance: true,
                    unit: Some(Unit::Credits),
                },
            ],
            ..SavedState::default()
        };
        let all = sources(&settings, &saved, &English);
        let labels: Vec<&str> = all.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["Claude · Account 1", "Claude · Work", "Meshy"]);
        let ids: Vec<&str> = all[0]
            .meters
            .iter()
            .map(|m| m.key.meter.0.as_str())
            .collect();
        assert_eq!(ids.last(), Some(&"weekly:fable"), "{ids:?}");
        assert!(ids.contains(&MeterId::WEEKLY));
        assert_eq!(all[2].meters[0].kind, SparkKind::Balance);
        assert_eq!(all[2].meters[0].unit, Some(Unit::Credits));

        // Only meters with a file stay; a service left without any goes.
        let dir = std::env::temp_dir().join(format!(
            "lastdrop-history-window-test-{}-{}",
            std::process::id(),
            Timestamp::now().as_nanosecond()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join(history::HistoryStore::file_name(&fable)), "").expect("write");
        let kept = with_history(all, &dir);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].meters.len(), 1);
        assert_eq!(kept[0].meters[0].key, fable);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tags_tell_sources_apart() {
        let a = SourceKey {
            provider: ProviderId::Claude,
            account: AccountKey::primary(),
        };
        let b = SourceKey {
            provider: ProviderId::Claude,
            account: AccountKey("work".to_owned()),
        };
        assert_ne!(source_tag(&a), source_tag(&b));
        assert!(source_tag(&a) <= 0xFF_FFFF);
    }
}
