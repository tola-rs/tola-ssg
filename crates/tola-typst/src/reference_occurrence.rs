//! Native link occurrences observed at Typst's output rules.
//!
//! `references()` reports what the links and refs a Bundle writes actually target after show
//! rules ran. Typst's own introspection cannot answer that: `query(link)` returns the elements
//! that were prepared — including ones no show rule let through — and no query result says which
//! element reached its output rule. This module observes that moment and carries the occurrences
//! to the query.
//!
//! # How an occurrence travels
//!
//! The wrapped output's `create` compiles through a child engine with a sink this module owns,
//! reads the occurrences that compilation recorded, and forwards the sink so the outer compilation
//! ends up with every record the unwrapped path would have left. Occurrences travel as
//! introspection entries over `Sink::introspection`: they belong to the memoized compilation's
//! recorded mutations, so a cache hit replays them instead of rerunning the observed rule, and an
//! evicted entry only makes the next compilation rerun that rule. The wrapped output's introspector
//! answers the occurrence selector with them.
//!
//! Two observation points cover every native reference:
//!
//! - **HTML**: the `link` output rule, which every HTML link, ref, footnote, and citation
//!   eventually lowers into. Its input is the element after show rules, so the destination it
//!   carries is the one the page uses.
//! - **Paged**: the final frames. A link that reached its rule leaves a `LinkMarker` tag there,
//!   and the nearest enclosing `LinkElem` or `RefElem` in the tag scope is the element that
//!   produced it. A link that only participated in measurement has no frame and is therefore
//!   absent, as it is from the page.
//!
//! A carried occurrence is a queryable element in its own right; it never joins a document tree, so
//! no author program can reach it and no page carries it.
//!
//! # Why recorded entries are read back through equality
//!
//! `Introspection` erases the payload type and exposes no accessor, and a replayed entry is the
//! only way a cached compilation can hand occurrences to a later round. Reading them back
//! therefore compares each recorded entry with a same-type probe whose `PartialEq` copies the
//! payload into a collector: the verdict is irrelevant, the collector is the reader. If Typst
//! exposes the payload directly, this protocol collapses into a plain read and the entry type loses
//! its probe side.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use typst::Library;
use typst::comemo::{Track, Tracked};
use typst::diag::{SourceDiagnostic, SourceResult, warning};
use typst::ecow::EcoVec;
use typst::engine::{Engine, Sink};
use typst::foundations::{
    Content, NativeElement, NativeRuleMap, Output, Packed, Selector, StyleChain, Target,
};
use typst::introspection::{History, Introspect, Introspection, Introspector, Location, Tag};
use typst::layout::{Frame, FrameItem};
use typst::model::{LinkElem, LinkMarker, LinkTarget, RefElem};
use typst::syntax::Span;

/// Marks the wrapped compilation whose introspector carries reference occurrences.
///
/// A compilation that answers no channel marker cannot report occurrences; the query then says
/// so instead of returning an empty list that looks like "this site writes no references".
#[typst::foundations::elem]
struct ReferenceChannelElem {}

/// Selects every carried occurrence.
fn occurrence_selector() -> Selector {
    ReferenceOccurrenceElem::ELEM.select()
}

/// Selects the marker a wrapped compilation answers with.
fn channel_selector() -> Selector {
    ReferenceChannelElem::ELEM.select()
}

/// One reference a compilation's output rules observed, carried to `references()` as a queryable
/// element.
#[typst::foundations::elem]
struct ReferenceOccurrenceElem {
    #[required]
    element: Content,
    #[required]
    destination: LinkTarget,
    #[required]
    location: Location,
}

/// One reference a compilation's output rules observed.
///
/// The element is the native `link` or `ref` that reached its rule, already through show rules, and
/// the location is its own, which realization gives every element it shows.
#[derive(Debug, Clone)]
pub struct ReferenceOccurrence {
    /// The native `link` or `ref` element that reached its output rule, already through show rules.
    pub element: Content,
    /// The destination that element carried when the rule ran: a link's target, or the label a ref
    /// names.
    pub destination: LinkTarget,
    /// The element's own location.
    pub location: Location,
}

/// The occurrences `introspector` carries, or `None` when this compilation has no channel.
///
/// Both reads are tracked, so a caller's result depends on what this compilation observed and a
/// later round sees a changed occurrence set as an unsettled dependency instead of a stale one.
pub fn observed_references(
    introspector: Tracked<dyn Introspector + '_>,
) -> Option<Vec<ReferenceOccurrence>> {
    if introspector.query(&channel_selector()).is_empty() {
        return None;
    }
    Some(
        introspector
            .query(&occurrence_selector())
            .iter()
            .filter_map(|element| {
                let occurrence = element.to_packed::<ReferenceOccurrenceElem>()?;
                Some(ReferenceOccurrence {
                    element: occurrence.element.clone(),
                    destination: occurrence.destination.clone(),
                    location: occurrence.location,
                })
            })
            .collect(),
    )
}

/// The occurrence an element produces as a reference, if it produces one.
///
/// A link states its destination; a ref states the label it names, which is the relation the
/// author wrote and the only place the label survives — the link a ref lowers to carries the
/// resolved target instead.
fn occurrence_of(element: &Content) -> Option<ReferenceOccurrence> {
    let destination = if let Some(link) = element.to_packed::<LinkElem>() {
        link.dest.clone()
    } else {
        LinkTarget::Label(element.to_packed::<RefElem>()?.target)
    };
    Some(ReferenceOccurrence {
        element: element.clone(),
        destination,
        location: element
            .location()
            .expect("realization locates every element it shows"),
    })
}

/// One occurrence recorded on the compilation's sink, or the probe that reads recorded ones.
#[derive(Debug)]
struct OccurrenceIntrospection {
    recorded: Option<Arc<ReferenceOccurrence>>,
    collector: Option<Arc<Mutex<Vec<ReferenceOccurrence>>>>,
}

impl Hash for OccurrenceIntrospection {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Replay clones the recorded entry, so a retained allocation is the one identity that
        // stays equal across rounds without depending on how `Content` hashes.
        self.recorded
            .as_ref()
            .map(|recorded| Arc::as_ptr(recorded) as usize)
            .hash(state);
    }
}

impl PartialEq for OccurrenceIntrospection {
    fn eq(&self, other: &Self) -> bool {
        for (recorded, collector) in [
            (&self.recorded, &other.collector),
            (&other.recorded, &self.collector),
        ] {
            if let (Some(recorded), Some(collector)) = (recorded, collector) {
                collector.lock().push((**recorded).clone());
            }
        }
        match (&self.recorded, &other.recorded) {
            (Some(mine), Some(theirs)) => Arc::ptr_eq(mine, theirs),
            (None, None) => true,
            _ => false,
        }
    }
}

impl Introspect for OccurrenceIntrospection {
    /// Observations are not part of what has to settle: reading occurrences is.
    type Output = ();

    fn introspect(&self, _: &mut Engine, _: Tracked<dyn Introspector + '_>) {}

    // A constant output cannot diverge, so this only runs if observations ever join convergence.
    fn diagnose(&self, _: &History<()>) -> SourceDiagnostic {
        warning!(
            Span::detached(),
            "reference occurrences did not settle between compilation rounds"
        )
    }
}

/// Records one occurrence on the compilation's sink.
fn record(engine: &mut Engine, occurrence: ReferenceOccurrence) {
    engine
        .sink
        .introspection(Introspection::new(OccurrenceIntrospection {
            recorded: Some(Arc::new(occurrence)),
            collector: None,
        }));
}

/// Reads every occurrence this compilation recorded on `sink`.
fn recorded(sink: &Sink) -> Vec<ReferenceOccurrence> {
    let collector = Arc::new(Mutex::new(Vec::new()));
    let probe = Introspection::new(OccurrenceIntrospection {
        recorded: None,
        collector: Some(Arc::clone(&collector)),
    });
    for entry in sink.introspections() {
        // The comparison is the read: `OccurrenceIntrospection::eq` moves the payload out.
        let _ = entry == &probe;
    }
    std::mem::take(&mut *collector.lock())
}

/// Moves every record of a child sink into the compilation's sink.
///
/// The wrapper compiles through a sink of its own to read what that compilation recorded, but the
/// outer sink must still end up with everything the unwrapped path would have put there — on the
/// success and the failure path alike.
fn forward(into: &mut Engine, mut sink: Sink) {
    for entry in sink.introspections() {
        into.sink.introspection(entry.clone());
    }
    into.sink.delayed_errors(sink.delayed());
    for warning in sink.clone().warnings() {
        into.sink.warn(warning);
    }
    for (value, styles) in sink.values() {
        into.sink.value(value, styles);
    }
}

/// The rule map Typst built, before [`install`] replaced the observed rule.
///
/// `NativeRuleMap` hashes its function pointers, so replacing a rule changes the library hash and
/// keeps this compilation's cache entries apart from an unobserved one's — no other cache marker
/// is needed. The snapshot has to come from a built library, so this stays a `OnceLock` rather
/// than a `LazyLock`.
static ORIGINAL_RULES: OnceLock<NativeRuleMap> = OnceLock::new();

/// Replaces the rules whose output this module observes.
///
/// Call once, right after a library is built: the first call snapshots the pristine map the
/// observer delegates to, and later calls only reinstall the same observer.
pub(crate) fn install(library: &mut Library) {
    ORIGINAL_RULES.get_or_init(|| library.rules.clone());
    library.rules.replace(Target::Html, observe_html_link);
}

/// Passes a link through its HTML output rule and records what reached it.
fn observe_html_link(
    elem: &Packed<LinkElem>,
    engine: &mut Engine,
    styles: StyleChain,
) -> SourceResult<Content> {
    let element: Content = elem.clone().pack();
    let occurrence = occurrence_of(&element).expect("the observed rule is the link rule");
    let rule = ORIGINAL_RULES
        .get()
        .expect("the observer is installed with the rules it delegates to")
        .get(Target::Html, &element)
        .expect("an HTML link keeps its HTML output rule");
    let output = rule.apply(&element, engine, styles)?;
    record(engine, occurrence);
    Ok(output)
}

/// The occurrences this compilation output's frames carry.
trait FrameOccurrences {
    fn occurrences(&self) -> Vec<ReferenceOccurrence>;
}

impl FrameOccurrences for typst_bundle::Bundle {
    fn occurrences(&self) -> Vec<ReferenceOccurrence> {
        let mut occurrences = Vec::new();
        let mut seen = HashSet::new();
        for file in self.files.values() {
            match file {
                typst_bundle::BundleFile::Document(typst_bundle::BundleDocument::Paged(
                    document,
                    _,
                )) => {
                    // A tag can span a page break, so one tag scope covers every page of its
                    // document.
                    let mut scope = Vec::new();
                    for page in document.pages() {
                        frame_occurrences(&page.frame, &mut scope, &mut seen, &mut occurrences);
                    }
                }
                typst_bundle::BundleFile::Document(typst_bundle::BundleDocument::Html(
                    document,
                )) => {
                    html_frame_occurrences(document.root(), &mut seen, &mut occurrences);
                }
                typst_bundle::BundleFile::Asset(_) => {}
            }
        }
        occurrences
    }
}

impl FrameOccurrences for typst_html::HtmlDocument {
    fn occurrences(&self) -> Vec<ReferenceOccurrence> {
        let mut occurrences = Vec::new();
        html_frame_occurrences(self.root(), &mut HashSet::new(), &mut occurrences);
        occurrences
    }
}

/// Collects the occurrences of one HTML element's `html.frame` subtrees.
fn html_frame_occurrences(
    element: &typst_html::HtmlElement,
    seen: &mut HashSet<Location>,
    occurrences: &mut Vec<ReferenceOccurrence>,
) {
    for node in &element.children {
        match node {
            typst_html::HtmlNode::Element(child) => {
                html_frame_occurrences(child, seen, occurrences);
            }
            typst_html::HtmlNode::Frame(frame) => {
                frame_occurrences(&frame.inner, &mut Vec::new(), seen, occurrences);
            }
            _ => {}
        }
    }
}

/// Collects the occurrences of one frame and the element each marker belongs to.
///
/// `scope` holds the tags still open where this frame begins, and continues across the pages of one
/// document. A paged link wraps its own body in a marker, so a
/// marker belongs to the nearest enclosing `LinkElem` or `RefElem` only when that element produced
/// it: a marker that merely sits inside a link came from a footnote or citation lowering of its
/// own, which this query does not report — the same references the query reported before this
/// channel, where a paged footnote or citation link never was a `link` element.
fn frame_occurrences(
    frame: &Frame,
    scope: &mut Vec<Content>,
    seen: &mut HashSet<Location>,
    occurrences: &mut Vec<ReferenceOccurrence>,
) {
    for (_, item) in frame.items() {
        match item {
            FrameItem::Tag(Tag::Start(content, _)) => {
                scope.push(content.clone());
                if content.is::<LinkMarker>()
                    && let Some(marker) = content.location()
                    && seen.insert(marker)
                    && let Some(occurrence) = producer_occurrence(scope, content)
                {
                    occurrences.push(occurrence);
                }
            }
            FrameItem::Tag(Tag::End(location, _, _)) => {
                if let Some(index) = scope
                    .iter()
                    .rposition(|content| content.location() == Some(*location))
                {
                    scope.truncate(index);
                }
            }
            FrameItem::Group(group) => {
                frame_occurrences(&group.frame, scope, seen, occurrences);
            }
            _ => {}
        }
    }
}

/// The occurrence a marker carries, when a link or ref produced that marker.
///
/// The paged link rule builds its marker around the link's own body, so a link owns a marker only
/// when the marker wraps exactly that body; a footnote or citation marker inside the link's body
/// wraps its own content and is not the link's output. A ref lowers to an untagged direct link, so
/// the nearest enclosing ref is the ref that produced the marker.
fn producer_occurrence(scope: &[Content], marker: &Content) -> Option<ReferenceOccurrence> {
    let owner = scope
        .iter()
        .rev()
        .find(|content| content.is::<LinkElem>() || content.is::<RefElem>())?;
    if let Some(link) = owner.to_packed::<LinkElem>()
        && link.body != marker.to_packed::<LinkMarker>()?.body
    {
        return None;
    }
    occurrence_of(owner)
}

/// An official compilation output together with the occurrences its realization produced.
///
/// Occurrences belong to the round that produced them: one value is created per introspection
/// iteration and dropped with it, so no compilation reads another's occurrences.
#[derive(Debug)]
pub(crate) struct ObservedOutput<T> {
    output: T,
    occurrences: Vec<ReferenceOccurrence>,
}

impl<T> ObservedOutput<T> {
    /// The official output this wrapper observed.
    pub(crate) fn into_inner(self) -> T {
        self.output
    }
}

impl<T: Output + FrameOccurrences + Send + Sync> Output for ObservedOutput<T> {
    fn target() -> Target {
        T::target()
    }

    fn create(engine: &mut Engine, content: &Content, styles: StyleChain) -> SourceResult<Self> {
        let mut sink = Sink::new();
        let output = {
            let mut child = Engine {
                world: engine.world,
                library: engine.library,
                introspector: engine.introspector,
                traced: engine.traced,
                sink: sink.track_mut(),
                route: engine.route.clone(),
            };
            T::create(&mut child, content, styles)
        };
        let observed = recorded(&sink);
        forward(engine, sink);
        let output = output?;
        let mut occurrences = observed;
        occurrences.extend(output.occurrences());
        Ok(Self {
            output,
            occurrences,
        })
    }

    fn introspector(&self) -> &dyn Introspector {
        self
    }
}

impl<T: Output + Send + Sync> Introspector for ObservedOutput<T> {
    /// Answers the two selectors this module owns and delegates every other read.
    ///
    /// Both selectors are matched exactly: they exist for `references()` and its channel check,
    /// not as queryable elements a program could compose into a selector.
    fn query(&self, selector: &Selector) -> EcoVec<Content> {
        if *selector == occurrence_selector() {
            return self
                .occurrences
                .iter()
                .map(|occurrence| {
                    ReferenceOccurrenceElem::new(
                        occurrence.element.clone(),
                        occurrence.destination.clone(),
                        occurrence.location,
                    )
                    .pack()
                })
                .collect();
        }
        if *selector == channel_selector() {
            return EcoVec::from_iter([ReferenceChannelElem::new().pack()]);
        }
        self.output.introspector().query(selector)
    }

    fn query_first(&self, selector: &Selector) -> Option<Content> {
        self.output.introspector().query_first(selector)
    }

    fn query_unique(&self, selector: &Selector) -> typst::diag::StrResult<Content> {
        self.output.introspector().query_unique(selector)
    }

    fn query_label(&self, label: typst::foundations::Label) -> typst::diag::StrResult<&Content> {
        self.output.introspector().query_label(label)
    }

    fn query_labelled(&self) -> EcoVec<Content> {
        self.output.introspector().query_labelled()
    }

    fn query_count_before(&self, selector: &Selector, end: Location) -> usize {
        self.output.introspector().query_count_before(selector, end)
    }

    fn label_count(&self, label: typst::foundations::Label) -> usize {
        self.output.introspector().label_count(label)
    }

    fn locator(&self, key: u128, base: Location) -> Option<Location> {
        self.output.introspector().locator(key, base)
    }

    fn pages(&self, location: Location) -> Option<std::num::NonZeroUsize> {
        self.output.introspector().pages(location)
    }

    fn page(&self, location: Location) -> Option<std::num::NonZeroUsize> {
        self.output.introspector().page(location)
    }

    fn position(&self, location: Location) -> Option<typst::introspection::DocumentPosition> {
        self.output.introspector().position(location)
    }

    fn page_numbering(&self, location: Location) -> Option<&typst::model::Numbering> {
        self.output.introspector().page_numbering(location)
    }

    fn page_supplement(&self, location: Location) -> Option<&Content> {
        self.output.introspector().page_supplement(location)
    }

    fn anchor(&self, location: Location) -> Option<&typst::ecow::EcoString> {
        self.output.introspector().anchor(location)
    }

    fn document(&self, location: Location) -> Option<Location> {
        self.output.introspector().document(location)
    }

    fn path(&self, location: Location) -> Option<&typst::syntax::VirtualPath> {
        self.output.introspector().path(location)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use tempfile::TempDir;
    use typst::engine::{Route, Traced};
    use typst::introspection::EmptyIntrospector;
    use typst::utils::Protected;

    use crate::{BundleCancellation, TypstWorld};

    fn world(source: &str) -> (TempDir, TypstWorld) {
        let directory = TempDir::new().unwrap();
        let main = directory.path().join("main.typ");
        fs::write(&main, source).unwrap();
        let world = TypstWorld::builder(&main, directory.path())
            .with_local_cache()
            .no_fonts()
            .build(&BundleCancellation::default())
            .expect("valid test world");
        (directory, world)
    }

    fn url(occurrence: &ReferenceOccurrence) -> String {
        match &occurrence.destination {
            LinkTarget::Dest(typst::model::Destination::Url(url)) => url.as_str().to_owned(),
            other => panic!("unexpected destination {other:?}"),
        }
    }

    /// What one round records on its sink is what a later round reads back, in order.
    #[test]
    fn recorded_occurrences_read_back_in_order() {
        let (_directory, world) = world("#document(\"index.html\")[Hello]");
        let world_ref: &dyn typst::World = &world;
        let traced = Traced::default();
        let mut sink = Sink::new();
        {
            let mut engine = Engine {
                world: world_ref.track(),
                library: world_ref.library(),
                introspector: Protected::new(EmptyIntrospector.track()),
                traced: traced.track(),
                sink: sink.track_mut(),
                route: Route::default(),
            };
            for (index, destination) in ["/first/", "/second/"].into_iter().enumerate() {
                record(
                    &mut engine,
                    ReferenceOccurrence {
                        element: Content::empty(),
                        destination: LinkTarget::Dest(typst::model::Destination::Url(
                            typst::model::Url::new(destination.to_owned()).unwrap(),
                        )),
                        location: Location::new(index as u128 + 1),
                    },
                );
            }
        }

        let recorded = recorded(&sink);
        assert_eq!(recorded.len(), 2);
        assert_eq!(url(&recorded[0]), "/first/");
        assert_eq!(url(&recorded[1]), "/second/");
    }
}
