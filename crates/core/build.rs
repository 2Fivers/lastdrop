//! Builds the string tables from `locales/*.toml`, one file per language with `en.toml` as the
//! source, into `$OUT_DIR/locales.rs`, which `src/grammar/strings.rs` includes. Every check a
//! translation has to pass runs here, so a missing or broken string stops the build:
//!
//! - each file has the `[language]` fields and exactly the keys of `en.toml`;
//! - each string has the `{placeholders}` of the English one;
//! - each plural string has exactly the forms of the language's plural rule
//!   (`src/grammar/plural.rs`, read here as it is compiled into the crate).
//!
//! The texts end up in the binary as `&'static str` arrays: nothing is parsed or allocated at
//! run time (docs/ARCHITECTURE.md, "Memory").

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

#[allow(dead_code)]
#[path = "src/grammar/plural.rs"]
mod plural;

use plural::{Plural, PluralRule};

/// The source language: every other file is checked against it.
const SOURCE: &str = "en";

/// One string of a locale file.
enum Entry {
    /// A plain string.
    Text(String),
    /// A plural string: its forms by their place in [`Plural::ALL`].
    Plural([Option<String>; Plural::ALL.len()]),
}

/// One locale file.
struct Locale {
    /// The BCP 47 tag: the file's name (`sv`, `pt-BR`).
    tag: String,
    /// The language's name in itself.
    name: String,
    /// The language's name in English.
    english_name: String,
    /// The `Language` variant and table struct: `english_name` without spaces.
    ident: String,
    rule: PluralRule,
    lowercase_names: bool,
    strings: BTreeMap<String, Entry>,
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    println!("cargo:rerun-if-changed=locales");
    println!("cargo:rerun-if-changed=src/grammar/plural.rs");

    let mut errors = Vec::new();
    let locales = read_all(&manifest.join("locales"), &mut errors);
    check(&locales, &mut errors);
    // Every problem of every file at once, so one build lists all a translation still needs.
    assert!(
        errors.is_empty(),
        "the locale files have errors (crates/core/locales/README.md says what each file \
         needs):\n{}",
        errors.join("\n")
    );

    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("locales.rs");
    fs::write(&out, generate(&locales)).expect("write locales.rs");
}

/// Every `*.toml` in `dir`, the source first, then by tag.
fn read_all(dir: &Path, errors: &mut Vec<String>) -> Vec<Locale> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|entry| entry.expect("locale directory entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    paths.sort();
    let mut locales: Vec<Locale> = paths
        .iter()
        .filter_map(|p| {
            read(p)
                .map_err(|e| errors.push(format!("locales/{}: {e}", file_name(p))))
                .ok()
        })
        .collect();
    locales.sort_by_key(|l| (l.tag != SOURCE, l.tag.clone()));
    locales
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn read(path: &Path) -> Result<Locale, String> {
    let tag = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("the file name is not a language tag")?
        .to_owned();
    if !is_tag(&tag) {
        return Err(format!(
            "`{tag}` is not a language tag (`de`, `pt-BR`); the file is named after its language"
        ));
    }
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut doc: toml::Table = text.parse().map_err(|e| format!("{e}"))?;

    let mut language = take_table(&mut doc, "language")?;
    let strings = take_table(&mut doc, "strings")?;
    if let Some(extra) = doc.keys().next() {
        return Err(format!(
            "unknown section `{extra}` (only [language] and [strings])"
        ));
    }

    let name = take_string(&mut language, "name")?;
    let english_name = take_string(&mut language, "english_name")?;
    let ident = ident(&english_name)?;
    let rule_name = take_string(&mut language, "plural")?;
    let rule = PluralRule::from_name(&rule_name).ok_or_else(|| {
        let names: Vec<&str> = PluralRule::ALL.iter().map(|r| r.name()).collect();
        format!(
            "unknown plural rule `{rule_name}`; one of: {} (src/grammar/plural.rs)",
            names.join(", ")
        )
    })?;
    let lowercase_names = match language.remove("lowercase_names") {
        Some(toml::Value::Boolean(b)) => b,
        Some(_) => return Err("[language] `lowercase_names` must be true or false".into()),
        None => return Err("[language] needs `lowercase_names`".into()),
    };
    if let Some(extra) = language.keys().next() {
        return Err(format!("[language] has an unknown field `{extra}`"));
    }

    let mut entries = BTreeMap::new();
    for (key, value) in strings {
        let entry = match value {
            toml::Value::String(s) => Entry::Text(s),
            toml::Value::Table(forms) => {
                let mut by_form: [Option<String>; Plural::ALL.len()] = Default::default();
                for (form, text) in forms {
                    let i = Plural::ALL
                        .iter()
                        .position(|p| p.name() == form)
                        .ok_or_else(|| format!("`{key}`: `{form}` is not a plural form"))?;
                    match text {
                        toml::Value::String(s) => by_form[i] = Some(s),
                        _ => return Err(format!("`{key}`.{form} must be a string")),
                    }
                }
                Entry::Plural(by_form)
            }
            _ => {
                return Err(format!(
                    "`{key}` must be a string or a table of plural forms"
                ));
            }
        };
        entries.insert(key, entry);
    }

    Ok(Locale {
        tag,
        name,
        english_name,
        ident,
        rule,
        lowercase_names,
        strings: entries,
    })
}

fn take_table(doc: &mut toml::Table, key: &str) -> Result<toml::Table, String> {
    match doc.remove(key) {
        Some(toml::Value::Table(t)) => Ok(t),
        Some(_) => Err(format!("`{key}` must be a section ([{key}])")),
        None => Err(format!("the [{key}] section is missing")),
    }
}

fn take_string(table: &mut toml::Table, key: &str) -> Result<String, String> {
    match table.remove(key) {
        Some(toml::Value::String(s)) if !s.trim().is_empty() => Ok(s),
        Some(_) => Err(format!("[language] `{key}` must be a non-empty string")),
        None => Err(format!("[language] needs `{key}`")),
    }
}

/// A BCP 47 tag as Lastdrop names files: a 2–3 letter language, then `-` subtags.
fn is_tag(tag: &str) -> bool {
    let mut parts = tag.split('-');
    let primary = parts.next().unwrap_or_default();
    (2..=3).contains(&primary.len())
        && primary.bytes().all(|b| b.is_ascii_lowercase())
        && parts.all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// `Norwegian Bokmal` → `NorwegianBokmal`.
fn ident(english_name: &str) -> Result<String, String> {
    let mut out = String::new();
    for word in english_name.split_whitespace() {
        if !word.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(format!(
                "[language] `english_name` must be plain ASCII words (`Norwegian Bokmal`), not `{english_name}`"
            ));
        }
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.extend(chars);
        }
    }
    if !out.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return Err(format!(
            "[language] `english_name` must start with a letter, not `{english_name}`"
        ));
    }
    Ok(out)
}

/// The `{name}` placeholders of a template, each once.
fn placeholders(template: &str) -> BTreeSet<&str> {
    template
        .split('{')
        .skip(1)
        .filter_map(|part| part.split_once('}').map(|(name, _)| name))
        .collect()
}

fn check(locales: &[Locale], errors: &mut Vec<String>) {
    let Some(source) = locales.iter().find(|l| l.tag == SOURCE) else {
        errors.push(format!(
            "locales/{SOURCE}.toml, the source of every language, is missing"
        ));
        return;
    };
    for (i, l) in locales.iter().enumerate() {
        let file = format!("locales/{}.toml", l.tag);
        let mut err = |e: String| errors.push(format!("{file}: {e}"));
        if let Some(other) = locales[..i].iter().find(|o| o.ident == l.ident) {
            err(format!(
                "`english_name` {:?} is taken by locales/{}.toml",
                l.english_name, other.tag
            ));
        }
        for key in source
            .strings
            .keys()
            .filter(|k| !l.strings.contains_key(*k))
        {
            err(format!("`{key}` is missing"));
        }
        for key in l
            .strings
            .keys()
            .filter(|k| !source.strings.contains_key(*k))
        {
            err(format!("`{key}` is not a key of locales/{SOURCE}.toml"));
        }
        for (key, entry) in &l.strings {
            let Some(english) = source.strings.get(key) else {
                continue;
            };
            match (entry, english) {
                (Entry::Text(text), Entry::Text(en)) => {
                    check_text(key, text, en, &mut err);
                }
                (Entry::Plural(forms), Entry::Plural(en_forms)) => {
                    let en = en_forms[Plural::ALL.len() - 1]
                        .as_deref()
                        .unwrap_or_default();
                    let has: Vec<Plural> = Plural::ALL
                        .into_iter()
                        .zip(forms)
                        .filter_map(|(p, f)| f.as_ref().map(|_| p))
                        .collect();
                    if has != l.rule.forms() {
                        let names = |ps: &[Plural]| {
                            ps.iter().map(|p| p.name()).collect::<Vec<_>>().join(", ")
                        };
                        err(format!(
                            "`{key}` has the forms {}; plural rule `{}` needs exactly {}",
                            names(&has),
                            l.rule.name(),
                            names(l.rule.forms())
                        ));
                    }
                    for (p, text) in Plural::ALL.into_iter().zip(forms) {
                        if let Some(text) = text {
                            check_text(&format!("{key}.{}", p.name()), text, en, &mut err);
                        }
                    }
                }
                (Entry::Text(_), Entry::Plural(_)) => {
                    err(format!(
                        "`{key}` is a plural string in English: give its forms"
                    ));
                }
                (Entry::Plural(_), Entry::Text(_)) => {
                    err(format!(
                        "`{key}` is a plain string in English, not a plural one"
                    ));
                }
            }
        }
    }
}

fn check_text(key: &str, text: &str, english: &str, err: &mut impl FnMut(String)) {
    if text.trim().is_empty() {
        err(format!("`{key}` is empty"));
    }
    let (have, want) = (placeholders(text), placeholders(english));
    if have != want {
        let list = |s: &BTreeSet<&str>| {
            s.iter()
                .map(|p| format!("{{{p}}}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        err(format!(
            "`{key}` has the placeholders [{}]; English has [{}]",
            list(&have),
            list(&want)
        ));
    }
}

fn generate(locales: &[Locale]) -> String {
    let source = &locales[0];
    let keys = |plural: bool| -> Vec<&str> {
        source
            .strings
            .iter()
            .filter(|(_, e)| matches!(e, Entry::Plural(_)) == plural)
            .map(|(k, _)| k.as_str())
            .collect()
    };
    let (texts, plurals) = (keys(false), keys(true));
    let forms = Plural::ALL.len();

    let mut o = String::from(
        "// Generated by crates/core/build.rs from crates/core/locales/*.toml. Do not edit.\n\n",
    );
    let _ = writeln!(
        o,
        "/// Every plain message key of the locale files, sorted by bytes: the index of a [`Msg`].\n\
         const KEYS: [&str; {}] = {texts:?};\n",
        texts.len()
    );
    let _ = writeln!(
        o,
        "/// Every plural message key, sorted by bytes: the index of a [`PluralMsg`].\n\
         const PLURAL_KEYS: [&str; {}] = {plurals:?};\n",
        plurals.len()
    );

    o.push_str(
        "/// A language with a string table: one per file in `crates/core/locales` (ROADMAP M4 \
         \"Language\").\n\
         #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]\npub enum Language {\n",
    );
    for l in locales {
        let fallback = if l.tag == SOURCE {
            ": the source, and the fallback for every language without a table"
        } else {
            ""
        };
        let _ = writeln!(
            o,
            "    /// {} (`{}`), from `locales/{}.toml`{fallback}.",
            l.english_name, l.tag, l.tag
        );
        if l.tag == SOURCE {
            o.push_str("    #[default]\n");
        }
        let _ = writeln!(o, "    {},", l.ident);
    }
    o.push_str("}\n\nimpl Language {\n");
    let _ = writeln!(
        o,
        "    /// Every language with a table, in the order a language picker lists them: English \
         first, then by tag.\n    pub const ALL: [Language; {}] = [{}];\n",
        locales.len(),
        locales
            .iter()
            .map(|l| format!("Language::{}", l.ident))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let arms = |value: &dyn Fn(&Locale) -> String| -> String {
        locales
            .iter()
            .map(|l| format!("            Language::{} => {},\n", l.ident, value(l)))
            .collect()
    };
    let _ = writeln!(
        o,
        "    /// The BCP 47 language tag (`en`, `sv`): the locale file's name, [`Strings::language`] \
         and the value of the language setting.\n    \
         pub const fn tag(self) -> &'static str {{\n        match self {{\n{}        }}\n    }}\n",
        arms(&|l| format!("{:?}", l.tag))
    );
    let _ = writeln!(
        o,
        "    /// The language's name in itself (`English`, `Svenska`), as language pickers show \
         it.\n    pub const fn native_name(self) -> &'static str {{\n        match self {{\n{}        \
         }}\n    }}\n",
        arms(&|l| format!("{:?}", l.name))
    );
    let _ = writeln!(
        o,
        "    /// The string table.\n    pub fn strings(self) -> &'static dyn Strings {{\n        \
         match self {{\n{}        }}\n    }}\n",
        arms(&|l| format!("&{}", l.ident))
    );
    let _ = writeln!(
        o,
        "    /// How a count picks its plural form.\n    \
         pub const fn plural_rule(self) -> PluralRule {{\n        match self {{\n{}        }}\n    \
         }}\n",
        arms(&|l| format!("PluralRule::{:?}", l.rule))
    );
    let _ = writeln!(
        o,
        "    /// Meter names start lower-case inside a sentence (`weekly, all models`).\n    \
         pub const fn lowercase_names(self) -> bool {{\n        match self {{\n{}        }}\n    \
         }}\n",
        arms(&|l| l.lowercase_names.to_string())
    );
    let _ = writeln!(
        o,
        "    const fn texts(self) -> &'static [&'static str; KEYS.len()] {{\n        \
         match self {{\n{}        }}\n    }}\n",
        arms(&|l| format!("&TEXTS_{}", l.ident.to_ascii_uppercase()))
    );
    let _ = writeln!(
        o,
        "    const fn plural_texts(self) -> &'static [[&'static str; {forms}]; PLURAL_KEYS.len()] \
         {{\n        match self {{\n{}        }}\n    }}\n}}\n",
        arms(&|l| format!("&PLURALS_{}", l.ident.to_ascii_uppercase()))
    );

    for l in locales {
        let upper = l.ident.to_ascii_uppercase();
        let _ = writeln!(o, "static TEXTS_{upper}: [&str; KEYS.len()] = [");
        for key in &texts {
            if let Some(Entry::Text(text)) = l.strings.get(*key) {
                let _ = writeln!(o, "    {text:?}, // {key}");
            }
        }
        o.push_str("];\n\n");
        let _ = writeln!(
            o,
            "static PLURALS_{upper}: [[&str; {forms}]; PLURAL_KEYS.len()] = ["
        );
        for key in &plurals {
            if let Some(Entry::Plural(by_form)) = l.strings.get(*key) {
                let list: Vec<String> = by_form
                    .iter()
                    .map(|f| format!("{:?}", f.as_deref().unwrap_or_default()))
                    .collect();
                let _ = writeln!(o, "    [{}], // {key}", list.join(", "));
            }
        }
        o.push_str("];\n\n");
        let _ = writeln!(
            o,
            "/// The {} table (`locales/{}.toml`).\n\
             #[derive(Clone, Copy, Debug, Default)]\npub struct {};\n\ntable!({});\n",
            l.english_name, l.tag, l.ident, l.ident
        );
    }
    o
}
