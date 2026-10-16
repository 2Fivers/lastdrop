//! Services described in JSON (docs/design/custom-services-spec.md): the file format (§2), its
//! checks, and the registry of the services loaded at start-up (§3, §4). Pure: the app reads the
//! files and calls [`install`]; `lastdrop-providers` reads them with the registry's definitions.
//!
//! A described service is [`crate::model::ProviderId::Custom`] with a [`CustomId`], an index into
//! the registry. The registry is filled once per process, before settings are read; its entries
//! live for the process (`'static`), so names and catalogues can be handed out like the built-in
//! services' constants.

use std::sync::RwLock;

use jiff::SignedDuration;
use serde::Deserialize;

use crate::model::{MeterDef, MeterKind, ProviderId, WindowLen};

/// The file format this build reads (`"format": 1`).
pub const FORMAT: u32 = 1;

/// The most described services loaded.
pub const MAX_SERVICES: usize = 64;

/// A described service: an index into the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CustomId(pub u16);

/// A window meter of a described service (spec §2).
#[derive(Clone, Debug, PartialEq)]
pub struct CustomMeter {
    /// Its id within the service.
    pub id: String,
    /// R§2's names.
    pub label: String,
    /// See [`CustomMeter::label`].
    pub short_name: String,
    /// See [`CustomMeter::label`].
    pub name: String,
    /// How the share used is found.
    pub used: Used,
    /// A pointer to the reset instant.
    pub resets_at: Option<String>,
    /// The window's length.
    pub window: WindowLen,
}

/// How a meter's share used is found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Used {
    /// `used` ÷ `limit`, both pointers.
    OfLimit {
        /// Pointer to the amount used.
        used: String,
        /// Pointer to the limit.
        limit: String,
    },
    /// A pointer to a percent, 0–100.
    Percent(String),
}

/// A balance of a described service (spec §2).
#[derive(Clone, Debug, PartialEq)]
pub struct CustomBalance {
    /// Its id within the service.
    pub id: String,
    /// R§2's names.
    pub label: String,
    /// See [`CustomBalance::label`].
    pub short_name: String,
    /// See [`CustomBalance::label`].
    pub name: String,
    /// Pointer to the amount.
    pub amount: String,
    /// Pointer to an amount taken off it.
    pub minus: Option<String>,
    /// Multiplies it.
    pub scale: f64,
    /// The currency.
    pub currency: Currency,
}

/// What a balance counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Currency {
    /// Credits.
    Credits,
    /// A fixed ISO 4217 code.
    Code(String),
    /// A pointer to the code.
    At(String),
}

/// A described service, checked (spec §2).
#[derive(Clone, Debug, PartialEq)]
pub struct CustomService {
    /// The id (settings, history, credentials).
    pub id: String,
    /// The name as shown.
    pub name: String,
    /// The monogram (R§6.4).
    pub monogram: String,
    /// The endpoint, `https://`.
    pub url: String,
    /// The header that carries the key.
    pub key_header: String,
    /// Put before the key.
    pub key_prefix: String,
    /// The usage page, `https://`.
    pub usage_page: Option<String>,
    /// Its window meters.
    pub meters: Vec<CustomMeter>,
    /// Its balances.
    pub balances: Vec<CustomBalance>,
}

/// A service in the registry, with what the rest of the app needs as `'static` data.
#[derive(Debug)]
pub struct Registered {
    /// The definition.
    pub service: CustomService,
    /// [`CustomService::id`].
    pub id: &'static str,
    /// [`CustomService::name`].
    pub name: &'static str,
    /// [`CustomService::monogram`].
    pub monogram: &'static str,
    /// The catalogue (R§2): every meter, then every balance, in the file's order.
    pub catalog: &'static [MeterDef],
}

static REGISTRY: RwLock<Vec<&'static Registered>> = RwLock::new(Vec::new());

/// Puts `services` in the registry, in that order, replacing what was there (start-up, once per
/// process; tests may call it again with the same services). Services past [`MAX_SERVICES`] are
/// left out. Each entry is leaked: it lives for the process.
pub fn install(services: Vec<CustomService>) {
    let entries: Vec<&'static Registered> = services
        .into_iter()
        .take(MAX_SERVICES)
        .map(|service| {
            let leak = |s: &str| -> &'static str { Box::leak(s.to_owned().into_boxed_str()) };
            let mut catalog = Vec::new();
            for (i, m) in service.meters.iter().enumerate() {
                catalog.push(MeterDef {
                    id: leak(&m.id),
                    short_label: leak(&m.label),
                    short_name: leak(&m.short_name),
                    name: leak(&m.name),
                    order: u16::try_from(i).unwrap_or(u16::MAX),
                    kind: MeterKind::Window {
                        window: m.window,
                        starts_on_use: false,
                    },
                });
            }
            for (i, b) in service.balances.iter().enumerate() {
                catalog.push(MeterDef {
                    id: leak(&b.id),
                    short_label: leak(&b.label),
                    short_name: leak(&b.short_name),
                    name: leak(&b.name),
                    order: u16::try_from(service.meters.len() + i).unwrap_or(u16::MAX),
                    kind: MeterKind::Balance,
                });
            }
            let registered = Registered {
                id: leak(&service.id),
                name: leak(&service.name),
                monogram: leak(&service.monogram),
                catalog: Box::leak(catalog.into_boxed_slice()),
                service,
            };
            &*Box::leak(Box::new(registered))
        })
        .collect();
    if let Ok(mut registry) = REGISTRY.write() {
        *registry = entries;
    }
}

/// The registered service `id`.
pub fn get(id: CustomId) -> Option<&'static Registered> {
    REGISTRY.read().ok()?.get(usize::from(id.0)).copied()
}

/// The registered service whose file `id` is `name`.
pub fn find(name: &str) -> Option<CustomId> {
    let registry = REGISTRY.read().ok()?;
    let index = registry.iter().position(|r| r.id == name)?;
    u16::try_from(index).ok().map(CustomId)
}

/// Every registered service, in the order loaded.
pub fn all() -> Vec<CustomId> {
    let len = REGISTRY.read().map_or(0, |r| r.len());
    (0..len)
        .filter_map(|i| u16::try_from(i).ok())
        .map(CustomId)
        .collect()
}

// ---- The file -----------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    format: u32,
    id: String,
    name: String,
    #[serde(default)]
    monogram: Option<String>,
    url: String,
    #[serde(default)]
    key_header: Option<String>,
    #[serde(default)]
    key_prefix: Option<String>,
    #[serde(default)]
    usage_page: Option<String>,
    #[serde(default)]
    meters: Vec<FileMeter>,
    #[serde(default)]
    balances: Vec<FileBalance>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileMeter {
    id: String,
    label: String,
    short_name: String,
    name: String,
    #[serde(default)]
    used: Option<String>,
    #[serde(default)]
    limit: Option<String>,
    #[serde(default)]
    used_percent: Option<String>,
    #[serde(default)]
    resets_at: Option<String>,
    window: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileBalance {
    id: String,
    label: String,
    short_name: String,
    name: String,
    amount: String,
    #[serde(default)]
    minus: Option<String>,
    #[serde(default)]
    scale: Option<f64>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    currency_at: Option<String>,
}

/// An id: 1–32 of `a-z 0-9 _ -`, starting with a letter.
fn valid_id(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id.starts_with(|c: char| c.is_ascii_lowercase())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// A JSON pointer (RFC 6901): empty, or `/…`, at most 256 characters.
fn valid_pointer(pointer: &str) -> bool {
    pointer.len() <= 256 && (pointer.is_empty() || pointer.starts_with('/'))
}

/// An `https://` URL with a host and no spaces or quotes, at most 512 characters.
fn valid_https(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    url.len() <= 512
        && host_end > 0
        && !rest[..host_end].contains('@')
        && url
            .bytes()
            .all(|b| b.is_ascii_graphic() && !b"\"<>\\^`{|}".contains(&b))
}

/// A text of 1–`max` characters with no control characters.
fn valid_text(text: &str, max: usize) -> bool {
    let n = text.chars().count();
    (1..=max).contains(&n) && !text.chars().any(char::is_control)
}

/// A header name a file may use for the key.
fn valid_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && ![
            "host",
            "cookie",
            "content-length",
            "transfer-encoding",
            "connection",
        ]
        .contains(&lower.as_str())
        && !lower.starts_with("proxy-")
}

/// A window of a meter: `month`, `week`, `day`, `<n>h`, `<n>d` (1–9999).
fn window_of(text: &str) -> Option<WindowLen> {
    match text {
        "month" => return Some(WindowLen::CalendarMonth),
        "week" => return Some(WindowLen::Fixed(SignedDuration::from_hours(7 * 24))),
        "day" => return Some(WindowLen::Fixed(SignedDuration::from_hours(24))),
        _ => {}
    }
    let unit = text.chars().last()?;
    let number = &text[..text.len() - unit.len_utf8()];
    let n: i64 = number.parse().ok().filter(|n| (1..=9999).contains(n))?;
    match unit {
        'h' => Some(WindowLen::Fixed(SignedDuration::from_hours(n))),
        'd' => Some(WindowLen::Fixed(SignedDuration::from_hours(n * 24))),
        _ => None,
    }
}

/// Checks a meter's or balance's id (unique among `ids`) and names.
fn check_names(
    ids: &[&str],
    what: &str,
    id: &str,
    label: &str,
    short: &str,
    name: &str,
) -> Result<(), String> {
    if !valid_id(id) || ids.contains(&id) {
        return Err(format!("{what} {id}: id: 1–32 of a-z 0-9 _ -, each once"));
    }
    if !valid_text(label, 5) || !valid_text(short, 40) || !valid_text(name, 40) {
        return Err(format!(
            "{what} {id}: label ≤ 5, short_name and name ≤ 40 characters"
        ));
    }
    Ok(())
}

/// Parses and checks a file (spec §2). The error names the first problem (never a value of the
/// file beyond its field names).
pub fn parse(text: &str) -> Result<CustomService, String> {
    let file: File = serde_json::from_str(text).map_err(|e| {
        format!(
            "not a service file (line {}, column {}): {}",
            e.line(),
            e.column(),
            match e.classify() {
                serde_json::error::Category::Data => "a field is missing or has the wrong type",
                _ => "not JSON",
            }
        )
    })?;
    if file.format != FORMAT {
        return Err(format!(
            "format {} (this build reads {FORMAT})",
            file.format
        ));
    }
    if !valid_id(&file.id) {
        return Err("id: 1–32 of a-z 0-9 _ -, starting with a letter".to_owned());
    }
    let builtin = ProviderId::ALL.iter().any(|p| p.as_str() == file.id);
    if builtin {
        return Err("id: the id of a built-in service".to_owned());
    }
    if !valid_text(&file.name, 32) {
        return Err("name: 1–32 characters".to_owned());
    }
    let monogram = match file.monogram {
        Some(m) if valid_text(&m, 2) => m,
        Some(_) => return Err("monogram: 1–2 characters".to_owned()),
        None => file
            .name
            .chars()
            .next()
            .into_iter()
            .flat_map(char::to_uppercase)
            .collect(),
    };
    if !valid_https(&file.url) {
        return Err("url: an https:// address".to_owned());
    }
    if let Some(page) = &file.usage_page
        && !valid_https(page)
    {
        return Err("usage_page: an https:// address".to_owned());
    }
    let key_header = file
        .key_header
        .unwrap_or_else(|| "Authorization".to_owned());
    if !valid_header(&key_header) {
        return Err("key_header: not a header a key may go in".to_owned());
    }
    let key_prefix = file.key_prefix.unwrap_or_else(|| {
        if key_header.eq_ignore_ascii_case("authorization") {
            "Bearer ".to_owned()
        } else {
            String::new()
        }
    });
    if key_prefix.len() > 32 || key_prefix.chars().any(char::is_control) {
        return Err("key_prefix: at most 32 characters".to_owned());
    }
    if file.meters.is_empty() && file.balances.is_empty() {
        return Err("no meters and no balances".to_owned());
    }
    if file.meters.len() + file.balances.len() > 16 {
        return Err("more than 16 meters and balances".to_owned());
    }
    let mut ids: Vec<&str> = Vec::new();
    let mut meters = Vec::new();
    for m in &file.meters {
        check_names(&ids, "meter", &m.id, &m.label, &m.short_name, &m.name)?;
        ids.push(&m.id);
        let used = match (&m.used, &m.limit, &m.used_percent) {
            (Some(used), Some(limit), None) if valid_pointer(used) && valid_pointer(limit) => {
                Used::OfLimit {
                    used: used.clone(),
                    limit: limit.clone(),
                }
            }
            (None, None, Some(percent)) if valid_pointer(percent) => Used::Percent(percent.clone()),
            _ => {
                return Err(format!(
                    "meter {}: either used and limit, or used_percent (JSON pointers)",
                    m.id
                ));
            }
        };
        if m.resets_at.as_deref().is_some_and(|p| !valid_pointer(p)) {
            return Err(format!("meter {}: resets_at: a JSON pointer", m.id));
        }
        let window = window_of(&m.window)
            .ok_or_else(|| format!("meter {}: window: month, week, day, <n>h or <n>d", m.id))?;
        meters.push(CustomMeter {
            id: m.id.clone(),
            label: m.label.clone(),
            short_name: m.short_name.clone(),
            name: m.name.clone(),
            used,
            resets_at: m.resets_at.clone(),
            window,
        });
    }
    let mut balances = Vec::new();
    for b in &file.balances {
        check_names(&ids, "balance", &b.id, &b.label, &b.short_name, &b.name)?;
        ids.push(&b.id);
        if !valid_pointer(&b.amount) || b.minus.as_deref().is_some_and(|p| !valid_pointer(p)) {
            return Err(format!("balance {}: amount and minus: JSON pointers", b.id));
        }
        let scale = b.scale.unwrap_or(1.0);
        if !scale.is_finite() || scale <= 0.0 {
            return Err(format!("balance {}: scale: a positive number", b.id));
        }
        let currency = match (&b.currency, &b.currency_at) {
            (Some(code), None)
                if code.len() == 3 && code.bytes().all(|c| c.is_ascii_uppercase()) =>
            {
                Currency::Code(code.clone())
            }
            (None, Some(at)) if valid_pointer(at) => Currency::At(at.clone()),
            (None, None) => Currency::Credits,
            _ => {
                return Err(format!(
                    "balance {}: currency (an ISO 4217 code) or currency_at (a pointer), not both",
                    b.id
                ));
            }
        };
        balances.push(CustomBalance {
            id: b.id.clone(),
            label: b.label.clone(),
            short_name: b.short_name.clone(),
            name: b.name.clone(),
            amount: b.amount.clone(),
            minus: b.minus.clone(),
            scale,
            currency,
        });
    }
    Ok(CustomService {
        id: file.id,
        name: file.name,
        monogram,
        url: file.url,
        key_header,
        key_prefix,
        usage_page: file.usage_page,
        meters,
        balances,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ELEVENLABS: &str = r#"{
        "format": 1, "id": "elevenlabs", "name": "ElevenLabs",
        "url": "https://api.elevenlabs.io/v1/user/subscription",
        "key_header": "xi-api-key",
        "meters": [ { "id": "characters", "label": "Chars", "short_name": "characters",
            "name": "Characters", "used": "/character_count", "limit": "/character_limit",
            "resets_at": "/next_character_count_reset_unix", "window": "month" } ]
    }"#;

    #[test]
    fn a_good_file_reads_with_its_defaults() {
        let s = parse(ELEVENLABS).expect("service");
        assert_eq!(s.id, "elevenlabs");
        assert_eq!(s.monogram, "E");
        assert_eq!(s.key_header, "xi-api-key");
        assert_eq!(s.key_prefix, "", "no Bearer outside Authorization");
        assert_eq!(s.meters[0].window, WindowLen::CalendarMonth);
        assert_eq!(
            s.meters[0].used,
            Used::OfLimit {
                used: "/character_count".to_owned(),
                limit: "/character_limit".to_owned()
            }
        );
        let bearer = parse(
            r#"{ "format": 1, "id": "deepseek", "name": "DeepSeek",
                 "url": "https://api.deepseek.com/user/balance",
                 "balances": [ { "id": "balance", "label": "Bal", "short_name": "balance",
                    "name": "Balance", "amount": "/balance_infos/0/total_balance",
                    "currency_at": "/balance_infos/0/currency" } ] }"#,
        )
        .expect("service");
        assert_eq!(bearer.key_header, "Authorization");
        assert_eq!(bearer.key_prefix, "Bearer ");
        assert_eq!(
            bearer.balances[0].currency,
            Currency::At("/balance_infos/0/currency".to_owned())
        );
    }

    #[test]
    fn bad_files_say_why() {
        let with = |from: &str, to: &str| ELEVENLABS.replace(from, to);
        for (bad, why) in [
            (with("\"format\": 1", "\"format\": 2"), "format"),
            (with("\"elevenlabs\"", "\"Eleven Labs\""), "id"),
            (with("\"elevenlabs\"", "\"claude\""), "built-in"),
            (
                with("https://api.elevenlabs.io", "http://api.elevenlabs.io"),
                "url",
            ),
            (
                with(
                    "https://api.elevenlabs.io",
                    "https://user@api.elevenlabs.io",
                ),
                "url",
            ),
            (with("xi-api-key", "Cookie"), "key_header"),
            (with("\"month\"", "\"fortnight\""), "window"),
            (with("\"Chars\"", "\"Characters\""), "label"),
            (
                with("\"/character_limit\"", "\"character_limit\""),
                "used and limit",
            ),
            (with("\"window\"", "\"windows\""), "not a service file"),
            ("not json".to_owned(), "not a service file"),
        ] {
            let error = parse(&bad).expect_err(why);
            assert!(error.contains(why), "{why}: {error}");
        }
    }

    #[test]
    fn windows_read_their_lengths() {
        assert_eq!(
            window_of("5h"),
            Some(WindowLen::Fixed(SignedDuration::from_hours(5)))
        );
        assert_eq!(
            window_of("30d"),
            Some(WindowLen::Fixed(SignedDuration::from_hours(720)))
        );
        assert_eq!(window_of("0h"), None);
        assert_eq!(window_of("h"), None);
        assert_eq!(window_of(""), None);
        assert_eq!(window_of("5é"), None, "never splits a character");
    }
}
