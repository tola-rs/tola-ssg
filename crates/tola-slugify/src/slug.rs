//! Turning text into a slug.

use std::borrow::Cow;

use unicode_normalization::{UnicodeNormalization, char::is_combining_mark, is_nfc, is_nfkc};

use crate::romanization;

/// The characters a name may hold: every script's own, or Latin script only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlugMode {
    /// Keep every script's letters, marks, and numbers.
    Unicode,
    /// Read the text in Latin script, naming each word and character it can.
    Ascii,
}

/// Which language's pronunciations name the Han text in a name.
///
/// Chinese names a word, then its characters; Japanese names a word in kana, which the
/// romanization rules read as Latin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HanPronunciations {
    /// A word from CC-CEDICT, otherwise the character's Unihan pronunciation.
    #[default]
    Chinese,
    /// A word from JMdict, whose pronunciation is kana.
    Japanese,
}

impl HanPronunciations {
    /// The pronunciations a site that declares `language` expects.
    ///
    /// The subtag before the first `-` names the language, and case does not matter there:
    /// `ja`, `JA`, and `ja-JP` read Han in Japanese, and every other tag, and no tag at all,
    /// reads it in Chinese.
    pub fn for_language(language: &str) -> Self {
        let (primary, _) = language.split_once('-').unwrap_or((language, ""));
        if primary.eq_ignore_ascii_case("ja") {
            Self::Japanese
        } else {
            Self::Chinese
        }
    }
}

/// How a name is written: which characters may survive, how it is cased, what separates
/// its words, and which language's pronunciations name its Han text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NamingRules {
    /// The characters a name may hold.
    pub mode: SlugMode,
    /// The case a name is written in.
    pub case: SlugCase,
    /// The separator between a name's words.
    pub separator: SlugSeparator,
    /// The language whose pronunciations name Han text in ASCII mode.
    ///
    /// Without `han-tables`, this choice is unused and Han text takes the character map.
    pub pronunciations: HanPronunciations,
}

impl Default for NamingRules {
    /// The rules a source's own layout recommends: a Unicode name, lowercase, dashed, with
    /// the Han text it holds read in Chinese where this build carries the tables.
    fn default() -> Self {
        Self {
            mode: SlugMode::Unicode,
            case: SlugCase::Lower,
            separator: SlugSeparator::Dash,
            pronunciations: HanPronunciations::default(),
        }
    }
}

/// The case a name is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlugCase {
    /// Lowercase every character.
    Lower,
    /// Uppercase every character.
    Upper,
    /// Uppercase the first character of each word.
    Capitalize,
    /// Keep the case the text was written in.
    Preserve,
}

/// The character between a name's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlugSeparator {
    /// `-`
    Dash,
    /// `_`
    Underscore,
}

impl SlugSeparator {
    const fn as_char(self) -> char {
        match self {
            Self::Dash => '-',
            Self::Underscore => '_',
        }
    }
}

/// Transform text without interpreting any character as a directory boundary.
///
/// Canonically equivalent input spellings meet the character policy in NFC form.
/// Rejected characters separate words before any pronunciation lookup or compatibility
/// folding, so neither transformation can erase a boundary or admit a rejected symbol.
///
/// An ASCII name takes the pronunciations the rules' language names for each allowed
/// span, then compatibility-folds and romanizes it. A character no table names falls to
/// the character map, and a script that spells its own pronunciation is romanized by a
/// rule (`しんぶん` is `shinbun`). The result is NFC normalized after case conversion.
/// Returns `None` when no retained character can name the segment on its own after
/// filtering and transliteration, before case conversion.
pub fn slugify_segment(text: &str, rules: NamingRules) -> Option<String> {
    let canonical = canonical_form(text);
    let segment = Segment::of(&canonical, rules);
    if !segment.renders_alone {
        return None;
    }
    Some(nfc_normalized(apply_case(segment, rules.case)))
}

fn canonical_form(text: &str) -> Cow<'_, str> {
    if text.is_ascii() || is_nfc(text) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.nfc().collect())
}

fn compatibility_form(text: &str) -> Cow<'_, str> {
    if text.is_ascii() || is_nfkc(text) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.nfkc().collect())
}

/// Lowercasing the whole string keeps contextual mappings such as Greek final
/// sigma, so it runs once over the result rather than per character. The scan
/// records whether anything can change, which lets an already-lowercase segment
/// through untouched.
fn apply_case(mut segment: Segment, case: SlugCase) -> String {
    match case {
        SlugCase::Lower if segment.needs_lowercase && segment.text.is_ascii() => {
            segment.text.make_ascii_lowercase();
            segment.text
        }
        SlugCase::Lower if segment.needs_lowercase => segment.text.to_lowercase(),
        SlugCase::Lower => segment.text,
        SlugCase::Upper if segment.text.is_ascii() => {
            segment.text.make_ascii_uppercase();
            segment.text
        }
        SlugCase::Upper => segment.text.to_uppercase(),
        SlugCase::Capitalize => capitalize_words(segment.text),
        SlugCase::Preserve => segment.text,
    }
}

/// Uppercasing can decompose a composed Greek character, so source normalization
/// does not replace this final check.
fn nfc_normalized(text: String) -> String {
    if text.is_ascii() || is_nfc(&text) {
        return text;
    }
    text.nfc().collect()
}

/// One name segment under construction, and the evidence the caller needs to
/// finish it.
struct Segment {
    text: String,
    pending_separator: bool,
    needs_lowercase: bool,
    renders_alone: bool,
    sep: char,
}

impl Segment {
    /// Every rejected character becomes a separator, which collapses and trims.
    fn of(text: &str, rules: NamingRules) -> Self {
        let mut segment = Self {
            text: String::with_capacity(text.len()),
            pending_separator: false,
            needs_lowercase: false,
            renders_alone: false,
            sep: rules.separator.as_char(),
        };
        if rules.mode == SlugMode::Unicode || text.is_ascii() {
            segment.push_all(text);
            return segment;
        }

        let mut romanized = String::new();
        for span in text.split(|c| is_boundary(c, rules.separator.as_char())) {
            if span.is_ascii() {
                segment.push_all(span);
            } else {
                // Dictionary keys retain their original compatibility spelling.
                #[cfg(feature = "han-tables")]
                let pronounced = match rules.pronunciations {
                    HanPronunciations::Chinese => tola_pronunciations::chinese_pronunciations(span),
                    HanPronunciations::Japanese => {
                        tola_pronunciations::japanese_pronunciations(span)
                    }
                };
                #[cfg(not(feature = "han-tables"))]
                let pronounced = Cow::Borrowed(span);
                let compatible = compatibility_form(&pronounced);
                segment.push_transliterated(&compatible, &mut romanized);
            }
            segment.boundary();
        }
        segment
    }

    fn push_transliterated(&mut self, text: &str, romanized: &mut String) {
        let mut rest = text;
        while let Some(character) = rest.chars().next() {
            let length = character.len_utf8();
            // Pronunciations and compatibility expansions can introduce fresh boundaries.
            if character.is_ascii() || !is_name_character(character) {
                self.push(character);
                rest = &rest[length..];
                continue;
            }
            romanized.clear();
            if let Some(consumed) = romanization::romanize(rest, romanized) {
                // A mark without a pronunciation must keep its neighbours apart.
                if romanized.is_empty() {
                    self.boundary();
                } else {
                    self.push_all(romanized);
                }
                rest = &rest[consumed..];
                continue;
            }
            match deunicode::deunicode_char(character) {
                Some(translated) => self.push_all(translated),
                None => self.boundary(),
            }
            rest = &rest[length..];
        }
    }

    /// Record that a separator belongs here, without emitting it until the next
    /// kept character proves it is interior rather than leading or trailing.
    fn boundary(&mut self) {
        self.pending_separator = !self.text.is_empty();
    }

    fn push(&mut self, c: char) {
        if is_boundary(c, self.sep) {
            self.boundary();
            return;
        }
        self.needs_lowercase |= needs_lowercasing(c);
        self.renders_alone |= !is_mark(c);
        if self.pending_separator {
            self.text.push(self.sep);
            self.pending_separator = false;
        }
        self.text.push(c);
    }

    fn push_all(&mut self, text: &str) {
        for c in text.chars() {
            self.push(c);
        }
    }
}

/// Whether a name segment keeps a character.
///
/// A name character is one a reader can see, distinguish, and store: a letter, a
/// number, a combining mark, or one of the punctuation characters listed here.
/// That punctuation is the URL-safe, filename-safe part of RFC 3986's path segment
/// characters — unreserved, the sub-delimiters, and `@`. Grouping and quoting
/// punctuation stays out, because half a pair is not a name, and `*` stays out
/// because no filename can hold it. Everything else becomes a word boundary.
///
/// A closed forbidden-character list can only exclude hazards someone already
/// thought of: it would admit unassigned and private-use code points, and every
/// character whose rendering is indistinguishable from the separator's, such as
/// U+2013 EN DASH.
#[inline]
fn is_name_character(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || is_name_punctuation(c);
    }
    !is_invisible_name_character(c) && (c.is_alphanumeric() || is_mark(c))
}

/// Whether a character is a combining mark, which draws only together with what
/// precedes it.
#[inline]
fn is_mark(c: char) -> bool {
    !c.is_ascii() && is_combining_mark(c)
}

#[inline]
fn is_name_punctuation(c: char) -> bool {
    matches!(
        c,
        '-' | '.' | '_' | '~' | '!' | '$' | '&' | '\'' | '+' | ',' | ';' | '=' | '@'
    )
}

/// The invisible characters a name would otherwise admit.
///
/// These are the `Default_Ignorable_Code_Point` characters whose general category
/// is a letter or a mark, which [`is_name_character`] would otherwise keep. Every
/// other default-ignorable character is already excluded: it is a control, a
/// format character, or an unassigned code point. A variation selector or a Hangul
/// filler occupies a position without drawing anything.
fn is_invisible_name_character(c: char) -> bool {
    matches!(
        c as u32,
        0x034F
            | 0x115F..=0x1160
            | 0x17B4..=0x17B5
            | 0x180B..=0x180D
            | 0x180F
            | 0x3164
            | 0xFE00..=0xFE0F
            | 0xFFA0
            | 0xE0100..=0xE01EF
    )
}

#[inline]
fn is_boundary(c: char, sep: char) -> bool {
    c == sep || !is_name_character(c)
}

/// Whether lowercasing a character could change it.
///
/// Any non-ASCII character reports `true`. A few cased letters, such as the
/// titlecase forms, are not `is_uppercase`, and testing each character against the
/// case tables costs more than the pass this flag exists to skip. Over-reporting
/// only runs a `to_lowercase` that is correct regardless; under-reporting would
/// emit un-lowercased text. Plain ASCII settles with a range comparison, which is
/// what source-derived segments are made of.
#[inline]
fn needs_lowercasing(c: char) -> bool {
    !c.is_ascii() || c.is_ascii_uppercase()
}

/// Uppercase the first character after `-` or `_` and lowercase the others individually.
///
/// A name segment keeps its words apart with those two characters alone — every other
/// boundary between words became the separator — so this word rule is not linguistic
/// title casing.
fn capitalize_words(text: String) -> String {
    if text.is_ascii() {
        let mut bytes = text.into_bytes();
        let mut at_word_start = true;
        for byte in &mut bytes {
            if *byte == b'-' || *byte == b'_' {
                at_word_start = true;
            } else {
                *byte = if at_word_start {
                    byte.to_ascii_uppercase()
                } else {
                    byte.to_ascii_lowercase()
                };
                at_word_start = false;
            }
        }
        return String::from_utf8(bytes).expect("ASCII casing preserves UTF-8");
    }
    let mut result = String::with_capacity(text.len());
    let mut at_word_start = true;

    for c in text.chars() {
        if c == '-' || c == '_' {
            result.push(c);
            at_word_start = true;
        } else if at_word_start {
            result.extend(c.to_uppercase());
            at_word_start = false;
        } else {
            result.extend(c.to_lowercase());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn naming_rules() -> impl Iterator<Item = NamingRules> {
        [SlugMode::Unicode, SlugMode::Ascii]
            .into_iter()
            .flat_map(|mode| {
                [
                    SlugCase::Lower,
                    SlugCase::Upper,
                    SlugCase::Capitalize,
                    SlugCase::Preserve,
                ]
                .into_iter()
                .flat_map(move |case| {
                    [SlugSeparator::Dash, SlugSeparator::Underscore]
                        .into_iter()
                        .flat_map(move |separator| {
                            [HanPronunciations::Chinese, HanPronunciations::Japanese]
                                .into_iter()
                                .map(move |pronunciations| NamingRules {
                                    mode,
                                    case,
                                    separator,
                                    pronunciations,
                                })
                        })
                })
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            failure_persistence: Some(Box::new(
                proptest::test_runner::FileFailurePersistence::Direct(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/target/proptest/slug.txt",
                )),
            )),
            ..ProptestConfig::default()
        })]

        #[test]
        fn canonical_spellings_agree(
            left in proptest::collection::vec(any::<char>(), 0..9),
            witness in proptest::sample::select(vec!["é", "ñ", "Å", "ΐ", "≠", "ᾳ"]),
            right in proptest::collection::vec(any::<char>(), 0..9),
        ) {
            let mut source: String = left.into_iter().collect();
            source.push_str(witness);
            source.extend(right);
            let nfd: String = source.nfd().collect();
            let nfc: String = source.nfc().collect();
            for rules in naming_rules() {
                let name = slugify_segment(&source, rules);
                prop_assert_eq!(&name, &slugify_segment(&nfd, rules));
                prop_assert_eq!(&name, &slugify_segment(&nfc, rules));
            }
        }

        #[test]
        fn ascii_names_stay_ascii(
            left in "[ -~]{0,8}",
            scalars in proptest::collection::vec(any::<char>(), 0..5),
            right in "[ -~]{0,8}",
        ) {
            let scalar_span: String = scalars.into_iter().collect();
            for witness in ["é", "Æ", "う", "한", "東京", "食べる", "うｐ主", "重庆", "3D打印机", "\u{323af}"] {
                let source = format!("a{left} {witness} {scalar_span} {right}z");
                for rules in naming_rules().filter(|rules| rules.mode == SlugMode::Ascii) {
                    let name = slugify_segment(&source, rules).expect("separated ASCII anchors name text");
                    prop_assert!(name.is_ascii(), "{:?} {:?}: {:?}", source, rules, name);
                }
            }
        }
    }

    fn rules(mode: SlugMode, case: SlugCase, separator: SlugSeparator) -> NamingRules {
        NamingRules {
            mode,
            case,
            separator,
            pronunciations: HanPronunciations::default(),
        }
    }

    /// Slug one segment under those rules.
    fn segment_slug(
        text: &str,
        mode: SlugMode,
        case: SlugCase,
        separator: SlugSeparator,
    ) -> Option<String> {
        slugify_segment(text, rules(mode, case, separator))
    }

    /// Slug one segment as a lowercase ASCII name with dashes.
    fn ascii_slug(text: &str) -> Option<String> {
        segment_slug(text, SlugMode::Ascii, SlugCase::Lower, SlugSeparator::Dash)
    }

    /// Slug one segment as a lowercase ASCII name whose Han text is read in Japanese.
    #[cfg(feature = "han-tables")]
    fn japanese_slug(text: &str) -> Option<String> {
        slugify_segment(
            text,
            NamingRules {
                mode: SlugMode::Ascii,
                pronunciations: HanPronunciations::Japanese,
                ..Default::default()
            },
        )
    }

    #[test]
    fn separators_never_split_segments() {
        assert_eq!(
            segment_slug(
                "posts/½/／Child＼Name\\Leaf",
                SlugMode::Ascii,
                SlugCase::Lower,
                SlugSeparator::Dash,
            )
            .as_deref(),
            Some("posts-1-2-child-name-leaf")
        );
        assert_eq!(
            segment_slug(
                "posts/Child\\Leaf",
                SlugMode::Unicode,
                SlugCase::Lower,
                SlugSeparator::Underscore,
            )
            .as_deref(),
            Some("posts_child_leaf")
        );
    }

    #[test]
    fn text_that_names_nothing_yields_no_name() {
        for mode in [SlugMode::Unicode, SlugMode::Ascii] {
            for text in [
                "",
                "#",
                "---",
                " /\\ \u{80}",
                "\u{200b}\u{feff}",
                "\u{ad}",
                // A mark draws only together with what precedes it.
                "\u{301}",
                "\u{301}\u{308}",
                "\u{fe0f}",
                // Unassigned, private use, and the last code point.
                "\u{378}",
                "\u{e000}",
                "\u{f0000}",
                "\u{10ffff}",
            ] {
                assert_eq!(
                    segment_slug(text, mode, SlugCase::Lower, SlugSeparator::Dash),
                    None,
                    "{text:?} {mode:?}"
                );
            }
        }
    }

    /// A character no name may keep separates the words around it, however often it repeats.
    #[test]
    fn rejected_characters_separate_words() {
        let cases = [
            0x2100,  // account of
            0x2260,  // not equal
            0x2603,  // snowman
            0x1f984, // unicorn
            0x200b,  // zero width space
            0x002f, 0x005c, // slash and backslash
            0x2122, // trademark
            0x0000, // NUL
            0x0022, // double quote
            0x001f, 0x007f, 0x009f, // C0 and C1 controls
            0x00ad, 0x200d, 0x200e, 0x202e, 0x2060, 0x2066, 0xfeff, // format characters
            0x180f, // a free variation selector
            0xe0001, 0xe007f, 0xfff9, // tags and interlinear annotation
            0x2010, 0x2013, 0x2212, // hyphen, en dash, minus sign
            0x00a9, 0x20ac, 0x1f308, // copyright, euro, rainbow
            0x0378,  // unassigned
            0xe000, 0xf0000, 0x10ffff, // private use and the last code point
        ];
        for code in cases {
            let rejected = char::from_u32(code).expect("case is a code point");
            for repeats in 1..=5 {
                let span = rejected.to_string().repeat(repeats);
                let source = format!("{span}left{span}right{span}");
                for mode in [SlugMode::Unicode, SlugMode::Ascii] {
                    for (separator, separator_char) in
                        [(SlugSeparator::Dash, '-'), (SlugSeparator::Underscore, '_')]
                    {
                        assert_eq!(
                            segment_slug(&source, mode, SlugCase::Lower, separator),
                            Some(format!("left{separator_char}right")),
                            "U+{code:04X} x{repeats} {mode:?} {separator:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn separator_look_alikes_share_one_name() {
        for text in [
            "release-notes",
            "release\u{2010}notes",
            "release\u{2013}notes",
            "release\u{2212}notes",
        ] {
            assert_eq!(
                segment_slug(
                    text,
                    SlugMode::Unicode,
                    SlugCase::Lower,
                    SlugSeparator::Dash
                )
                .as_deref(),
                Some("release-notes"),
                "{text:?}"
            );
        }
    }

    #[test]
    fn canonical_spellings_keep_expected_names() {
        for (composed, decomposed, case, unicode, ascii) in [
            (
                "caf\u{e9}",
                "cafe\u{301}",
                SlugCase::Lower,
                Some("café"),
                Some("cafe"),
            ),
            ("≠", "=\u{338}", SlugCase::Lower, None, None),
            (
                "a≠b",
                "a=\u{338}b",
                SlugCase::Lower,
                Some("a-b"),
                Some("a-b"),
            ),
            (
                "\u{1fb3}",
                "α\u{345}",
                SlugCase::Capitalize,
                Some("ΑΙ"),
                Some("A"),
            ),
        ] {
            for (mode, expected) in [(SlugMode::Unicode, unicode), (SlugMode::Ascii, ascii)] {
                for spelling in [composed, decomposed] {
                    assert_eq!(
                        segment_slug(spelling, mode, case, SlugSeparator::Dash).as_deref(),
                        expected,
                        "{spelling:?} {mode:?} {case:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn name_is_nfc_after_case_conversion() {
        assert_eq!(
            segment_slug(
                "cafe\u{301}",
                SlugMode::Unicode,
                SlugCase::Preserve,
                SlugSeparator::Dash,
            )
            .as_deref(),
            Some("café")
        );
        assert_eq!(
            segment_slug(
                "\u{390}",
                SlugMode::Unicode,
                SlugCase::Upper,
                SlugSeparator::Dash,
            )
            .as_deref(),
            Some("Ϊ\u{301}")
        );
        assert_eq!(
            segment_slug(
                "e\u{301}",
                SlugMode::Unicode,
                SlugCase::Lower,
                SlugSeparator::Dash,
            )
            .as_deref(),
            Some("é")
        );
    }

    #[test]
    fn titlecase_letters_lowercase_fully() {
        for (input, expected) in [("ǅ", "ǆ"), ("ᾼ", "ᾳ"), ("ΟΔΟΣ", "οδος")] {
            assert_eq!(
                segment_slug(
                    input,
                    SlugMode::Unicode,
                    SlugCase::Lower,
                    SlugSeparator::Dash,
                )
                .as_deref(),
                Some(expected),
                "{input:?}"
            );
        }
    }

    #[test]
    fn naming_rules_write_their_name() {
        for (mode, case, separator, input, expected) in [
            (
                SlugMode::Unicode,
                SlugCase::Lower,
                SlugSeparator::Dash,
                "北京 ΟΣ Café",
                "北京-ος-café",
            ),
            (
                SlugMode::Ascii,
                SlugCase::Lower,
                SlugSeparator::Dash,
                "北京 Café",
                "bei-jing-cafe",
            ),
            (
                SlugMode::Ascii,
                SlugCase::Lower,
                SlugSeparator::Dash,
                "重庆",
                "chong-qing",
            ),
            (
                // A script no table names keeps its own letters: the vowels it does not write
                // stay unwritten.
                SlugMode::Ascii,
                SlugCase::Lower,
                SlugSeparator::Dash,
                "مرحبا",
                "mrhb",
            ),
            (
                SlugMode::Unicode,
                SlugCase::Upper,
                SlugSeparator::Dash,
                "straße",
                "STRASSE",
            ),
            (
                SlugMode::Unicode,
                SlugCase::Capitalize,
                SlugSeparator::Underscore,
                "hELLO-world_cAFÉ (tEST)",
                "Hello-World_Café_Test",
            ),
            (
                SlugMode::Ascii,
                SlugCase::Capitalize,
                SlugSeparator::Dash,
                "+hELLO_1wORLD-xY",
                "+hello_1world-Xy",
            ),
            (
                SlugMode::Unicode,
                SlugCase::Preserve,
                SlugSeparator::Underscore,
                "__My:::  Café__",
                "My_Café",
            ),
            (
                SlugMode::Unicode,
                SlugCase::Lower,
                SlugSeparator::Dash,
                "C++ Guide R&D! v1.2 @Home",
                "c++-guide-r&d!-v1.2-@home",
            ),
            (
                SlugMode::Ascii,
                SlugCase::Lower,
                SlugSeparator::Dash,
                "C++ Guide R&D! v1.2 @Home",
                "c++-guide-r&d!-v1.2-@home",
            ),
        ] {
            assert_eq!(
                segment_slug(input, mode, case, separator).as_deref(),
                Some(expected),
                "{input:?} {mode:?} {case:?} {separator:?}"
            );
        }
    }

    #[cfg(feature = "han-tables")]
    #[test]
    fn han_reads_in_chinese() {
        assert_eq!(ascii_slug("中文").as_deref(), Some("zhong-wen"));
        assert_eq!(ascii_slug("中文abc").as_deref(), Some("zhong-wen-abc"));
        assert_eq!(ascii_slug("重庆").as_deref(), Some("chong-qing"));
        assert_eq!(ascii_slug("银行").as_deref(), Some("yin-hang"));
        assert_eq!(ascii_slug("音乐").as_deref(), Some("yin-yue"));
    }

    #[test]
    fn kana_reads_as_hepburn() {
        assert_eq!(ascii_slug("しんぶん").as_deref(), Some("shinbun"));
        assert_eq!(ascii_slug("まっちゃ").as_deref(), Some("matcha"));
        assert_eq!(ascii_slug("コンピュータ").as_deref(), Some("konpyuuta"));
        assert_eq!(ascii_slug("コーヒー 屋").as_deref(), Some("koohii-wu"));
        assert_eq!(ascii_slug("いすゞ").as_deref(), Some("isuzu"));
        assert_eq!(ascii_slug("すゞめ").as_deref(), Some("suzume"));
    }

    #[test]
    fn mark_without_pronunciation_separates() {
        assert_eq!(ascii_slug("aーb").as_deref(), Some("a-b"));
        assert_eq!(ascii_slug("aっb").as_deref(), Some("a-b"));
    }

    #[test]
    fn hangul_reads_as_revised_romanization() {
        assert_eq!(ascii_slug("한국어").as_deref(), Some("hangugeo"));
        assert_eq!(ascii_slug("서울 大学").as_deref(), Some("seoul-da-xue"));
    }

    #[cfg(feature = "han-tables")]
    #[test]
    fn han_reads_in_japanese() {
        assert_eq!(japanese_slug("東京").as_deref(), Some("toukyou"));
        assert_eq!(japanese_slug("新聞").as_deref(), Some("shinbun"));
        assert_eq!(japanese_slug("食べる").as_deref(), Some("taberu"));
        // The same text under Chinese pronunciations reads the words it knows in Chinese.
        assert_eq!(ascii_slug("東京").as_deref(), Some("dong-jing"));
    }

    #[cfg(feature = "han-tables")]
    #[test]
    fn japanese_words_keep_boundaries() {
        assert_eq!(japanese_slug("政府・与党").as_deref(), Some("seifu-yotou"));
    }

    #[cfg(feature = "han-tables")]
    #[test]
    fn japanese_compatibility_keys_resolve() {
        assert_eq!(japanese_slug("うｐ主").as_deref(), Some("upunushi"));
    }

    #[test]
    fn language_tag_picks_han_pronunciations() {
        for (language, expected) in [
            ("ja", HanPronunciations::Japanese),
            ("JA", HanPronunciations::Japanese),
            ("ja-JP", HanPronunciations::Japanese),
            ("zh", HanPronunciations::Chinese),
            ("zh-Hant-TW", HanPronunciations::Chinese),
            ("en", HanPronunciations::Chinese),
            ("japanese", HanPronunciations::Chinese),
            ("", HanPronunciations::Chinese),
        ] {
            assert_eq!(
                HanPronunciations::for_language(language),
                expected,
                "{language:?}"
            );
        }
    }
}
