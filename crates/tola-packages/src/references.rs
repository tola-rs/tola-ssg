//! Native reference occurrences resolved after reaching Typst's output rules.
//!
//! `tola_typst::reference_occurrence` records links and refs emitted into outputs; this module
//! resolves them against Bundle outputs and applies query filters. All graph and selector reads
//! remain tracked by Typst introspection.
//!
//! # Label scope
//!
//! A Bundle shares one label namespace across all documents, so duplicate labels across documents
//! resolve as `ambiguous-label` in [`labelled_location`].

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use tola_address::{
    BrowserRef, OutputPath, SiteRef, SiteUrlMount, UrlPath, asset_url_from_output,
    classify_browser_ref, route_for_output,
};
use tola_typst::reference_occurrence::{ReferenceOccurrence, observed_references};
use typst::comemo::Tracked;
use typst::diag::{At, SourceDiagnostic, SourceResult, error, warning};
use typst::engine::Engine;
use typst::foundations::{
    Array, Content, Context, Dict, Func, IntoValue, Label, LocatableSelector, NativeElement,
    NativeFunc, Selector, Value, func,
};
use typst::introspection::{History, Introspect, Introspector, Location, here};
use typst::model::{AssetElem, Destination, DocumentElem, LinkElem, LinkTarget, RefElem};
use typst::syntax::Span;

pub(super) fn references_func() -> Func {
    Func::from(tola_references::data())
}

#[func(contextual)]
fn tola_references(
    engine: &mut Engine,
    context: Tracked<Context>,
    span: Span,
    /// The native documents containing the selected elements, or none for every document.
    #[named]
    #[default(Value::None)]
    from: Value,
    /// The selected ancestor regions containing the reference occurrence, or none for every region.
    #[named]
    #[default(Value::None)]
    from_within: Value,
    /// The selected native outputs or elements the references target, or none for every target.
    #[named]
    #[default(Value::None)]
    to: Value,
    /// The selected ancestor regions containing a known native target, or none for every region.
    #[named]
    #[default(Value::None)]
    to_within: Value,
) -> SourceResult<Array> {
    let mount = crate::library::site_mount(engine, span)?;
    let cur = here(context).at(span)?;
    let query = ReferenceQuery {
        from: endpoint_selection(from, cur, span)?,
        from_within: region_selection(from_within, span)?,
        to: endpoint_selection(to, cur, span)?,
        to_within: region_selection(to_within, span)?,
    };
    let outcome = engine.introspect(ReferenceQueryIntrospection { query, mount, span });
    // Current document or selected sources may appear only after the Bundle settles.
    if let Some(failure) = outcome.failure {
        engine.delay::<()>(Err(vec![failure.diagnostic(span)].into()));
    }
    Ok(outcome.records)
}

#[derive(Debug, Clone, PartialEq, Hash)]
enum EndpointSelection {
    All,
    CurrentDocument(Location),
    Selector(Selector),
}

fn endpoint_selection(val: Value, cur: Location, span: Span) -> SourceResult<EndpointSelection> {
    Ok(match val {
        Value::None => EndpointSelection::All,
        Value::Auto => EndpointSelection::CurrentDocument(cur),
        val => EndpointSelection::Selector(val.cast::<LocatableSelector>().at(span)?.0),
    })
}

fn region_selection(val: Value, span: Span) -> SourceResult<Option<Selector>> {
    match val {
        Value::None => Ok(None),
        val => Ok(Some(val.cast::<LocatableSelector>().at(span)?.0)),
    }
}

#[derive(Debug, Clone, PartialEq, Hash)]
struct ReferenceQuery {
    from: EndpointSelection,
    from_within: Option<Selector>,
    to: EndpointSelection,
    to_within: Option<Selector>,
}

#[derive(Debug, Clone, PartialEq, Hash)]
struct ReferenceQueryIntrospection {
    query: ReferenceQuery,
    mount: SiteUrlMount,
    span: Span,
}

#[derive(Debug, Clone, PartialEq, Hash)]
struct RefQueryResult {
    records: Array,
    failure: Option<ReferenceQueryFailure>,
}

#[derive(Debug, Clone, PartialEq, Hash)]
enum ReferenceQueryFailure {
    OutsideDocument,
    SourceOutsideDocument,
    MissingChannel,
}

impl ReferenceQueryFailure {
    fn diagnostic(&self, span: Span) -> SourceDiagnostic {
        match self {
            Self::OutsideDocument => error!(
                span, "`auto` needs a document";
                hint: "call `references` inside a `document(…)` body"
            ),
            Self::SourceOutsideDocument => error!(
                span, "`from` selects content outside a published document";
                hint: "select a document or content written inside one, not an asset"
            ),
            Self::MissingChannel => error!(
                span, "this compilation records no references";
                hint: "build the site with `tola` so `references` can read what its links and refs target"
            ),
        }
    }
}

impl Introspect for ReferenceQueryIntrospection {
    type Output = RefQueryResult;

    fn introspect(
        &self,
        _: &mut Engine,
        introspector: Tracked<dyn Introspector + '_>,
    ) -> RefQueryResult {
        match self.records(introspector) {
            Ok(records) => RefQueryResult {
                records,
                failure: None,
            },
            Err(failure) => RefQueryResult {
                records: Array::new(),
                failure: Some(failure),
            },
        }
    }

    fn diagnose(&self, _: &History<RefQueryResult>) -> SourceDiagnostic {
        warning!(
            self.span, "`references` did not settle, so these results may be incomplete";
            hint: "render the list outside the regions `from-within` reads"
        )
    }
}

impl ReferenceQueryIntrospection {
    fn records(
        &self,
        introspector: Tracked<dyn Introspector + '_>,
    ) -> Result<Array, ReferenceQueryFailure> {
        let out_idx = native_output_index(introspector);
        let srcs = source_documents(&self.query.from, introspector, &out_idx)?;
        let targets = selected_targets(&self.query.to, introspector)?;
        let mut refs = document_occurrences(introspector, &out_idx)?;
        order_references(&mut refs, introspector);

        let mut records = Array::new();
        let mut owner = None;
        for ref_ in &refs {
            // Retained owners are disjoint, so the previous one decides nested containment.
            if owner.is_some_and(|loc: Location| {
                region_matches(
                    ref_.occ.location,
                    Some(&Selector::Location(loc)),
                    introspector,
                )
            }) {
                continue;
            }
            owner = Some(ref_.occ.location);
            let native = out_idx
                .paths
                .get(&ref_.out)
                .expect("observed occurrence names a published output");
            if srcs.as_ref().is_some_and(|s| !s.contains(&ref_.doc))
                || !region_matches(
                    ref_.occ.location,
                    self.query.from_within.as_ref(),
                    introspector,
                )
            {
                continue;
            }
            let res = resolve_target(
                &ref_.occ.destination,
                native,
                introspector,
                &out_idx,
                &self.mount,
            );
            if targets.as_ref().is_some_and(|t| !t.matches(&res))
                || self.query.to_within.as_ref().is_some_and(|region| {
                    res.native_location()
                        .is_none_or(|loc| !region_matches(loc, Some(region), introspector))
                })
            {
                continue;
            }
            records.push(ref_.fields(native, &res));
        }
        Ok(records)
    }
}

fn endpoint_selector(
    sel: &EndpointSelection,
    introspector: Tracked<dyn Introspector + '_>,
) -> Result<Option<Selector>, ReferenceQueryFailure> {
    Ok(match sel {
        EndpointSelection::All => None,
        EndpointSelection::CurrentDocument(loc) => Some(Selector::Location(
            introspector
                .document(*loc)
                .ok_or(ReferenceQueryFailure::OutsideDocument)?,
        )),
        EndpointSelection::Selector(sel) => Some(sel.clone()),
    })
}

fn source_documents(
    sel: &EndpointSelection,
    introspector: Tracked<dyn Introspector + '_>,
    out_idx: &NativeOutputIndex,
) -> Result<Option<HashSet<Location>>, ReferenceQueryFailure> {
    let Some(sel) = endpoint_selector(sel, introspector)? else {
        return Ok(None);
    };
    let mut docs = HashSet::new();
    for content in introspector.query(&sel) {
        let loc = content
            .location()
            .expect("locatable selections have locations");
        let doc = introspector
            .document(loc)
            .ok_or(ReferenceQueryFailure::SourceOutsideDocument)?;
        let out = location_output(doc, introspector)
            .ok_or(ReferenceQueryFailure::SourceOutsideDocument)?;
        if !out_idx.paths.contains_key(&out) {
            return Err(ReferenceQueryFailure::SourceOutsideDocument);
        }
        docs.insert(doc);
    }
    Ok(Some(docs))
}

#[derive(Default)]
struct TargetSelection {
    outputs: BTreeSet<OutputPath>,
    elements: HashSet<Location>,
}

impl TargetSelection {
    fn matches(&self, res: &ReferenceResolution) -> bool {
        let ReferenceResolution::Found(target) = res else {
            return false;
        };
        self.outputs.contains(target.output())
            || matches!(target, RefTarget::NativeElement { loc, .. } if self.elements.contains(loc))
    }
}

fn selected_targets(
    sel: &EndpointSelection,
    introspector: Tracked<dyn Introspector + '_>,
) -> Result<Option<TargetSelection>, ReferenceQueryFailure> {
    let Some(sel) = endpoint_selector(sel, introspector)? else {
        return Ok(None);
    };
    let mut targets = TargetSelection::default();
    for content in introspector.query(&sel) {
        let loc = content
            .location()
            .expect("locatable selections have locations");
        if content.to_packed::<DocumentElem>().is_some()
            || content.to_packed::<AssetElem>().is_some()
        {
            if let Some(out) = location_output(loc, introspector) {
                targets.outputs.insert(out);
            }
        } else {
            targets.elements.insert(loc);
        }
    }
    Ok(Some(targets))
}

fn region_matches(
    loc: Location,
    ancestor: Option<&Selector>,
    introspector: Tracked<dyn Introspector + '_>,
) -> bool {
    ancestor.is_none_or(|anc| {
        !introspector
            .query(&descendants(Selector::Location(loc), anc.clone()))
            .is_empty()
    })
}

fn descendants(selector: Selector, ancestor: Selector) -> Selector {
    Selector::Within {
        selector: Arc::new(selector),
        ancestor: Arc::new(ancestor),
    }
}

/// Maps each `RefElem`'s lowered `LinkElem` location back to the originating `RefElem`.
#[comemo::memoize]
fn ref_lowerings(
    introspector: Tracked<dyn Introspector + '_>,
) -> Arc<HashMap<Location, ReferenceOccurrence>> {
    let mut lowerings = HashMap::new();
    for content in introspector.query(&Selector::Elem(RefElem::ELEM, None)) {
        let Some(ref_el) = content.to_packed::<RefElem>() else {
            continue;
        };
        let Some(loc) = content.location() else {
            continue;
        };
        let lowered = introspector.query(&descendants(
            Selector::Elem(LinkElem::ELEM, None),
            Selector::Location(loc),
        ));
        let Some(link) = lowered
            .iter()
            .find(|link| link.span() == content.span())
            .and_then(Content::location)
        else {
            continue;
        };
        lowerings.insert(
            link,
            ReferenceOccurrence {
                element: content.clone(),
                destination: LinkTarget::Label(ref_el.target),
                location: loc,
            },
        );
    }
    Arc::new(lowerings)
}

struct DocumentOccurrence {
    occ: ReferenceOccurrence,
    doc: Location,
    out: OutputPath,
}

impl DocumentOccurrence {
    fn fields(&self, doc: &NativeOutput, res: &ReferenceResolution) -> Value {
        let mut fields = res.fields();
        let doc_dict = crate::dict([
            ("output", self.out.as_str().into_value()),
            ("route", doc.route.as_str().into_value()),
            ("location", self.doc.into_value()),
        ]);
        fields.insert("element".into(), self.occ.element.clone().into_value());
        fields.insert("document".into(), Value::Dict(doc_dict));
        fields.insert(
            "destination".into(),
            self.occ.destination.clone().into_value(),
        );
        fields.into_value()
    }
}

fn document_occurrences(
    introspector: Tracked<dyn Introspector + '_>,
    out_idx: &NativeOutputIndex,
) -> Result<Vec<DocumentOccurrence>, ReferenceQueryFailure> {
    let Some(occurrences) = observed_references(introspector) else {
        return Err(ReferenceQueryFailure::MissingChannel);
    };
    let lowerings = ref_lowerings(introspector);
    let mut refs = Vec::new();
    for occ in occurrences {
        let occ = lowerings.get(&occ.location).cloned().unwrap_or(occ);
        let (Some(doc), Some(out)) = (
            introspector.document(occ.location),
            location_output(occ.location, introspector),
        ) else {
            continue;
        };
        if !out_idx.paths.contains_key(&out) {
            continue;
        }
        refs.push(DocumentOccurrence { occ, doc, out });
    }
    Ok(refs)
}

/// Orders occurrences deterministically by Bundle document and element discovery order.
fn order_references(refs: &mut [DocumentOccurrence], introspector: Tracked<dyn Introspector + '_>) {
    let docs = discovery_order(introspector, &output_selector());
    let elems = discovery_order(
        introspector,
        &Selector::Or(
            [LinkElem::ELEM.select(), RefElem::ELEM.select()]
                .into_iter()
                .collect(),
        ),
    );
    refs.sort_by_key(|r| {
        let doc = docs
            .get(&r.doc)
            .expect("observed occurrence belongs to a queried document");
        let elem = elems
            .get(&r.occ.location)
            .expect("observed occurrence is a link or ref queried here");
        (*doc, *elem)
    });
}

fn discovery_order(
    introspector: Tracked<dyn Introspector + '_>,
    sel: &Selector,
) -> HashMap<Location, usize> {
    introspector
        .query(sel)
        .iter()
        .enumerate()
        .filter_map(|(i, c)| Some((c.location()?, i)))
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum NativeOutputKind {
    Document,
    Asset,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct NativeOutput {
    loc: Location,
    route: UrlPath,
}

fn location_output(
    loc: Location,
    introspector: Tracked<dyn Introspector + '_>,
) -> Option<OutputPath> {
    let path = introspector.path(loc)?.get_with_slash();
    OutputPath::parse(path.trim_start_matches('/')).ok()
}

fn output_selector() -> Selector {
    Selector::Or(
        [DocumentElem::ELEM.select(), AssetElem::ELEM.select()]
            .into_iter()
            .collect(),
    )
}

#[derive(Default, PartialEq, Eq, Hash)]
struct NativeOutputIndex {
    paths: BTreeMap<OutputPath, NativeOutput>,
}

impl NativeOutputIndex {
    fn register(&mut self, out: OutputPath, kind: NativeOutputKind, loc: Location) {
        let route = match kind {
            NativeOutputKind::Document => route_for_output(&out),
            NativeOutputKind::Asset => asset_url_from_output(&out),
        };
        self.paths.insert(out, NativeOutput { loc, route });
    }
}

fn native_output_index(introspector: Tracked<dyn Introspector + '_>) -> NativeOutputIndex {
    let mut idx = NativeOutputIndex::default();
    for content in introspector.query(&output_selector()) {
        let loc = content.location().expect("queried outputs have locations");
        let Some(out) = location_output(loc, introspector) else {
            continue;
        };
        let kind = if content.to_packed::<DocumentElem>().is_some() {
            NativeOutputKind::Document
        } else {
            NativeOutputKind::Asset
        };
        idx.register(out, kind, loc);
    }
    idx
}

enum RefTarget {
    NativeOutput {
        out: OutputPath,
        route: UrlPath,
        loc: Location,
    },
    NativeElement {
        out: OutputPath,
        route: UrlPath,
        loc: Location,
        anchor: Option<String>,
    },
    Url {
        out: OutputPath,
        route: UrlPath,
        query: Option<String>,
        fragment: Option<String>,
    },
}

impl RefTarget {
    fn output(&self) -> &OutputPath {
        match self {
            Self::NativeOutput { out, .. }
            | Self::NativeElement { out, .. }
            | Self::Url { out, .. } => out,
        }
    }

    fn native_location(&self) -> Option<Location> {
        match self {
            Self::NativeOutput { loc, .. } | Self::NativeElement { loc, .. } => Some(*loc),
            Self::Url { .. } => None,
        }
    }

    fn fields(&self) -> Dict {
        match self {
            Self::NativeOutput { out, route, loc } => {
                target_fields("output", Some(out), route, None, None, Some(*loc))
            }
            Self::NativeElement {
                out,
                route,
                loc,
                anchor,
            } => target_fields(
                "element",
                Some(out),
                route,
                None,
                anchor.as_deref(),
                Some(*loc),
            ),
            Self::Url {
                out,
                route,
                query,
                fragment,
            } => target_fields(
                "url",
                Some(out),
                route,
                query.as_deref(),
                fragment.as_deref(),
                None,
            ),
        }
    }
}

struct MissingUrlTarget {
    route: UrlPath,
    query: Option<String>,
    fragment: Option<String>,
}

enum ReferenceResolution {
    Found(RefTarget),
    External,
    MissingUrl(MissingUrlTarget),
    Unresolved(UnresolvedReason),
}

impl ReferenceResolution {
    fn native_location(&self) -> Option<Location> {
        match self {
            Self::Found(target) => target.native_location(),
            Self::External | Self::MissingUrl(_) | Self::Unresolved(_) => None,
        }
    }

    fn fields(&self) -> Dict {
        let (res, reason, target) = match self {
            Self::Found(target) => ("found", None, Some(target.fields())),
            Self::External => ("external", None, None),
            Self::Unresolved(reason) => ("unresolved", Some(*reason), None),
            Self::MissingUrl(target) => (
                "unresolved",
                Some(UnresolvedReason::NoSuchTarget),
                Some(target_fields(
                    "url",
                    None,
                    &target.route,
                    target.query.as_deref(),
                    target.fragment.as_deref(),
                    None,
                )),
            ),
        };
        crate::dict([
            ("resolution", res.into_value()),
            (
                "reason",
                reason.map(UnresolvedReason::into_fields).into_value(),
            ),
            ("target", target.into_value()),
        ])
    }
}

fn target_fields(
    kind: &str,
    out: Option<&OutputPath>,
    route: &UrlPath,
    query: Option<&str>,
    fragment: Option<&str>,
    loc: Option<Location>,
) -> Dict {
    crate::dict([
        ("kind", kind.into_value()),
        ("output", out.map(OutputPath::as_str).into_value()),
        ("route", route.as_str().into_value()),
        ("query", query.into_value()),
        ("fragment", fragment.into_value()),
        ("location", loc.into_value()),
    ])
}

#[derive(Clone, Copy)]
enum UnresolvedReason {
    OutsideMount,
    InvalidDestination,
    NoSuchTarget,
    PositionalDestination,
    LabelHasNoLocation,
    AmbiguousLabel,
}

impl UnresolvedReason {
    const fn tag(self) -> &'static str {
        match self {
            Self::OutsideMount => "outside-mount",
            Self::InvalidDestination => "invalid-destination",
            Self::NoSuchTarget => "no-such-target",
            Self::PositionalDestination => "positional-destination",
            Self::LabelHasNoLocation => "label-has-no-location",
            Self::AmbiguousLabel => "ambiguous-label",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::OutsideMount => "the destination lies outside `site.base-path`",
            Self::InvalidDestination => "the destination is not a URL this site can read",
            Self::NoSuchTarget => "no page or file this site publishes answers the destination",
            Self::PositionalDestination => {
                "the destination is a page position instead of a site target"
            }
            Self::LabelHasNoLocation => "the label's content has no location in this build",
            Self::AmbiguousLabel => "more than one element has the destination label",
        }
    }

    fn help(self) -> &'static str {
        match self {
            Self::OutsideMount => {
                "start root-relative URLs with `site.base-path`, or make the link relative"
            }
            Self::InvalidDestination | Self::PositionalDestination => {
                "point the link at a URL, a label, or an element location"
            }
            Self::NoSuchTarget => "point the link at a page or file the site publishes",
            Self::LabelHasNoLocation => "link to this label somewhere in the site",
            Self::AmbiguousLabel => "give the content you mean its own label",
        }
    }

    fn into_fields(self) -> Dict {
        crate::dict([
            ("tag", self.tag().into_value()),
            ("message", self.message().into_value()),
            ("help", self.help().into_value()),
        ])
    }
}

fn labelled_location(
    label: Label,
    introspector: Tracked<dyn Introspector + '_>,
) -> Result<Location, UnresolvedReason> {
    let matches = introspector.query(&Selector::Label(label));
    match matches.len() {
        0 => Err(UnresolvedReason::NoSuchTarget),
        1 => matches[0]
            .location()
            .ok_or(UnresolvedReason::LabelHasNoLocation),
        _ => Err(UnresolvedReason::AmbiguousLabel),
    }
}

fn location_target(
    loc: Location,
    introspector: Tracked<dyn Introspector + '_>,
    out_idx: &NativeOutputIndex,
) -> Option<RefTarget> {
    let out = location_output(loc, introspector)?;
    let native = out_idx.paths.get(&out)?;
    let route = native.route.clone();
    Some(if native.loc == loc {
        RefTarget::NativeOutput { out, route, loc }
    } else {
        RefTarget::NativeElement {
            out,
            route,
            loc,
            anchor: introspector.anchor(loc).map(ToString::to_string),
        }
    })
}

fn resolve_target(
    dest: &LinkTarget,
    src: &NativeOutput,
    introspector: Tracked<dyn Introspector + '_>,
    out_idx: &NativeOutputIndex,
    mount: &SiteUrlMount,
) -> ReferenceResolution {
    let dest = match dest {
        LinkTarget::Label(label) => match labelled_location(*label, introspector) {
            Ok(loc) => Destination::Location(loc),
            Err(reason) => return ReferenceResolution::Unresolved(reason),
        },
        LinkTarget::Dest(dest) => dest.clone(),
    };
    match dest {
        Destination::Url(url) => match classify_browser_ref(&src.route, mount, None, &url) {
            Ok(BrowserRef::Site(ref_)) => url_target(&ref_, out_idx),
            Ok(BrowserRef::External) => ReferenceResolution::External,
            Ok(BrowserRef::OutsideSite(_)) => {
                ReferenceResolution::Unresolved(UnresolvedReason::OutsideMount)
            }
            Err(_) => ReferenceResolution::Unresolved(UnresolvedReason::InvalidDestination),
        },
        Destination::Location(loc) => match location_target(loc, introspector, out_idx) {
            Some(target) => ReferenceResolution::Found(target),
            None => ReferenceResolution::Unresolved(UnresolvedReason::NoSuchTarget),
        },
        Destination::Position(_) => {
            ReferenceResolution::Unresolved(UnresolvedReason::PositionalDestination)
        }
    }
}

fn url_target(ref_: &SiteRef, out_idx: &NativeOutputIndex) -> ReferenceResolution {
    let out = OutputPath::from_route(&ref_.route);
    let query = ref_.query.clone();
    let fragment = ref_.decoded_fragment().map(|f| f.into_owned());
    match out_idx.paths.get_key_value(&out) {
        Some((out, native)) => ReferenceResolution::Found(RefTarget::Url {
            out: out.clone(),
            route: native.route.clone(),
            query,
            fragment,
        }),
        None => ReferenceResolution::MissingUrl(MissingUrlTarget {
            route: ref_.route.clone(),
            query,
            fragment,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guide_index() -> NativeOutputIndex {
        let mut idx = NativeOutputIndex::default();
        idx.register(
            OutputPath::parse("guide/index.html").unwrap(),
            NativeOutputKind::Document,
            Location::new(1),
        );
        idx.register(
            OutputPath::parse("guide.pdf").unwrap(),
            NativeOutputKind::Asset,
            Location::new(2),
        );
        idx
    }

    fn url_resolution(addr: &str, idx: &NativeOutputIndex) -> ReferenceResolution {
        url_target(&SiteRef::parse(addr).unwrap(), idx)
    }

    #[test]
    fn target_selection_identity() {
        let out = OutputPath::parse("guide/index.html").unwrap();
        let route = route_for_output(&out);
        let first = Location::new(1);
        let second = Location::new(2);
        let sel = TargetSelection {
            elements: HashSet::from([first]),
            ..Default::default()
        };
        for (loc, expected) in [(first, true), (second, false)] {
            let res = ReferenceResolution::Found(RefTarget::NativeElement {
                out: out.clone(),
                route: route.clone(),
                loc,
                anchor: Some("shared".into()),
            });
            assert_eq!(sel.matches(&res), expected);
        }
        assert!(!sel.matches(&url_resolution("/guide/#shared", &guide_index())));
        let outputs = TargetSelection {
            outputs: BTreeSet::from([out]),
            ..Default::default()
        };
        assert!(outputs.matches(&url_resolution("/guide/#shared", &guide_index())));
    }
}
