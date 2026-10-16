//! Plural rules: how a count picks its plural form (`1 minute`, `5 minutes`; Polish `2 minuty`,
//! `5 minut`), from CLDR's plural rules for whole numbers. Lastdrop only counts whole things
//! (minutes, hours, days, weeks), so the forms CLDR keeps for fractions are left out.
//!
//! Each locale file names its rule (`plural = "one"`, `crates/core/locales/README.md`). The build
//! script reads this same file, so a plural string must have exactly the forms its rule picks.
//!
//! | Rule | Forms | Languages |
//! |---|---|---|
//! | `other` | other | Japanese, Chinese, Korean, Vietnamese, Thai, Indonesian |
//! | `one` | one (1), other | English, Swedish, German, Dutch, Danish, Norwegian, Finnish, Estonian, Greek, Hungarian, Turkish, Bulgarian; Spanish, Italian and Portuguese (Portugal) below a million |
//! | `zero-one` | one (0, 1), other | French below a million, Portuguese (Brazil), Hindi, Persian |
//! | `czech` | one (1), few (2–4), other | Czech, Slovak |
//! | `polish` | one (1), few (2–4, 22–24, …), many | Polish |
//! | `russian` | one (1, 21, 31, …), few (2–4, 22–24, …), many | Russian, Ukrainian, Belarusian |
//! | `croatian` | one (1, 21, 31, …), few (2–4, 22–24, …), other | Croatian, Serbian, Bosnian |
//!
//! CLDR gives Spanish, Italian, French and Portuguese a `many` form for exact millions
//! (`1 000 000 de minutes`); no count Lastdrop shows gets near one. A language whose rule is not
//! here (Arabic, Lithuanian, Slovenian, …) needs a new [`PluralRule`] with its forms and tests.

/// A CLDR plural category: the form a count takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Plural {
    /// `zero`.
    Zero,
    /// `one`: the singular, in most languages for 1 only.
    One,
    /// `two`.
    Two,
    /// `few` (Polish `2 minuty`).
    Few,
    /// `many` (Polish `5 minut`).
    Many,
    /// `other`: the form of every count the other categories do not take.
    Other,
}

impl Plural {
    /// Every category, in CLDR order. A category's place here is its index in a plural table.
    pub const ALL: [Plural; 6] = [
        Plural::Zero,
        Plural::One,
        Plural::Two,
        Plural::Few,
        Plural::Many,
        Plural::Other,
    ];

    /// The category's name in a locale file (`one`, `other`).
    pub const fn name(self) -> &'static str {
        match self {
            Plural::Zero => "zero",
            Plural::One => "one",
            Plural::Two => "two",
            Plural::Few => "few",
            Plural::Many => "many",
            Plural::Other => "other",
        }
    }
}

/// How a language picks the plural form of a whole number (module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PluralRule {
    /// No plural forms: every count is `other`.
    Other,
    /// 1 is `one`, everything else `other`.
    One,
    /// 0 and 1 are `one`, everything else `other`.
    ZeroOne,
    /// 1 is `one`, 2–4 `few`, everything else `other`.
    Czech,
    /// 1 is `one`; 2–4, 22–24, 32–34, … (not 12–14) `few`; everything else `many`.
    Polish,
    /// 1, 21, 31, … (not 11) `one`; 2–4, 22–24, … (not 12–14) `few`; everything else `many`.
    Russian,
    /// As [`PluralRule::Russian`], with `other` in place of `many`.
    Croatian,
}

impl PluralRule {
    /// Every rule.
    pub const ALL: [PluralRule; 7] = [
        PluralRule::Other,
        PluralRule::One,
        PluralRule::ZeroOne,
        PluralRule::Czech,
        PluralRule::Polish,
        PluralRule::Russian,
        PluralRule::Croatian,
    ];

    /// The rule's name in a locale file (`plural = "one"`).
    pub const fn name(self) -> &'static str {
        match self {
            PluralRule::Other => "other",
            PluralRule::One => "one",
            PluralRule::ZeroOne => "zero-one",
            PluralRule::Czech => "czech",
            PluralRule::Polish => "polish",
            PluralRule::Russian => "russian",
            PluralRule::Croatian => "croatian",
        }
    }

    /// The rule a locale file names.
    pub fn from_name(name: &str) -> Option<PluralRule> {
        PluralRule::ALL.into_iter().find(|r| r.name() == name)
    }

    /// The forms the rule picks, in CLDR order: exactly the forms each plural string of the
    /// language has.
    pub const fn forms(self) -> &'static [Plural] {
        match self {
            PluralRule::Other => &[Plural::Other],
            PluralRule::One | PluralRule::ZeroOne => &[Plural::One, Plural::Other],
            PluralRule::Czech | PluralRule::Croatian => &[Plural::One, Plural::Few, Plural::Other],
            PluralRule::Polish | PluralRule::Russian => &[Plural::One, Plural::Few, Plural::Many],
        }
    }

    /// The form of the count `n` (a negative count takes the form of its size).
    pub const fn category(self, n: i64) -> Plural {
        let n = n.unsigned_abs();
        let (n10, n100) = (n % 10, n % 100);
        let few = matches!(n10, 2..=4) && !matches!(n100, 12..=14);
        match self {
            PluralRule::Other => Plural::Other,
            PluralRule::One if n == 1 => Plural::One,
            PluralRule::ZeroOne if n <= 1 => Plural::One,
            PluralRule::One | PluralRule::ZeroOne => Plural::Other,
            PluralRule::Czech => match n {
                1 => Plural::One,
                2..=4 => Plural::Few,
                _ => Plural::Other,
            },
            PluralRule::Polish if n == 1 => Plural::One,
            PluralRule::Polish if few => Plural::Few,
            PluralRule::Polish => Plural::Many,
            PluralRule::Russian | PluralRule::Croatian if n10 == 1 && n100 != 11 => Plural::One,
            PluralRule::Russian | PluralRule::Croatian if few => Plural::Few,
            PluralRule::Russian => Plural::Many,
            PluralRule::Croatian => Plural::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forms_of(rule: PluralRule, counts: &[i64]) -> Vec<Plural> {
        counts.iter().map(|n| rule.category(*n)).collect()
    }

    #[test]
    fn each_rule_picks_exactly_its_forms() {
        for rule in PluralRule::ALL {
            let mut seen: Vec<Plural> = (0..=10_000).map(|n| rule.category(n)).collect();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen, rule.forms(), "{}", rule.name());
            assert_eq!(PluralRule::from_name(rule.name()), Some(rule));
        }
        assert_eq!(PluralRule::from_name("arabic"), None);
    }

    #[test]
    fn rules_follow_cldr_for_whole_numbers() {
        use Plural::{Few, Many, One, Other};
        let counts = [0, 1, 2, 5, 11, 12, 21, 22, 25, 101, 111, 112, 122];
        assert_eq!(
            forms_of(PluralRule::One, &counts),
            [
                Other, One, Other, Other, Other, Other, Other, Other, Other, Other, Other, Other,
                Other
            ]
        );
        assert_eq!(
            forms_of(PluralRule::ZeroOne, &counts[..3]),
            [One, One, Other]
        );
        assert_eq!(
            forms_of(PluralRule::Czech, &counts),
            [
                Other, One, Few, Other, Other, Other, Other, Other, Other, Other, Other, Other,
                Other
            ]
        );
        assert_eq!(
            forms_of(PluralRule::Polish, &counts),
            [
                Many, One, Few, Many, Many, Many, Many, Few, Many, Many, Many, Many, Few
            ]
        );
        assert_eq!(
            forms_of(PluralRule::Russian, &counts),
            [
                Many, One, Few, Many, Many, Many, One, Few, Many, One, Many, Many, Few
            ]
        );
        assert_eq!(
            forms_of(PluralRule::Croatian, &counts),
            [
                Other, One, Few, Other, Other, Other, One, Few, Other, One, Other, Other, Few
            ]
        );
        assert_eq!(PluralRule::Other.category(1), Other);
        assert_eq!(PluralRule::One.category(-1), One);
    }
}
