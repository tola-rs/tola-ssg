//! Iconify JSON parsed into the validated icons of one collection.

use std::collections::{BTreeMap, BTreeSet};

mod read;

use read::{IconOverrides, IconRotation};

const MAX_RESOLVED_SVG_BYTES: usize = 128 * 1024 * 1024;

use crate::{
    identity::validate_name, svg::SvgDocument, IconCollection, InvalidCollection, SvgIcon, ViewBox,
};

#[derive(Clone, Copy)]
struct ResolvedIcon<'a> {
    source_icon_name: &'a str,
    body: &'a str,
    view_box: ViewBox,
    rotation: IconRotation,
    h_flip: bool,
    v_flip: bool,
}

impl IconOverrides {
    fn inherit_view_box(&self, parent: ViewBox) -> Result<ViewBox, InvalidCollection> {
        ViewBox::new(
            self.left.unwrap_or(parent.left()),
            self.top.unwrap_or(parent.top()),
            self.width.unwrap_or(parent.width()),
            self.height.unwrap_or(parent.height()),
        )
        .map_err(|_| InvalidCollection::Geometry)
    }
}

impl ResolvedIcon<'_> {
    fn apply_alias(&self, alias: &IconOverrides) -> Result<Self, InvalidCollection> {
        Ok(Self {
            source_icon_name: self.source_icon_name,
            body: self.body,
            view_box: alias.inherit_view_box(self.view_box)?,
            rotation: self.rotation.combine(alias.rotate.unwrap_or_default()),
            h_flip: self.h_flip ^ alias.h_flip.unwrap_or(false),
            v_flip: self.v_flip ^ alias.v_flip.unwrap_or(false),
        })
    }

    fn document(&self) -> Result<SvgDocument, crate::InvalidSvg> {
        let mut left = self.view_box.left();
        let mut top = self.view_box.top();
        let mut width = self.view_box.width();
        let mut height = self.view_box.height();
        let mut rotation = self.rotation;
        // Iconify's builder flips before rotation and changes the view-box origin with a flip.
        let flip = match (self.h_flip, self.v_flip) {
            (true, true) => {
                rotation = rotation.combine(IconRotation::HALF_TURN);
                String::new()
            }
            (true, false) => {
                let transform = format!("translate({} {}) scale(-1 1)", width + left, -top);
                left = 0.0;
                top = 0.0;
                transform
            }
            (false, true) => {
                let transform = format!("translate({} {}) scale(1 -1)", -left, height + top);
                left = 0.0;
                top = 0.0;
                transform
            }
            _ => String::new(),
        };
        let turn = match rotation.quarter_turns() {
            1 => {
                let pivot = height / 2.0 + top;
                format!("rotate(90 {pivot} {pivot})")
            }
            2 => format!("rotate(180 {} {})", width / 2.0 + left, height / 2.0 + top),
            3 => {
                let pivot = width / 2.0 + left;
                format!("rotate(-90 {pivot} {pivot})")
            }
            _ => String::new(),
        };
        if matches!(rotation.quarter_turns(), 1 | 3) {
            (left, top) = (top, left);
            (width, height) = (height, width);
        }
        let view_box = ViewBox::new(left, top, width, height)?;
        let transform = if turn.is_empty() {
            flip
        } else if flip.is_empty() {
            turn
        } else {
            let mut transform = turn;
            transform.reserve_exact(1 + flip.len());
            transform.push(' ');
            transform.push_str(&flip);
            transform
        };
        SvgDocument::from_iconify(self.body, view_box, &transform)
    }
}

pub(super) fn parse(bytes: &[u8]) -> Result<IconCollection, InvalidCollection> {
    parse_with_budget(bytes, MAX_RESOLVED_SVG_BYTES)
}

fn parse_with_budget(bytes: &[u8], limit: usize) -> Result<IconCollection, InvalidCollection> {
    let collection = read::parse(bytes)?;
    let root = ViewBox::new(
        collection.left,
        collection.top,
        collection.width,
        collection.height,
    )
    .map_err(|_| InvalidCollection::Geometry)?;
    let mut resolved = BTreeMap::new();
    for (name, icon) in &collection.icons {
        validate_name(name)?;
        let overrides = &icon.overrides;
        let view_box = overrides.inherit_view_box(root)?;
        resolved.insert(
            name.as_str(),
            ResolvedIcon {
                source_icon_name: name.as_str(),
                body: &icon.body,
                view_box,
                rotation: collection
                    .rotate
                    .combine(overrides.rotate.unwrap_or_default()),
                h_flip: collection.h_flip ^ overrides.h_flip.unwrap_or(false),
                v_flip: collection.v_flip ^ overrides.v_flip.unwrap_or(false),
            },
        );
    }
    for name in collection.aliases.keys() {
        validate_name(name)?;
        if collection.icons.contains_key(name) {
            return Err(InvalidCollection::DuplicateIcon { name: name.clone() });
        }
    }
    for name in collection.aliases.keys().map(String::as_str) {
        if resolved.contains_key(name) {
            continue;
        }
        let mut path = Vec::new();
        let mut visiting = BTreeSet::new();
        let mut parent = name;
        while !resolved.contains_key(parent) {
            if !visiting.insert(parent) {
                return Err(InvalidCollection::AliasCycle {
                    name: parent.to_owned(),
                });
            }
            let alias =
                collection
                    .aliases
                    .get(parent)
                    .ok_or_else(|| InvalidCollection::AliasTarget {
                        name: name.to_owned(),
                    })?;
            path.push((parent, alias));
            parent = &alias.parent;
        }
        let mut icon = resolved[parent];
        for (name, alias) in path.into_iter().rev() {
            icon = icon.apply_alias(&alias.overrides)?;
            resolved.insert(name, icon);
        }
    }
    for (character, target) in &collection.chars {
        if !character.split('-').all(|scalar| {
            !scalar.is_empty()
                && scalar.bytes().all(|byte| byte.is_ascii_hexdigit())
                && u32::from_str_radix(scalar, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .is_some()
        }) {
            return Err(InvalidCollection::CharacterKey);
        }
        if !resolved.contains_key(target.as_str()) {
            return Err(InvalidCollection::CharacterTarget);
        }
        if resolved.contains_key(character.as_str()) && character != target {
            return Err(InvalidCollection::DuplicateIcon {
                name: character.clone(),
            });
        }
    }
    let mut icons = BTreeMap::new();
    let mut shared_svg = BTreeMap::<_, SvgIcon>::new();
    let mut remaining = limit;
    for (name, icon) in resolved {
        let key = (
            icon.source_icon_name,
            [
                icon.view_box.left().to_bits(),
                icon.view_box.top().to_bits(),
                icon.view_box.width().to_bits(),
                icon.view_box.height().to_bits(),
            ],
            icon.rotation.quarter_turns(),
            icon.h_flip,
            icon.v_flip,
        );
        let svg = if let Some(svg) = shared_svg.get(&key) {
            svg.clone()
        } else {
            // Charge each variant's raw body, then its normalized SVG: normalization must not
            // hide repeated work on a body several variants share.
            remaining = remaining
                .checked_sub(icon.body.len())
                .ok_or(InvalidCollection::ExpandedTooLarge { limit })?;
            let document = icon.document().map_err(|source| InvalidCollection::Svg {
                name: name.to_owned(),
                source,
            })?;
            let serialized_len = document.serialized_len();
            remaining = remaining
                .checked_sub(serialized_len)
                .ok_or(InvalidCollection::ExpandedTooLarge { limit })?;
            let svg =
                document
                    .into_icon(serialized_len)
                    .map_err(|source| InvalidCollection::Svg {
                        name: name.to_owned(),
                        source,
                    })?;
            shared_svg.insert(key, svg.clone());
            svg
        };
        icons.insert(name.to_owned(), svg);
    }
    Ok(IconCollection {
        source_prefix: Some(collection.prefix),
        source_license: collection.metadata.and_then(|metadata| metadata.license),
        icons,
        characters: collection.chars,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IconPaint, InvalidSvg};

    #[test]
    fn every_imported_name_resolves_geometry() {
        let collection = parse(
            br#"{
            "prefix":"upstream", "left":2, "top":3, "width":20, "height":10,
            "icons":{"base":{"body":"<path fill=\"currentColor\" d=\"M2 3h20v10H2z\"/>"}},
            "aliases":{"wide":{"parent":"base","width":30},"turned":{"parent":"wide","rotate":1}},
            "chars":{"e001":"turned"}
        }"#,
        )
        .unwrap();
        assert_eq!(collection.get("base").unwrap().view_box().left(), 2.0);
        assert_eq!(collection.get("wide").unwrap().aspect_ratio(), 3.0);
        let turned = collection.get("turned").unwrap();
        assert_eq!(
            turned.view_box(),
            ViewBox::new(3.0, 2.0, 10.0, 30.0).unwrap()
        );
        assert!(turned.svg().contains("rotate(90 8 8)"));
        assert_eq!(collection.get("e001").unwrap().svg(), turned.svg());
        assert_eq!(turned.paint(), IconPaint::CurrentColor);
        assert_eq!(
            collection.names().collect::<Vec<_>>(),
            ["base", "turned", "wide"]
        );
    }

    #[test]
    fn collection_rotation_applies_to_each_icon() {
        let collection = parse(br#"{"prefix":"source","rotate":1,"icons":{"mark":{"body":"<path/>","width":12,"height":24}}}"#).unwrap();
        assert_eq!(collection.get("mark").unwrap().aspect_ratio(), 2.0);
    }

    #[test]
    fn aliases_compose_over_icon_transforms() {
        let collection = parse(
            br#"{
            "prefix":"source", "left":2, "top":3, "width":20, "height":10,
            "icons":{"mark":{"body":"<path/>","hFlip":true}},
            "aliases":{"turn":{"parent":"mark","rotate":1},"plain":{"parent":"mark","hFlip":true}}
        }"#,
        )
        .unwrap();
        assert!(collection
            .get("turn")
            .unwrap()
            .svg()
            .contains("rotate(90 5 5) translate(22 -3) scale(-1 1)"));
        let plain = collection.get("plain").unwrap();
        assert_eq!(
            plain.view_box(),
            ViewBox::new(2.0, 3.0, 20.0, 10.0).unwrap()
        );
        assert!(!plain.svg().contains("transform="));
    }

    #[test]
    fn definitions_stay_outside_the_transform() {
        let collection = parse(br##"{"prefix":"source","icons":{"mark":{"body":"<defs><linearGradient id=\"paint\"><stop stop-color=\"red\"/></linearGradient></defs><path fill=\"url(#paint)\"/>","rotate":1}}}"##).unwrap();
        let svg = collection.get("mark").unwrap().svg();
        let xml = roxmltree::Document::parse(svg).unwrap();
        let root = xml.root_element();
        let children = root
            .children()
            .filter(|child| child.is_element())
            .collect::<Vec<_>>();
        assert_eq!(children[0].tag_name().name(), "defs");
        assert_eq!(children[1].tag_name().name(), "g");
        assert_eq!(children[1].attribute("transform"), Some("rotate(90 8 8)"));
    }

    #[test]
    fn alias_failures_report_typed_errors() {
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
        assert!(matches!(parse(br#"{"prefix":"a","icons":{"x":{"body":"<path/>"}},"aliases":{"x":{"parent":"x"}}}"#), Err(InvalidCollection::DuplicateIcon { .. })));
        assert!(matches!(parse(br#"{"prefix":"a","icons":{"x":{"body":"<path/>"}},"aliases":{"a":{"parent":"x","width":1e300,"height":1e-300}}}"#), Err(InvalidCollection::Geometry)));
    }

    #[test]
    fn invalid_character_mappings_are_rejected() {
        for character in ["", "not-hex", "110000", "ffffffffffffffff"] {
            let source = serde_json::json!({"prefix":"a","icons":{"x":{"body":"<path/>"}},"chars":{character:"x"}});
            assert!(
                matches!(
                    parse(source.to_string().as_bytes()),
                    Err(InvalidCollection::CharacterKey)
                ),
                "{character}"
            );
        }
        assert!(matches!(
            parse(br#"{"prefix":"a","icons":{},"chars":{"e001":"missing"}}"#),
            Err(InvalidCollection::CharacterTarget)
        ));
        let collection = parse(br#"{"prefix":"a","icons":{"artist":{"body":"<path/>"}},"chars":{"1f9d1-200d-1f3a8":"artist"}}"#).unwrap();
        assert!(collection.get("1f9d1-200d-1f3a8").is_some());
    }

    #[test]
    fn invalid_icon_fails_the_whole_import() {
        let source =
            br#"{"prefix":"a","icons":{"good":{"body":"<path/>"},"bad":{"body":"<path"}}}"#;
        assert!(
            matches!(parse(source), Err(InvalidCollection::Svg { name, source: InvalidSvg::Xml { .. } }) if name == "bad")
        );
        let source = br#"{"prefix":"a","icons":{"bad":{"body":"<path d=\"M0\"/>"}}}"#;
        assert!(matches!(
            parse(source),
            Err(InvalidCollection::Svg {
                source: InvalidSvg::Path,
                ..
            })
        ));
    }

    #[test]
    fn expanded_import_budget_is_enforced() {
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

    #[test]
    fn import_exposes_source_metadata() {
        let collection = parse(br#"{"prefix":"source","icons":{},"info":{"license":{"title":"MIT License","spdx":"MIT","url":"https://opensource.org/license/mit"}}}"#).unwrap();
        assert_eq!(collection.source_prefix(), Some("source"));
        let license = collection.license().unwrap();
        assert_eq!(license.title(), Some("MIT License"));
        assert_eq!(license.spdx(), Some("MIT"));
        assert_eq!(license.url(), Some("https://opensource.org/license/mit"));
    }
}
