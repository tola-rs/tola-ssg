//! Paint resolution: the sources an element's rendered content paints from.

use super::{properties::PaintValue, InvalidSvg, SvgChild, SvgDocument, SvgElement};
use crate::IconPaint;

const MAX_RENDERED_VISITS: usize = super::MAX_SVG_NODES as usize * 8;

// Only the transient import tree keeps specified paints; repeated resource expansion
// resolves inheritance from these values without reparsing CSS bytes.
#[derive(Debug, Default)]
pub(super) struct SpecifiedPaint {
    fill: Option<PaintValue>,
    stroke: Option<PaintValue>,
    stop: Option<PaintValue>,
    flood: Option<PaintValue>,
    lighting: Option<PaintValue>,
    color: Option<PaintValue>,
    controls: [ControlValue; 10],
    geometry_can_paint: bool,
    href: Option<String>,
    filter: Vec<String>,
    mask: Vec<String>,
    clip: Vec<String>,
}

impl SpecifiedPaint {
    pub(super) fn for_element(element: &SvgElement) -> Self {
        let paint = |name| element.property(name).map(parse_validated_paint);
        let urls = |name| {
            element
                .property(name)
                .map(|value| {
                    super::properties::local_urls(value).expect("validated SVG resource URLs")
                })
                .unwrap_or_default()
        };
        Self {
            fill: paint("fill"),
            stroke: paint("stroke"),
            stop: paint("stop-color"),
            flood: paint("flood-color"),
            lighting: paint("lighting-color"),
            color: paint("color"),
            href: element.attributes.get("href").map(|value| {
                super::properties::local_reference(value).expect("validated SVG href")
            }),
            filter: urls("filter"),
            mask: urls("mask"),
            clip: urls("clip-path"),
            controls: PaintControl::ALL.map(|control| control.parse(element)),
            geometry_can_paint: match element.name.as_str() {
                "path" => element
                    .attributes
                    .get("d")
                    .is_some_and(|path| !path.trim().is_empty()),
                "rect" => !["width", "height"].into_iter().any(|dimension| {
                    element.attributes.get(dimension).is_some_and(|length| {
                        length
                            .trim()
                            .parse::<svgtypes::Length>()
                            .expect("validated SVG rectangle length")
                            .number
                            == 0.0
                    })
                }),
                _ => true,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
enum ControlValue {
    Inherit,
    Initial,
    #[default]
    Unset,
    Value(bool),
}

#[derive(Clone, Copy, Debug, Default)]
struct PaintSources {
    current_color: bool,
    fixed: bool,
    changes_color: bool,
}

impl PaintSources {
    const CURRENT: Self = Self {
        current_color: true,
        fixed: false,
        changes_color: false,
    };
    const FIXED: Self = Self {
        current_color: false,
        fixed: true,
        changes_color: false,
    };

    fn combine(&mut self, other: Self) {
        self.current_color |= other.current_color;
        self.fixed |= other.fixed;
        self.changes_color |= other.changes_color;
    }

    fn classification(self) -> IconPaint {
        match (self.current_color, self.fixed || self.changes_color) {
            (true, true) => IconPaint::Mixed,
            (true, false) => IconPaint::CurrentColor,
            _ => IconPaint::Fixed,
        }
    }
}

#[derive(Clone, Debug)]
struct ComputedPaint {
    fill: PaintValue,
    stroke: PaintValue,
    stop: PaintValue,
    flood: PaintValue,
    lighting: PaintValue,
    color: PaintValue,
    visible: bool,
    fill_visible: bool,
    stroke_visible: bool,
    has_stroke_width: bool,
    displayed: bool,
    opacity_visible: bool,
    stop_visible: bool,
    flood_visible: bool,
    alpha_mask: bool,
    blends: bool,
}

impl Default for ComputedPaint {
    fn default() -> Self {
        Self {
            fill: PaintValue::Fixed,
            stroke: PaintValue::None,
            stop: PaintValue::Fixed,
            flood: PaintValue::Fixed,
            lighting: PaintValue::Fixed,
            color: PaintValue::CurrentColor,
            visible: true,
            fill_visible: true,
            stroke_visible: true,
            has_stroke_width: true,
            displayed: true,
            opacity_visible: true,
            stop_visible: true,
            flood_visible: true,
            alpha_mask: false,
            blends: false,
        }
    }
}

impl ComputedPaint {
    fn for_element(&self, element: &SvgElement) -> Self {
        let specified = &element.paint;
        let inherited = |property: &Option<PaintValue>,
                         default: &PaintValue,
                         initial: PaintValue| match property {
            None | Some(PaintValue::Inherit | PaintValue::Unset) => default.clone(),
            Some(PaintValue::Initial) => initial,
            Some(paint) => paint.clone(),
        };
        let color = match &specified.color {
            Some(color @ (PaintValue::Fixed | PaintValue::Transparent)) => color.clone(),
            Some(PaintValue::Initial) => PaintValue::Fixed,
            None | Some(PaintValue::CurrentColor | PaintValue::Inherit | PaintValue::Unset) => {
                self.color.clone()
            }
            Some(PaintValue::None | PaintValue::Resource(_)) => {
                unreachable!("the color property accepts only colors")
            }
        };
        let explicit_inheritance =
            |property: &Option<PaintValue>, parent: &PaintValue| match property {
                None | Some(PaintValue::Initial | PaintValue::Unset) => PaintValue::Fixed,
                Some(PaintValue::Inherit) => parent.clone(),
                Some(paint) => paint.clone(),
            };
        Self {
            fill: inherited(&specified.fill, &self.fill, PaintValue::Fixed),
            stroke: inherited(&specified.stroke, &self.stroke, PaintValue::None),
            stop: explicit_inheritance(&specified.stop, &self.stop),
            flood: explicit_inheritance(&specified.flood, &self.flood),
            lighting: explicit_inheritance(&specified.lighting, &self.lighting),
            color,
            visible: PaintControl::Visibility.resolve(element, self.visible),
            fill_visible: PaintControl::FillOpacity.resolve(element, self.fill_visible),
            stroke_visible: PaintControl::StrokeOpacity.resolve(element, self.stroke_visible),
            has_stroke_width: PaintControl::StrokeWidth.resolve(element, self.has_stroke_width),
            displayed: PaintControl::Display.resolve(element, self.displayed),
            opacity_visible: PaintControl::Opacity.resolve(element, self.opacity_visible),
            stop_visible: PaintControl::StopOpacity.resolve(element, self.stop_visible),
            flood_visible: PaintControl::FloodOpacity.resolve(element, self.flood_visible),
            alpha_mask: PaintControl::MaskType.resolve(element, self.alpha_mask),
            blends: PaintControl::Blend.resolve(element, self.blends),
        }
    }

    fn resolve_color<'a>(&'a self, paint: &'a PaintValue) -> &'a PaintValue {
        match paint {
            PaintValue::CurrentColor => &self.color,
            _ => paint,
        }
    }
}

#[derive(Clone, Copy)]
enum PaintControl {
    Visibility,
    FillOpacity,
    StrokeOpacity,
    StrokeWidth,
    Display,
    Opacity,
    StopOpacity,
    FloodOpacity,
    MaskType,
    Blend,
}

impl PaintControl {
    /// Declaration order, which is also the index order of `SpecifiedPaint::controls`.
    const ALL: [Self; 10] = [
        Self::Visibility,
        Self::FillOpacity,
        Self::StrokeOpacity,
        Self::StrokeWidth,
        Self::Display,
        Self::Opacity,
        Self::StopOpacity,
        Self::FloodOpacity,
        Self::MaskType,
        Self::Blend,
    ];

    fn parse(self, element: &SvgElement) -> ControlValue {
        let name = match self {
            Self::Visibility => "visibility",
            Self::FillOpacity => "fill-opacity",
            Self::StrokeOpacity => "stroke-opacity",
            Self::StrokeWidth => "stroke-width",
            Self::Display => "display",
            Self::Opacity => "opacity",
            Self::StopOpacity => "stop-opacity",
            Self::FloodOpacity => "flood-opacity",
            Self::MaskType => "mask-type",
            Self::Blend => "mix-blend-mode",
        };
        match element.property(name) {
            Some("inherit") => ControlValue::Inherit,
            Some("initial") => ControlValue::Initial,
            None | Some("unset") => ControlValue::Unset,
            Some(value) => ControlValue::Value(match self {
                Self::Visibility => value == "visible",
                Self::Display => value != "none",
                Self::MaskType => value == "alpha",
                Self::Blend => value != "normal",
                Self::StrokeWidth => {
                    value
                        .parse::<svgtypes::Length>()
                        .expect("validated SVG stroke width")
                        .number
                        > 0.0
                }
                _ => positive_opacity(value),
            }),
        }
    }

    fn resolve(self, element: &SvgElement, parent: bool) -> bool {
        let initial = !matches!(self, Self::MaskType | Self::Blend);
        match element.paint.controls[self as usize] {
            ControlValue::Inherit => parent,
            ControlValue::Initial => initial,
            ControlValue::Unset => {
                if matches!(
                    self,
                    Self::Visibility | Self::FillOpacity | Self::StrokeOpacity | Self::StrokeWidth
                ) {
                    parent
                } else {
                    initial
                }
            }
            ControlValue::Value(value) => value,
        }
    }
}

fn parse_validated_paint(source: &str) -> PaintValue {
    super::properties::parse_paint(source).expect("SVG paint was validated at import")
}

// One cumulative budget covers every rendered reference role, including geometry-only clips.
// The document graph separately proves target compatibility, acyclicity and maximum depth.
struct RenderTraversal<'a> {
    document: &'a SvgDocument,
    computed: Vec<ComputedPaint>,
    remaining: usize,
}

pub(super) fn validate_rendered(document: &SvgDocument) -> Result<IconPaint, InvalidSvg> {
    let mut traversal = RenderTraversal {
        document,
        computed: vec![ComputedPaint::default(); document.elements.len()],
        remaining: MAX_RENDERED_VISITS,
    };
    traversal.inherit(0, &ComputedPaint::default());
    Ok(traversal
        .rendered(0, &ComputedPaint::default(), false)?
        .classification())
}

impl RenderTraversal<'_> {
    fn visit(&mut self) -> Result<(), InvalidSvg> {
        self.remaining = self.remaining.checked_sub(1).ok_or(InvalidSvg::limit())?;
        Ok(())
    }

    fn inherit(&mut self, index: usize, parent: &ComputedPaint) {
        let element = &self.document.elements[index];
        let inherited = parent.for_element(element);
        self.computed[index] = inherited.clone();
        for child in &element.children {
            if let SvgChild::Element(child) = child {
                self.inherit(*child, &inherited);
            }
        }
    }

    fn rendered(
        &mut self,
        index: usize,
        parent: &ComputedPaint,
        instanced: bool,
    ) -> Result<PaintSources, InvalidSvg> {
        self.visit()?;
        let element = &self.document.elements[index];
        let inherited = parent.for_element(element);
        if element.is_resource_or_description()
            || (element.name == "symbol" && !instanced)
            || !inherited.displayed
            || !inherited.opacity_visible
        {
            return Ok(PaintSources::default());
        }
        let mut sources = PaintSources::default();
        if element.name == "use" {
            if let Some(id) = &element.paint.href {
                let target = self.document.identifiers[id];
                sources.combine(self.rendered(target, &inherited, true)?);
            }
        } else if inherited.visible
            && matches!(
                element.name.as_str(),
                "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon"
            )
        {
            // Non-drawing shapes still traverse paint servers to preserve the import work budget.
            let mut shape_sources = PaintSources::default();
            if element.name != "line" && inherited.fill_visible {
                shape_sources.combine(self.paint(inherited.resolve_color(&inherited.fill))?);
            }
            if inherited.stroke_visible && inherited.has_stroke_width {
                shape_sources.combine(self.paint(inherited.resolve_color(&inherited.stroke))?);
            }
            if element.paint.geometry_can_paint {
                sources.combine(shape_sources);
            }
        }
        for child in &element.children {
            self.visit()?;
            if let SvgChild::Element(child) = child {
                sources.combine(self.rendered(*child, &inherited, false)?);
            }
        }
        if !element.paint.filter.is_empty() {
            for id in &element.paint.filter {
                let filter_sources = self.filter(self.document.identifiers[id])?;
                sources.combine(filter_sources);
            }
            // A filter can transform both RGB and alpha; currentColor alone does not prove
            // equivalence between an SVG image and a color-following CSS mask.
            sources.changes_color = true;
        }
        if inherited.blends {
            sources.changes_color = true;
        }
        for id in &element.paint.mask {
            let target = self.document.identifiers[id];
            let inherited = self.computed[target].clone();
            let mask_sources = self.resource_children(target, &inherited)?;
            if mask_sources.current_color
                && (!self.computed[target].alpha_mask || mask_sources.changes_color)
            {
                sources.current_color = true;
                sources.changes_color = true;
            }
        }
        self.clipping(index)?;
        Ok(sources)
    }

    fn clipping(&mut self, index: usize) -> Result<(), InvalidSvg> {
        for id in &self.document.elements[index].paint.clip {
            let target = self.document.identifiers[id];
            let inherited = self.computed[target].clone();
            self.clip_geometry(target, &inherited)?;
        }
        Ok(())
    }

    // Clip silhouettes depend on geometry, visibility and nested clipping, not fill/stroke,
    // opacity, masks or filters. Walking them as painted content would charge irrelevant edges.
    fn clip_geometry(&mut self, index: usize, parent: &ComputedPaint) -> Result<(), InvalidSvg> {
        self.visit()?;
        let element = &self.document.elements[index];
        let inherited = parent.for_element(element);
        if !inherited.displayed
            || matches!(
                element.name.as_str(),
                "defs"
                    | "linearGradient"
                    | "radialGradient"
                    | "stop"
                    | "mask"
                    | "filter"
                    | "pattern"
                    | "title"
                    | "desc"
            )
        {
            return Ok(());
        }
        if element.name == "use" {
            if let Some(id) = &element.paint.href {
                self.clip_geometry(self.document.identifiers[id], &inherited)?;
            }
        }
        for child in &element.children {
            self.visit()?;
            if let SvgChild::Element(child) = child {
                self.clip_geometry(*child, &inherited)?;
            }
        }
        self.clipping(index)
    }

    fn paint(&mut self, paint: &PaintValue) -> Result<PaintSources, InvalidSvg> {
        match paint {
            PaintValue::None | PaintValue::Transparent => Ok(PaintSources::default()),
            PaintValue::CurrentColor => Ok(PaintSources::CURRENT),
            PaintValue::Fixed => Ok(PaintSources::FIXED),
            PaintValue::Inherit | PaintValue::Initial | PaintValue::Unset => {
                unreachable!("CSS-wide paints are resolved at their owning property")
            }
            PaintValue::Resource(id) => {
                let index = self.document.identifiers[id.as_ref()];
                match self.document.elements[index].name.as_str() {
                    "linearGradient" | "radialGradient" => self.gradient(index),
                    "pattern" => self.pattern(index),
                    _ => unreachable!("paint server target was validated"),
                }
            }
        }
    }

    fn gradient(&mut self, index: usize) -> Result<PaintSources, InvalidSvg> {
        let inherited = self.computed[index].clone();
        let content = self.template_content(index)?;
        let mut sources = PaintSources::default();
        let mut visible = false;
        for child in &self.document.elements[content].children {
            self.visit()?;
            if let SvgChild::Element(child) = child {
                let stop = &self.document.elements[*child];
                if stop.name != "stop" {
                    continue;
                }
                let computed = inherited.for_element(stop);
                let paint = computed.resolve_color(&computed.stop);
                visible |= computed.stop_visible
                    && !matches!(paint, PaintValue::None | PaintValue::Transparent);
                // SVG interpolates non-premultiplied colors, so even a transparent stop
                // contributes RGB whenever another stop supplies visible alpha.
                sources.combine(match paint {
                    PaintValue::Transparent => PaintSources::FIXED,
                    _ => self.paint(paint)?,
                });
            }
        }
        Ok(if visible {
            sources
        } else {
            PaintSources::default()
        })
    }

    fn pattern(&mut self, index: usize) -> Result<PaintSources, InvalidSvg> {
        let inherited = self.computed[index].clone();
        let content = self.template_content(index)?;
        self.resource_children(content, &inherited)
    }

    // SVG paint-server templates contribute child content, not the template's computed styles.
    // Descriptive children do not suppress templating; referenced content inherits from its host.
    fn template_content(&mut self, mut index: usize) -> Result<usize, InvalidSvg> {
        loop {
            self.visit()?;
            let element = &self.document.elements[index];
            for child in &element.children {
                self.visit()?;
                if matches!(child, SvgChild::Element(child)
                    if !matches!(self.document.elements[*child].name.as_str(), "title" | "desc"))
                {
                    return Ok(index);
                }
            }
            let Some(id) = &element.paint.href else {
                return Ok(index);
            };
            index = self.document.identifiers[id];
        }
    }

    fn resource_children(
        &mut self,
        index: usize,
        inherited: &ComputedPaint,
    ) -> Result<PaintSources, InvalidSvg> {
        let mut sources = PaintSources::default();
        for child in &self.document.elements[index].children {
            self.visit()?;
            if let SvgChild::Element(child) = child {
                sources.combine(self.rendered(*child, inherited, false)?);
            }
        }
        Ok(sources)
    }

    fn filter(&mut self, index: usize) -> Result<PaintSources, InvalidSvg> {
        self.visit()?;
        let mut sources = PaintSources::default();
        for child in &self.document.elements[index].children {
            self.visit()?;
            let SvgChild::Element(child) = child else {
                continue;
            };
            let primitive = &self.document.elements[*child];
            let computed = &self.computed[*child];
            let paint = match primitive.name.as_str() {
                "feFlood" | "feDropShadow" if computed.flood_visible => {
                    Some(computed.resolve_color(&computed.flood).clone())
                }
                "feDiffuseLighting" | "feSpecularLighting" => {
                    Some(computed.resolve_color(&computed.lighting).clone())
                }
                _ => None,
            };
            if let Some(paint) = paint {
                sources.combine(self.paint(&paint)?);
            }
        }
        Ok(sources)
    }
}

fn positive_opacity(value: &str) -> bool {
    value
        .trim_end_matches('%')
        .parse::<f64>()
        .expect("validated SVG opacity")
        > 0.0
}

#[cfg(test)]
mod tests {
    use super::{ComputedPaint, RenderTraversal, SvgDocument};
    use crate::{IconPaint, InvalidSvg, SvgIcon};

    fn paint(body: &str) -> IconPaint {
        SvgIcon::parse(format!(r#"<svg viewBox="0 0 24 24">{body}</svg>"#))
            .unwrap()
            .paint()
    }

    /// A parsed document holding `body` inside the 24x24 viewBox this module's tests use.
    fn svg_document(body: &str) -> SvgDocument {
        SvgDocument::parse(&format!(r#"<svg viewBox="0 0 24 24">{body}</svg>"#)).unwrap()
    }

    /// A traversal of `document` with `remaining` reference expansions left and the root inherited.
    fn render_traversal(document: &SvgDocument, remaining: usize) -> RenderTraversal<'_> {
        let mut traversal = RenderTraversal {
            document,
            computed: vec![ComputedPaint::default(); document.elements.len()],
            remaining,
        };
        traversal.inherit(0, &ComputedPaint::default());
        traversal
    }

    #[test]
    fn empty_geometry_contributes_no_paint() {
        for shape in [
            r#"<path fill="red" stroke="red"/>"#,
            r#"<path d="" fill="red" stroke="red"/>"#,
            r#"<path d=" &#9;&#10; " fill="red" stroke="red"/>"#,
            r#"<rect width="0" height="12" fill="red" stroke="red"/>"#,
            r#"<rect width="12" height="0" fill="red" stroke="red"/>"#,
            r#"<rect width="0px" height="12" fill="red"/>"#,
            r#"<rect width="0%" height="12" fill="red"/>"#,
            r#"<rect width="0em" height="12" fill="red"/>"#,
            r#"<rect width="-0cm" height="12" fill="red"/>"#,
            r#"<rect width="0e2" height="12" fill="red"/>"#,
        ] {
            assert_eq!(
                paint(&format!(
                    r#"{shape}<path d="M0 0h12v12z" fill="currentColor"/>"#
                )),
                IconPaint::CurrentColor,
                "{shape}",
            );
        }
    }

    #[test]
    fn stroked_paths_keep_their_paint() {
        for path in ["M0 0L12 0", "M0 0L0 0"] {
            assert_eq!(
                paint(&format!(
                    r#"<path d="{path}" fill="none" stroke="red" stroke-linecap="round"/><path d="M0 0h12v12z" fill="currentColor"/>"#
                )),
                IconPaint::Mixed,
                "{path}",
            );
        }
    }

    #[test]
    fn empty_geometry_keeps_resource_effects() {
        for shape in [
            r##"<path filter="url(#effect)"/>"##,
            r##"<rect width="0" height="12" filter="url(#effect)"/>"##,
        ] {
            assert_eq!(
                paint(&format!(
                    r##"<defs><filter id="effect"><feFlood flood-color="red"/></filter></defs>{shape}<path d="M0 0h12v12z" fill="currentColor"/>"##
                )),
                IconPaint::Mixed,
                "{shape}",
            );
        }
    }

    #[test]
    fn empty_geometry_keeps_reference_budget() {
        for shape in [
            r##"<path fill="url(#paint)"/>"##,
            r##"<rect width="0" height="12" fill="url(#paint)"/>"##,
        ] {
            let document = svg_document(&format!(
                r##"<defs><linearGradient id="paint"><stop/><stop/><stop/></linearGradient></defs>{shape}"##
            ));
            document.validate_references().unwrap();
            let mut traversal = render_traversal(&document, 6);
            assert!(matches!(
                traversal.rendered(0, &ComputedPaint::default(), false),
                Err(InvalidSvg::Limit { .. })
            ));
        }
    }

    #[test]
    fn color_function_alpha_affects_paint() {
        for components in ["rgb(255 0 0", "oklch(60% .2 40"] {
            for (alpha, expected) in [
                ("0", IconPaint::CurrentColor),
                ("none", IconPaint::CurrentColor),
                (".5", IconPaint::Mixed),
            ] {
                assert_eq!(
                    paint(&format!(
                        r#"<path d="M0 0h12v12z" fill="currentColor"/><path d="M0 0h12v12z" fill="{components} / {alpha})"/>"#
                    )),
                    expected,
                    "{components} / {alpha})"
                );
            }
        }
    }

    #[test]
    fn template_paints_resolve_per_host() {
        let body = r##"<defs><linearGradient id="base"><stop stop-color="currentColor"/><stop offset="1" stop-color="currentColor" stop-opacity="0"/></linearGradient><linearGradient id="fixed" href="#base" color="red"/><linearGradient id="dynamic" href="#base"/><path id="shape" d="M0 0h12v12z"/></defs><use href="#shape" fill="url(#dynamic)"/><use href="#shape" fill="url(#fixed)"/>"##;
        assert_eq!(paint(body), IconPaint::Mixed);
        assert_eq!(
            paint(&body.replace("url(#fixed)", "url(#dynamic)")),
            IconPaint::CurrentColor
        );
    }

    #[test]
    fn transparent_stops_contribute_color() {
        for transparent_stop in [
            r#"stop-color="rgba(255, 0, 0, 0)""#,
            r#"stop-color="currentColor" color="rgba(255, 0, 0, 0)""#,
        ] {
            assert_eq!(
                paint(&format!(
                    r##"<defs><linearGradient id="paint"><stop stop-color="currentColor"/><stop offset="1" {transparent_stop}/></linearGradient></defs><rect width="24" height="24" fill="url(#paint)"/>"##
                )),
                IconPaint::Mixed,
                "{transparent_stop}",
            );
        }
        assert_eq!(
            paint(
                r##"<defs><linearGradient id="paint"><stop stop-color="currentColor" stop-opacity="0"/><stop offset="1" stop-color="red"/></linearGradient></defs><rect width="24" height="24" fill="url(#paint)"/>"##
            ),
            IconPaint::Mixed,
        );
    }

    #[test]
    fn transparent_stops_follow_inheritance() {
        let definitions = r##"<defs><linearGradient id="base" stop-color="rgba(255, 0, 0, 0)"><stop stop-color="currentColor"/><stop offset="1" stop-color="inherit"/></linearGradient><linearGradient id="paint" href="#base" stop-color="rgba(255, 0, 0, 0)"/></defs>"##;
        for resource in ["base", "paint"] {
            assert_eq!(
                paint(&format!(
                    r##"{definitions}<rect width="24" height="24" fill="url(#{resource})"/>"##
                )),
                IconPaint::Mixed,
                "{resource}",
            );
        }
        let templates = r##"<defs><linearGradient id="base"><stop stop-color="currentColor"/><stop offset="1" stop-color="rgba(255, 0, 0, 0)"/></linearGradient><linearGradient id="paint" href="#base" color="rgba(0, 0, 255, 0)"/></defs>"##;
        assert_eq!(
            paint(&format!(
                r##"{templates}<path d="M0 0h12v12z" fill="currentColor"/><rect width="24" height="24" fill="url(#paint)"/>"##
            )),
            IconPaint::CurrentColor,
        );
        assert_eq!(
            paint(&format!(
                r##"{templates}<rect width="24" height="24" fill="url(#base)"/>"##
            )),
            IconPaint::Mixed,
        );
    }

    #[test]
    fn transparent_paint_does_not_fix_colors() {
        for stops in [
            r#"<stop stop-color="red" stop-opacity="0"/><stop offset="1" stop-color="currentColor" stop-opacity="0"/>"#,
            r#"<stop stop-color="rgba(255, 0, 0, 0)"/><stop offset="1" stop-color="currentColor" color="transparent"/>"#,
        ] {
            assert_eq!(
                paint(&format!(
                    r##"<defs><linearGradient id="paint">{stops}</linearGradient></defs><path d="M0 0h12v12z" fill="currentColor"/><rect width="24" height="24" fill="url(#paint)"/>"##
                )),
                IconPaint::CurrentColor,
                "{stops}",
            );
        }
        assert_eq!(
            paint(
                r##"<defs><linearGradient id="unused"><stop stop-color="currentColor"/><stop offset="1" stop-color="rgba(255, 0, 0, 0)"/></linearGradient></defs><path d="M0 0h12v12z" fill="currentColor"/><g color="rgba(255, 0, 0, 0)"><path d="M0 0h12v12z" fill="currentColor" stroke="currentColor"/></g><path d="M0 0h12v12z" fill="transparent" stroke="#f000"/>"##
            ),
            IconPaint::CurrentColor,
        );
    }

    #[test]
    fn paint_server_expansion_is_budgeted() {
        for (body, budget) in [
            (
                r#"<linearGradient id="resource"><stop/><stop/><stop/></linearGradient>"#,
                3,
            ),
            (
                r#"<filter id="resource"><feFlood/><feFlood/><feFlood/></filter>"#,
                2,
            ),
            (
                r#"<pattern id="resource"><title>A</title><desc>B</desc></pattern>"#,
                2,
            ),
        ] {
            let document = svg_document(body);
            let mut traversal = render_traversal(&document, budget);
            let index = document.identifiers["resource"];
            let expansion = match document.elements[index].name.as_str() {
                "linearGradient" => traversal.gradient(index),
                "filter" => traversal.filter(index),
                "pattern" => traversal.pattern(index),
                _ => unreachable!(),
            };
            assert!(matches!(expansion, Err(InvalidSvg::Limit { .. })), "{body}");
        }
    }

    #[test]
    fn current_color_follows_css_keywords() {
        assert_eq!(
            paint(r#"<g fill="CURRENTCOLOR"><path d="M0 0h12v12z"/></g>"#),
            IconPaint::CurrentColor
        );
        assert_eq!(
            paint(r#"<g color="red"><path d="M0 0h12v12z" fill="currentColor"/></g>"#),
            IconPaint::Fixed
        );
        assert_eq!(
            paint(r#"<g fill="none" stroke="currentColor"><path d="M0 0L12 12"/></g>"#),
            IconPaint::CurrentColor
        );
    }

    #[test]
    fn paint_sources_decide_the_classification() {
        assert_eq!(
            paint(
                r#"<path d="M0 0h12v12z" fill="currentColor"/><path d="M0 0h12v12z" fill="red"/>"#
            ),
            IconPaint::Mixed
        );
        assert_eq!(
            paint(r#"<path d="M0 0h12v12z" fill="currentColor"/><path d="M0 0h12v12z"/>"#),
            IconPaint::Mixed
        );
        assert_eq!(
            paint(r#"<path d="M0 0h12v12z" fill="red"/><path d="M0 0h12v12z" fill="blue"/>"#),
            IconPaint::Fixed
        );
        assert_eq!(
            paint(r#"<title>currentColor</title><path d="M0 0h12v12z" fill="red"/>"#),
            IconPaint::Fixed
        );
    }

    #[test]
    fn hidden_paint_does_not_affect_output() {
        assert_eq!(
            paint(
                r#"<defs><path id="unused" d="M0 0h12v12z" fill="currentColor"/></defs><path d="M0 0h12v12z" fill="red"/>"#
            ),
            IconPaint::Fixed
        );
        assert_eq!(
            paint(r#"<path d="M0 0h12v12z" fill="currentColor" style="fill: red"/>"#),
            IconPaint::Fixed
        );
        assert_eq!(
            paint(r#"<path d="M0 0h12v12z" style="fill: currentColor !important; fill: red"/>"#),
            IconPaint::CurrentColor
        );
        assert_eq!(
            paint(
                r#"<path d="M0 0h12v12z" fill="currentColor"/><g fill-opacity="0"><path d="M0 0h12v12z" fill="red"/></g>"#
            ),
            IconPaint::CurrentColor
        );
        assert_eq!(
            paint(
                r#"<g fill="currentColor" stroke="red" stroke-width="0px"><path d="M0 0h12v12z"/></g>"#
            ),
            IconPaint::CurrentColor
        );
        assert_eq!(
            paint(
                r#"<path d="M0 0h12v12z" fill="currentColor"/><path d="M0 0h12v12z" fill="red" opacity="0"/>"#
            ),
            IconPaint::CurrentColor
        );
    }

    #[test]
    fn reachable_resources_contribute_paint() {
        assert_eq!(
            paint(
                r##"<defs><linearGradient id="paint"><stop stop-color="currentColor"/><stop offset="1" stop-color="red"/></linearGradient></defs><path d="M0 0h12v12z" fill="url(#paint)"/>"##
            ),
            IconPaint::Mixed
        );
        assert_eq!(
            paint(
                r##"<defs><path id="mark" d="M0 0h12v12z" fill="currentColor"/></defs><use href="#mark"/>"##
            ),
            IconPaint::CurrentColor
        );
        assert_eq!(
            paint(
                r##"<defs><path id="mark" d="M0 0h12v12z"/></defs><use href="#mark" fill="currentColor"/>"##
            ),
            IconPaint::CurrentColor
        );
    }

    #[test]
    fn css_wide_keywords_resolve_per_property() {
        for attribute in [
            r#"visibility="HIDDEN""#,
            r#"style="visibility: h\69 dden""#,
            r#"display=" NONE ""#,
        ] {
            assert_eq!(
                paint(&format!(
                    r#"<path d="M0 0h12v12z" fill="currentColor"/><path d="M0 0h12v12z" fill="red" {attribute}/>"#
                )),
                IconPaint::CurrentColor,
                "{attribute}",
            );
        }
        assert_eq!(
            paint(
                r#"<g visibility="hidden"><path d="M0 0h12v12z" fill="red" visibility="inherit"/><path d="M0 0h12v12z" fill="currentColor" visibility="initial"/></g>"#
            ),
            IconPaint::CurrentColor,
        );
        assert_eq!(
            paint(
                r#"<g fill-opacity="0"><path d="M0 0h12v12z" fill="red" fill-opacity="inherit"/><path d="M0 0h12v12z" fill="currentColor" fill-opacity="initial"/></g>"#
            ),
            IconPaint::CurrentColor,
        );
        assert_eq!(
            paint(
                r#"<g fill="none" stroke="red" stroke-width="0"><path d="M0 0L12 12" stroke-width="inherit"/><path d="M0 0L12 12" stroke="currentColor" stroke-width="initial"/></g>"#
            ),
            IconPaint::CurrentColor,
        );
    }

    #[test]
    fn unset_differs_from_inherit_for_resources() {
        assert_eq!(
            paint(
                r##"<defs><linearGradient id="paint" stop-opacity="0"><stop stop-color="red" stop-opacity="inherit"/><stop offset="1" stop-color="currentColor" stop-opacity="unset"/></linearGradient></defs><path d="M0 0h12v12z" fill="url(#paint)"/>"##
            ),
            IconPaint::Mixed,
        );
        assert_eq!(
            paint(
                r##"<defs mask-type="alpha"><mask id="cutout" mask-type="inherit"><path d="M0 0h12v12z" fill="currentColor"/></mask></defs><path d="M0 0h12v12z" fill="currentColor" mask="url(#cutout)"/>"##
            ),
            IconPaint::CurrentColor,
        );
        assert_eq!(
            paint(
                r##"<defs mask-type="alpha"><mask id="cutout" mask-type="unset"><path d="M0 0h12v12z" fill="currentColor"/></mask></defs><path d="M0 0h12v12z" fill="currentColor" mask="url(#cutout)"/>"##
            ),
            IconPaint::Mixed,
        );
    }

    #[test]
    fn alpha_masks_keep_color_dependencies() {
        assert_eq!(
            paint(
                r##"<defs><filter id="alpha"><feColorMatrix type="luminanceToAlpha"/></filter><mask id="cutout" mask-type="alpha"><path d="M0 0h12v12z" fill="currentColor" filter="url(#alpha)"/></mask></defs><path d="M0 0h12v12z" fill="currentColor" mask="url(#cutout)"/>"##
            ),
            IconPaint::Mixed
        );
        assert_eq!(
            paint(
                r##"<defs><mask id="inner"><path d="M0 0h12v12z" fill="currentColor"/></mask><mask id="outer" mask-type="alpha"><path d="M0 0h12v12z" fill="currentColor" mask="url(#inner)"/></mask></defs><path d="M0 0h12v12z" fill="currentColor" mask="url(#outer)"/>"##
            ),
            IconPaint::Mixed
        );
        assert_eq!(
            paint(
                r##"<defs><mask id="cutout" mask-type="alpha"><path d="M0 0h12v12z" fill="currentColor"/></mask></defs><path d="M0 0h12v12z" fill="currentColor" mask="url(#cutout)"/>"##
            ),
            IconPaint::CurrentColor
        );
    }

    #[test]
    fn clipping_shares_the_rendered_budget() {
        fn paint_with_budget(body: &str, budget: usize) -> Result<IconPaint, InvalidSvg> {
            let document = svg_document(body);
            document.validate_references()?;
            Ok(render_traversal(&document, budget)
                .rendered(0, &ComputedPaint::default(), false)?
                .classification())
        }
        let definitions = r##"<defs><path id="shape" d="M0 0h12v12z"/><clipPath id="base"><use href="#shape"/><use href="#shape"/></clipPath><clipPath id="nested" clip-path="url(#base)"><use href="#shape"/></clipPath></defs>"##;
        let one = format!(
            r##"{definitions}<path d="M0 0h12v12z" fill="currentColor" clip-path="url(#nested)"/>"##
        );
        assert_eq!(
            paint_with_budget(&one, 24).unwrap(),
            IconPaint::CurrentColor
        );
        let repeated = format!(
            r##"{one}<path d="M0 0h12v12z" fill="currentColor" clip-path="url(#nested)"/>"##
        );
        assert!(matches!(
            paint_with_budget(&repeated, 24),
            Err(InvalidSvg::Limit { .. })
        ));
        let overridden = format!(
            r##"{one}<path d="M0 0h12v12z" fill="currentColor" clip-path="url(#nested)" style="clip-path:none"/>"##
        );
        assert_eq!(
            paint_with_budget(&overridden, 24).unwrap(),
            IconPaint::CurrentColor
        );
        let unused = format!(r##"{definitions}<path d="M0 0h12v12z" fill="currentColor"/>"##);
        assert_eq!(
            paint_with_budget(&unused, 5).unwrap(),
            IconPaint::CurrentColor
        );
        let geometry_only = r##"<defs><linearGradient id="paint"><stop/><stop/><stop/></linearGradient><clipPath id="clip"><path d="M0 0h12v12z" fill="url(#paint)" opacity="0"/></clipPath></defs><path d="M0 0h12v12z" fill="currentColor" clip-path="url(#clip)"/>"##;
        assert_eq!(
            paint_with_budget(geometry_only, 8).unwrap(),
            IconPaint::CurrentColor
        );
    }

    #[test]
    fn blend_mode_resolution_uses_inheritance() {
        for mode in ["normal", "inherit", "INITIAL"] {
            assert_eq!(
                paint(&format!(
                    r#"<path d="M0 0h12v12z" fill="currentColor" style="mix-blend-mode:{mode}"/>"#
                )),
                IconPaint::CurrentColor,
            );
        }
        assert_eq!(
            paint(
                r##"<defs style="mix-blend-mode:multiply"><pattern id="tiles"><path d="M0 0h12v12z" fill="currentColor" style="mix-blend-mode:inherit"/></pattern></defs><path d="M0 0h12v12z" fill="url(#tiles)"/>"##
            ),
            IconPaint::CurrentColor,
        );
        assert_eq!(
            paint(
                r##"<defs><pattern id="tiles" style="mix-blend-mode:multiply"><path d="M0 0h12v12z" fill="currentColor" style="mix-blend-mode:inherit"/></pattern></defs><path d="M0 0h12v12z" fill="url(#tiles)"/>"##
            ),
            IconPaint::Mixed,
        );
    }

    #[test]
    fn paint_servers_inherit_from_their_host() {
        assert_eq!(
            paint(
                r##"<defs><linearGradient id="base"><stop stop-color="currentColor"/></linearGradient><linearGradient id="paint" href="#base" color="red"/></defs><path d="M0 0h12v12z" fill="url(#paint)"/>"##
            ),
            IconPaint::Fixed,
        );
        assert_eq!(
            paint(
                r##"<defs><linearGradient id="base" color="red"><stop stop-color="currentColor"/></linearGradient><linearGradient id="paint" href="#base"/></defs><path d="M0 0h12v12z" fill="url(#paint)"/>"##
            ),
            IconPaint::CurrentColor,
        );
        assert_eq!(
            paint(
                r##"<defs><pattern id="base"><path d="M0 0h12v12z" fill="currentColor"/></pattern><pattern id="paint" href="#base" color="red"><title>Tile</title></pattern></defs><path d="M0 0h12v12z" fill="url(#paint)"/>"##
            ),
            IconPaint::Fixed,
        );
        assert_eq!(
            paint(
                r##"<defs><pattern id="base" color="red"><path d="M0 0h12v12z" fill="currentColor"/></pattern><pattern id="middle" href="#base" color="blue"><desc>Shared tile</desc></pattern><pattern id="paint" href="#middle"><title>Tile</title></pattern></defs><path d="M0 0h12v12z" fill="url(#paint)"/>"##
            ),
            IconPaint::CurrentColor,
        );
        assert_eq!(
            paint(
                r##"<defs><linearGradient id="base"><stop stop-color="red"/></linearGradient><linearGradient id="paint" href="#base"><stop stop-color="currentColor"/></linearGradient></defs><path d="M0 0h12v12z" fill="url(#paint)"/>"##
            ),
            IconPaint::CurrentColor,
        );
    }

    #[test]
    fn inherited_keywords_stay_dynamic() {
        assert_eq!(
            paint(
                r##"<defs><linearGradient id="paint" stop-color="currentColor"><stop stop-color="unset"/></linearGradient></defs><path d="M0 0h12v12z" fill="url(#paint)"/>"##
            ),
            IconPaint::Fixed
        );
        assert_eq!(
            paint(r#"<g fill="currentColor"><path d="M0 0h12v12z" fill="initial"/></g>"#),
            IconPaint::Fixed
        );
        assert_eq!(
            paint(
                r##"<defs><linearGradient id="paint" stop-color="currentColor"><stop stop-color="inherit"/></linearGradient></defs><path d="M0 0h12v12z" fill="url(#paint)"/>"##
            ),
            IconPaint::CurrentColor
        );
        assert_eq!(
            paint(
                r#"<g fill="currentColor"><path d="M0 0h12v12z"/><path d="M0 0h12v12z" style="mix-blend-mode:multiply"/></g>"#
            ),
            IconPaint::Mixed
        );
    }
}
