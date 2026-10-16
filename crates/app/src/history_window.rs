//! The usage history window (docs/design/charts-spec.md) in the dialog process: the services
//! with history, a range, and the charts `lastdrop_core::charts` builds from the history files of
//! the selected service, read when the window opens and when the service or range changes.
//!
//! Threading: the main thread runs the window; the files are read on it (a few megabytes at
//! most, charts-spec §8).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::charts::{self, ChartMeter, HistoryInput, HistoryModel, Range};
use lastdrop_core::flyout::SparkKind;
use lastdrop_core::grammar::strings::text;
use lastdrop_core::grammar::{LocaleFormat, Msg, Strings};
use lastdrop_core::model::{MeterKey, MeterKind, SourceKey, catalog, fnv1a64};
use lastdrop_core::settings::Settings;
use lastdrop_core::stats::Sample;
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
