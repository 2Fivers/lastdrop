//! Theme files (docs/design/themes-spec.md §3): the built-in themes and the user's, one JSON file
//! each in `%APPDATA%\Lastdrop\themes`, named after the theme's id. Both processes read them; only
//! the dialog process (Theme Studio) writes them.

use std::path::{Path, PathBuf};

use lastdrop_core::grammar::strings::{Msg, Strings};
use lastdrop_core::theme::{self, BUILT_IN, DEFAULT_ID, Theme};

/// The largest theme file read.
pub const MAX_FILE: u64 = 64 << 10;

/// One theme a picker lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeEntry {
    /// Its id: a built-in one or the file's name without `.json`.
    pub id: String,
    /// Its name as shown.
    pub name: String,
    /// One of [`BUILT_IN`].
    pub built_in: bool,
}

/// The file of the user theme `id`.
pub fn file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

/// A built-in theme's name in the chosen language.
pub fn built_in_name(id: &str, strings: &dyn Strings) -> Option<String> {
    let msg = match id {
        DEFAULT_ID => Msg::ThemeWindows,
        "segmented" => Msg::ThemeSegmented,
        "compact" => Msg::ThemeCompact,
        "ember" => Msg::ThemeEmber,
        _ => return None,
    };
    Some(strings.template(msg).to_owned())
}

/// An id is a user theme's file name: lower-case ASCII letters, digits and `-` only (what
/// [`theme::id_for`] makes), so a settings value can never name a file elsewhere.
fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= theme::MAX_NAME + 3
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Reads the user theme `id`; `None` (logged) when it is missing, too large or broken.
fn read_file(dir: &Path, id: &str) -> Option<Theme> {
    if !safe_id(id) {
        log::warn!("themes: {id:?} is not a theme id");
        return None;
    }
    let path = file(dir, id);
    match std::fs::metadata(&path) {
        Ok(m) if m.len() > MAX_FILE => {
            log::warn!("themes: {id}.json is larger than 64 KB; ignored");
            return None;
        }
        Ok(_) => {}
        Err(e) => {
            log::warn!("themes: {id}.json not read: {e}");
            return None;
        }
    }
    let json = std::fs::read_to_string(&path)
        .inspect_err(|e| log::warn!("themes: {id}.json not read: {e}"))
        .ok()?;
    Theme::parse(&json, id)
        .inspect_err(|e| log::warn!("themes: {id}.json ignored: {e}"))
        .ok()
}

/// The theme of the settings' `theme` id: a built-in one, else the user's file, else (missing or
/// broken) the Windows theme.
pub fn load(dir: &Path, id: Option<&str>) -> Theme {
    let id = id.unwrap_or(DEFAULT_ID);
    Theme::built_in(id)
        .or_else(|| read_file(dir, id))
        .unwrap_or_default()
}

/// The built-in themes, then the user's files that parse, by name.
pub fn list(dir: &Path, strings: &dyn Strings) -> Vec<ThemeEntry> {
    let mut out: Vec<ThemeEntry> = BUILT_IN
        .iter()
        .map(|id| ThemeEntry {
            id: (*id).to_owned(),
            name: built_in_name(id, strings).unwrap_or_else(|| (*id).to_owned()),
            built_in: true,
        })
        .collect();
    let mut user: Vec<ThemeEntry> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let id = name.to_str()?.strip_suffix(".json")?.to_owned();
            if !safe_id(&id) || BUILT_IN.contains(&id.as_str()) {
                return None;
            }
            let theme = read_file(dir, &id)?;
            Some(ThemeEntry {
                id,
                name: theme.name,
                built_in: false,
            })
        })
        .collect();
    user.sort_by_key(|a| a.name.to_lowercase());
    out.extend(user);
    out
}

/// Writes `theme` as the user theme named after it ([`theme::id_for`]), replacing a file of the
/// same id, atomically; returns the id.
pub fn save(dir: &Path, theme: &Theme) -> std::io::Result<String> {
    std::fs::create_dir_all(dir)?;
    let id = theme::id_for(&theme.name);
    crate::persist::write_atomic(&file(dir, &id), theme.to_json().as_bytes())?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use lastdrop_core::grammar::English;
    use lastdrop_core::theme::{Density, ThemeColor};

    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lastdrop-themes-test-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn saved_themes_load_and_list_after_the_built_ins() {
        let dir = temp_dir();
        let mut theme = Theme::built_in("compact").expect("built in");
        theme.name = "Night Shift".to_owned();
        theme.accent = Some(ThemeColor::rgb(0x33_66_99));
        let id = save(&dir, &theme).expect("save");
        assert_eq!(id, "night-shift");
        assert_eq!(load(&dir, Some(&id)), theme);
        std::fs::write(dir.join("broken.json"), "{").expect("write");
        std::fs::write(dir.join("Evil Name.json"), "{}").expect("write");
        let names: Vec<String> = list(&dir, &English).into_iter().map(|e| e.name).collect();
        assert_eq!(
            names,
            ["Windows", "Segmented", "Compact", "Ember", "Night Shift"]
        );
        // Missing, broken or unsafe ids fall back to the Windows theme.
        assert_eq!(load(&dir, Some("broken")), Theme::default());
        assert_eq!(load(&dir, Some("..\\settings")), Theme::default());
        assert_eq!(load(&dir, None), Theme::default());
        assert_eq!(load(&dir, Some("compact")).layout.density, Density::Compact);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
