//! Local sound changes for Hangul syllable runs, using Revised Romanization spellings.
//!
//! Liaison, nasalization, palatalization, and aspiration follow the syllable boundary;
//! tensing is not reflected in the spelling. See the [RR sound-change rules] and
//! [Standard Pronunciation §12].
//!
//! This is not lexical analysis. Y-vowel insertion is a syllable-boundary heuristic,
//! and ㄼ defaults to ㅂ before nasals. Compound boundaries, noun-preserved ㅎ, and
//! personal-name conventions can require an explicit site spelling instead.
//!
//! [RR sound-change rules]: https://m.korean.go.kr/front/page/pageView.do?mn_id=99&page_id=P000150
//! [Standard Pronunciation §12]: https://korean.go.kr/front/onlineQna/onlineQnaView.do?mn_id=216&pageIndex=1&qna_seq=317199

/// The initial jamo, in the standard's order.
const INITIALS: [&str; 19] = [
    "g", "kk", "n", "d", "tt", "r", "m", "b", "pp", "s", "ss", "", "j", "jj", "ch", "k", "t", "p",
    "h",
];

/// The medial jamo, in the standard's order.
const MEDIALS: [&str; 21] = [
    "a", "ae", "ya", "yae", "eo", "e", "yeo", "ye", "o", "wa", "wae", "oe", "yo", "u", "wo", "we",
    "wi", "yu", "eu", "ui", "i",
];

/// The coda jamo read before a consonant or at the end of a word.
const CODAS: [&str; 28] = [
    "", "k", "k", "k", "n", "n", "n", "t", "l", "k", "m", "l", "l", "l", "p", "l", "m", "p", "p",
    "t", "t", "ng", "t", "t", "k", "t", "p", "t",
];

/// The coda jamo read before a syllable that begins with a vowel.
const CODAS_OPEN: [&str; 28] = [
    "", "g", "kk", "ks", "n", "nj", "n", "d", "r", "lg", "lm", "lb", "ls", "lt", "lp", "r", "m",
    "b", "ps", "s", "ss", "ng", "j", "ch", "k", "t", "p", "",
];

// Initial jamo the sound changes name.
const KIYEOK: usize = 0;
const NIEUN: usize = 2;
const DIGEUT: usize = 3;
const RIEUL: usize = 5;
const MIEUM: usize = 6;
const BIEUP: usize = 7;
const SIOS: usize = 9;
const IEUNG: usize = 11;
const JIEUT: usize = 12;
const HIEUT: usize = 18;

// Medial jamo the sound changes name.
const YA: usize = 2;
const YAE: usize = 3;
const YEO: usize = 6;
const YE: usize = 7;
const YO: usize = 12;
const YU: usize = 17;
const I: usize = 20;

// Coda jamo the sound changes name.
const C_KIYEOK: usize = 1;
const C_KKIYEOK: usize = 2;
const C_NIEUN_JIEUT: usize = 5;
const C_NIEUN_HIEUT: usize = 6;
const C_DIGEUT: usize = 7;
const C_RIEUL: usize = 8;
const C_RIEUL_KIYEOK: usize = 9;
const C_RIEUL_BIEUP: usize = 11;
const C_RIEUL_THIEUTH: usize = 13;
const C_RIEUL_PIEUP: usize = 14;
const C_RIEUL_HIEUT: usize = 15;
const C_BIEUP: usize = 17;
const C_BIEUP_SIOS: usize = 18;
const C_JIEUT: usize = 22;
const C_KHIEUKH: usize = 24;
const C_THIEUTH: usize = 25;
const C_PHIEUPH: usize = 26;
const C_HIEUT: usize = 27;

/// One syllable's three jamo, by their index in the jamo tables above.
#[derive(Clone, Copy)]
struct Syllable {
    initial: usize,
    medial: usize,
    coda: usize,
}

/// The jamo of one Hangul syllable.
fn syllable(character: char) -> Option<Syllable> {
    let index = (character as u32).checked_sub(0xAC00)?;
    if index >= 11172 {
        return None;
    }
    Some(Syllable {
        initial: (index / 588) as usize,
        medial: ((index % 588) / 28) as usize,
        coda: (index % 28) as usize,
    })
}

/// Append the romanization of the Hangul at the start of `text`, and report the bytes it read.
///
/// Answers `None` when `text` does not start with a Hangul syllable, and appends nothing
/// in that case.
pub(super) fn romanize(text: &str, out: &mut String) -> Option<usize> {
    let mut consumed = 0;
    // The syllable before decides this one's first jamo.
    let mut carried: Option<&'static str> = None;
    while let Some(character) = text[consumed..].chars().next() {
        let Some(current) = syllable(character) else {
            break;
        };
        let next = text[consumed + character.len_utf8()..]
            .chars()
            .next()
            .and_then(syllable);
        let opening = match carried.take() {
            Some(opening) => opening,
            None => INITIALS[current.initial],
        };
        let (coda, carried_opening) = match next {
            Some(next) => {
                let (coda, opening) = join(current.coda, next.initial, next.medial);
                (coda, Some(opening))
            }
            None => (CODAS[current.coda], None),
        };
        out.push_str(opening);
        out.push_str(MEDIALS[current.medial]);
        out.push_str(coda);
        carried = carried_opening;
        consumed += character.len_utf8();
    }
    (consumed > 0).then_some(consumed)
}

/// The coda contribution and following onset, after their boundary sound changes.
fn join(coda: usize, initial: usize, medial: usize) -> (&'static str, &'static str) {
    if coda == 0 {
        return ("", INITIALS[initial]);
    }
    if initial == IEUNG {
        if matches!(medial, YA | YAE | YEO | YE | YO | YU) {
            return match coda {
                C_KIYEOK | C_KKIYEOK | C_KHIEUKH | C_RIEUL_KIYEOK => ("ng", "n"),
                C_DIGEUT | C_THIEUTH => ("n", "n"),
                C_BIEUP | C_PHIEUPH | C_RIEUL_BIEUP | C_RIEUL_PIEUP | C_BIEUP_SIOS => ("m", "n"),
                C_RIEUL => ("l", "l"),
                _ => (CODAS_OPEN[coda], ""),
            };
        }
        if coda == C_DIGEUT && medial == I {
            return ("", "j");
        }
        // ㄾ palatalizes to ㅊ like ㅌ, and its ㄹ still belongs to the coda: 벼훑이 is
        // 벼훌치.
        if coda == C_THIEUTH && medial == I {
            return ("", "ch");
        }
        if coda == C_RIEUL_THIEUTH && medial == I {
            return ("l", "ch");
        }
        return (CODAS_OPEN[coda], "");
    }
    if initial == HIEUT {
        if coda == C_DIGEUT && medial == I {
            return ("", "ch");
        }
        // ㄾ keeps its ㄹ here as it does before a vowel: 훑히다 is 훌치다.
        if coda == C_RIEUL_THIEUTH && medial == I {
            return ("l", "ch");
        }
        // Aspiration precedes coda neutralization inside a stem-plus-suffix pair.
        return match coda {
            C_RIEUL_KIYEOK => ("l", "k"),
            C_RIEUL_BIEUP => ("l", "p"),
            C_NIEUN_JIEUT => ("n", "ch"),
            C_JIEUT => ("", "ch"),
            _ => match CODAS[coda] {
                "k" => ("", "k"),
                "t" => ("", "t"),
                "p" => ("", "p"),
                closed_coda => (closed_coda, INITIALS[HIEUT]),
            },
        };
    }
    if matches!(initial, NIEUN | MIEUM | RIEUL) {
        let closed_coda = if coda == C_RIEUL_BIEUP {
            "p"
        } else {
            CODAS[coda]
        };
        if (closed_coda == "l" && initial == NIEUN)
            || (matches!(closed_coda, "n" | "l") && initial == RIEUL)
        {
            return ("l", "l");
        }
        // Coda neutralization precedes nasal assimilation.
        let nasal_coda = match closed_coda {
            "k" => "ng",
            "t" => "n",
            "p" => "m",
            _ => closed_coda,
        };
        let opening = if initial == RIEUL {
            "n"
        } else {
            INITIALS[initial]
        };
        return (nasal_coda, opening);
    }
    if matches!(coda, C_HIEUT | C_NIEUN_HIEUT | C_RIEUL_HIEUT) {
        let merged = match initial {
            KIYEOK => Some("k"),
            DIGEUT => Some("t"),
            BIEUP => Some("p"),
            JIEUT => Some("ch"),
            // RR does not reflect the tensing after ㅎ.
            SIOS => Some("s"),
            _ => None,
        };
        if let Some(merged) = merged {
            let coda = match coda {
                C_NIEUN_HIEUT => "n",
                C_RIEUL_HIEUT => "l",
                _ => "",
            };
            return (coda, merged);
        }
    }
    (CODAS[coda], INITIALS[initial])
}

#[cfg(test)]
mod tests {
    use super::romanize;

    fn romanize_run(text: &str) -> String {
        let mut out = String::new();
        assert_eq!(
            romanize(text, &mut out),
            Some(text.len()),
            "{text:?} reads whole"
        );
        out
    }

    #[test]
    fn syllables_follow_rr_spellings() {
        for (text, expected) in [("한국", "hanguk"), ("서울", "seoul")] {
            assert_eq!(romanize_run(text), expected, "{text:?}");
        }
    }

    #[test]
    fn tensing_is_not_transcribed() {
        assert_eq!(romanize_run("압구정"), "apgujeong");
    }

    #[test]
    fn rieul_between_vowels_is_r() {
        for (text, expected) in [("구리", "guri"), ("설악", "seorak")] {
            assert_eq!(romanize_run(text), expected, "{text:?}");
        }
    }

    #[test]
    fn coda_nasalization_follows_sound() {
        for (text, expected) in [
            ("백마", "baengma"),
            ("십만", "simman"),
            ("몫몫이", "mongmoksi"),
            ("첫눈", "cheonnun"),
            ("젖멍울", "jeonmeongul"),
            ("놓는", "nonneun"),
            ("밟는", "bamneun"),
        ] {
            assert_eq!(romanize_run(text), expected, "{text:?}");
        }
    }

    #[test]
    fn rieul_after_codas_becomes_n() {
        for (text, expected) in [
            ("종로", "jongno"),
            ("백로", "baengno"),
            ("왕십리", "wangsimni"),
        ] {
            assert_eq!(romanize_run(text), expected, "{text:?}");
        }
    }

    #[test]
    fn rieul_assimilation_is_lateral() {
        for (text, expected) in [
            ("신라", "silla"),
            ("별내", "byeollae"),
            ("뚫는", "ttulleun"),
            ("울릉", "ulleung"),
        ] {
            assert_eq!(romanize_run(text), expected, "{text:?}");
        }
    }

    #[test]
    fn compound_y_vowels_gain_consonants() {
        for (text, expected) in [("학여울", "hangnyeoul"), ("알약", "allyak")] {
            assert_eq!(romanize_run(text), expected, "{text:?}");
        }
    }

    #[test]
    fn stops_palatalize_before_i() {
        for (text, expected) in [
            ("같이", "gachi"),
            ("해돋이", "haedoji"),
            ("굳히다", "guchida"),
            ("핥이다", "halchida"),
            ("벼훑이", "byeohulchi"),
            ("훑히다", "hulchida"),
        ] {
            assert_eq!(romanize_run(text), expected, "{text:?}");
        }
    }

    #[test]
    fn hieut_aspirates_adjacent_stops() {
        for (text, expected) in [
            ("좋고", "joko"),
            ("놓다", "nota"),
            ("낳지", "nachi"),
            ("잡혀", "japyeo"),
            ("많다", "manta"),
            ("밝히다", "balkida"),
            ("넓히다", "neolpida"),
            ("꽂히다", "kkochida"),
            ("앉히다", "anchida"),
            ("맏형", "matyeong"),
        ] {
            assert_eq!(romanize_run(text), expected, "{text:?}");
        }
    }

    #[test]
    fn hieut_drops_before_vowels() {
        for (text, expected) in [
            ("놓아", "noa"),
            ("많아", "mana"),
            ("싫어", "sireo"),
            ("닳아", "dara"),
        ] {
            assert_eq!(romanize_run(text), expected, "{text:?}");
        }
    }

    #[test]
    fn hieut_drops_before_sios() {
        assert_eq!(romanize_run("좋소"), "joso");
    }

    #[test]
    fn only_hangul_syllables_are_read() {
        let mut out = String::new();
        assert_eq!(romanize("한국어 漢字", &mut out), Some("한국어".len()));
        assert_eq!(out, "hangugeo");
        assert!(romanize("漢字", &mut out).is_none());
    }
}
