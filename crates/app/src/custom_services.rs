//! The services described in JSON (docs/design/custom-services-spec.md §3): the files Lastdrop
//! ships and the user's in `%APPDATA%\Lastdrop\providers\`, read at start-up, before the settings,
//! by both processes, and put in `lastdrop_core::custom`'s registry.

use std::path::Path;

use lastdrop_core::custom::{self, CustomService};

/// The files Lastdrop ships (spec §7).
pub const BUNDLED: [(&str, &str); 3] = [
    ("deepseek.json", include_str!("../providers/deepseek.json")),
    (
        "elevenlabs.json",
        include_str!("../providers/elevenlabs.json"),
    ),
    (
        "openrouter.json",
        include_str!("../providers/openrouter.json"),
    ),
];

/// The most files read from the user's folder.
pub const MAX_FILES: usize = 64;

/// The largest file read, in bytes.
pub const MAX_FILE_BYTES: u64 = 64 << 10;

/// The services of the bundled files and of `user` (`(file name, contents)`, sorted by name): a
/// user file with a bundled file's id replaces it in place, the rest follow in name order; a file
/// that does not follow the spec, or repeats an id another user file took, is left out and logged
/// (its name and why, never its contents).
pub fn merge(user: &[(String, String)]) -> Vec<CustomService> {
    let mut services: Vec<CustomService> = BUNDLED
        .iter()
        .filter_map(|(name, text)| match custom::parse(text) {
            Ok(service) => Some(service),
            Err(e) => {
                log::error!("providers: bundled {name}: {e}");
                None
            }
        })
        .collect();
    let bundled = services.len();
    for (name, text) in user {
        let service = match custom::parse(text) {
            Ok(service) => service,
            Err(e) => {
                log::warn!("providers: {name} left out: {e}");
                continue;
            }
        };
        match services.iter().position(|s| s.id == service.id) {
            Some(i) if i < bundled => {
                log::info!("providers: {name} replaces the bundled {}", service.id);
                services[i] = service;
            }
            Some(_) => log::warn!(
                "providers: {name} left out: another file has the id {}",
                service.id
            ),
            None => services.push(service),
        }
    }
    services
}

/// The `*.json` files of `dir` (at most [`MAX_FILES`], each at most [`MAX_FILE_BYTES`]), sorted by
/// name, as `(file name, contents)`. A missing folder has none.
pub fn read_folder(dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(String, std::path::PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            name.to_ascii_lowercase()
                .ends_with(".json")
                .then(|| (name, e.path()))
        })
        .collect();
    files.sort();
    if files.len() > MAX_FILES {
        log::warn!(
            "providers: {} files; only the first {MAX_FILES} are read",
            files.len()
        );
        files.truncate(MAX_FILES);
    }
    files
        .into_iter()
        .filter_map(|(name, path)| {
            let size = std::fs::metadata(&path)
                .map(|m| m.len())
                .unwrap_or(u64::MAX);
            if size > MAX_FILE_BYTES {
                log::warn!("providers: {name} left out: larger than 64 KiB");
                return None;
            }
            match std::fs::read_to_string(&path) {
                Ok(text) => Some((name, text)),
                Err(e) => {
                    log::warn!("providers: {name} left out: {e}");
                    None
                }
            }
        })
        .collect()
}

/// Reads the bundled files and the user's folder `dir` and installs the services (start-up).
pub fn load(dir: &Path) {
    let services = merge(&read_folder(dir));
    let ids: Vec<&str> = services.iter().map(|s| s.id.as_str()).collect();
    log::info!("providers: described services {ids:?}");
    custom::install(services);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_files_follow_the_spec() {
        let services = merge(&[]);
        let ids: Vec<&str> = services.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["deepseek", "elevenlabs", "openrouter"]);
    }

    #[test]
    fn user_files_replace_add_or_are_left_out() {
        let own = |id: &str, name: &str| {
            format!(
                r#"{{ "format": 1, "id": "{id}", "name": "{name}", "url": "https://example.invalid/x",
                     "balances": [ {{ "id": "b", "label": "B", "short_name": "b", "name": "B",
                     "amount": "/a" }} ] }}"#
            )
        };
        let user = vec![
            ("a.json".to_owned(), own("acme", "Acme")),
            ("b.json".to_owned(), own("deepseek", "My DeepSeek")),
            ("c.json".to_owned(), own("acme", "Acme again")),
            ("d.json".to_owned(), "not json".to_owned()),
        ];
        let services = merge(&user);
        let names: Vec<&str> = services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["My DeepSeek", "ElevenLabs", "OpenRouter", "Acme"]);
    }
}
