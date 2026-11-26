//! Natives for `@tola/hash`: BLAKE3 digests and lowercase hex encoding.

use std::fmt::Write as _;

use tola_typst::ContentDigest;
use typst::foundations::{Bytes, Str, func};

/// Hash bytes with BLAKE3.
#[func]
pub(super) fn tola_blake3(value: Bytes) -> Bytes {
    Bytes::new(ContentDigest::of(value.as_slice()).as_bytes().to_vec())
}

/// Spell bytes as lowercase hexadecimal, two digits per byte.
#[func]
pub(super) fn tola_bytes_to_hex(value: Bytes) -> Str {
    let mut out = String::with_capacity(value.as_slice().len() * 2);
    for byte in value.as_slice() {
        write!(out, "{byte:02x}").expect("writing to String is infallible");
    }
    out.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABC: &str = "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85";

    #[test]
    fn blake3_abc_vector() {
        let digest = tola_blake3(Bytes::new(b"abc".to_vec()));
        assert_eq!(tola_bytes_to_hex(digest).as_str(), ABC);
    }
}
