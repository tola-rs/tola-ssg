//! Native reference occurrences, resolved after they reached Typst's output rules.
//!
//! `tola_typst::reference_occurrence` records the links and refs that actually reached an output;
//! this module resolves them against the Bundle's outputs and applies the query's filters. Native
//! locations identify elements; URL targets identify outputs and suffixes. A native target reports
//! the exporter's own anchor rather than a spelling this module invents, and a URL target reports
//! the fragment as written — whether an output answers it is the build's question, not the query's.
//! All graph and selector reads remain tracked by Typst introspection.
//!
//! # Label scope
//!
//! A Bundle holds one label namespace, not one per document. Two documents may each declare
//! `<intro>`, and nothing reports it: a bare `query(<intro>)` returns both matches. The duplicate
//! surfaces only when a link or ref resolves the label, and then [`labelled_location`] finds two
//! matches and answers `ambiguous-label`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

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

use tola_address::{
    OutputPath, ResolvedBrowserReference, SiteReference, SiteUrlMount, UrlPath,
    asset_url_from_output, resolve_browser_reference, route_for_output,
};

use tola_typst::reference_occurrence::{ReferenceOccurrence, observed_references};

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
    let current = here(context).at(span)?;
    let query = ReferenceQuery {
        from: endpoint_selection(from, current, span)?,
        from_within: region_selection(from_within, span)?,
        to: endpoint_selection(to, current, span)?,
        to_within: region_selection(to_within, span)?,
    };
    let outcome = engine.introspect(ReferenceQueryIntrospection { query, mount, span });
    // A current document or selected source may appear only after the Bundle settles.
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

fn endpoint_selection(
    value: Value,
    current: Location,
    span: Span,
) -> SourceResult<EndpointSelection> {
    Ok(match value {
        Value::None => EndpointSelection::All,
        Value::Auto => EndpointSelection::CurrentDocument(current),
        value => EndpointSelection::Selector(value.cast::<LocatableSelector>().at(span)?.0),
    })
}

fn region_selection(value: Value, span: Span) -> SourceResult<Option<Selector>> {
    match value {
        Value::None => Ok(None),
        value => Ok(Some(value.cast::<LocatableSelector>().at(span)?.0)),
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
struct ReferenceQueryOutcome {
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
    type Output = ReferenceQueryOutcome;

    fn introspect(
        &self,
        _: &mut Engine,
        introspector: Tracked<dyn Introspector + '_>,
    ) -> ReferenceQueryOutcome {
        match self.records(introspector) {
            Ok(records) => ReferenceQueryOutcome {
                records,
                failure: None,
            },
            Err(failure) => ReferenceQueryOutcome {
                records: Array::new(),
                failure: Some(failure),
            },
        }
    }

    fn diagnose(&self, _: &History<ReferenceQueryOutcome>) -> SourceDiagnostic {
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
        let output_index = native_output_index(introspector);
        let sources = source_documents(&self.query.from, introspector, &output_index)?;
        let targets = selected_targets(&self.query.to, introspector)?;
        let mut references = document_occurrences(introspector, &output_index)?;
        order_references(&mut references, introspector);
        let mut records = Array::new();
        let mut owner = None;
        for reference in &references {
            // Retained owners are disjoint, so the last one alone decides whether this
            // occurrence is nested inside a reference already reported.
            if owner.is_some_and(|owner: Location| {
                region_matches(
                    reference.occurrence.location,
                    Some(&Selector::Location(owner)),
                    introspector,
                )
            }) {
                continue;
            }
            owner = Some(reference.occurrence.location);
            let native = output_index
                .paths
                .get(&reference.output)
                .expect("an observed occurrence names a published output");
            if sources
                .as_ref()
                .is_some_and(|sources| !sources.contains(&reference.document))
                || !region_matches(
                    reference.occurrence.location,
                    self.query.from_within.as_ref(),
                    introspector,
                )
            {
                continue;
            }
            let resolution = resolve_target(
                &reference.occurrence.destination,
                native,
                introspector,
                &output_index,
                &self.mount,
            );
            if targets
                .as_ref()
                .is_some_and(|targets| !targets.matches(&resolution))
                || self.query.to_within.as_ref().is_some_and(|region| {
                    resolution.native_location().is_none_or(|location| {
                        !region_matches(location, Some(region), introspector)
                    })
                })
            {
                continue;
            }
            records.push(reference.fields(native, &resolution));
        }
        Ok(records)
    }
}

fn endpoint_selector(
    selection: &EndpointSelection,
    introspector: Tracked<dyn Introspector + '_>,
) -> Result<Option<Selector>, ReferenceQueryFailure> {
    Ok(match selection {
        EndpointSelection::All => None,
        EndpointSelection::CurrentDocument(location) => Some(Selector::Location(
            introspector
                .document(*location)
                .ok_or(ReferenceQueryFailure::OutsideDocument)?,
        )),
        EndpointSelection::Selector(selector) => Some(selector.clone()),
    })
}

fn source_documents(
    selection: &EndpointSelection,
    introspector: Tracked<dyn Introspector + '_>,
    output_index: &NativeOutputIndex,
) -> Result<Option<HashSet<Location>>, ReferenceQueryFailure> {
    let Some(selector) = endpoint_selector(selection, introspector)? else {
        return Ok(None);
    };
    let mut documents = HashSet::new();
    for content in introspector.query(&selector) {
        let location = content
            .location()
            .expect("locatable selections have locations");
        let document = introspector
            .document(location)
            .ok_or(ReferenceQueryFailure::SourceOutsideDocument)?;
        let output = introspector
            .path(document)
            .and_then(|path| logical_output(path.get_with_slash()))
            .ok_or(ReferenceQueryFailure::SourceOutsideDocument)?;
        if !output_index.paths.contains_key(&output) {
            return Err(ReferenceQueryFailure::SourceOutsideDocument);
        }
        documents.insert(document);
    }
    Ok(Some(documents))
}

#[derive(Default)]
struct TargetSelection {
    outputs: BTreeSet<OutputPath>,
    elements: HashSet<Location>,
}

impl TargetSelection {
    fn matches(&self, resolution: &ReferenceResolution) -> bool {
        let ReferenceResolution::Found(target) = resolution else {
            return false;
        };
        self.outputs.contains(target.output())
            || matches!(target, ResolvedTarget::NativeElement { location, .. } if self.elements.contains(location))
    }
}

fn selected_targets(
    selection: &EndpointSelection,
    introspector: Tracked<dyn Introspector + '_>,
) -> Result<Option<TargetSelection>, ReferenceQueryFailure> {
    let Some(selector) = endpoint_selector(selection, introspector)? else {
        return Ok(None);
    };
    let mut targets = TargetSelection::default();
    for content in introspector.query(&selector) {
        let location = content
            .location()
            .expect("locatable selections have locations");
        if content.to_packed::<DocumentElem>().is_some()
            || content.to_packed::<AssetElem>().is_some()
        {
            if let Some(output) = introspector
                .path(location)
                .and_then(|path| logical_output(path.get_with_slash()))
            {
                targets.outputs.insert(output);
            }
        } else {
            targets.elements.insert(location);
        }
    }
    Ok(Some(targets))
}

fn region_matches(
    location: Location,
    ancestor: Option<&Selector>,
    introspector: Tracked<dyn Introspector + '_>,
) -> bool {
    ancestor.is_none_or(|ancestor| {
        !introspector
            .query(&descendants(Selector::Location(location), ancestor.clone()))
            .is_empty()
    })
}

fn descendants(selector: Selector, ancestor: Selector) -> Selector {
    Selector::Within {
        selector: Arc::new(selector),
        ancestor: Arc::new(ancestor),
    }
}

/// The link each ref lowers to, keyed by that link's location.
///
/// A ref does not reach an HTML output as itself: it lowers to a link inside the ref's own range.
/// Realization hands a built-in rule's output the input element's span, so that link carries the
/// ref's own span, while any `link(…)` call — including one a show rule writes in a ref's place —
/// carries the span of its call site and stays a link. Looking a lowering up per ref keeps the work
/// linear in the Bundle's references, and refs written by one expression stay apart because each is
/// searched inside its own range. A link this map does not name is a link the site wrote.
#[comemo::memoize]
fn ref_lowerings(
    introspector: Tracked<dyn Introspector + '_>,
) -> Arc<HashMap<Location, ReferenceOccurrence>> {
    let mut lowerings = HashMap::new();
    for content in introspector.query(&Selector::Elem(RefElem::ELEM, None)) {
        let Some(reference) = content.to_packed::<RefElem>() else {
            continue;
        };
        let Some(location) = content.location() else {
            continue;
        };
        let lowered = introspector.query(&descendants(
            Selector::Elem(LinkElem::ELEM, None),
            Selector::Location(location),
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
                destination: LinkTarget::Label(reference.target),
                location,
            },
        );
    }
    Arc::new(lowerings)
}

/// One observed occurrence with the document it belongs to.
struct DocumentOccurrence {
    occurrence: ReferenceOccurrence,
    /// The document element containing the occurrence.
    document: Location,
    /// The output that document publishes.
    output: OutputPath,
}

impl DocumentOccurrence {
    fn fields(&self, document: &NativeOutput, resolution: &ReferenceResolution) -> Value {
        let mut fields = resolution.fields();
        fields.insert(
            "element".into(),
            self.occurrence.element.clone().into_value(),
        );
        let mut source = Dict::new();
        source.insert("output".into(), self.output.as_str().into_value());
        source.insert("route".into(), document.route.as_str().into_value());
        source.insert("location".into(), self.document.into_value());
        fields.insert("document".into(), source.into_value());
        fields.insert(
            "destination".into(),
            self.occurrence.destination.clone().into_value(),
        );
        fields.into_value()
    }
}

/// Every occurrence this compilation observed, with the document it belongs to.
///
/// A compilation that carries no occurrence channel cannot report references; saying so beats an
/// empty list that reads as "this site writes no references".
fn document_occurrences(
    introspector: Tracked<dyn Introspector + '_>,
    output_index: &NativeOutputIndex,
) -> Result<Vec<DocumentOccurrence>, ReferenceQueryFailure> {
    let Some(occurrences) = observed_references(introspector) else {
        return Err(ReferenceQueryFailure::MissingChannel);
    };
    let lowerings = ref_lowerings(introspector);
    let mut references = Vec::new();
    for occurrence in occurrences {
        let occurrence = lowerings
            .get(&occurrence.location)
            .cloned()
            .unwrap_or(occurrence);
        let (Some(document), Some(output)) = (
            introspector.document(occurrence.location),
            introspector
                .path(occurrence.location)
                .and_then(|path| logical_output(path.get_with_slash())),
        ) else {
            continue;
        };
        if !output_index.paths.contains_key(&output) {
            continue;
        }
        references.push(DocumentOccurrence {
            occurrence,
            document,
            output,
        });
    }
    Ok(references)
}

/// Orders occurrences the way a Bundle writes them: by document, then by realization order.
///
/// The channel hands occurrences over in the order they were recorded — parallel document
/// compilations merge in document order, and a nested realization finishes inside its parent —
/// but a cached compilation replays its records wherever the cache is consulted, so arrival order
/// is not the order to report. The sort is stable, so occurrences of one element keep the order
/// they were recorded in.
fn order_references(
    references: &mut [DocumentOccurrence],
    introspector: Tracked<dyn Introspector + '_>,
) {
    let documents = discovery_order(
        introspector,
        &Selector::Or(
            [DocumentElem::ELEM.select(), AssetElem::ELEM.select()]
                .into_iter()
                .collect(),
        ),
    );
    let elements = discovery_order(
        introspector,
        &Selector::Or(
            [LinkElem::ELEM.select(), RefElem::ELEM.select()]
                .into_iter()
                .collect(),
        ),
    );
    references.sort_by_key(|reference| {
        let document = documents
            .get(&reference.document)
            .expect("an observed occurrence belongs to a queried document");
        let element = elements
            .get(&reference.occurrence.location)
            .expect("an observed occurrence is a link or ref this compilation queried");
        (*document, *element)
    });
}

/// The discovery order of every located element a selector matches.
///
/// Bundle introspection orders elements the way realization discovered them, which is the order
/// the site writes: documents in Bundle order, and inside one document the parents before the
/// children they contain.
fn discovery_order(
    introspector: Tracked<dyn Introspector + '_>,
    selector: &Selector,
) -> HashMap<Location, usize> {
    introspector
        .query(selector)
        .iter()
        .enumerate()
        .filter_map(|(index, content)| Some((content.location()?, index)))
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum NativeOutputKind {
    Document,
    Asset,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct NativeOutput {
    location: Location,
    route: UrlPath,
}

fn logical_output(path: &str) -> Option<OutputPath> {
    OutputPath::parse(path.trim_start_matches('/')).ok()
}

#[derive(Default, PartialEq, Eq, Hash)]
struct NativeOutputIndex {
    paths: BTreeMap<OutputPath, NativeOutput>,
}

impl NativeOutputIndex {
    fn register(&mut self, output: OutputPath, kind: NativeOutputKind, location: Location) {
        let route = match kind {
            NativeOutputKind::Document => route_for_output(&output),
            NativeOutputKind::Asset => asset_url_from_output(&output),
        };
        self.paths.insert(output, NativeOutput { location, route });
    }
}

fn native_output_index(introspector: Tracked<dyn Introspector + '_>) -> NativeOutputIndex {
    let selector = Selector::Or(
        [DocumentElem::ELEM.select(), AssetElem::ELEM.select()]
            .into_iter()
            .collect(),
    );
    let mut output_index = NativeOutputIndex::default();
    for content in introspector.query(&selector) {
        let location = content.location().expect("queried outputs have locations");
        let Some(output) = introspector
            .path(location)
            .and_then(|path| logical_output(path.get_with_slash()))
        else {
            continue;
        };
        let kind = if content.to_packed::<DocumentElem>().is_some() {
            NativeOutputKind::Document
        } else {
            NativeOutputKind::Asset
        };
        output_index.register(output, kind, location);
    }
    output_index
}

enum ResolvedTarget {
    NativeOutput {
        output: OutputPath,
        route: UrlPath,
        location: Location,
    },
    NativeElement {
        output: OutputPath,
        route: UrlPath,
        location: Location,
        anchor: Option<String>,
    },
    Url {
        output: OutputPath,
        route: UrlPath,
        query: Option<String>,
        fragment: Option<String>,
    },
}

impl ResolvedTarget {
    fn output(&self) -> &OutputPath {
        match self {
            Self::NativeOutput { output, .. }
            | Self::NativeElement { output, .. }
            | Self::Url { output, .. } => output,
        }
    }

    fn native_location(&self) -> Option<Location> {
        match self {
            Self::NativeOutput { location, .. } | Self::NativeElement { location, .. } => {
                Some(*location)
            }
            Self::Url { .. } => None,
        }
    }

    fn fields(&self) -> Dict {
        match self {
            Self::NativeOutput {
                output,
                route,
                location,
            } => target_fields("output", Some(output), route, None, None, Some(*location)),
            Self::NativeElement {
                output,
                route,
                location,
                anchor,
            } => target_fields(
                "element",
                Some(output),
                route,
                None,
                anchor.as_deref(),
                Some(*location),
            ),
            Self::Url {
                output,
                route,
                query,
                fragment,
            } => target_fields(
                "url",
                Some(output),
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
    Found(ResolvedTarget),
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
        let (resolution, reason, target) = match self {
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
        let mut fields = Dict::new();
        fields.insert("resolution".into(), resolution.into_value());
        fields.insert(
            "reason".into(),
            reason.map(UnresolvedReason::into_fields).into_value(),
        );
        fields.insert("target".into(), target.into_value());
        fields
    }
}

fn target_fields(
    kind: &str,
    output: Option<&OutputPath>,
    route: &UrlPath,
    query: Option<&str>,
    fragment: Option<&str>,
    location: Option<Location>,
) -> Dict {
    let mut fields = Dict::new();
    fields.insert("kind".into(), kind.into_value());
    fields.insert("output".into(), output.map(OutputPath::as_str).into_value());
    fields.insert("route".into(), route.as_str().into_value());
    fields.insert("query".into(), query.into_value());
    fields.insert("fragment".into(), fragment.into_value());
    fields.insert("location".into(), location.into_value());
    fields
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
        let mut fields = Dict::new();
        fields.insert("tag".into(), self.tag().into_value());
        fields.insert("message".into(), self.message().into_value());
        fields.insert("help".into(), self.help().into_value());
        fields
    }
}

/// The element a label names, which a Bundle holds once.
///
/// The namespace spans the whole Bundle, so the same label in two documents is ambiguous even
/// when each document links its own. The caller reports `AmbiguousLabel` rather than picking one.
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
    location: Location,
    introspector: Tracked<dyn Introspector + '_>,
    output_index: &NativeOutputIndex,
) -> Option<ResolvedTarget> {
    let output = logical_output(introspector.path(location)?.get_with_slash())?;
    let native = output_index.paths.get(&output)?;
    let route = native.route.clone();
    Some(if native.location == location {
        ResolvedTarget::NativeOutput {
            output,
            route,
            location,
        }
    } else {
        ResolvedTarget::NativeElement {
            output,
            route,
            location,
            anchor: introspector.anchor(location).map(ToString::to_string),
        }
    })
}

fn resolve_target(
    destination: &LinkTarget,
    source: &NativeOutput,
    introspector: Tracked<dyn Introspector + '_>,
    output_index: &NativeOutputIndex,
    mount: &SiteUrlMount,
) -> ReferenceResolution {
    let destination = match destination {
        LinkTarget::Label(label) => match labelled_location(*label, introspector) {
            Ok(location) => Destination::Location(location),
            Err(reason) => return ReferenceResolution::Unresolved(reason),
        },
        LinkTarget::Dest(destination) => destination.clone(),
    };
    match destination {
        Destination::Url(url) => {
            match resolve_browser_reference(&source.route, mount, None, &url) {
                Ok(ResolvedBrowserReference::Site(reference)) => {
                    url_target(&reference, output_index)
                }
                Ok(ResolvedBrowserReference::External) => ReferenceResolution::External,
                Ok(ResolvedBrowserReference::OutsideSite(_)) => {
                    ReferenceResolution::Unresolved(UnresolvedReason::OutsideMount)
                }
                Err(_) => ReferenceResolution::Unresolved(UnresolvedReason::InvalidDestination),
            }
        }
        Destination::Location(location) => {
            match location_target(location, introspector, output_index) {
                Some(target) => ReferenceResolution::Found(target),
                None => ReferenceResolution::Unresolved(UnresolvedReason::NoSuchTarget),
            }
        }
        Destination::Position(_) => {
            ReferenceResolution::Unresolved(UnresolvedReason::PositionalDestination)
        }
    }
}

/// A URL names an output and its suffix, not a native element, so `to` selections match it by
/// output.
fn url_target(reference: &SiteReference, output_index: &NativeOutputIndex) -> ReferenceResolution {
    let output = OutputPath::from_route(&reference.route);
    let query = reference.query.clone();
    let fragment = reference
        .decoded_fragment()
        .map(|fragment| fragment.into_owned());
    match output_index.paths.get_key_value(&output) {
        Some((output, native)) => ReferenceResolution::Found(ResolvedTarget::Url {
            output: output.clone(),
            route: native.route.clone(),
            query,
            fragment,
        }),
        None => ReferenceResolution::MissingUrl(MissingUrlTarget {
            route: reference.route.clone(),
            query,
            fragment,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guide_index() -> NativeOutputIndex {
        let mut index = NativeOutputIndex::default();
        index.register(
            OutputPath::parse("guide/index.html").unwrap(),
            NativeOutputKind::Document,
            Location::new(1),
        );
        index.register(
            OutputPath::parse("guide.pdf").unwrap(),
            NativeOutputKind::Asset,
            Location::new(2),
        );
        index
    }

    fn url_resolution(address: &str, index: &NativeOutputIndex) -> ReferenceResolution {
        url_target(&SiteReference::parse(address).unwrap(), index)
    }
    #[test]
    fn native_locations_define_target_identity() {
        let output = OutputPath::parse("guide/index.html").unwrap();
        let route = route_for_output(&output);
        let first = Location::new(1);
        let second = Location::new(2);
        let selection = TargetSelection {
            elements: HashSet::from([first]),
            ..Default::default()
        };
        for (location, expected) in [(first, true), (second, false)] {
            let resolution = ReferenceResolution::Found(ResolvedTarget::NativeElement {
                output: output.clone(),
                route: route.clone(),
                location,
                anchor: Some("shared".into()),
            });
            assert_eq!(selection.matches(&resolution), expected);
        }
        assert!(!selection.matches(&url_resolution("/guide/#shared", &guide_index())));
        let outputs = TargetSelection {
            outputs: BTreeSet::from([output]),
            ..Default::default()
        };
        assert!(outputs.matches(&url_resolution("/guide/#shared", &guide_index())));
    }
}
