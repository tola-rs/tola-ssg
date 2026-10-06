//! The fonts this build has.

use std::io::Read;
use std::sync::LazyLock;

use typst::foundations::Bytes;
use typst::text::{Font, FontInfo};

/// The container `just scripts::fonts` writes: an index of the embedded fonts, then one zstd
/// frame of their bytes.
static CONTAINER: &[u8] = include_bytes!("../../../assets/embedded-fonts.bin");

/// The bytes every embedded container opens with.
const MAGIC: &[u8] = b"TOLAFNT1";

/// One font the container has.
struct CarriedFont {
    /// The upstream font file's name.
    name: &'static str,
    /// Where the font's bytes start in the decoded payload.
    offset: usize,
    /// How many bytes the font takes.
    length: usize,
}

/// The container's index, and the payload it addresses.
struct CarriedFonts {
    fonts: Vec<CarriedFont>,
    payload: Vec<u8>,
}

/// The embedded fonts, parsed and decoded by the first request.
static CARRIED: LazyLock<CarriedFonts> = LazyLock::new(|| {
    let (count, mut index) = CONTAINER
        .strip_prefix(MAGIC)
        .expect("the embedded font container opens with its magic")
        .split_at_checked(4)
        .expect("an embedded container names how many fonts it holds");
    let count = u32::from_le_bytes(count.try_into().expect("a count is four bytes"));
    let mut fonts = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let (name_length, entry) = index.split_first().expect("an entry names its font");
        let (name, entry) = entry
            .split_at_checked(*name_length as usize)
            .expect("an embedded font's name fits its container");
        let (offset, entry) = entry.split_at_checked(4).expect("an entry holds an offset");
        let (length, entry) = entry.split_at_checked(4).expect("an entry holds a length");
        fonts.push(CarriedFont {
            name: std::str::from_utf8(name).expect("an embedded font's name is UTF-8"),
            offset: u32::from_le_bytes(offset.try_into().expect("an offset is four bytes"))
                as usize,
            length: u32::from_le_bytes(length.try_into().expect("a length is four bytes")) as usize,
        });
        index = entry;
    }
    let payload = decompress(index);
    for font in &fonts {
        assert!(
            font.offset + font.length <= payload.len(),
            "the embedded font `{}` addresses its payload",
            font.name
        );
    }
    CarriedFonts { fonts, payload }
});

/// Decode the zstd frame one embedded container holds.
fn decompress(compressed: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    ruzstd::decoding::StreamingDecoder::new(compressed)
        .expect("the embedded font container holds one zstd frame")
        .read_to_end(&mut payload)
        .expect("the embedded font payload decodes");
    // The payload backs the fonts for the process, so its growth slack is not kept.
    payload.shrink_to_fit();
    payload
}

/// The fonts this build has, one per face the container holds.
pub(crate) fn fonts() -> impl Iterator<Item = (Font, FontInfo)> {
    CARRIED.fonts.iter().flat_map(|carried| {
        let bytes = Bytes::new(&CARRIED.payload[carried.offset..carried.offset + carried.length]);
        Font::iter(bytes).map(|font| {
            let info = font.info().clone();
            (font, info)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::{CARRIED, fonts};
    use typst::foundations::Bytes;
    use typst::text::Font;

    #[test]
    fn every_embedded_font_parses() {
        for carried in &CARRIED.fonts {
            let bytes = &CARRIED.payload[carried.offset..carried.offset + carried.length];
            assert!(
                Font::iter(Bytes::new(bytes)).next().is_some(),
                "{}",
                carried.name
            );
        }
    }

    #[test]
    fn embedded_fonts_name_the_default_families() {
        let families: Vec<_> = fonts().map(|(_, info)| info.family).collect();
        for family in [
            "DejaVu Sans Mono",
            "Libertinus Serif",
            "New Computer Modern",
            "New Computer Modern Math",
        ] {
            assert!(
                families.iter().any(|carried| carried.as_str() == family),
                "{family} is not embedded"
            );
        }
    }
}
