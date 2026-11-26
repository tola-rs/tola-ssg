//! Iconify JSON resolution into validated SVG icons.

use std::collections::{BTreeMap, BTreeSet};

mod read;

use read::{IconOverrides, IconRotation};

const MAX_RESOLVED_SVG_BYTES: usize = 128 * 1024 * 1024;

use crate::{
    identity::validate_name, svg::SvgDocument, IconCollection, InvalidCollection, SvgIcon, ViewBox,
};

#[derive(Clone, Copy)]
struct IconSpec<'a> {
    source_name: &'a str,
    body: &'a str,
    vb: ViewBox,
    rotate: IconRotation,
    h_flip: bool,
    v_flip: bool,
}

impl IconOverrides {
    fn inherit_vb(&self, parent: ViewBox) -> Result<ViewBox, InvalidCollection> {
        ViewBox::new(
            self.left.unwrap_or(parent.left()),
            self.top.unwrap_or(parent.top()),
            self.width.unwrap_or(parent.width()),
            self.height.unwrap_or(parent.height()),
        )
        .map_err(|_| InvalidCollection::Geometry)
    }
}

impl IconSpec<'_> {
    fn apply_alias(&self, alias: &IconOverrides) -> Result<Self, InvalidCollection> {
        Ok(Self {
            source_name: self.source_name,
            body: self.body,
            vb: alias.inherit_vb(self.vb)?,
            rotate: self.rotate.combine(alias.rotate.unwrap_or_default()),
            h_flip: self.h_flip ^ alias.h_flip.unwrap_or(false),
            v_flip: self.v_flip ^ alias.v_flip.unwrap_or(false),
        })
    }

    fn document(&self) -> Result<SvgDocument, crate::InvalidSvg> {
        let mut left = self.vb.left();
        let mut top = self.vb.top();
        let mut w = self.vb.width();
        let mut h = self.vb.height();
        let mut rot = self.rotate;
        // Iconify flips before rotating and shifts non-zero view-box origins on single-axis flips.
        let flip = match (self.h_flip, self.v_flip) {
            (true, true) => {
                rot = rot.combine(IconRotation::HALF_TURN);
                String::new()
            }
            (true, false) => {
                let t = format!("translate({} {}) scale(-1 1)", w + left, -top);
                left = 0.0;
                top = 0.0;
                t
            }
            (false, true) => {
                let t = format!("translate({} {}) scale(1 -1)", -left, h + top);
                left = 0.0;
                top = 0.0;
                t
            }
            _ => String::new(),
        };
        let turn = match rot.quarter_turns() {
            1 => {
                let pivot = h / 2.0 + top;
                format!("rotate(90 {pivot} {pivot})")
            }
            2 => format!("rotate(180 {} {})", w / 2.0 + left, h / 2.0 + top),
            3 => {
                let pivot = w / 2.0 + left;
                format!("rotate(-90 {pivot} {pivot})")
            }
            _ => String::new(),
        };
        if matches!(rot.quarter_turns(), 1 | 3) {
            (left, top) = (top, left);
            (w, h) = (h, w);
        }
        let vb = ViewBox::new(left, top, w, h)?;
        let transform = match (turn.is_empty(), flip.is_empty()) {
            (true, _) => flip,
            (_, true) => turn,
            (false, false) => format!("{turn} {flip}"),
        };
        SvgDocument::from_iconify(self.body, vb, &transform)
    }
}

pub(super) fn parse(bytes: &[u8]) -> Result<IconCollection, InvalidCollection> {
    parse_with_budget(bytes, MAX_RESOLVED_SVG_BYTES)
}

fn is_valid_char_key(key: &str) -> bool {
    key.split('-').all(|hex| {
        !hex.is_empty()
            && hex.bytes().all(|b| b.is_ascii_hexdigit())
            && u32::from_str_radix(hex, 16)
                .ok()
                .and_then(char::from_u32)
                .is_some()
    })
}

fn parse_with_budget(bytes: &[u8], limit: usize) -> Result<IconCollection, InvalidCollection> {
    let raw = read::parse(bytes)?;
    let root = ViewBox::new(raw.left, raw.top, raw.width, raw.height)
        .map_err(|_| InvalidCollection::Geometry)?;
    let mut resolved = BTreeMap::new();
    for (name, icon) in &raw.icons {
        validate_name(name)?;
        let ov = &icon.overrides;
        resolved.insert(
            name.as_str(),
            IconSpec {
                source_name: name.as_str(),
                body: &icon.body,
                vb: ov.inherit_vb(root)?,
                rotate: raw.rotate.combine(ov.rotate.unwrap_or_default()),
                h_flip: raw.h_flip ^ ov.h_flip.unwrap_or(false),
                v_flip: raw.v_flip ^ ov.v_flip.unwrap_or(false),
            },
        );
    }
    for name in raw.aliases.keys() {
        validate_name(name)?;
        if raw.icons.contains_key(name) {
            return Err(InvalidCollection::DuplicateIcon { name: name.clone() });
        }
    }
    for name in raw.aliases.keys().map(String::as_str) {
        if resolved.contains_key(name) {
            continue;
        }
        let mut chain = Vec::new();
        let mut visiting = BTreeSet::new();
        let mut parent = name;
        while !resolved.contains_key(parent) {
            if !visiting.insert(parent) {
                return Err(InvalidCollection::AliasCycle {
                    name: parent.to_owned(),
                });
            }
            let alias = raw
                .aliases
                .get(parent)
                .ok_or_else(|| InvalidCollection::AliasTarget {
                    name: name.to_owned(),
                })?;
            chain.push((parent, alias));
            parent = &alias.parent;
        }
        let mut icon = resolved[parent];
        for (alias_name, alias) in chain.into_iter().rev() {
            icon = icon.apply_alias(&alias.overrides)?;
            resolved.insert(alias_name, icon);
        }
    }
    for (ch, target) in &raw.chars {
        if !is_valid_char_key(ch) {
            return Err(InvalidCollection::CharacterKey);
        }
        if !resolved.contains_key(target.as_str()) {
            return Err(InvalidCollection::CharacterTarget);
        }
        if resolved.contains_key(ch.as_str()) && ch != target {
            return Err(InvalidCollection::DuplicateIcon { name: ch.clone() });
        }
    }
    let mut icons = BTreeMap::new();
    let mut shared = BTreeMap::<_, SvgIcon>::new();
    let mut remaining = limit;
    for (name, icon) in resolved {
        let key = (
            icon.source_name,
            [
                icon.vb.left().to_bits(),
                icon.vb.top().to_bits(),
                icon.vb.width().to_bits(),
                icon.vb.height().to_bits(),
            ],
            icon.rotate.quarter_turns(),
            icon.h_flip,
            icon.v_flip,
        );
        let svg = if let Some(svg) = shared.get(&key) {
            svg.clone()
        } else {
            // Charge each unique variant's raw body plus serialized SVG so normalization cannot hide work.
            remaining = remaining
                .checked_sub(icon.body.len())
                .ok_or(InvalidCollection::ExpandedTooLarge { limit })?;
            let doc = icon.document().map_err(|source| InvalidCollection::Svg {
                name: name.to_owned(),
                source,
            })?;
            let len = doc.serialized_len();
            remaining = remaining
                .checked_sub(len)
                .ok_or(InvalidCollection::ExpandedTooLarge { limit })?;
            let svg = doc
                .into_icon(len)
                .map_err(|source| InvalidCollection::Svg {
                    name: name.to_owned(),
                    source,
                })?;
            shared.insert(key, svg.clone());
            svg
        };
        icons.insert(name.to_owned(), svg);
    }
    Ok(IconCollection {
        source_prefix: Some(raw.prefix),
        source_license: raw.metadata.and_then(|m| m.license),
        icons,
        characters: raw.chars,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IconPaint, InvalidSvg};

    #[test]
    fn imported_geometry_and_aliases() {
        let col = parse(
            br#"{
            "prefix":"upstream", "left":2, "top":3, "width":20, "height":10,
            "icons":{"base":{"body":"<path fill=\"currentColor\" d=\"M2 3h20v10H2z\"/>"}},
            "aliases":{"wide":{"parent":"base","width":30},"turned":{"parent":"wide","rotate":1}},
            "chars":{"e001":"turned"}
        }"#,
        )
        .unwrap();
        assert_eq!(col.get("base").unwrap().view_box().left(), 2.0);
        assert_eq!(col.get("wide").unwrap().aspect_ratio(), 3.0);
        let turned = col.get("turned").unwrap();
        assert_eq!(
            turned.view_box(),
            ViewBox::new(3.0, 2.0, 10.0, 30.0).unwrap()
        );
        assert!(turned.svg().contains("rotate(90 8 8)"));
        assert_eq!(col.get("e001").unwrap().svg(), turned.svg());
        assert_eq!(turned.paint(), IconPaint::CurrentColor);
        assert_eq!(col.names().collect::<Vec<_>>(), ["base", "turned", "wide"]);
    }

    #[test]
    fn alias_errors() {
        assert!(matches!(
            parse(
                br#"{"prefix":"a","icons":{},"aliases":{"x":{"parent":"y"},"y":{"parent":"x"}}}"#
            ),
            Err(InvalidCollection::AliasCycle { .. })
        ));
        assert!(matches!(
            parse(br#"{"prefix":"a","icons":{},"aliases":{"x":{"parent":"missing"}}}"#),
            Err(InvalidCollection::AliasTarget { .. })
        ));
        assert!(matches!(
            parse(br#"{"prefix":"a","icons":{"x":{"body":"<path/>"}},"aliases":{"x":{"parent":"x"}}}"#),
            Err(InvalidCollection::DuplicateIcon { .. })
        ));
        assert!(matches!(
            parse(br#"{"prefix":"a","icons":{"x":{"body":"<path/>"}},"aliases":{"a":{"parent":"x","width":1e300,"height":1e-300}}}"#),
            Err(InvalidCollection::Geometry)
        ));
    }

    #[test]
    fn invalid_icon_fails_import() {
        let src = br#"{"prefix":"a","icons":{"good":{"body":"<path/>"},"bad":{"body":"<path"}}}"#;
        assert!(
            matches!(parse(src), Err(InvalidCollection::Svg { name, source: InvalidSvg::Xml { .. } }) if name == "bad")
        );
        let src = br#"{"prefix":"a","icons":{"bad":{"body":"<path d=\"M0\"/>"}}}"#;
        assert!(matches!(
            parse(src),
            Err(InvalidCollection::Svg {
                source: InvalidSvg::Path,
                ..
            })
        ));
    }

    #[test]
    fn expanded_budget_enforced() {
        let body = format!(
            "<title>&amp;&lt;&gt;&quot;&#13;&#10;&#9; 星</title><path{}d=\"M0 0h12v12z\" fill=\"currentColor\"/>",
            " ".repeat(128)
        );
        let base = serde_json::json!({"prefix":"source","icons":{"base":{"body":body}}});
        let single = parse(base.to_string().as_bytes()).unwrap();
        let budget = body.len() + single.get("base").unwrap().svg().len();
        assert_eq!(
            parse_with_budget(base.to_string().as_bytes(), budget)
                .unwrap()
                .get("base")
                .unwrap()
                .svg(),
            single.get("base").unwrap().svg()
        );
        assert!(matches!(
            parse_with_budget(base.to_string().as_bytes(), budget - 1),
            Err(InvalidCollection::ExpandedTooLarge { .. })
        ));

        let mut variants = base;
        variants["aliases"] = serde_json::json!({"wide":{"parent":"base","width":32}});
        assert!(matches!(
            parse_with_budget(variants.to_string().as_bytes(), budget),
            Err(InvalidCollection::ExpandedTooLarge { .. })
        ));
        let wide = parse(variants.to_string().as_bytes()).unwrap();
        let both = budget + body.len() + wide.get("wide").unwrap().svg().len();
        assert_eq!(
            parse_with_budget(variants.to_string().as_bytes(), both)
                .unwrap()
                .get("wide")
                .unwrap()
                .aspect_ratio(),
            2.0
        );
        assert!(matches!(
            parse_with_budget(variants.to_string().as_bytes(), both - 1),
            Err(InvalidCollection::ExpandedTooLarge { .. })
        ));

        variants["aliases"] = serde_json::json!({"same":{"parent":"base"}});
        assert_eq!(
            parse_with_budget(variants.to_string().as_bytes(), budget)
                .unwrap()
                .get("same")
                .unwrap()
                .svg(),
            single.get("base").unwrap().svg()
        );
    }
}
