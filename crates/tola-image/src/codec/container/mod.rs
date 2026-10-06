//! Reading a container's header: the integers it stores, and the color metadata it carries.

mod bmp;
mod gif;
mod jpeg;
mod png;
mod svg;
mod webp;

pub(in crate::codec) use bmp::scan_bmp;
pub(in crate::codec) use gif::scan_gif;
pub(in crate::codec) use jpeg::jpeg_has_icc;
pub(in crate::codec) use png::scan_png;
pub(in crate::codec) use svg::{inspect_svg, looks_like_xml};
pub(in crate::codec) use webp::scan_webp;

pub(super) fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

pub(super) fn le32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

pub(super) fn le16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}
