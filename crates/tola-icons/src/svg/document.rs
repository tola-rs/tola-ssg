//! Transient SVG import tree and structural/reference validation.

use std::{collections::BTreeMap, sync::Arc};

use roxmltree::Node;

use super::{geometry, InvalidSvg, SvgIcon, ValidatedSvg, ViewBox, MAX_SVG_BYTES};

mod paint;
mod properties;
mod serialize;

use paint::validate_rendered;
use properties::{parse_style, validate_attribute, ReferenceKind, StyleDeclaration};
pub(super) use serialize::ReferenceSpan;

const SVG_NS: &str = "http://www.w3.org/2000/svg";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const MAX_SVG_NODES: u32 = 131_072;
const MAX_SVG_DEPTH: usize = 128;
const MAX_XML_NAME_BYTES: usize = 128;

#[derive(Debug)]
enum SvgChild {
    Element(usize),
    Text(String),
}

#[derive(Debug)]
struct SvgElement {
    name: String,
    attrs: BTreeMap<String, String>,
    decls: BTreeMap<String, StyleDeclaration>,
    paint: paint::SpecifiedPaint,
    children: Vec<SvgChild>,
}

impl SvgElement {
    fn prop(&self, name: &str) -> Option<&str> {
        self.decls
            .get(name)
            .map(|decl| decl.value.as_str())
            .or_else(|| self.attrs.get(name).map(String::as_str))
            .map(str::trim)
    }

    fn is_resource_or_desc(&self) -> bool {
        matches!(
            self.name.as_str(),
            "defs"
                | "linearGradient"
                | "radialGradient"
                | "stop"
                | "clipPath"
                | "mask"
                | "filter"
                | "pattern"
                | "title"
                | "desc"
        )
    }
}

fn is_allowed_element(name: &str) -> bool {
    matches!(
        name,
        "svg"
            | "g"
            | "defs"
            | "path"
            | "rect"
            | "circle"
            | "ellipse"
            | "line"
            | "polyline"
            | "polygon"
            | "linearGradient"
            | "radialGradient"
            | "stop"
            | "clipPath"
            | "mask"
            | "use"
            | "symbol"
            | "pattern"
            | "title"
            | "desc"
            | "filter"
            | "feBlend"
            | "feColorMatrix"
            | "feComponentTransfer"
            | "feComposite"
            | "feConvolveMatrix"
            | "feDiffuseLighting"
            | "feDisplacementMap"
            | "feDistantLight"
            | "feDropShadow"
            | "feFlood"
            | "feFuncA"
            | "feFuncB"
            | "feFuncG"
            | "feFuncR"
            | "feGaussianBlur"
            | "feMerge"
            | "feMergeNode"
            | "feMorphology"
            | "feOffset"
            | "fePointLight"
            | "feSpecularLighting"
            | "feSpotLight"
            | "feTile"
            | "feTurbulence"
    )
}

#[derive(Debug)]
pub(crate) struct SvgDocument {
    elements: Vec<SvgElement>,
    ids: BTreeMap<String, usize>,
}

impl SvgDocument {
    pub(crate) fn from_iconify(
        body: &str,
        vb: ViewBox,
        transform: &str,
    ) -> Result<Self, InvalidSvg> {
        let mut src = format!(
            r#"<svg xmlns="{SVG_NS}" xmlns:xlink="{XLINK_NS}" width="{}" height="{}" viewBox="{vb}">"#,
            vb.width(),
            vb.height(),
        );
        if src
            .len()
            .saturating_add(body.len())
            .saturating_add("</svg>".len())
            > MAX_SVG_BYTES
        {
            return Err(InvalidSvg::limit());
        }
        src.reserve_exact(body.len() + "</svg>".len());
        src.push_str(body);
        src.push_str("</svg>");
        let mut doc = SvgDocument::parse(&src)?;
        if !transform.is_empty() {
            validate_attribute("g", "transform", transform)?;
            let mut defs = Vec::new();
            let mut gfx = Vec::new();
            for child in std::mem::take(&mut doc.elements[0].children) {
                if matches!(&child, SvgChild::Element(idx) if matches!(doc.elements[*idx].name.as_str(), "defs" | "title" | "desc"))
                {
                    defs.push(child);
                } else {
                    gfx.push(child);
                }
            }
            let wrapper = doc.elements.len();
            doc.elements.push(SvgElement {
                name: "g".to_owned(),
                attrs: BTreeMap::from([("transform".to_owned(), transform.to_owned())]),
                decls: BTreeMap::new(),
                paint: paint::SpecifiedPaint::default(),
                children: gfx,
            });
            defs.push(SvgChild::Element(wrapper));
            doc.elements[0].children = defs;
        }
        Ok(doc)
    }

    pub(crate) fn into_icon(self, serialized_len: usize) -> Result<SvgIcon, InvalidSvg> {
        let (vb, aspect_ratio) = self.geometry()?;
        self.validate_references()?;
        let paint = validate_rendered(&self)?;
        if serialized_len > MAX_SVG_BYTES {
            return Err(InvalidSvg::limit());
        }
        let mut svg = String::with_capacity(serialized_len);
        let mut refs = Vec::new();
        self.write(&mut svg, &mut refs);
        Ok(SvgIcon(Arc::new(ValidatedSvg {
            svg: svg.into_boxed_str(),
            vb,
            aspect_ratio,
            paint,
            longest_id: self.ids.keys().map(String::len).max().unwrap_or(0),
            refs: refs.into_boxed_slice(),
        })))
    }

    pub(super) fn parse(src: &str) -> Result<Self, InvalidSvg> {
        let xml = roxmltree::Document::parse_with_options(
            src,
            roxmltree::ParsingOptions {
                nodes_limit: MAX_SVG_NODES,
                ..Default::default()
            },
        )
        .map_err(|err| InvalidSvg::Xml {
            line: err.pos().row,
            column: err.pos().col,
        })?;
        let root = xml.root_element();
        if root.tag_name().name() != "svg"
            || root.tag_name().namespace().is_some_and(|ns| ns != SVG_NS)
        {
            return Err(InvalidSvg::Root);
        }
        if xml.descendants().any(|n| n.is_pi()) {
            return Err(InvalidSvg::Element {
                element: "processing-instruction".to_owned(),
            });
        }
        let mut doc = Self {
            elements: Vec::new(),
            ids: BTreeMap::new(),
        };
        doc.import_element(root, 0, root.tag_name().namespace().is_none())?;
        Ok(doc)
    }

    fn import_element(
        &mut self,
        node: Node<'_, '_>,
        depth: usize,
        root_unqualified: bool,
    ) -> Result<usize, InvalidSvg> {
        if depth > MAX_SVG_DEPTH {
            return Err(InvalidSvg::limit());
        }
        let name = node.tag_name().name();
        if name.len() > MAX_XML_NAME_BYTES {
            return Err(InvalidSvg::limit());
        }
        if node
            .tag_name()
            .namespace()
            .map_or(!root_unqualified, |ns| ns != SVG_NS)
            || !is_allowed_element(name)
        {
            return Err(InvalidSvg::Element {
                element: name.to_owned(),
            });
        }
        let idx = self.elements.len();
        let mut attrs = BTreeMap::new();
        let mut decls = BTreeMap::new();
        for attr in node.attributes() {
            if attr.name().len() > MAX_XML_NAME_BYTES {
                return Err(InvalidSvg::limit());
            }
            let attr_name = match attr.namespace() {
                None => attr.name().to_owned(),
                Some(XLINK_NS) if attr.name() == "href" => {
                    if node.attribute("href").is_some_and(|h| h != attr.value()) {
                        return Err(InvalidSvg::Value {
                            element: name.to_owned(),
                            attribute: "href".to_owned(),
                        });
                    }
                    "href".to_owned()
                }
                Some(XML_NS) if matches!(attr.name(), "space" | "lang") => {
                    format!("xml:{}", attr.name())
                }
                _ => {
                    return Err(InvalidSvg::Attribute {
                        element: name.to_owned(),
                        attribute: attr.name().to_owned(),
                    });
                }
            };
            let val = properties::normalize_property(&attr_name, attr.value());
            validate_attribute(name, &attr_name, &val)?;
            if attr_name == "style" {
                decls = parse_style(name, attr.value())?;
            }
            if attr_name == "id" {
                let id = attr.value();
                if self.ids.insert(id.to_owned(), idx).is_some() {
                    return Err(InvalidSvg::DuplicateId { id: id.to_owned() });
                }
            }
            attrs.insert(attr_name, val);
        }
        self.elements.push(SvgElement {
            name: name.to_owned(),
            attrs,
            decls,
            paint: paint::SpecifiedPaint::default(),
            children: Vec::new(),
        });
        self.elements[idx].paint = paint::SpecifiedPaint::for_element(&self.elements[idx]);
        let mut children = Vec::new();
        for child in node.children() {
            if child.is_element() {
                children.push(SvgChild::Element(self.import_element(
                    child,
                    depth + 1,
                    root_unqualified,
                )?));
            } else if child.is_text() {
                let text = child.text().unwrap_or_default();
                if !text.trim().is_empty() && !matches!(name, "title" | "desc") {
                    return Err(InvalidSvg::Element {
                        element: "text".to_owned(),
                    });
                }
                if !text.is_empty() {
                    children.push(SvgChild::Text(text.to_owned()));
                }
            }
        }
        self.elements[idx].children = children;
        Ok(idx)
    }

    fn geometry(&self) -> Result<(ViewBox, f64), InvalidSvg> {
        let root = &self.elements[0];
        let w = root
            .attrs
            .get("width")
            .map(|v| geometry::absolute_dimension(v))
            .transpose()?;
        let h = root
            .attrs
            .get("height")
            .map(|v| geometry::absolute_dimension(v))
            .transpose()?;
        let vb = if let Some(val) = root.attrs.get("viewBox") {
            geometry::parse_view_box(val)?
        } else {
            ViewBox::new(
                0.0,
                0.0,
                w.ok_or(InvalidSvg::Geometry)?,
                h.ok_or(InvalidSvg::Geometry)?,
            )?
        };
        let aspect_ratio = match (w, h) {
            (Some(w), Some(h)) => ViewBox::new(0.0, 0.0, w, h)?.aspect_ratio(),
            _ => vb.aspect_ratio(),
        };
        Ok((vb, aspect_ratio))
    }

    fn validate_references(&self) -> Result<(), InvalidSvg> {
        let mut edges = vec![Vec::new(); self.elements.len()];
        for (idx, el) in self.elements.iter().enumerate() {
            edges[idx].extend(el.children.iter().filter_map(|child| match child {
                SvgChild::Element(c) => Some(*c),
                SvgChild::Text(_) => None,
            }));
            for (attr, val) in &el.attrs {
                // Validate parsed declarations, while keeping raw `style` text for rendering.
                if attr == "style" {
                    continue;
                }
                self.add_references(idx, attr, val, &mut edges)?;
            }
            for (prop, decl) in &el.decls {
                self.add_references(idx, prop, &decl.value, &mut edges)?;
            }
        }
        let mut visits = vec![0_u8; self.elements.len()];
        let mut depth = vec![0_usize; self.elements.len()];
        for start in 0..self.elements.len() {
            if visits[start] != 0 {
                continue;
            }
            visits[start] = 1;
            let mut stack = vec![(start, 0)];
            while let Some((idx, next)) = stack.last_mut() {
                let Some(&target) = edges[*idx].get(*next) else {
                    depth[*idx] = edges[*idx].iter().map(|t| depth[*t]).max().unwrap_or(0) + 1;
                    if depth[*idx] > MAX_SVG_DEPTH {
                        return Err(InvalidSvg::limit());
                    }
                    visits[*idx] = 2;
                    stack.pop();
                    continue;
                };
                *next += 1;
                match visits[target] {
                    1 => return Err(InvalidSvg::ReferenceCycle),
                    0 => {
                        if stack.len() >= MAX_SVG_DEPTH {
                            return Err(InvalidSvg::limit());
                        }
                        visits[target] = 1;
                        stack.push((target, 0));
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn add_references(
        &self,
        idx: usize,
        attr: &str,
        val: &str,
        edges: &mut [Vec<usize>],
    ) -> Result<(), InvalidSvg> {
        let Some(kind) = properties::reference_attribute(attr) else {
            return Ok(());
        };
        let refs = match kind {
            ReferenceKind::Id => return Ok(()),
            ReferenceKind::Fragment => vec![properties::local_reference(val)?],
            ReferenceKind::IdRefs => {
                for id in val.split_ascii_whitespace() {
                    if !self.ids.contains_key(id) {
                        return Err(InvalidSvg::MissingReference { id: id.to_owned() });
                    }
                }
                return Ok(());
            }
            ReferenceKind::Urls => properties::local_urls(val)?,
        };
        for id in refs {
            let &target = self
                .ids
                .get(&id)
                .ok_or_else(|| InvalidSvg::MissingReference { id: id.clone() })?;
            let target_name = self.elements[target].name.as_str();
            let compat = match attr {
                "fill" | "stroke" => {
                    matches!(target_name, "linearGradient" | "radialGradient" | "pattern")
                }
                "clip-path" => target_name == "clipPath",
                "mask" => target_name == "mask",
                "filter" => target_name == "filter",
                "href" => match self.elements[idx].name.as_str() {
                    "linearGradient" | "radialGradient" => {
                        matches!(target_name, "linearGradient" | "radialGradient")
                    }
                    "pattern" => target_name == "pattern",
                    "use" => !self.elements[target].is_resource_or_desc(),
                    _ => false,
                },
                _ => true,
            };
            if !compat {
                return Err(InvalidSvg::ReferenceTarget {
                    attribute: attr.to_owned(),
                    id,
                });
            }
            edges[idx].push(target);
        }
        Ok(())
    }
}
