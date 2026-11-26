//! Text-to-slug transformation.

use std::borrow::Cow;

use unicode_normalization::{UnicodeNormalization, char::is_combining_mark, is_nfc, is_nfkc};

use crate::romanization;

/// Character set allowed in a generated slug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlugMode {
    /// Retain letters, combining marks, and numbers from any script.
    Unicode,
    /// Transliterate non-ASCII spans to Latin script.
    Ascii,
}

/// Language used to pronounce Han characters in ASCII mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HanPronunciations {
    /// Pronounce words via CC-CEDICT and single characters via Unihan.
    #[default]
    Chinese,
    /// Pronounce words via JMdict kana followed by Hepburn romanization.
    Japanese,
}

impl HanPronunciations {
    /// Select the Han pronunciation mode for a BCP-47 language tag.
    pub fn for_language(language: &str) -> Self {
        let (primary, _) = language.split_once('-').unwrap_or((language, ""));
        if primary.eq_ignore_ascii_case("ja") {
            Self::Japanese
        } else {
            Self::Chinese
        }
    }
}

/// Configuration controlling character filtering, casing, separator, and Han pronunciation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NamingRules {
    /// Allowed character script mode.
    pub mode: SlugMode,
    /// Case transformation applied to the slug.
    pub case: SlugCase,
    /// Separator inserted between words.
    pub separator: SlugSeparator,
    /// Language used to pronounce Han text in ASCII mode.
    pub pronunciations: HanPronunciations,
}

impl Default for NamingRules {
    /// Default rules: Unicode mode, lowercase, dash-separated, Chinese Han pronunciations.
    fn default() -> Self {
        Self {
            mode: SlugMode::Unicode,
            case: SlugCase::Lower,
            separator: SlugSeparator::Dash,
            pronunciations: HanPronunciations::default(),
        }
    }
}

/// Case transformation applied to a slug segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlugCase {
    /// Lowercase every character.
    Lower,
    /// Uppercase every character.
    Upper,
    /// Uppercase the first character of each word and lowercase the rest.
    Capitalize,
    /// Keep the original character casing.
    Preserve,
}

/// Word separator character in a slug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlugSeparator {
    /// Hyphen-minus (`-`).
    Dash,
    /// Underscore (`_`).
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

/// Transform a single path segment into a slug without interpreting directory separators.
///
/// Input is NFC-normalized before filtering, rejected characters split words before
/// pronunciation lookup or NFKC folding so boundaries are never erased, and the final
/// cased output is re-normalized to NFC. Returns `None` if no standalone character remains.
pub fn slugify_segment(text: &str, rules: NamingRules) -> Option<String> {
    let canonical = canonical_form(text);
    let seg = Segment::of(&canonical, rules);
    if !seg.renders_alone {
        return None;
    }
    Some(nfc_normalized(apply_case(seg, rules.case)))
}

fn canonical_form(src: &str) -> Cow<'_, str> {
    if src.is_ascii() || is_nfc(src) {
        return Cow::Borrowed(src);
    }
    Cow::Owned(src.nfc().collect())
}

fn compatibility_form(src: &str) -> Cow<'_, str> {
    if src.is_ascii() || is_nfkc(src) {
        return Cow::Borrowed(src);
    }
    Cow::Owned(src.nfkc().collect())
}

/// Lowercase the whole segment at once to preserve contextual rules like Greek final sigma.
fn apply_case(mut seg: Segment, case: SlugCase) -> String {
    match case {
        SlugCase::Lower if seg.needs_lowercase && seg.out.is_ascii() => {
            seg.out.make_ascii_lowercase();
            seg.out
        }
        SlugCase::Lower if seg.needs_lowercase => seg.out.to_lowercase(),
        SlugCase::Lower => seg.out,
        SlugCase::Upper if seg.out.is_ascii() => {
            seg.out.make_ascii_uppercase();
            seg.out
        }
        SlugCase::Upper => seg.out.to_uppercase(),
        SlugCase::Capitalize => capitalize_words(seg.out),
        SlugCase::Preserve => seg.out,
    }
}

/// Re-normalize to NFC because uppercasing can decompose characters such as precomposed Greek.
fn nfc_normalized(src: String) -> String {
    if src.is_ascii() || is_nfc(&src) {
        return src;
    }
    src.nfc().collect()
}

struct Segment {
    out: String,
    pending_sep: bool,
    needs_lowercase: bool,
    renders_alone: bool,
    sep: char,
}

impl Segment {
    fn of(src: &str, rules: NamingRules) -> Self {
        let mut seg = Self {
            out: String::with_capacity(src.len()),
            pending_sep: false,
            needs_lowercase: false,
            renders_alone: false,
            sep: rules.separator.as_char(),
        };
        if rules.mode == SlugMode::Unicode || src.is_ascii() {
            seg.push_all(src);
            return seg;
        }

        let mut buf = String::new();
        for span in src.split(|ch| is_boundary(ch, rules.separator.as_char())) {
            if span.is_ascii() {
                seg.push_all(span);
            } else {
                // Dictionary keys retain their original compatibility spelling before NFKC.
                #[cfg(feature = "han-tables")]
                let pronounced = match rules.pronunciations {
                    HanPronunciations::Chinese => tola_pronunciations::chinese_pronunciations(span),
                    HanPronunciations::Japanese => {
                        tola_pronunciations::japanese_pronunciations(span)
                    }
                };
                #[cfg(not(feature = "han-tables"))]
                let pronounced = Cow::Borrowed(span);
                let compat = compatibility_form(&pronounced);
                seg.push_transliterated(&compat, &mut buf);
            }
            seg.boundary();
        }
        seg
    }

    fn push_transliterated(&mut self, src: &str, buf: &mut String) {
        let mut rest = src;
        while let Some(ch) = rest.chars().next() {
            let len = ch.len_utf8();
            if ch.is_ascii() || !is_name_character(ch) {
                self.push(ch);
                rest = &rest[len..];
                continue;
            }
            buf.clear();
            if let Some(consumed) = romanization::romanize(rest, buf) {
                // Unpronounced kana marks (leading っ/ー/ゞ) must still separate neighbours.
                if buf.is_empty() {
                    self.boundary();
                } else {
                    self.push_all(buf);
                }
                rest = &rest[consumed..];
                continue;
            }
            match deunicode::deunicode_char(ch) {
                Some(ascii) => self.push_all(ascii),
                None => self.boundary(),
            }
            rest = &rest[len..];
        }
    }

    fn boundary(&mut self) {
        self.pending_sep = !self.out.is_empty();
    }

    fn push(&mut self, ch: char) {
        if is_boundary(ch, self.sep) {
            self.boundary();
            return;
        }
        self.needs_lowercase |= needs_lowercasing(ch);
        self.renders_alone |= !is_mark(ch);
        if self.pending_sep {
            self.out.push(self.sep);
            self.pending_sep = false;
        }
        self.out.push(ch);
    }

    fn push_all(&mut self, src: &str) {
        for ch in src.chars() {
            self.push(ch);
        }
    }
}

/// Allowlist of letters, numbers, combining marks, and path-safe RFC 3986 punctuation
/// (excluding grouping/quoting chars, `*`, and lookalike dashes such as U+2013).
#[inline]
fn is_name_character(ch: char) -> bool {
    if ch.is_ascii() {
        return ch.is_ascii_alphanumeric() || is_name_punctuation(ch);
    }
    !is_invisible_name_character(ch) && (ch.is_alphanumeric() || is_mark(ch))
}

#[inline]
fn is_mark(ch: char) -> bool {
    !ch.is_ascii() && is_combining_mark(ch)
}

#[inline]
fn is_name_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '-' | '.' | '_' | '~' | '!' | '$' | '&' | '\'' | '+' | ',' | ';' | '=' | '@'
    )
}

/// `Default_Ignorable_Code_Point` letters/marks that carry no visible glyph (e.g. variation
/// selectors and Hangul fillers).
fn is_invisible_name_character(ch: char) -> bool {
    matches!(
        ch as u32,
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
fn is_boundary(ch: char, sep: char) -> bool {
    ch == sep || !is_name_character(ch)
}

/// Over-report non-ASCII chars so titlecase letters don't skip `to_lowercase`.
#[inline]
fn needs_lowercasing(ch: char) -> bool {
    !ch.is_ascii() || ch.is_ascii_uppercase()
}

fn capitalize_words(src: String) -> String {
    if src.is_ascii() {
        let mut bytes = src.into_bytes();
        let mut word_start = true;
        for b in &mut bytes {
            if *b == b'-' || *b == b'_' {
                word_start = true;
            } else {
                *b = if word_start {
                    b.to_ascii_uppercase()
                } else {
                    b.to_ascii_lowercase()
                };
                word_start = false;
            }
        }
        return String::from_utf8(bytes).expect("ASCII casing preserves UTF-8");
    }
    let mut out = String::with_capacity(src.len());
    let mut word_start = true;

    for ch in src.chars() {
        if ch == '-' || ch == '_' {
            out.push(ch);
            word_start = true;
        } else if word_start {
            out.extend(ch.to_uppercase());
            word_start = false;
        } else {
            out.extend(ch.to_lowercase());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn naming_rules() -> Vec<NamingRules> {
        let mut out = Vec::new();
        for mode in [SlugMode::Unicode, SlugMode::Ascii] {
            for case in [
                SlugCase::Lower,
                SlugCase::Upper,
                SlugCase::Capitalize,
                SlugCase::Preserve,
            ] {
                for separator in [SlugSeparator::Dash, SlugSeparator::Underscore] {
                    for pronunciations in [HanPronunciations::Chinese, HanPronunciations::Japanese]
                    {
                        out.push(NamingRules {
                            mode,
                            case,
                            separator,
                            pronunciations,
                        });
                    }
                }
            }
        }
        out
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
        fn canonical_spellings(
            left in proptest::collection::vec(any::<char>(), 0..9),
            witness in proptest::sample::select(vec!["é", "ñ", "Å", "ΐ", "≠", "ᾳ"]),
            right in proptest::collection::vec(any::<char>(), 0..9),
        ) {
            let mut src: String = left.into_iter().collect();
            src.push_str(witness);
            src.extend(right);
            let nfd: String = src.nfd().collect();
            let nfc: String = src.nfc().collect();
            for rules in naming_rules() {
                let name = slugify_segment(&src, rules);
                prop_assert_eq!(&name, &slugify_segment(&nfd, rules));
                prop_assert_eq!(&name, &slugify_segment(&nfc, rules));
            }
        }

        #[test]
        fn ascii_stays_ascii(
            left in "[ -~]{0,8}",
            scalars in proptest::collection::vec(any::<char>(), 0..5),
            right in "[ -~]{0,8}",
        ) {
            let span: String = scalars.into_iter().collect();
            for witness in ["é", "Æ", "う", "한", "東京", "食べる", "うｐ主", "重庆", "3D打印机", "\u{323af}"] {
                let src = format!("a{left} {witness} {span} {right}z");
                for rules in naming_rules().into_iter().filter(|r| r.mode == SlugMode::Ascii) {
                    let name = slugify_segment(&src, rules).expect("ASCII anchors produce a slug");
                    prop_assert!(name.is_ascii(), "{:?} {:?}: {:?}", src, rules, name);
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

    fn seg_slug(
        src: &str,
        mode: SlugMode,
        case: SlugCase,
        separator: SlugSeparator,
    ) -> Option<String> {
        slugify_segment(src, rules(mode, case, separator))
    }

    fn ascii_slug(src: &str) -> Option<String> {
        seg_slug(src, SlugMode::Ascii, SlugCase::Lower, SlugSeparator::Dash)
    }

    #[cfg(feature = "han-tables")]
    fn ja_slug(src: &str) -> Option<String> {
        slugify_segment(
            src,
            NamingRules {
                mode: SlugMode::Ascii,
                pronunciations: HanPronunciations::Japanese,
                ..Default::default()
            },
        )
    }

    #[test]
    fn separators_stay_segment_local() {
        assert_eq!(
            seg_slug(
                "posts/½/／Child＼Name\\Leaf",
                SlugMode::Ascii,
                SlugCase::Lower,
                SlugSeparator::Dash,
            )
            .as_deref(),
            Some("posts-1-2-child-name-leaf")
        );
        assert_eq!(
            seg_slug(
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
    fn slugless_yields_none() {
        for mode in [SlugMode::Unicode, SlugMode::Ascii] {
            for src in [
                "",
                "#",
                "---",
                " /\\ \u{80}",
                "\u{200b}\u{feff}",
                "\u{ad}",
                "\u{301}",
                "\u{301}\u{308}",
                "\u{fe0f}",
                "\u{378}",
                "\u{e000}",
                "\u{f0000}",
                "\u{10ffff}",
            ] {
                assert_eq!(
                    seg_slug(src, mode, SlugCase::Lower, SlugSeparator::Dash),
                    None,
                    "{src:?} {mode:?}"
                );
            }
        }
    }

    #[test]
    fn rejected_chars_split_words() {
        let cases = [
            0x2100, 0x2260, 0x2603, 0x1f984, 0x200b, 0x002f, 0x005c, 0x2122, 0x0000, 0x0022,
            0x001f, 0x007f, 0x009f, 0x00ad, 0x200d, 0x200e, 0x202e, 0x2060, 0x2066, 0xfeff, 0x180f,
            0xe0001, 0xe007f, 0xfff9, 0x2010, 0x2013, 0x2212, 0x00a9, 0x20ac, 0x1f308, 0x0378,
            0xe000, 0xf0000, 0x10ffff,
        ];
        for cp in cases {
            let ch = char::from_u32(cp).expect("valid scalar");
            for reps in 1..=5 {
                let span = ch.to_string().repeat(reps);
                let src = format!("{span}left{span}right{span}");
                for mode in [SlugMode::Unicode, SlugMode::Ascii] {
                    for (sep, sep_ch) in
                        [(SlugSeparator::Dash, '-'), (SlugSeparator::Underscore, '_')]
                    {
                        assert_eq!(
                            seg_slug(&src, mode, SlugCase::Lower, sep),
                            Some(format!("left{sep_ch}right")),
                            "U+{cp:04X} x{reps} {mode:?} {sep:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn dash_lookalikes_match() {
        for src in [
            "release-notes",
            "release\u{2010}notes",
            "release\u{2013}notes",
            "release\u{2212}notes",
        ] {
            assert_eq!(
                seg_slug(src, SlugMode::Unicode, SlugCase::Lower, SlugSeparator::Dash).as_deref(),
                Some("release-notes"),
                "{src:?}"
            );
        }
    }

    #[test]
    fn titlecase_lowercases() {
        for (src, expected) in [("ǅ", "ǆ"), ("ᾼ", "ᾳ"), ("ΟΔΟΣ", "οδος")] {
            assert_eq!(
                seg_slug(src, SlugMode::Unicode, SlugCase::Lower, SlugSeparator::Dash).as_deref(),
                Some(expected),
                "{src:?}"
            );
        }
    }

    #[test]
    fn mode_case_and_sep() {
        for (mode, case, sep, src, expected) in [
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
                seg_slug(src, mode, case, sep).as_deref(),
                Some(expected),
                "{src:?} {mode:?} {case:?} {sep:?}"
            );
        }
    }

    #[cfg(feature = "han-tables")]
    #[test]
    fn han_chinese() {
        assert_eq!(ascii_slug("中文").as_deref(), Some("zhong-wen"));
        assert_eq!(ascii_slug("中文abc").as_deref(), Some("zhong-wen-abc"));
        assert_eq!(ascii_slug("重庆").as_deref(), Some("chong-qing"));
        assert_eq!(ascii_slug("银行").as_deref(), Some("yin-hang"));
        assert_eq!(ascii_slug("音乐").as_deref(), Some("yin-yue"));
    }

    #[test]
    fn kana_hepburn() {
        assert_eq!(ascii_slug("しんぶん").as_deref(), Some("shinbun"));
        assert_eq!(ascii_slug("まっちゃ").as_deref(), Some("matcha"));
        assert_eq!(ascii_slug("コンピュータ").as_deref(), Some("konpyuuta"));
        assert_eq!(ascii_slug("コーヒー 屋").as_deref(), Some("koohii-wu"));
        assert_eq!(ascii_slug("いすゞ").as_deref(), Some("isuzu"));
        assert_eq!(ascii_slug("すゞめ").as_deref(), Some("suzume"));
    }

    #[test]
    fn bare_mark_splits() {
        assert_eq!(ascii_slug("aーb").as_deref(), Some("a-b"));
        assert_eq!(ascii_slug("aっb").as_deref(), Some("a-b"));
    }

    #[test]
    fn hangul_rr() {
        assert_eq!(ascii_slug("한국어").as_deref(), Some("hangugeo"));
        assert_eq!(ascii_slug("서울 大学").as_deref(), Some("seoul-da-xue"));
    }

    #[cfg(feature = "han-tables")]
    #[test]
    fn han_japanese() {
        assert_eq!(ja_slug("東京").as_deref(), Some("toukyou"));
        assert_eq!(ja_slug("新聞").as_deref(), Some("shinbun"));
        assert_eq!(ja_slug("食べる").as_deref(), Some("taberu"));
        assert_eq!(ascii_slug("東京").as_deref(), Some("dong-jing"));
    }

    #[cfg(feature = "han-tables")]
    #[test]
    fn japanese_word_boundaries() {
        assert_eq!(ja_slug("政府・与党").as_deref(), Some("seifu-yotou"));
    }

    #[cfg(feature = "han-tables")]
    #[test]
    fn japanese_compat_keys() {
        assert_eq!(ja_slug("うｐ主").as_deref(), Some("upunushi"));
    }

    #[test]
    fn han_by_lang_tag() {
        for (lang, expected) in [
            ("ja", HanPronunciations::Japanese),
            ("JA", HanPronunciations::Japanese),
            ("ja-JP", HanPronunciations::Japanese),
            ("zh", HanPronunciations::Chinese),
            ("zh-Hant-TW", HanPronunciations::Chinese),
            ("en", HanPronunciations::Chinese),
            ("japanese", HanPronunciations::Chinese),
            ("", HanPronunciations::Chinese),
        ] {
            assert_eq!(HanPronunciations::for_language(lang), expected, "{lang:?}");
        }
    }
}
