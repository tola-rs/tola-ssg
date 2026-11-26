//! Paint classification over reachable rendered SVG content.

use super::{properties::PaintValue, InvalidSvg, SvgChild, SvgDocument, SvgElement};
use crate::IconPaint;

const MAX_RENDERED_VISITS: usize = super::MAX_SVG_NODES as usize * 8;

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
    pub(super) fn for_element(el: &SvgElement) -> Self {
        let paint = |name| el.prop(name).map(parse_validated_paint);
        let urls = |name| {
            el.prop(name)
                .map(|v| super::properties::local_urls(v).expect("validated SVG resource URLs"))
                .unwrap_or_default()
        };
        Self {
            fill: paint("fill"),
            stroke: paint("stroke"),
            stop: paint("stop-color"),
            flood: paint("flood-color"),
            lighting: paint("lighting-color"),
            color: paint("color"),
            href: el
                .attrs
                .get("href")
                .map(|v| super::properties::local_reference(v).expect("validated SVG href")),
            filter: urls("filter"),
            mask: urls("mask"),
            clip: urls("clip-path"),
            controls: PaintControl::ALL.map(|c| c.parse(el)),
            geometry_can_paint: match el.name.as_str() {
                "path" => el.attrs.get("d").is_some_and(|d| !d.trim().is_empty()),
                "rect" => !["width", "height"].into_iter().any(|dim| {
                    el.attrs.get(dim).is_some_and(|len| {
                        len.trim()
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
    fn for_element(&self, el: &SvgElement) -> Self {
        let spec = &el.paint;
        let inherited =
            |prop: &Option<PaintValue>, parent: &PaintValue, initial: PaintValue| match prop {
                None | Some(PaintValue::Inherit | PaintValue::Unset) => parent.clone(),
                Some(PaintValue::Initial) => initial,
                Some(paint) => paint.clone(),
            };
        let color = match &spec.color {
            Some(c @ (PaintValue::Fixed | PaintValue::Transparent)) => c.clone(),
            Some(PaintValue::Initial) => PaintValue::Fixed,
            None | Some(PaintValue::CurrentColor | PaintValue::Inherit | PaintValue::Unset) => {
                self.color.clone()
            }
            Some(PaintValue::None | PaintValue::Resource(_)) => {
                unreachable!("color property accepts only colors")
            }
        };
        let non_inherited = |prop: &Option<PaintValue>, parent: &PaintValue| match prop {
            None | Some(PaintValue::Initial | PaintValue::Unset) => PaintValue::Fixed,
            Some(PaintValue::Inherit) => parent.clone(),
            Some(paint) => paint.clone(),
        };
        Self {
            fill: inherited(&spec.fill, &self.fill, PaintValue::Fixed),
            stroke: inherited(&spec.stroke, &self.stroke, PaintValue::None),
            stop: non_inherited(&spec.stop, &self.stop),
            flood: non_inherited(&spec.flood, &self.flood),
            lighting: non_inherited(&spec.lighting, &self.lighting),
            color,
            visible: PaintControl::Visibility.resolve(el, self.visible),
            fill_visible: PaintControl::FillOpacity.resolve(el, self.fill_visible),
            stroke_visible: PaintControl::StrokeOpacity.resolve(el, self.stroke_visible),
            has_stroke_width: PaintControl::StrokeWidth.resolve(el, self.has_stroke_width),
            displayed: PaintControl::Display.resolve(el, self.displayed),
            opacity_visible: PaintControl::Opacity.resolve(el, self.opacity_visible),
            stop_visible: PaintControl::StopOpacity.resolve(el, self.stop_visible),
            flood_visible: PaintControl::FloodOpacity.resolve(el, self.flood_visible),
            alpha_mask: PaintControl::MaskType.resolve(el, self.alpha_mask),
            blends: PaintControl::Blend.resolve(el, self.blends),
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
    /// Order matching `SpecifiedPaint::controls`.
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

    fn parse(self, el: &SvgElement) -> ControlValue {
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
        match el.prop(name) {
            Some("inherit") => ControlValue::Inherit,
            Some("initial") => ControlValue::Initial,
            None | Some("unset") => ControlValue::Unset,
            Some(val) => ControlValue::Value(match self {
                Self::Visibility => val == "visible",
                Self::Display => val != "none",
                Self::MaskType => val == "alpha",
                Self::Blend => val != "normal",
                Self::StrokeWidth => {
                    val.parse::<svgtypes::Length>()
                        .expect("validated SVG stroke width")
                        .number
                        > 0.0
                }
                _ => positive_opacity(val),
            }),
        }
    }

    fn resolve(self, el: &SvgElement, parent: bool) -> bool {
        let initial = !matches!(self, Self::MaskType | Self::Blend);
        match el.paint.controls[self as usize] {
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
            ControlValue::Value(val) => val,
        }
    }
}

fn parse_validated_paint(src: &str) -> PaintValue {
    super::properties::parse_paint(src).expect("SVG paint validated at import")
}

struct RenderTraversal<'a> {
    doc: &'a SvgDocument,
    computed: Vec<ComputedPaint>,
    remaining: usize,
}

pub(super) fn validate_rendered(doc: &SvgDocument) -> Result<IconPaint, InvalidSvg> {
    let mut traversal = RenderTraversal {
        doc,
        computed: vec![ComputedPaint::default(); doc.elements.len()],
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

    fn inherit(&mut self, idx: usize, parent: &ComputedPaint) {
        let el = &self.doc.elements[idx];
        let inherited = parent.for_element(el);
        self.computed[idx] = inherited.clone();
        for child in &el.children {
            if let SvgChild::Element(child) = child {
                self.inherit(*child, &inherited);
            }
        }
    }

    fn rendered(
        &mut self,
        idx: usize,
        parent: &ComputedPaint,
        instanced: bool,
    ) -> Result<PaintSources, InvalidSvg> {
        self.visit()?;
        let el = &self.doc.elements[idx];
        let inherited = parent.for_element(el);
        if el.is_resource_or_desc()
            || (el.name == "symbol" && !instanced)
            || !inherited.displayed
            || !inherited.opacity_visible
        {
            return Ok(PaintSources::default());
        }
        let mut sources = PaintSources::default();
        if el.name == "use" {
            if let Some(id) = &el.paint.href {
                let target = self.doc.ids[id];
                sources.combine(self.rendered(target, &inherited, true)?);
            }
        } else if inherited.visible
            && matches!(
                el.name.as_str(),
                "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon"
            )
        {
            // Non-drawing shapes still walk paint servers to preserve the expansion work budget.
            let mut shape_sources = PaintSources::default();
            if el.name != "line" && inherited.fill_visible {
                shape_sources.combine(self.paint(inherited.resolve_color(&inherited.fill))?);
            }
            if inherited.stroke_visible && inherited.has_stroke_width {
                shape_sources.combine(self.paint(inherited.resolve_color(&inherited.stroke))?);
            }
            if el.paint.geometry_can_paint {
                sources.combine(shape_sources);
            }
        }
        for child in &el.children {
            self.visit()?;
            if let SvgChild::Element(child) = child {
                sources.combine(self.rendered(*child, &inherited, false)?);
            }
        }
        if !el.paint.filter.is_empty() {
            for id in &el.paint.filter {
                sources.combine(self.filter(self.doc.ids[id])?);
            }
            // Filters can transform RGB and alpha independently of `currentColor`.
            sources.changes_color = true;
        }
        if inherited.blends {
            sources.changes_color = true;
        }
        for id in &el.paint.mask {
            let target = self.doc.ids[id];
            let target_paint = self.computed[target].clone();
            let mask_sources = self.resource_children(target, &target_paint)?;
            if mask_sources.current_color
                && (!self.computed[target].alpha_mask || mask_sources.changes_color)
            {
                sources.current_color = true;
                sources.changes_color = true;
            }
        }
        self.clipping(idx)?;
        Ok(sources)
    }

    fn clipping(&mut self, idx: usize) -> Result<(), InvalidSvg> {
        for id in &self.doc.elements[idx].paint.clip {
            let target = self.doc.ids[id];
            let target_paint = self.computed[target].clone();
            self.clip_geometry(target, &target_paint)?;
        }
        Ok(())
    }

    // Clip silhouettes depend only on geometry, visibility, and nested clips.
    fn clip_geometry(&mut self, idx: usize, parent: &ComputedPaint) -> Result<(), InvalidSvg> {
        self.visit()?;
        let el = &self.doc.elements[idx];
        let inherited = parent.for_element(el);
        if !inherited.displayed || (el.is_resource_or_desc() && el.name != "clipPath") {
            return Ok(());
        }
        if el.name == "use" {
            if let Some(id) = &el.paint.href {
                self.clip_geometry(self.doc.ids[id], &inherited)?;
            }
        }
        for child in &el.children {
            self.visit()?;
            if let SvgChild::Element(child) = child {
                self.clip_geometry(*child, &inherited)?;
            }
        }
        self.clipping(idx)
    }

    fn paint(&mut self, paint: &PaintValue) -> Result<PaintSources, InvalidSvg> {
        match paint {
            PaintValue::None | PaintValue::Transparent => Ok(PaintSources::default()),
            PaintValue::CurrentColor => Ok(PaintSources::CURRENT),
            PaintValue::Fixed => Ok(PaintSources::FIXED),
            PaintValue::Inherit | PaintValue::Initial | PaintValue::Unset => {
                unreachable!("CSS-wide paints resolved at owning property")
            }
            PaintValue::Resource(id) => {
                let idx = self.doc.ids[id.as_ref()];
                match self.doc.elements[idx].name.as_str() {
                    "linearGradient" | "radialGradient" => self.gradient(idx),
                    "pattern" => self.pattern(idx),
                    _ => unreachable!("paint server target validated"),
                }
            }
        }
    }

    fn gradient(&mut self, idx: usize) -> Result<PaintSources, InvalidSvg> {
        let inherited = self.computed[idx].clone();
        let content = self.template_content(idx)?;
        let mut sources = PaintSources::default();
        let mut visible = false;
        for child in &self.doc.elements[content].children {
            self.visit()?;
            if let SvgChild::Element(child) = child {
                let stop = &self.doc.elements[*child];
                if stop.name != "stop" {
                    continue;
                }
                let computed = inherited.for_element(stop);
                let paint = computed.resolve_color(&computed.stop);
                visible |= computed.stop_visible
                    && !matches!(paint, PaintValue::None | PaintValue::Transparent);
                // SVG interpolates non-premultiplied RGB even across zero-alpha stops.
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

    fn pattern(&mut self, idx: usize) -> Result<PaintSources, InvalidSvg> {
        let inherited = self.computed[idx].clone();
        let content = self.template_content(idx)?;
        self.resource_children(content, &inherited)
    }

    // Paint-server templates supply children; referenced content inherits from its host element.
    fn template_content(&mut self, mut idx: usize) -> Result<usize, InvalidSvg> {
        loop {
            self.visit()?;
            let el = &self.doc.elements[idx];
            for child in &el.children {
                self.visit()?;
                if matches!(child, SvgChild::Element(c)
                    if !matches!(self.doc.elements[*c].name.as_str(), "title" | "desc"))
                {
                    return Ok(idx);
                }
            }
            let Some(id) = &el.paint.href else {
                return Ok(idx);
            };
            idx = self.doc.ids[id];
        }
    }

    fn resource_children(
        &mut self,
        idx: usize,
        inherited: &ComputedPaint,
    ) -> Result<PaintSources, InvalidSvg> {
        let mut sources = PaintSources::default();
        for child in &self.doc.elements[idx].children {
            self.visit()?;
            if let SvgChild::Element(child) = child {
                sources.combine(self.rendered(*child, inherited, false)?);
            }
        }
        Ok(sources)
    }

    fn filter(&mut self, idx: usize) -> Result<PaintSources, InvalidSvg> {
        self.visit()?;
        let mut sources = PaintSources::default();
        for child in &self.doc.elements[idx].children {
            self.visit()?;
            let SvgChild::Element(child) = child else {
                continue;
            };
            let prim = &self.doc.elements[*child];
            let computed = &self.computed[*child];
            let paint = match prim.name.as_str() {
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

fn positive_opacity(val: &str) -> bool {
    val.trim_end_matches('%')
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

    fn svg_doc(body: &str) -> SvgDocument {
        SvgDocument::parse(&format!(r#"<svg viewBox="0 0 24 24">{body}</svg>"#)).unwrap()
    }

    fn render_traversal(doc: &SvgDocument, remaining: usize) -> RenderTraversal<'_> {
        let mut t = RenderTraversal {
            doc,
            computed: vec![ComputedPaint::default(); doc.elements.len()],
            remaining,
        };
        t.inherit(0, &ComputedPaint::default());
        t
    }

    #[test]
    fn template_per_host() {
        let body = r##"<defs><linearGradient id="base"><stop stop-color="currentColor"/><stop offset="1" stop-color="currentColor" stop-opacity="0"/></linearGradient><linearGradient id="fixed" href="#base" color="red"/><linearGradient id="dynamic" href="#base"/><path id="shape" d="M0 0h12v12z"/></defs><use href="#shape" fill="url(#dynamic)"/><use href="#shape" fill="url(#fixed)"/>"##;
        assert_eq!(paint(body), IconPaint::Mixed);
        assert_eq!(
            paint(&body.replace("url(#fixed)", "url(#dynamic)")),
            IconPaint::CurrentColor
        );
    }

    #[test]
    fn transparent_paint_not_fixed() {
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
    fn current_color_keywords() {
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
    fn paint_classification() {
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
    fn hidden_paint_ignored() {
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
    fn css_wide_keywords() {
        for attr in [
            r#"visibility="HIDDEN""#,
            r#"style="visibility: h\69 dden""#,
            r#"display=" NONE ""#,
        ] {
            assert_eq!(
                paint(&format!(
                    r#"<path d="M0 0h12v12z" fill="currentColor"/><path d="M0 0h12v12z" fill="red" {attr}/>"#
                )),
                IconPaint::CurrentColor,
                "{attr}",
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
    fn unset_vs_inherit_resources() {
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
    fn alpha_mask_dependencies() {
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
    fn clip_shares_budget() {
        fn paint_with_budget(body: &str, budget: usize) -> Result<IconPaint, InvalidSvg> {
            let doc = svg_doc(body);
            doc.validate_references()?;
            Ok(render_traversal(&doc, budget)
                .rendered(0, &ComputedPaint::default(), false)?
                .classification())
        }
        let defs = r##"<defs><path id="shape" d="M0 0h12v12z"/><clipPath id="base"><use href="#shape"/><use href="#shape"/></clipPath><clipPath id="nested" clip-path="url(#base)"><use href="#shape"/></clipPath></defs>"##;
        let one = format!(
            r##"{defs}<path d="M0 0h12v12z" fill="currentColor" clip-path="url(#nested)"/>"##
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
        let unused = format!(r##"{defs}<path d="M0 0h12v12z" fill="currentColor"/>"##);
        assert_eq!(
            paint_with_budget(&unused, 5).unwrap(),
            IconPaint::CurrentColor
        );
        let geom_only = r##"<defs><linearGradient id="paint"><stop/><stop/><stop/></linearGradient><clipPath id="clip"><path d="M0 0h12v12z" fill="url(#paint)" opacity="0"/></clipPath></defs><path d="M0 0h12v12z" fill="currentColor" clip-path="url(#clip)"/>"##;
        assert_eq!(
            paint_with_budget(geom_only, 8).unwrap(),
            IconPaint::CurrentColor
        );
    }

    #[test]
    fn blend_mode_inheritance() {
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
}
