//! The transient import tree one SVG document is read into, and the validation it passes.

use std::{collections::BTreeMap, sync::Arc};

use roxmltree::Node;

use super::{geometry, InvalidSvg, SvgIcon, ValidatedSvg, ViewBox, MAX_SVG_BYTES};

mod paint;
mod properties;
mod serialize;

use paint::validate_rendered;
use properties::{parse_style, validate_attribute, ReferenceKind, StyleDeclaration};
pub(super) use serialize::ReferenceSpan;

const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";
const XLINK_NAMESPACE: &str = "http://www.w3.org/1999/xlink";
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
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
    attributes: BTreeMap<String, String>,
    declarations: BTreeMap<String, StyleDeclaration>,
    paint: paint::SpecifiedPaint,
    children: Vec<SvgChild>,
}

impl SvgElement {
    fn property(&self, name: &str) -> Option<&str> {
        self.declarations
            .get(name)
            .map(|declaration| declaration.value.as_str())
            .or_else(|| self.attributes.get(name).map(String::as_str))
            .map(str::trim)
    }

    fn is_resource_or_description(&self) -> bool {
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

#[derive(Debug)]
pub(crate) struct SvgDocument {
    elements: Vec<SvgElement>,
    identifiers: BTreeMap<String, usize>,
}

impl SvgDocument {
    pub(crate) fn from_iconify(
        body: &str,
        view_box: ViewBox,
        transform: &str,
    ) -> Result<Self, InvalidSvg> {
        let mut source = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{}" height="{}" viewBox="{view_box}">"#,
            view_box.width(),
            view_box.height(),
        );
        if source
            .len()
            .saturating_add(body.len())
            .saturating_add("</svg>".len())
            > MAX_SVG_BYTES
        {
            return Err(InvalidSvg::limit());
        }
        source.reserve_exact(body.len() + "</svg>".len());
        source.push_str(body);
        source.push_str("</svg>");
        let mut document = SvgDocument::parse(&source)?;
        if !transform.is_empty() {
            validate_attribute("g", "transform", transform)?;
            let mut definitions = Vec::new();
            let mut graphics = Vec::new();
            for child in std::mem::take(&mut document.elements[0].children) {
                if matches!(&child, SvgChild::Element(index) if matches!(document.elements[*index].name.as_str(), "defs" | "title" | "desc"))
                {
                    definitions.push(child);
                } else {
                    graphics.push(child);
                }
            }
            let wrapper = document.elements.len();
            document.elements.push(SvgElement {
                name: "g".to_owned(),
                attributes: BTreeMap::from([("transform".to_owned(), transform.to_owned())]),
                declarations: BTreeMap::new(),
                paint: paint::SpecifiedPaint::default(),
                children: graphics,
            });
            definitions.push(SvgChild::Element(wrapper));
            document.elements[0].children = definitions;
        }
        Ok(document)
    }

    pub(crate) fn into_icon(self, serialized_len: usize) -> Result<SvgIcon, InvalidSvg> {
        let document = self;
        let (view_box, aspect_ratio) = document.geometry()?;
        document.validate_references()?;
        let paint = validate_rendered(&document)?;
        if serialized_len > MAX_SVG_BYTES {
            return Err(InvalidSvg::limit());
        }
        let mut svg = String::with_capacity(serialized_len);
        let mut references = Vec::new();
        document.write(&mut svg, &mut references);
        Ok(SvgIcon(Arc::new(ValidatedSvg {
            svg: svg.into_boxed_str(),
            view_box,
            aspect_ratio,
            paint,
            longest_id: document
                .identifiers
                .keys()
                .map(String::len)
                .max()
                .unwrap_or(0),
            references: references.into_boxed_slice(),
        })))
    }

    pub(super) fn parse(source: &str) -> Result<Self, InvalidSvg> {
        let xml = roxmltree::Document::parse_with_options(
            source,
            roxmltree::ParsingOptions {
                nodes_limit: MAX_SVG_NODES,
                ..Default::default()
            },
        )
        .map_err(|error| InvalidSvg::Xml {
            line: error.pos().row,
            column: error.pos().col,
        })?;
        let root = xml.root_element();
        if root.tag_name().name() != "svg"
            || root
                .tag_name()
                .namespace()
                .is_some_and(|namespace| namespace != SVG_NAMESPACE)
        {
            return Err(InvalidSvg::Root);
        }
        if xml.descendants().any(|node| node.is_pi()) {
            return Err(InvalidSvg::Element {
                element: "processing-instruction".to_owned(),
            });
        }
        let mut document = Self {
            elements: Vec::new(),
            identifiers: BTreeMap::new(),
        };
        document.import_element(root, 0, root.tag_name().namespace().is_none())?;
        Ok(document)
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
            .map_or(!root_unqualified, |namespace| namespace != SVG_NAMESPACE)
            || !matches!(
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
        {
            return Err(InvalidSvg::Element {
                element: name.to_owned(),
            });
        }
        let index = self.elements.len();
        let mut attributes = BTreeMap::new();
        let mut declarations = BTreeMap::new();
        for attribute in node.attributes() {
            if attribute.name().len() > MAX_XML_NAME_BYTES {
                return Err(InvalidSvg::limit());
            }
            let attribute_name = match attribute.namespace() {
                None => attribute.name().to_owned(),
                Some(XLINK_NAMESPACE) if attribute.name() == "href" => {
                    if node
                        .attribute("href")
                        .is_some_and(|href| href != attribute.value())
                    {
                        return Err(InvalidSvg::Value {
                            element: name.to_owned(),
                            attribute: "href".to_owned(),
                        });
                    }
                    "href".to_owned()
                }
                Some(XML_NAMESPACE) if matches!(attribute.name(), "space" | "lang") => {
                    format!("xml:{}", attribute.name())
                }
                _ => {
                    return Err(InvalidSvg::Attribute {
                        element: name.to_owned(),
                        attribute: attribute.name().to_owned(),
                    });
                }
            };
            let value = properties::normalize_property(&attribute_name, attribute.value());
            validate_attribute(name, &attribute_name, &value)?;
            if attribute_name == "style" {
                declarations = parse_style(name, attribute.value())?;
            }
            if attribute_name == "id" {
                let id = attribute.value();
                if self.identifiers.insert(id.to_owned(), index).is_some() {
                    return Err(InvalidSvg::DuplicateId { id: id.to_owned() });
                }
            }
            attributes.insert(attribute_name, value);
        }
        self.elements.push(SvgElement {
            name: name.to_owned(),
            attributes,
            declarations,
            paint: paint::SpecifiedPaint::default(),
            children: Vec::new(),
        });
        self.elements[index].paint = paint::SpecifiedPaint::for_element(&self.elements[index]);
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
        self.elements[index].children = children;
        Ok(index)
    }

    fn geometry(&self) -> Result<(ViewBox, f64), InvalidSvg> {
        let root = &self.elements[0];
        let explicit_width = root
            .attributes
            .get("width")
            .map(|width| geometry::absolute_dimension(width))
            .transpose()?;
        let explicit_height = root
            .attributes
            .get("height")
            .map(|height| geometry::absolute_dimension(height))
            .transpose()?;
        let view_box = if let Some(value) = root.attributes.get("viewBox") {
            geometry::parse_view_box(value)?
        } else {
            ViewBox::new(
                0.0,
                0.0,
                explicit_width.ok_or(InvalidSvg::Geometry)?,
                explicit_height.ok_or(InvalidSvg::Geometry)?,
            )?
        };
        let aspect_ratio = match (explicit_width, explicit_height) {
            (Some(width), Some(height)) => ViewBox::new(0.0, 0.0, width, height)?.aspect_ratio(),
            _ => view_box.aspect_ratio(),
        };
        Ok((view_box, aspect_ratio))
    }

    fn validate_references(&self) -> Result<(), InvalidSvg> {
        let mut edges = vec![Vec::new(); self.elements.len()];
        for (index, element) in self.elements.iter().enumerate() {
            edges[index].extend(element.children.iter().filter_map(|child| match child {
                SvgChild::Element(child) => Some(*child),
                SvgChild::Text(_) => None,
            }));
            for (attribute, value) in &element.attributes {
                // Validate effective declarations, but retain the original style for rendering.
                if attribute == "style" {
                    continue;
                }
                self.add_references(index, attribute, value, &mut edges)?;
            }
            for (property, declaration) in &element.declarations {
                self.add_references(index, property, &declaration.value, &mut edges)?;
            }
        }
        let mut visits = vec![0_u8; self.elements.len()];
        let mut expansion_depth = vec![0_usize; self.elements.len()];
        for start in 0..self.elements.len() {
            if visits[start] != 0 {
                continue;
            }
            visits[start] = 1;
            let mut stack = vec![(start, 0)];
            while let Some((index, next)) = stack.last_mut() {
                let Some(&target) = edges[*index].get(*next) else {
                    expansion_depth[*index] = edges[*index]
                        .iter()
                        .map(|target| expansion_depth[*target])
                        .max()
                        .unwrap_or(0)
                        + 1;
                    if expansion_depth[*index] > MAX_SVG_DEPTH {
                        return Err(InvalidSvg::limit());
                    }
                    visits[*index] = 2;
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
        index: usize,
        attribute: &str,
        value: &str,
        edges: &mut [Vec<usize>],
    ) -> Result<(), InvalidSvg> {
        let Some(kind) = properties::reference_attribute(attribute) else {
            return Ok(());
        };
        let references = match kind {
            ReferenceKind::Id => return Ok(()),
            ReferenceKind::Fragment => vec![properties::local_reference(value)?],
            ReferenceKind::IdRefs => {
                for id in value.split_ascii_whitespace() {
                    if !self.identifiers.contains_key(id) {
                        return Err(InvalidSvg::MissingReference { id: id.to_owned() });
                    }
                }
                return Ok(());
            }
            ReferenceKind::Urls => properties::local_urls(value)?,
        };
        for id in references {
            let &target = self
                .identifiers
                .get(&id)
                .ok_or_else(|| InvalidSvg::MissingReference { id: id.clone() })?;
            let target_name = self.elements[target].name.as_str();
            let compatible = match attribute {
                "fill" | "stroke" => {
                    matches!(target_name, "linearGradient" | "radialGradient" | "pattern")
                }
                "clip-path" => target_name == "clipPath",
                "mask" => target_name == "mask",
                "filter" => target_name == "filter",
                "href" => match self.elements[index].name.as_str() {
                    "linearGradient" | "radialGradient" => {
                        matches!(target_name, "linearGradient" | "radialGradient")
                    }
                    "pattern" => target_name == "pattern",
                    "use" => !self.elements[target].is_resource_or_description(),
                    _ => false,
                },
                _ => true,
            };
            if !compatible {
                return Err(InvalidSvg::ReferenceTarget {
                    attribute: attribute.to_owned(),
                    id,
                });
            }
            edges[index].push(target);
        }
        Ok(())
    }
}
