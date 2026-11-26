//! Hepburn romanization of kana runs (syllabary, sokuon, chōonpu, iteration marks).
//!
//! Sokuon uses traditional `tch` before `ch` (not 2025 Cabinet `cch`). Particles stay literal
//! (`は` -> `ha`), and callers must apply NFKC before passing halfwidth katakana.

pub(super) fn romanize(src: &str, out: &mut String) -> Option<usize> {
    let mut off = 0;
    let mut doubled = false;
    let mut last: Option<Syllable> = None;
    while let Some(ch) = src[off..].chars().next() {
        let Some(kana) = to_hiragana(ch) else {
            break;
        };
        match kana {
            'っ' => {
                off += ch.len_utf8();
                doubled = true;
                continue;
            }
            'ー' => {
                off += ch.len_utf8();
                if let Some(v) = last.and_then(Syllable::last_vowel) {
                    out.push(v);
                }
                continue;
            }
            'ゝ' | 'ゞ' => {
                off += ch.len_utf8();
                let repeated = last.map(|syl| if kana == 'ゞ' { syl.voiced() } else { syl });
                if let Some(syl) = repeated {
                    out.push_str(syl.rom);
                    last = Some(syl);
                }
                continue;
            }
            _ => {}
        }
        let Some((syl, len)) = syllable_at(&src[off..]) else {
            break;
        };
        if syl.first == 'ん' {
            // Separate syllabic `ん` from following vowel or y-glide (`しんいち` -> `shin'ichi`).
            match syllable_at(&src[off + len..]) {
                Some((next, _)) if starts_with_vowel(next.rom) || next.rom.starts_with('y') => {
                    out.push_str("n'");
                }
                _ => out.push('n'),
            }
        } else if doubled {
            match syl.rom.chars().next() {
                Some('c') => out.push('t'),
                Some(c) if !is_vowel(c) => out.push(c),
                _ => {}
            }
            out.push_str(syl.rom);
        } else {
            out.push_str(syl.rom);
        }
        doubled = false;
        last = Some(syl);
        off += len;
    }
    (off > 0).then_some(off)
}

#[derive(Clone, Copy)]
struct Syllable {
    rom: &'static str,
    first: char,
    second: Option<char>,
}

impl Syllable {
    fn last_vowel(self) -> Option<char> {
        self.rom.chars().last().filter(|ch| is_vowel(*ch))
    }

    fn voiced(self) -> Self {
        let Some(first) = voiced_kana(self.first) else {
            return self;
        };
        let rom = match self.second {
            Some(second) => digraph(first, second),
            None => syllable(first),
        };
        match rom {
            Some(rom) => Self {
                rom,
                first,
                second: self.second,
            },
            None => self,
        }
    }
}

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

fn syllable_at(src: &str) -> Option<(Syllable, usize)> {
    let first = src.chars().next()?;
    let kana = to_hiragana(first)?;
    let pair = src[first.len_utf8()..]
        .chars()
        .next()
        .and_then(to_hiragana)
        .and_then(|second| digraph(kana, second).map(|rom| (rom, second)));
    if let Some((rom, second)) = pair {
        return Some((
            Syllable {
                rom,
                first: kana,
                second: Some(second),
            },
            first.len_utf8() + second.len_utf8(),
        ));
    }
    syllable(kana).map(|rom| {
        (
            Syllable {
                rom,
                first: kana,
                second: None,
            },
            first.len_utf8(),
        )
    })
}

/// Normalize katakana to hiragana (shifted by `-0x60`), preserving `ー` and iteration marks.
fn to_hiragana(ch: char) -> Option<char> {
    match ch {
        '\u{3041}'..='\u{3096}' | '\u{30fc}' => Some(ch),
        '\u{30a1}'..='\u{30f6}' => char::from_u32(ch as u32 - 0x60),
        '\u{309d}' | '\u{30fd}' => Some('\u{309d}'),
        '\u{309e}' | '\u{30fe}' => Some('\u{309e}'),
        _ => None,
    }
}

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

fn is_vowel(ch: char) -> bool {
    matches!(ch, 'a' | 'i' | 'u' | 'e' | 'o')
}

fn starts_with_vowel(rom: &str) -> bool {
    rom.chars().next().is_some_and(is_vowel)
}

#[cfg(test)]
mod tests {
    use super::romanize;

    fn read(src: &str) -> String {
        let mut out = String::new();
        assert_eq!(romanize(src, &mut out), Some(src.len()), "{src:?}");
        out
    }

    #[test]
    fn hepburn_syllables() {
        for (src, expected) in [
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
            assert_eq!(read(src), expected, "{src:?}");
        }
    }

    #[test]
    fn katakana_syllabary() {
        for (src, expected) in [
            ("コンピュータ", "konpyuuta"),
            ("コーヒー", "koohii"),
            ("ファン", "fan"),
            ("ヴァイオリン", "vaiorin"),
        ] {
            assert_eq!(read(src), expected, "{src:?}");
        }
    }

    #[test]
    fn loanword_digraphs() {
        for (src, expected) in [
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
            assert_eq!(read(src), expected, "{src:?}");
        }
    }

    #[test]
    fn iteration_marks() {
        for (src, expected) in [
            ("いすゞ", "isuzu"),
            ("すゞめ", "suzume"),
            ("みすゞ", "misuzu"),
            ("たゞ", "tada"),
            ("バナヽ", "banana"),
            ("ハヽ", "haha"),
            ("ヾ", ""),
        ] {
            assert_eq!(read(src), expected, "{src:?}");
        }
    }

    #[test]
    fn sokuon_doubling() {
        for (src, expected) in [
            ("まっちゃ", "matcha"),
            ("てんぷら", "tenpura"),
            ("がっこう", "gakkou"),
            ("にっぽん", "nippon"),
        ] {
            assert_eq!(read(src), expected, "{src:?}");
        }
    }
}
