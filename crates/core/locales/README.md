# Lastdrop's languages

One TOML file per language, named after its tag; `en.toml` is the source every other file is
checked against. At build time `crates/core/build.rs` checks every file and turns them into the
string tables of `src/grammar/strings.rs`. The texts end up in the binary as they are: nothing
is read, parsed or allocated at run time.

## Add a language

1. Copy `en.toml` to `<tag>.toml`, where the tag is how Windows names the display language:
   `de`, `fr`, `nb`, `pt-BR`. A file for a language (`de`) serves every region of it (`de-DE`,
   `de-AT`). A file with a region (`pt-BR`) serves that region only.
2. Fill in `[language]` (below).
3. Translate every string under `[strings]`. Keep the keys and the `{placeholders}`.
4. Run `cargo build`. It lists everything a file lacks or gets wrong, then
   `cargo test -p lastdrop-core` checks the shape of each string.
5. To look at it, run `lastdrop.exe --demo --language <tag> --preview-flyout flyout.png`, and the
   same with `--preview-hovercard`, `--preview-settings` and `--preview-about`.
   `--preview-settings out\settings.png` writes every page of the settings window, and a few of
   its states, at its preferred size and its smallest (`out\settings-general-680x520.png`, …):
   the smallest is where a long text runs out of room first. Add `--dark` or `--scale 1.5` to
   see it as more users will. [CONTRIBUTING.md](../../../CONTRIBUTING.md#previews) lists every
   preview and option.

The language appears in the settings window by its own name and is picked automatically when
it is one of Windows' display languages. Nothing in the code needs to change.

## `[language]`

| Field | Meaning |
|---|---|
| `name` | The language's name in itself (`Deutsch`), as the language picker shows it. |
| `english_name` | Its name in English (`German`), plain ASCII words. It becomes the name of the language in the code (`Language::German`). |
| `plural` | How a count picks its plural form (below). |
| `lowercase_names` | `true` when a meter name starts lower-case inside a sentence (`weekly, all models`). `false` for languages that capitalise nouns (German). |

## Strings

- **Placeholders** (`{time}`, `{pct}`, `{service}`, …) must all be kept, in any order. The
  build checks them against English. What each one holds is listed at the top of
  `src/grammar/strings.rs`.
- **Percent is always "used"**, never "left" (`42 % used`). The one exception is `{left}` on the
  hover card's OK sentence, the share left at the reset.
- **Spoken percentages** (`a11y.*`, `flyout.*.a11y`) are written `{n} percent` with a bare
  number. Never write `{pct} percent`, since `{pct}` already carries the sign.
- **Joined fragments** keep the shape of the English one. A leading space or `, ` stays,
  because it is glued to other text, and so does a closing full stop or `…`. A text that English
  ends without a full stop may still end in an abbreviation's (French `auj.`, German `Tsd.`).
  The tests check this.
- **Capitals:** titles and verdicts are capitalised by the code after filling, so a template
  may start with `{meter}`, `{label}` or `{short_name}`.
- **Short tokens** (`5h`, `Wk`, `{n}d`) sit in the taskbar readout. Keep them as short as the
  English ones. Widths are measured from the strings, so nothing is cut off, but a longer token
  takes room from the bar.
- **Not in these files:** numbers, percent signs, times, weekday and month names come from the
  Windows regional format.

## Plural strings

A string that counts something has one text per plural form:

```toml
"unit.minute" = { one = "{n} minute", other = "{n} minutes" }
```

Each language gives exactly the forms its `plural` rule picks. Polish (`plural = "polish"`):

```toml
"unit.minute" = { one = "{n} minuta", few = "{n} minuty", many = "{n} minut" }
```

| Rule | Forms | Languages |
|---|---|---|
| `other` | other | Japanese, Chinese, Korean, Vietnamese, Thai, Indonesian |
| `one` | one (1), other | English, Swedish, German, Dutch, Danish, Norwegian, Finnish, Estonian, Greek, Hungarian, Turkish, Bulgarian; Spanish, Italian, Portuguese (Portugal) |
| `zero-one` | one (0, 1), other | French, Portuguese (Brazil), Hindi, Persian |
| `czech` | one (1), few (2–4), other | Czech, Slovak |
| `polish` | one (1), few (2–4, 22–24, …), many | Polish |
| `russian` | one (1, 21, 31, …), few (2–4, 22–24, …), many | Russian, Ukrainian, Belarusian |
| `croatian` | one (1, 21, 31, …), few (2–4, 22–24, …), other | Croatian, Serbian, Bosnian |

A balance's unit is a plural string without its number, because the template places the
amount (`{amount} {unit}`) and the flyout shows the two apart:

```toml
"unit.credits" = { one = "credit", other = "credits" }
```

The number as shown picks the form: amounts are rounded to whole numbers, so 1.4 credits read
`1 credit` and 1.5 `2 credits`. In `card.balance.left` the unit counts the top-up, `{ref}`
(`1 of 3,260 credits left`); everywhere else it counts `{amount}`.

The rules are CLDR's for whole numbers (`src/grammar/plural.rs`). A language with another rule
(Arabic, Lithuanian, Slovenian, …) needs a new `PluralRule` there, with its tests. Languages
written right to left also need layout work in the flyout and the readout first.

## Add or change a message (developers)

1. Add the key with its text to `en.toml` and to every other file. The build names each file
   that lacks it.
2. Declare it in `src/grammar/strings.rs` as `Name = "key",`, under `Msg`, or under `PluralMsg`
   for a plural string.

A key in `en.toml` that nothing declares, or a declaration whose key is not in `en.toml`, stops
the build.
