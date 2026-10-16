//! Described services as services (custom-services-spec §4): a registry of their own, so this
//! file is a test binary of its own (the registry is per process).

use lastdrop_core::custom::{self, CustomId};
use lastdrop_core::glyph::ProviderGlyph;
use lastdrop_core::model::{MeterKind, ProviderId, catalog};
use lastdrop_core::settings::{ProviderEntry, Settings};

const DEEPSEEK: &str = r#"{
    "format": 1, "id": "deepseek", "name": "DeepSeek",
    "url": "https://api.deepseek.com/user/balance",
    "balances": [ { "id": "balance", "label": "Bal", "short_name": "balance", "name": "Balance",
        "amount": "/balance_infos/0/total_balance", "currency_at": "/balance_infos/0/currency" } ]
}"#;

const OPENROUTER: &str = r#"{
    "format": 1, "id": "openrouter", "name": "OpenRouter", "monogram": "OR",
    "url": "https://openrouter.ai/api/v1/credits",
    "meters": [ { "id": "key", "label": "Key", "short_name": "key limit", "name": "Key limit",
        "used_percent": "/data/percent", "window": "month" } ],
    "balances": [ { "id": "credits", "label": "Cr", "short_name": "credits", "name": "Credits",
        "amount": "/data/total_credits", "minus": "/data/total_usage", "currency": "USD" } ]
}"#;

#[test]
fn described_services_are_services() {
    let services = vec![
        custom::parse(DEEPSEEK).expect("deepseek"),
        custom::parse(OPENROUTER).expect("openrouter"),
    ];
    custom::install(services);

    let deepseek = ProviderId::from_id("deepseek").expect("loaded");
    assert_eq!(deepseek, ProviderId::Custom(CustomId(0)));
    assert_eq!(deepseek.as_str(), "deepseek");
    assert_eq!(deepseek.display_name(), "DeepSeek");
    assert_eq!(deepseek.glyph(), ProviderGlyph::Monogram("D".to_owned()));
    assert!(deepseek.uses_api_key());
    assert_eq!(deepseek.client_name(), None);

    // Written and read back by its id.
    let json = serde_json::to_string(&deepseek).expect("serialize");
    assert_eq!(json, "\"deepseek\"");
    assert_eq!(
        serde_json::from_str::<ProviderId>(&json).expect("deserialize"),
        deepseek
    );
    assert!(serde_json::from_str::<ProviderId>("\"gone\"").is_err());

    // The catalogue: meters first, then balances, in the file's order.
    let openrouter = ProviderId::from_id("openrouter").expect("loaded");
    let defs = catalog(openrouter);
    assert_eq!(defs.len(), 2);
    assert_eq!(defs[0].id, "key");
    assert!(matches!(defs[0].kind, MeterKind::Window { .. }));
    assert_eq!(defs[1].id, "credits");
    assert_eq!(defs[1].kind, MeterKind::Balance);
    assert_eq!(openrouter.glyph(), ProviderGlyph::Monogram("OR".to_owned()));

    // In settings like any service: listed, turned off, after the built-in ones.
    let every = ProviderId::every();
    assert_eq!(&every[every.len() - 2..], [deepseek, openrouter]);
    let settings = Settings::default();
    let entry = settings.provider(deepseek).expect("listed");
    assert!(!entry.enabled);

    // A settings entry of a service whose file is gone is kept as it is, never shown.
    let s: Settings = serde_json::from_str(
        r#"{ "version": 1, "providers": [ { "id": "gone", "enabled": true } ] }"#,
    )
    .expect("settings");
    assert!(matches!(s.providers[0], ProviderEntry::Unknown(_)));
    let out = serde_json::to_value(&s).expect("serialize");
    assert_eq!(out["providers"][0]["id"], "gone");
}
