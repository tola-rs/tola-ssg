//! Revised Romanization (RR) for Hangul syllable runs.
//!
//! Applies boundary liaison, nasalization, palatalization, and aspiration; tensing is omitted
//! per RR. Y-vowel insertion is a syllable-boundary heuristic and `ㄼ` defaults to `ㅂ` before
//! nasals without lexical compound analysis.

const INITIALS: [&str; 19] = [
    "g", "kk", "n", "d", "tt", "r", "m", "b", "pp", "s", "ss", "", "j", "jj", "ch", "k", "t", "p",
    "h",
];

const MEDIALS: [&str; 21] = [
    "a", "ae", "ya", "yae", "eo", "e", "yeo", "ye", "o", "wa", "wae", "oe", "yo", "u", "wo", "we",
    "wi", "yu", "eu", "ui", "i",
];

/// Coda before a consonant or word end.
const CODAS: [&str; 28] = [
    "", "k", "k", "k", "n", "n", "n", "t", "l", "k", "m", "l", "l", "l", "p", "l", "m", "p", "p",
    "t", "t", "ng", "t", "t", "k", "t", "p", "t",
];

/// Coda before a vowel-initial syllable (`ㅇ`).
const CODAS_OPEN: [&str; 28] = [
    "", "g", "kk", "ks", "n", "nj", "n", "d", "r", "lg", "lm", "lb", "ls", "lt", "lp", "r", "m",
    "b", "ps", "s", "ss", "ng", "j", "ch", "k", "t", "p", "",
];

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

const YA: usize = 2;
const YAE: usize = 3;
const YEO: usize = 6;
const YE: usize = 7;
const YO: usize = 12;
const YU: usize = 17;
const I: usize = 20;

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

#[derive(Clone, Copy)]
struct Syllable {
    init: usize,
    med: usize,
    coda: usize,
}

fn syllable(ch: char) -> Option<Syllable> {
    let idx = (ch as u32).checked_sub(0xAC00)?;
    if idx >= 11172 {
        return None;
    }
    Some(Syllable {
        init: (idx / 588) as usize,
        med: ((idx % 588) / 28) as usize,
        coda: (idx % 28) as usize,
    })
}

pub(super) fn romanize(src: &str, out: &mut String) -> Option<usize> {
    let mut off = 0;
    let mut carried: Option<&'static str> = None;
    while let Some(ch) = src[off..].chars().next() {
        let Some(cur) = syllable(ch) else {
            break;
        };
        let next = src[off + ch.len_utf8()..].chars().next().and_then(syllable);
        let init = carried.take().unwrap_or(INITIALS[cur.init]);
        let (coda, next_init) = match next {
            Some(next) => {
                let (coda, opening) = join(cur.coda, next.init, next.med);
                (coda, Some(opening))
            }
            None => (CODAS[cur.coda], None),
        };
        out.push_str(init);
        out.push_str(MEDIALS[cur.med]);
        out.push_str(coda);
        carried = next_init;
        off += ch.len_utf8();
    }
    (off > 0).then_some(off)
}

fn join(coda: usize, init: usize, med: usize) -> (&'static str, &'static str) {
    if coda == 0 {
        return ("", INITIALS[init]);
    }
    if init == IEUNG {
        if matches!(med, YA | YAE | YEO | YE | YO | YU) {
            return match coda {
                C_KIYEOK | C_KKIYEOK | C_KHIEUKH | C_RIEUL_KIYEOK => ("ng", "n"),
                C_DIGEUT | C_THIEUTH => ("n", "n"),
                C_BIEUP | C_PHIEUPH | C_RIEUL_BIEUP | C_RIEUL_PIEUP | C_BIEUP_SIOS => ("m", "n"),
                C_RIEUL => ("l", "l"),
                _ => (CODAS_OPEN[coda], ""),
            };
        }
        if coda == C_DIGEUT && med == I {
            return ("", "j");
        }
        // `ㄾ` palatalizes to `ㅊ` while retaining its leading `ㄹ` (`벼훑이` -> `벼훌치`).
        if coda == C_THIEUTH && med == I {
            return ("", "ch");
        }
        if coda == C_RIEUL_THIEUTH && med == I {
            return ("l", "ch");
        }
        return (CODAS_OPEN[coda], "");
    }
    if init == HIEUT {
        if coda == C_DIGEUT && med == I {
            return ("", "ch");
        }
        // `ㄾ` retains `ㄹ` before `히` (`훑히다` -> `훌치다`).
        if coda == C_RIEUL_THIEUTH && med == I {
            return ("l", "ch");
        }
        // Aspiration precedes coda neutralization across stem+suffix boundaries.
        return match coda {
            C_RIEUL_KIYEOK => ("l", "k"),
            C_RIEUL_BIEUP => ("l", "p"),
            C_NIEUN_JIEUT => ("n", "ch"),
            C_JIEUT => ("", "ch"),
            _ => match CODAS[coda] {
                "k" => ("", "k"),
                "t" => ("", "t"),
                "p" => ("", "p"),
                closed => (closed, INITIALS[HIEUT]),
            },
        };
    }
    if matches!(init, NIEUN | MIEUM | RIEUL) {
        let closed = if coda == C_RIEUL_BIEUP {
            "p"
        } else {
            CODAS[coda]
        };
        if (closed == "l" && init == NIEUN) || (matches!(closed, "n" | "l") && init == RIEUL) {
            return ("l", "l");
        }
        // Coda neutralization precedes nasal assimilation.
        let nasal = match closed {
            "k" => "ng",
            "t" => "n",
            "p" => "m",
            _ => closed,
        };
        let opening = if init == RIEUL { "n" } else { INITIALS[init] };
        return (nasal, opening);
    }
    if matches!(coda, C_HIEUT | C_NIEUN_HIEUT | C_RIEUL_HIEUT) {
        let merged = match init {
            KIYEOK => Some("k"),
            DIGEUT => Some("t"),
            BIEUP => Some("p"),
            JIEUT => Some("ch"),
            // RR omits tensing after `ㅎ`.
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
    (CODAS[coda], INITIALS[init])
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
    fn rr_basic() {
        for (src, expected) in [("한국", "hanguk"), ("서울", "seoul")] {
            assert_eq!(read(src), expected, "{src:?}");
        }
    }

    #[test]
    fn rieul_intervocalic() {
        for (src, expected) in [("구리", "guri"), ("설악", "seorak")] {
            assert_eq!(read(src), expected, "{src:?}");
        }
    }

    #[test]
    fn coda_nasalization() {
        for (src, expected) in [
            ("백마", "baengma"),
            ("십만", "simman"),
            ("몫몫이", "mongmoksi"),
            ("첫눈", "cheonnun"),
            ("젖멍울", "jeonmeongul"),
            ("놓는", "nonneun"),
            ("밟는", "bamneun"),
        ] {
            assert_eq!(read(src), expected, "{src:?}");
        }
    }

    #[test]
    fn rieul_to_n() {
        for (src, expected) in [
            ("종로", "jongno"),
            ("백로", "baengno"),
            ("왕십리", "wangsimni"),
        ] {
            assert_eq!(read(src), expected, "{src:?}");
        }
    }

    #[test]
    fn rieul_assimilation() {
        for (src, expected) in [
            ("신라", "silla"),
            ("별내", "byeollae"),
            ("뚫는", "ttulleun"),
            ("울릉", "ulleung"),
        ] {
            assert_eq!(read(src), expected, "{src:?}");
        }
    }

    #[test]
    fn y_vowel_insertion() {
        for (src, expected) in [("학여울", "hangnyeoul"), ("알약", "allyak")] {
            assert_eq!(read(src), expected, "{src:?}");
        }
    }

    #[test]
    fn palatalization() {
        for (src, expected) in [
            ("같이", "gachi"),
            ("해돋이", "haedoji"),
            ("굳히다", "guchida"),
            ("핥이다", "halchida"),
            ("벼훑이", "byeohulchi"),
            ("훑히다", "hulchida"),
        ] {
            assert_eq!(read(src), expected, "{src:?}");
        }
    }

    #[test]
    fn hieut_aspiration() {
        for (src, expected) in [
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
            assert_eq!(read(src), expected, "{src:?}");
        }
    }

    #[test]
    fn stops_at_non_hangul() {
        let mut out = String::new();
        assert_eq!(romanize("한국어 漢字", &mut out), Some("한국어".len()));
        assert_eq!(out, "hangugeo");
        assert!(romanize("漢字", &mut out).is_none());
    }
}
