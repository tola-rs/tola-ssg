//! Kana read as Hepburn: the syllabary, the sokuon, the long-vowel mark, and the marks
//! that repeat a syllable.
//!
//! A syllable is one or two kana. The ASCII spellings are Hepburn-style: `し` is `shi`,
//! `ち` is `chi`, `つ` is `tsu`, and ん is `n`. Sokuon doubles the next consonant,
//! with traditional `tch` before `ch` (`まっちゃ` is `matcha`); ー repeats the
//! preceding vowel. This `tch` convention is not the 2025 Cabinet notification's `cch`.
//!
//! An iteration mark repeats the syllable before it, which is what the mark says: ゝ reads
//! as that syllable and ゞ reads it with its first kana voiced, so いすゞ is `isuzu` and
//! すゞめ is `suzume`. ヽ and ヾ are the same two marks in katakana.
//!
//! What a rule cannot know stays as it is: a particle is read as the kana it is written
//! with (`は` is `ha`), because a name has no sentence to place it in. Halfwidth
//! katakana is read after compatibility normalization, which the caller applies.

/// Append the romanization of the kana at the start of `text`, and report the bytes it read.
///
/// Answers `None` when `text` does not start with kana, and appends nothing in that case. A
/// mark with no syllable to hold back, lengthen, or repeat — the text starts with it — has
/// no pronunciation: the bytes it read are still reported, and the caller decides what that
/// means for a name.
pub(super) fn romanize(text: &str, out: &mut String) -> Option<usize> {
    let mut consumed = 0;
    let mut doubled = false;
    // The syllable before, which the marks around this one are read against.
    let mut last: Option<Syllable> = None;
    while let Some(character) = text[consumed..].chars().next() {
        let Some(kana) = to_hiragana(character) else {
            break;
        };
        match kana {
            // っ says nothing of its own: the next syllable's consonant doubles.
            'っ' => {
                consumed += character.len_utf8();
                doubled = true;
                continue;
            }
            // ー repeats the vowel of the syllable it follows.
            'ー' => {
                consumed += character.len_utf8();
                if let Some(vowel) = last.and_then(Syllable::last_vowel) {
                    out.push(vowel);
                }
                continue;
            }
            // ゝ repeats the syllable before it, which ゞ repeats with its first kana voiced.
            'ゝ' | 'ゞ' => {
                consumed += character.len_utf8();
                let repeated = last.map(|syllable| match kana {
                    'ゞ' => syllable.voiced(),
                    _ => syllable,
                });
                if let Some(repeated) = repeated {
                    out.push_str(repeated.romanization);
                    last = Some(repeated);
                }
                continue;
            }
            _ => {}
        }
        let Some((syllable, length)) = syllable_at(&text[consumed..]) else {
            break;
        };
        if syllable.first == 'ん' {
            // ん before a vowel or a y-syllable needs its own boundary: しんいち is
            // Shin'ichi, not Shinichi.
            match syllable_at(&text[consumed + length..]) {
                Some((next, _))
                    if starts_with_vowel(next.romanization) || starts_with_y(next.romanization) =>
                {
                    out.push_str("n'");
                }
                _ => out.push('n'),
            }
        } else if doubled {
            match syllable.romanization.chars().next() {
                Some('c') => out.push('t'),
                Some(consonant) if !is_vowel(consonant) => out.push(consonant),
                _ => {}
            }
            out.push_str(syllable.romanization);
        } else {
            out.push_str(syllable.romanization);
        }
        doubled = false;
        last = Some(syllable);
        consumed += length;
    }
    // Read nothing, answer nothing: the caller's own policy still owns that character.
    (consumed > 0).then_some(consumed)
}

/// One kana syllable: the romanization it carries, and the kana it is written with.
#[derive(Clone, Copy)]
struct Syllable {
    romanization: &'static str,
    /// The kana the syllable is spelled with, in hiragana; the second one is a small kana.
    first: char,
    second: Option<char>,
}

impl Syllable {
    /// The vowel the syllable ends with, which the long-vowel mark repeats.
    fn last_vowel(self) -> Option<char> {
        self.romanization
            .chars()
            .last()
            .filter(|character| is_vowel(*character))
    }

    /// The syllable as ゞ repeats it: the same syllable with its first kana voiced.
    ///
    /// A syllable whose first kana has no voiced form — あ, ん, ら — comes back as it is,
    /// because the mark says no more than the kana it repeats.
    fn voiced(self) -> Self {
        let Some(first) = voiced_kana(self.first) else {
            return self;
        };
        let romanization = match self.second {
            Some(second) => digraph(first, second),
            None => syllable(first),
        };
        match romanization {
            Some(romanization) => Self {
                romanization,
                first,
                second: self.second,
            },
            None => self,
        }
    }
}

/// The kana a dakuten voices one syllable with: す is written ず, は is written ば.
fn voiced_kana(kana: char) -> Option<char> {
    Some(match kana {
        'か' => 'が',
        'き' => 'ぎ',
        'く' => 'ぐ',
        'け' => 'げ',
        'こ' => 'ご',
        'さ' => 'ざ',
        'し' => 'じ',
        'す' => 'ず',
        'せ' => 'ぜ',
        'そ' => 'ぞ',
        'た' => 'だ',
        'ち' => 'ぢ',
        'つ' => 'づ',
        'て' => 'で',
        'と' => 'ど',
        'は' => 'ば',
        'ひ' => 'び',
        'ふ' => 'ぶ',
        'へ' => 'べ',
        'ほ' => 'ぼ',
        'う' => 'ゔ',
        _ => return None,
    })
}

/// The syllable the kana at the start of `text` spells, and how many bytes it is.
fn syllable_at(text: &str) -> Option<(Syllable, usize)> {
    let first = text.chars().next()?;
    let kana = to_hiragana(first)?;
    let pair = text[first.len_utf8()..]
        .chars()
        .next()
        .and_then(to_hiragana)
        .and_then(|second| digraph(kana, second).map(|romanization| (romanization, second)));
    if let Some((romanization, second)) = pair {
        return Some((
            Syllable {
                romanization,
                first: kana,
                second: Some(second),
            },
            first.len_utf8() + second.len_utf8(),
        ));
    }
    syllable(kana).map(|romanization| {
        (
            Syllable {
                romanization,
                first: kana,
                second: None,
            },
            first.len_utf8(),
        )
    })
}

/// The kana the caller writes, read as the hiragana this module's tables name.
///
/// Katakana is the same syllabary shifted by 0x60; ー and the iteration marks belong to both.
fn to_hiragana(character: char) -> Option<char> {
    match character {
        '\u{3041}'..='\u{3096}' | '\u{30fc}' => Some(character),
        '\u{30a1}'..='\u{30f6}' => char::from_u32(character as u32 - 0x60),
        '\u{309d}' | '\u{30fd}' => Some('\u{309d}'),
        '\u{309e}' | '\u{30fe}' => Some('\u{309e}'),
        _ => None,
    }
}

/// The romanization of one kana.
fn syllable(kana: char) -> Option<&'static str> {
    Some(match kana {
        'あ' | 'ぁ' => "a",
        'い' | 'ぃ' => "i",
        'う' | 'ぅ' => "u",
        'え' | 'ぇ' => "e",
        'お' | 'ぉ' => "o",
        'か' => "ka",
        'き' => "ki",
        'く' => "ku",
        'け' => "ke",
        'こ' => "ko",
        'が' => "ga",
        'ぎ' => "gi",
        'ぐ' => "gu",
        'げ' => "ge",
        'ご' => "go",
        'さ' => "sa",
        'し' => "shi",
        'す' => "su",
        'せ' => "se",
        'そ' => "so",
        'ざ' => "za",
        'じ' => "ji",
        'ず' => "zu",
        'ぜ' => "ze",
        'ぞ' => "zo",
        'た' => "ta",
        'ち' => "chi",
        'つ' => "tsu",
        'て' => "te",
        'と' => "to",
        'だ' => "da",
        'ぢ' => "ji",
        'づ' => "zu",
        'で' => "de",
        'ど' => "do",
        'な' => "na",
        'に' => "ni",
        'ぬ' => "nu",
        'ね' => "ne",
        'の' => "no",
        'は' => "ha",
        'ひ' => "hi",
        'ふ' => "fu",
        'へ' => "he",
        'ほ' => "ho",
        'ば' => "ba",
        'び' => "bi",
        'ぶ' => "bu",
        'べ' => "be",
        'ぼ' => "bo",
        'ぱ' => "pa",
        'ぴ' => "pi",
        'ぷ' => "pu",
        'ぺ' => "pe",
        'ぽ' => "po",
        'ま' => "ma",
        'み' => "mi",
        'む' => "mu",
        'め' => "me",
        'も' => "mo",
        'ゃ' | 'や' => "ya",
        'ゅ' | 'ゆ' => "yu",
        'ょ' | 'よ' => "yo",
        'ら' => "ra",
        'り' => "ri",
        'る' => "ru",
        'れ' => "re",
        'ろ' => "ro",
        'ゎ' | 'わ' => "wa",
        'ゐ' => "i",
        'ゑ' => "e",
        'を' => "o",
        'ん' => "n",
        'ゔ' => "vu",
        _ => return None,
    })
}

/// The romanization of two kana that spell one syllable.
fn digraph(first: char, second: char) -> Option<&'static str> {
    Some(match (first, second) {
        ('き', 'ゃ') => "kya",
        ('き', 'ゅ') => "kyu",
        ('き', 'ょ') => "kyo",
        ('ぎ', 'ゃ') => "gya",
        ('ぎ', 'ゅ') => "gyu",
        ('ぎ', 'ょ') => "gyo",
        ('し', 'ゃ') => "sha",
        ('し', 'ゅ') => "shu",
        ('し', 'ょ') => "sho",
        ('し', 'ぇ') => "she",
        ('じ', 'ゃ') => "ja",
        ('じ', 'ゅ') => "ju",
        ('じ', 'ょ') => "jo",
        ('じ', 'ぇ') => "je",
        ('ち', 'ゃ') => "cha",
        ('ち', 'ゅ') => "chu",
        ('ち', 'ょ') => "cho",
        ('ち', 'ぇ') => "che",
        ('ぢ', 'ゃ') => "ja",
        ('ぢ', 'ゅ') => "ju",
        ('ぢ', 'ょ') => "jo",
        ('に', 'ゃ') => "nya",
        ('に', 'ゅ') => "nyu",
        ('に', 'ょ') => "nyo",
        ('ひ', 'ゃ') => "hya",
        ('ひ', 'ゅ') => "hyu",
        ('ひ', 'ょ') => "hyo",
        ('び', 'ゃ') => "bya",
        ('び', 'ゅ') => "byu",
        ('び', 'ょ') => "byo",
        ('ぴ', 'ゃ') => "pya",
        ('ぴ', 'ゅ') => "pyu",
        ('ぴ', 'ょ') => "pyo",
        ('み', 'ゃ') => "mya",
        ('み', 'ゅ') => "myu",
        ('み', 'ょ') => "myo",
        ('り', 'ゃ') => "rya",
        ('り', 'ゅ') => "ryu",
        ('り', 'ょ') => "ryo",
        ('く', 'ぁ') => "kwa",
        ('く', 'ぃ') => "kwi",
        ('く', 'ぇ') => "kwe",
        ('く', 'ぉ') => "kwo",
        ('ぐ', 'ぁ') => "gwa",
        ('ぐ', 'ぃ') => "gwi",
        ('ぐ', 'ぇ') => "gwe",
        ('ぐ', 'ぉ') => "gwo",
        ('す', 'ぃ') => "si",
        ('す', 'ぇ') => "swe",
        ('ず', 'ぃ') => "zi",
        ('き', 'ぇ') => "kye",
        ('ぎ', 'ぇ') => "gye",
        ('に', 'ぇ') => "nye",
        ('ひ', 'ぇ') => "hye",
        ('び', 'ぇ') => "bye",
        ('ぴ', 'ぇ') => "pye",
        ('み', 'ぇ') => "mye",
        ('り', 'ぇ') => "rye",
        ('ふ', 'ぁ') => "fa",
        ('ふ', 'ぃ') => "fi",
        ('ふ', 'ぇ') => "fe",
        ('ふ', 'ぉ') => "fo",
        ('ふ', 'ゅ') => "fyu",
        ('ふ', 'ょ') => "fyo",
        ('ゔ', 'ぁ') => "va",
        ('ゔ', 'ぃ') => "vi",
        ('ゔ', 'ぇ') => "ve",
        ('ゔ', 'ぉ') => "vo",
        ('ゔ', 'ゅ') => "vyu",
        ('て', 'ぃ') => "ti",
        ('て', 'ゅ') => "tyu",
        ('で', 'ぃ') => "di",
        ('で', 'ゅ') => "dyu",
        ('と', 'ぅ') => "tu",
        ('ど', 'ぅ') => "du",
        ('う', 'ぁ') => "wa",
        ('う', 'ぃ') => "wi",
        ('う', 'ぇ') => "we",
        ('う', 'ぉ') => "wo",
        ('い', 'ぇ') => "ye",
        ('つ', 'ぁ') => "tsa",
        ('つ', 'ぃ') => "tsi",
        ('つ', 'ぇ') => "tse",
        ('つ', 'ぉ') => "tso",
        _ => return None,
    })
}

fn is_vowel(character: char) -> bool {
    matches!(character, 'a' | 'i' | 'u' | 'e' | 'o')
}

fn starts_with_vowel(romanization: &str) -> bool {
    romanization.chars().next().is_some_and(is_vowel)
}

fn starts_with_y(romanization: &str) -> bool {
    romanization.starts_with('y')
}

#[cfg(test)]
mod tests {
    use super::romanize;

    fn read(text: &str) -> String {
        let mut out = String::new();
        assert_eq!(
            romanize(text, &mut out),
            Some(text.len()),
            "{text:?} reads whole"
        );
        out
    }

    #[test]
    fn syllables_follow_hepburn() {
        for (text, expected) in [
            ("しんぶん", "shinbun"),
            ("ぎんざ", "ginza"),
            ("かんぱい", "kanpai"),
            ("ふじさん", "fujisan"),
            ("きゃく", "kyaku"),
            ("しゃしん", "shashin"),
            ("ちゅうごく", "chuugoku"),
            ("じんじゃ", "jinja"),
            ("にほんご", "nihongo"),
            ("を", "o"),
        ] {
            assert_eq!(read(text), expected, "{text:?}");
        }
    }

    #[test]
    fn katakana_reads_as_the_same_syllabary() {
        for (text, expected) in [
            ("コンピュータ", "konpyuuta"),
            ("コーヒー", "koohii"),
            ("ファン", "fan"),
            ("ヴァイオリン", "vaiorin"),
        ] {
            assert_eq!(read(text), expected, "{text:?}");
        }
    }

    #[test]
    fn loanword_combinations_keep_one_syllable() {
        for (text, expected) in [
            ("クォーツ", "kwootsu"),
            ("クァルテット", "kwarutetto"),
            ("グァテマラ", "gwatemara"),
            ("スィート", "siito"),
            ("ズィーク", "ziiku"),
            ("インタヴュー", "intavyuu"),
            ("キェ", "kye"),
            ("フョ", "fyo"),
            ("ウァ", "wa"),
        ] {
            assert_eq!(read(text), expected, "{text:?}");
        }
    }

    #[test]
    fn iteration_marks_repeat_syllable() {
        for (text, expected) in [
            ("いすゞ", "isuzu"),
            ("すゞめ", "suzume"),
            ("みすゞ", "misuzu"),
            ("たゞ", "tada"),
            ("バナヽ", "banana"),
            ("ハヽ", "haha"),
            // A mark with no syllable before it is read as nothing.
            ("ヾ", ""),
        ] {
            assert_eq!(read(text), expected, "{text:?}");
        }
    }

    #[test]
    fn doubled_consonants_close_the_syllable() {
        for (text, expected) in [
            ("まっちゃ", "matcha"),
            ("てんぷら", "tenpura"),
            ("がっこう", "gakkou"),
            ("にっぽん", "nippon"),
        ] {
            assert_eq!(read(text), expected, "{text:?}");
        }
    }

    #[test]
    fn nasal_syllable_keeps_its_boundary() {
        assert_eq!(read("しんいち"), "shin'ichi");
        assert_eq!(read("こんにちは"), "konnichiha");
    }

    #[test]
    fn non_kana_stops_the_transliteration() {
        let mut out = String::new();
        assert_eq!(romanize("カナ漢字", &mut out), Some("カナ".len()));
        assert_eq!(out, "kana");
    }
}
