//! References resolved from the sealed site output.

pub(crate) mod diagnostic;
mod document;
mod target;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::output::semantics::ResponseMediaType;
use crate::site::SiteIndex;
use tola_address::{OutputPath, SiteReference, SiteUrlMount, UrlPath};

use document::{DocumentReferences, UncheckedBase};

/// All references found in the final HTML outputs of one sealed site.
#[derive(Clone, Default)]
pub struct References(Arc<ReferenceSet>);

#[derive(Default)]
struct ReferenceSet {
    url_mount: SiteUrlMount,
    groups: Vec<Arc<DocumentReferences>>,
    incoming: BTreeMap<UrlPath, Vec<usize>>,
    outgoing: BTreeMap<OutputPath, usize>,
    len: usize,
}

struct ReferenceIter<'a> {
    groups: std::slice::Iter<'a, Arc<DocumentReferences>>,
    current: std::slice::Iter<'a, Reference>,
    remaining: usize,
}

impl<'a> Iterator for ReferenceIter<'a> {
    type Item = &'a Reference;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(reference) = self.current.next() {
                self.remaining -= 1;
                return Some(reference);
            }
            self.current = self.groups.next()?.references.iter();
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for ReferenceIter<'_> {}
impl std::iter::FusedIterator for ReferenceIter<'_> {}

/// The destination class a reference belongs to, used by diagnostics and inspection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceCategory {
    Navigation,
    Resource,
    Fragment,
    External,
}

impl ReferenceCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Navigation => "navigation",
            Self::Resource => "resource",
            Self::Fragment => "fragment",
            Self::External => "external",
        }
    }
}

/// Resolution result for one URL found in final HTML.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReferenceResolution {
    Found { target: ReferenceTarget },
    External,
    Unresolved { reason: UnresolvedReferenceReason },
}

/// The canonical site resource and browser suffix selected by a valid reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceTarget {
    url: UrlPath,
    query: Option<String>,
    fragment: Option<String>,
}

impl ReferenceTarget {
    fn new(url: UrlPath, reference: SiteReference) -> Self {
        let fragment = reference.decoded_fragment().map(Cow::into_owned);
        Self {
            url,
            query: reference.query,
            fragment,
        }
    }

    /// Without a deployment mount, query, or fragment.
    pub fn url(&self) -> &UrlPath {
        &self.url
    }

    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    /// Decoded fragment without the leading hash sign; its meaning belongs to the target media type.
    pub fn fragment(&self) -> Option<&str> {
        self.fragment.as_deref()
    }
}

/// Precise failure that prevented a local reference from resolving.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnresolvedReferenceReason {
    OutsideSiteMount {
        destination: String,
        required_prefix: String,
    },
    InvalidBrowserReference {
        message: String,
    },
    TargetMissing {
        /// Browser address the same route serves under its trailing-slash spelling, when the
        /// author likely meant that address.
        suggestion: Option<String>,
    },
    /// A fragment the target page publishes no element id for.
    ///
    /// Typst assigns an id only to elements something links to, so a heading that no link, ref,
    /// or outline targets has none. A hand-written URL fragment to such a heading fails here even
    /// though the label exists; an explicit `html.h2(id: …)` always publishes the id.
    FragmentMissing {
        fragment: String,
        available: Arc<[String]>,
    },
    DocumentNotAllowed {
        reference: String,
        document: tola_address::UrlPath,
    },
    MediaTypeMismatch {
        reference_use: tola_typst::HtmlReferenceUse,
        required: ResponseMediaType,
        declared: ResponseMediaType,
    },
}

/// One outgoing reference together with its final site-wide resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    page: OutputPath,
    destination: String,
    html_context: String,
    category: ReferenceCategory,
    resolution: ReferenceResolution,
    origin: Option<tola_diagnostic::Location>,
}

impl Reference {
    pub fn page(&self) -> &OutputPath {
        &self.page
    }

    pub fn destination(&self) -> &str {
        &self.destination
    }

    pub fn html_context(&self) -> &str {
        &self.html_context
    }

    /// Absent when the element came from outside the site, such as a package.
    pub fn origin(&self) -> Option<&tola_diagnostic::Location> {
        self.origin.as_ref()
    }

    pub fn category(&self) -> ReferenceCategory {
        self.category
    }

    pub fn resolution(&self) -> &ReferenceResolution {
        &self.resolution
    }
}

impl References {
    pub(crate) fn from_site(
        index: &SiteIndex,
        url_mount: &SiteUrlMount,
        world: &tola_typst::TypstWorld,
        cancellation: &BuildCancellation,
        previous: Option<&References>,
    ) -> Result<Self, BuildCancelled> {
        cancellation.ensure_active()?;
        let previous = previous.filter(|previous| &previous.0.url_mount == url_mount);
        let mut changed_groups = None;
        let mut page_count = 0;
        for (page, inventory) in index.html_pages() {
            cancellation.ensure_active()?;
            let cached = previous.and_then(|previous| {
                previous
                    .0
                    .outgoing
                    .get(&page.output)
                    .map(|&position| &previous.0.groups[position])
            });
            let group = DocumentReferences::read(
                cached,
                index.address(),
                page,
                inventory,
                url_mount,
                world,
                cancellation,
            )?;
            // An unchanged group keeps its predecessor by Arc rather than by copying references.
            let unchanged_prefix = previous
                .and_then(|previous| previous.0.groups.get(page_count))
                .is_some_and(|at_position| Arc::ptr_eq(at_position, &group));
            if changed_groups.is_some() || !unchanged_prefix {
                let groups = changed_groups.get_or_insert_with(|| {
                    previous.map_or_else(Vec::new, |previous| {
                        previous.0.groups[..page_count].to_vec()
                    })
                });
                groups.push(group);
            }
            page_count += 1;
        }
        cancellation.ensure_active()?;
        let groups = match changed_groups {
            Some(groups) => groups,
            None => match previous {
                Some(previous) if previous.0.groups.len() == page_count => {
                    return Ok(previous.clone());
                }
                Some(previous) => previous.0.groups[..page_count].to_vec(),
                None => Vec::new(),
            },
        };
        let mut incoming = BTreeMap::<UrlPath, Vec<usize>>::new();
        let mut outgoing = BTreeMap::new();
        let mut len = 0;
        for (position, group) in groups.iter().enumerate() {
            cancellation.ensure_active()?;
            outgoing.insert(group.output.clone(), position);
            len += group.references.len();
            for url in group.incoming.keys() {
                cancellation.ensure_active()?;
                incoming.entry(url.clone()).or_default().push(position);
            }
        }
        cancellation.ensure_active()?;
        Ok(Self(Arc::new(ReferenceSet {
            url_mount: url_mount.clone(),
            groups,
            incoming,
            outgoing,
            len,
        })))
    }

    pub fn len(&self) -> usize {
        self.0.len
    }

    pub fn is_empty(&self) -> bool {
        self.0.len == 0
    }

    pub fn references(&self) -> impl ExactSizeIterator<Item = &Reference> {
        ReferenceIter {
            groups: self.0.groups.iter(),
            current: [].iter(),
            remaining: self.0.len,
        }
    }

    /// Every page whose document base keeps its references from being checked.
    pub(crate) fn unchecked_bases(&self) -> impl Iterator<Item = (&OutputPath, &UncheckedBase)> {
        self.0.groups.iter().filter_map(|group| {
            group
                .unchecked_base
                .as_ref()
                .map(|base| (&group.output, base))
        })
    }

    /// In page and DOM order.
    pub fn incoming(&self, url: &UrlPath) -> impl Iterator<Item = &Reference> {
        self.0
            .incoming
            .get_key_value(url)
            .into_iter()
            .flat_map(move |(url, groups)| {
                groups.iter().flat_map(move |&position| {
                    let group = &self.0.groups[position];
                    group.incoming[url]
                        .iter()
                        .map(move |&position| &group.references[position])
                })
            })
    }

    /// Includes unresolved references.
    pub fn outgoing(&self, page: &OutputPath) -> impl Iterator<Item = &Reference> {
        self.0
            .outgoing
            .get(page)
            .into_iter()
            .flat_map(|&position| self.0.groups[position].references.iter())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use crate::site::HtmlPage;

    use super::*;

    /// Resolve every reference of `site` without a previous revision.
    fn site_references(
        site: &SiteIndex,
        mount: &SiteUrlMount,
        world: &tola_typst::TypstWorld,
    ) -> References {
        References::from_site(site, mount, world, &BuildCancellation::new(), None).unwrap()
    }

    fn candidate(
        source: &str,
    ) -> anyhow::Result<(
        TempDir,
        SiteIndex,
        crate::output::graph::OutputGraph,
        tola_typst::TypstWorld,
    )> {
        let directory = TempDir::new().unwrap();
        let main = directory.path().join("site.typ");
        std::fs::write(&main, source).unwrap();
        let world = tola_typst::TypstWorld::builder(&main, directory.path())
            .with_local_cache()
            .no_fonts()
            .build(&tola_typst::BundleCancellation::default())
            .unwrap();
        let cancellation = tola_typst::BundleCancellation::default();
        let compilation = tola_typst::compile_bundle_world(&world, &cancellation).unwrap();
        let documents = crate::compiler::documents::from_compilation(&compilation, &cancellation)?;
        let export = compilation.export_default(&cancellation, None).unwrap();
        let mut graph = crate::output::graph::OutputGraphBuilder::new();
        crate::compiler::outputs::insert_bundle(&mut graph, export.entries().iter().cloned())
            .unwrap();
        let graph = graph.finish();
        let site_index = SiteIndex::from_output_graph(&graph, documents)?;
        Ok((directory, site_index, graph, world))
    }

    fn reindex(
        source: &SiteIndex,
        graph: &crate::output::graph::OutputGraph,
        mut update_route: impl FnMut(&mut HtmlPage),
    ) -> SiteIndex {
        let html_outputs = graph
            .outputs()
            .iter()
            .filter(|output| output.kind() == crate::output::graph::OutputKind::HtmlDocument)
            .map(|output| output.path())
            .collect::<std::collections::BTreeSet<_>>();
        let documents = source
            .html_pages()
            .filter(|(page, _)| html_outputs.contains(&page.output))
            .map(|(page, inventory)| {
                let mut page = page.clone();
                update_route(&mut page);
                crate::compiler::documents::RealizedPage {
                    page,
                    inventory: Arc::clone(inventory),
                }
            });
        SiteIndex::from_output_graph(graph, documents).unwrap()
    }

    /// The same site resolved twice: from `previous`, and with no previous graph.
    fn cached_and_fresh(
        site: &SiteIndex,
        mount: &SiteUrlMount,
        previous: &References,
        world: &tola_typst::TypstWorld,
    ) -> (References, References) {
        let cancellation = BuildCancellation::new();
        let fresh = References::from_site(site, mount, world, &cancellation, None).unwrap();
        let cached =
            References::from_site(site, mount, world, &cancellation, Some(previous)).unwrap();
        (cached, fresh)
    }

    /// A retained reference graph answers every query a cold graph answers, after the content
    /// behind it changed.
    #[test]
    fn cached_references_match_fresh() {
        // One row per shape of edit: a shifted source position, a repaired fragment set, and a
        // reloaded reference policy.
        let (_directory, site, _graph, world) =
            candidate("#document(\"index.html\")[\n  #html.a(href: \"/missing/\")[Missing]\n]")
                .unwrap();
        let mount = SiteUrlMount::root();
        let previous = site_references(&site, &mount, &world);
        assert_eq!(
            previous.references().next().unwrap().origin().unwrap().line,
            Some(2)
        );
        let shifted = format!(
            "\n{}",
            "#document(\"index.html\")[\n  #html.a(href: \"/missing/\")[Missing]\n]"
        );
        let (_directory, changed, _graph, world) = candidate(&shifted).unwrap();
        let (cached, fresh) = cached_and_fresh(&changed, &mount, &previous, &world);
        assert_eq!(
            cached.references().collect::<Vec<_>>(),
            fresh.references().collect::<Vec<_>>(),
            "a retained graph answers like a cold one"
        );
        assert_eq!(
            cached.references().next().unwrap().origin().unwrap().line,
            Some(3),
            "a retained graph reports the edited position"
        );

        // Repaired fragments: the retained graph follows every added and removed id.
        let source = |fragments: &str| {
            format!(
                r#"#document("kept.html")[#html.a(href: "/target/#kept")[Kept]]
#document("removed.html")[#html.a(href: "/target/#gone")[Gone]]
#document("missing.html")[
  #html.a(href: "/target/#missing")[First]
  #html.a(href: "/target/#missing")[Second]
]
#document("stable.html")[#html.a(href: "https://example.test/")[External]]
#document("target/index.html")[{fragments}]"#,
            )
        };
        let (_directory, site, _graph, world) = candidate(&source(
            r#"#html.h1(id: "kept")[Kept] #html.h2(id: "gone")[Gone]"#,
        ))
        .unwrap();
        let mut previous = site_references(&site, &mount, &world);
        for fragments in [
            r#"#html.h1(id: "kept")[Kept] #html.h2(id: "gone")[Gone] #html.h2(id: "extra")[Extra]"#,
            r#"#html.h1(id: "kept")[Kept] #html.h2(id: "extra")[Extra]"#,
            r#"#html.h1(id: "kept")[Kept] #html.h2(id: "missing")[Repaired]"#,
        ] {
            let (_directory, changed, _graph, world) = candidate(&source(fragments)).unwrap();
            let (cached, fresh) = cached_and_fresh(&changed, &mount, &previous, &world);
            assert_eq!(
                cached.references().collect::<Vec<_>>(),
                fresh.references().collect::<Vec<_>>(),
                "a retained graph answers like a cold one"
            );
            previous = cached;
        }
        let missing = OutputPath::parse("missing.html").unwrap();
        assert!(
            previous.outgoing(&missing).all(|reference| matches!(
                reference.resolution(),
                ReferenceResolution::Found { .. }
            ))
        );
        let removed = OutputPath::parse("removed.html").unwrap();
        assert!(matches!(
            previous.outgoing(&removed).next().unwrap().resolution(),
            ReferenceResolution::Unresolved {
                reason: UnresolvedReferenceReason::FragmentMissing { .. }
            }
        ));

        // A reloaded policy re-severities the retained failures without re-resolving them.
        use crate::config::ReferenceLevel;
        use tola_diagnostic::Severity;
        let (directory, site, _graph, world) = candidate(
            r#"#document("index.html")[
  #html.a(href: "/missing/")[Missing]
  #html.img(src: "/missing.png")
  #html.a(href: "/target/#missing")[Fragment]
]
#document("target/index.html")[#html.h1(id: "present")[Present]]"#,
        )
        .unwrap();
        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        let previous = site_references(&site, &mount, &world);
        let errors =
            diagnostic::diagnostics(&previous, &config, &BuildCancellation::new()).unwrap();
        assert_eq!(errors.len(), 3);
        assert!(
            errors
                .iter()
                .all(|diagnostic| diagnostic.severity == Severity::Error)
        );
        config.build.references.navigation = ReferenceLevel::Warn;
        config.build.references.resources = ReferenceLevel::Warn;
        config.build.references.fragments = ReferenceLevel::Warn;
        let (cached, fresh) = cached_and_fresh(&site, &mount, &previous, &world);
        assert_eq!(
            cached.references().collect::<Vec<_>>(),
            fresh.references().collect::<Vec<_>>(),
            "the retained failures keep their shape"
        );
        let warnings =
            diagnostic::diagnostics(&cached, &config, &BuildCancellation::new()).unwrap();
        assert_eq!(warnings.len(), 3);
        assert!(
            warnings
                .iter()
                .all(|diagnostic| diagnostic.severity == Severity::Warning)
        );
    }

    #[test]
    fn absolute_url_configured_origin_external() {
        let (_directory, site, _graph, world) = candidate(
            r#"#document("index.html")[#html.a(href: "https://example.test/guide/")[Absolute]]
#document("guide/index.html")[Guide]"#,
        )
        .unwrap();
        let mount = SiteUrlMount::root();
        let page = OutputPath::parse("index.html").unwrap();
        let cancellation = BuildCancellation::new();
        let references = References::from_site(&site, &mount, &world, &cancellation, None).unwrap();
        // The build cannot observe what another site serves on its own origin, so an absolute
        // URL is never resolved against this build's outputs even when it names `site.origin`.
        assert_eq!(
            references.outgoing(&page).next().unwrap().resolution(),
            &ReferenceResolution::External
        );
        // The build proves nothing about what another site serves, so no diagnostic is produced.
        let config = crate::config::tests::load_test_config(
            _directory.path(),
            "[site]\norigin = \"https://example.test\"\n",
        );
        let diagnostics = crate::site::references::diagnostic::diagnostics(
            &references,
            &config,
            &BuildCancellation::new(),
        )
        .unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    /// A reference the build cannot check is diagnosed once, with the code that names why, and
    /// an unresolvable one keeps the authored range it came from.
    #[test]
    fn uncheckable_and_missing_references_are_diagnosed() {
        let (directory, site, _graph, world) = candidate(
            r#"#document("index.html", html.html(
  html.head(html.base(href: "https://other.test/"))
  + html.body(html.a(href: "/missing/")[Missing])
))"#,
        )
        .unwrap();
        let index = site_references(&site, &SiteUrlMount::root(), &world);
        let page = OutputPath::parse("index.html").unwrap();
        assert_eq!(
            index.outgoing(&page).next().unwrap().resolution(),
            &ReferenceResolution::External,
            "an external base resolves every relative reference external"
        );
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let diagnostics =
            diagnostic::diagnostics(&index, &config, &BuildCancellation::new()).unwrap();
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(
            diagnostics[0].code,
            tola_diagnostic::codes::reference::BASE_HREF_EXTERNAL_ORIGIN
        );

        let (directory, site, _graph, world) = candidate(
            r#"#document("index.html")[
  #html.a(href: "/nowhere.html")[Missing]
]"#,
        )
        .unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let index = site_references(&site, &SiteUrlMount::root(), &world);
        let diagnostics =
            diagnostic::diagnostics(&index, &config, &BuildCancellation::new()).unwrap();
        let location = diagnostics
            .first()
            .expect("the missing page is reported")
            .location
            .as_ref()
            .expect("the reference is located in its source");
        assert_eq!(location.path, "site.typ");
        assert_eq!((location.line, location.column), (Some(2), Some(4)));
        let range = location
            .range
            .expect("the element keeps its authored range");
        assert_eq!((range.start.line, range.start.character), (1, 3));
        assert_eq!((range.end.line, range.end.character), (1, 41));
    }

    #[test]
    fn cached_groups_track_alias_precedence() {
        use crate::output::graph::{OutputFile, OutputGraphBuilder};
        use crate::output::semantics::OutputDeclaration;

        let (_directory, source, original, world) = candidate(
            r#"#document("links/index.html")[
  #html.a(href: "/target/index.html?view=card")[Alias]
  #html.a(href: "/target/")[Clean]
  #html.img(src: "/target/index.html")
]
#document("stable.html")[#html.a(href: "/stable-target/")[Stable]]
#document("stable-target/index.html")[Stable]
#document("target/index.html")[Target]"#,
        )
        .unwrap();
        let mount = SiteUrlMount::root();
        for (declaration, route, expected) in [
            (None, None, None),
            (
                Some(OutputDeclaration::html_document()),
                None,
                Some("/target/"),
            ),
            (
                Some(OutputDeclaration::html_document()),
                Some("/moved/"),
                Some("/moved/"),
            ),
            (
                Some(OutputDeclaration::opaque(ResponseMediaType::HTML)),
                None,
                Some("/target/index.html"),
            ),
            (None, None, None),
        ] {
            let mut graph = OutputGraphBuilder::new();
            for output in original.outputs() {
                match output.path().as_str() {
                    "target/index.html" => {
                        if let Some(declaration) = &declaration {
                            graph
                                .insert(OutputFile::from_byte_owner(
                                    output.path().clone(),
                                    declaration.clone(),
                                    output.owner().clone(),
                                    output.bytes_owner(),
                                ))
                                .unwrap();
                        }
                    }
                    _ => graph.insert(output.clone()).unwrap(),
                }
            }
            let graph = graph.finish();
            let site = reindex(&source, &graph, |page| {
                if page.output.as_str() == "target/index.html"
                    && let Some(route) = route
                {
                    page.permalink = UrlPath::parse(route).unwrap();
                }
            });
            let index = site_references(&site, &mount, &world);
            let links = OutputPath::parse("links/index.html").unwrap();
            let alias = index.outgoing(&links).next().unwrap();
            match expected {
                Some(expected) => {
                    let ReferenceResolution::Found { target } = alias.resolution() else {
                        panic!("alias did not resolve: {alias:?}");
                    };
                    assert_eq!(target.url().as_str(), expected);
                    assert_eq!(target.query(), Some("view=card"));
                }
                None => assert_eq!(
                    alias.resolution(),
                    &ReferenceResolution::Unresolved {
                        reason: UnresolvedReferenceReason::TargetMissing { suggestion: None },
                    },
                ),
            }
        }

        // A graph that no longer holds the alias page resolves nothing for it.
        let mut graph = OutputGraphBuilder::new();
        for output in original.outputs().iter().filter(|output| {
            matches!(
                output.path().as_str(),
                "stable.html" | "stable-target/index.html"
            )
        }) {
            graph.insert(output.clone()).unwrap();
        }
        let graph = graph.finish();
        let site = reindex(&source, &graph, |_| {});
        let without_links = site_references(&site, &mount, &world);
        let links = OutputPath::parse("links/index.html").unwrap();
        assert_eq!(without_links.outgoing(&links).count(), 0);
        assert!(site_references(&SiteIndex::default(), &mount, &world).is_empty());
    }

    #[test]
    fn only_typed_uses_see_media_changes() {
        use crate::output::graph::OutputGraphBuilder;
        use crate::output::semantics::OutputDeclaration;

        let (_directory, source, original, world) = candidate(
            r#"#document("typed.html", html.html(html.head(
  html.link(rel: "stylesheet", href: "/typed.bin")
)))
#document("plain.html")[#html.a(href: "/typed.bin")[Download]]"#,
        )
        .unwrap();
        let mount = SiteUrlMount::root();
        let bytes: Arc<[u8]> = Arc::from(b"same response bytes".as_slice());
        for media_type in [
            ResponseMediaType::CSS,
            ResponseMediaType::JAVASCRIPT,
            ResponseMediaType::parse("text/javascript; charset=utf-8").unwrap(),
            ResponseMediaType::CSS,
        ] {
            let mut graph = OutputGraphBuilder::new();
            graph.insert_graph(&original).unwrap();
            graph
                .insert_configured_asset(
                    "mapped-resource",
                    OutputPath::parse("typed.bin").unwrap(),
                    OutputDeclaration::opaque(media_type.clone()),
                    Arc::clone(&bytes),
                )
                .unwrap();
            let graph = graph.finish();
            let output = graph
                .outputs()
                .iter()
                .find(|output| output.path().as_str() == "typed.bin")
                .unwrap();
            assert_eq!(output.bytes(), bytes.as_ref());
            let site = reindex(&source, &graph, |_| {});
            let index = site_references(&site, &mount, &world);
            let typed = OutputPath::parse("typed.html").unwrap();
            let resolution = index.outgoing(&typed).next().unwrap().resolution();
            if media_type == ResponseMediaType::CSS {
                assert!(matches!(resolution, ReferenceResolution::Found { .. }));
            } else {
                assert!(matches!(
                    resolution,
                    ReferenceResolution::Unresolved {
                        reason: UnresolvedReferenceReason::MediaTypeMismatch { declared, .. }
                    } if declared == &media_type
                ));
            }
        }
    }

    #[test]
    fn cancellation_preempts_reference_checks() {
        let (directory, site, _graph, world) =
            candidate(r#"#document("index.html")[#html.a(href: "/missing/")[Missing]]"#).unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let mount = SiteUrlMount::root();
        let cached = site_references(&site, &mount, &world);
        let canceller = crate::cancellation::BuildCanceller::new();
        let cancellation = canceller.token();
        canceller.cancel();

        for previous in [None, Some(&cached)] {
            assert!(matches!(
                References::from_site(&site, &mount, &world, &cancellation, previous),
                Err(BuildCancelled)
            ));
        }
        assert!(matches!(
            diagnostic::diagnostics(&cached, &config, &cancellation),
            Err(BuildCancelled)
        ));
    }

    /// A destination resolves against the final DOM: the anchors a page ends up with, the URL
    /// its escaped filename publishes, and the fragment a media target carries.
    #[test]
    fn destination_resolves_against_final_dom() {
        let (_directory, site_index, _graph, world) = candidate(
            r#"#document("index.html")[
  #html.a(href: "/post/#intro")[Post]
  #html.a(href: "/100%25.html")[Percent-named page]
  #html.a(href: "/c%23notes.html")[Hash-named page]
]
#document("post/index.html")[#html.h1(id: "intro")[Intro]]
#document("100%.html")[Percent-named page]
#document("c#notes.html")[#html.h1(id: "intro")[Intro]]"#,
        )
        .unwrap();
        let references = site_references(&site_index, &SiteUrlMount::root(), &world);
        let page = OutputPath::parse("index.html").unwrap();

        assert_eq!(references.references().len(), 3);
        assert_eq!(
            references
                .outgoing(&page)
                .map(|reference| {
                    reference.html_context().to_owned()
                        + " -> "
                        + match reference.resolution() {
                            ReferenceResolution::Found { target } => target.url().as_str(),
                            resolution => panic!("did not resolve: {resolution:?}"),
                        }
                })
                .collect::<Vec<_>>(),
            [
                "a[href] -> /post/",
                "a[href] -> /100%.html",
                "a[href] -> /c#notes.html",
            ]
        );

        // A fragment a media target owns is kept, not folded into the file's address.
        let (_directory, site_index, _graph, world) = candidate(
            r#"
#document("index.html")[
  #link(<pdf-part>)[PDF section]
  #html.a(href: "/clip.mp4#t=10")[Video]
]
#document("paper.pdf")[#rect(width: 1pt, height: 1pt) <pdf-part>]
#asset("clip.mp4", "video")
"#,
        )
        .unwrap();
        let references = site_references(&site_index, &SiteUrlMount::root(), &world);
        for (reference, path, fragment) in [
            (
                references.references().next().unwrap(),
                "/paper.pdf",
                "pdf-part",
            ),
            (references.references().nth(1).unwrap(), "/clip.mp4", "t=10"),
        ] {
            let ReferenceResolution::Found { target } = reference.resolution() else {
                panic!("expected a resolved media target: {reference:?}");
            };
            assert_eq!(target.url().as_str(), path);
            assert_eq!(target.fragment(), Some(fragment));
        }
    }

    #[test]
    fn backlinks_share_one_canonical_target() {
        let (_directory, site, _graph, world) = candidate(
            r#"#document("index.html")[
  #html.a(href: "/docs/post/index.html?view=card#part%2520x")[Alias]
  #html.a(href: "/docs/post/?view=list#part%2520x")[Clean]
  #html.a(href: "/docs/missing/")[Missing]
]
#document("post/index.html")[#html.h1(id: "part%20x")[Post]]"#,
        )
        .unwrap();
        let references = site_references(
            &site,
            &SiteUrlMount::from_base_path("/docs/").unwrap(),
            &world,
        );
        let url = UrlPath::parse("/post/").unwrap();
        let incoming = references.incoming(&url).collect::<Vec<_>>();
        assert_eq!(incoming.len(), 2);
        for (reference, query) in incoming.into_iter().zip(["view=card", "view=list"]) {
            let ReferenceResolution::Found { target } = reference.resolution() else {
                panic!("expected a resolved backlink");
            };
            assert_eq!(target.url(), &url);
            assert_eq!(target.query(), Some(query));
            assert_eq!(target.fragment(), Some("part%20x"));
            assert_eq!(reference.page().as_str(), "index.html");
        }
        assert_eq!(
            references
                .outgoing(&OutputPath::parse("index.html").unwrap())
                .count(),
            3
        );
        assert!(
            references
                .incoming(&UrlPath::parse("/missing/").unwrap())
                .next()
                .is_none()
        );
    }

    #[test]
    fn fragment_failures_follow_target_requirements() {
        use crate::config::ReferenceLevel;
        use tola_diagnostic::Severity;
        use tola_diagnostic::codes::reference::{
            FRAGMENT_MISSING, NAVIGATION_MISSING, RESOURCE_MISSING,
        };

        for (source, category, code, severity) in [
            (
                r##"#document("index.html")[#html.script(src: "#intro") #html.h1(id: "intro")[Intro]]"##,
                ReferenceCategory::Resource,
                RESOURCE_MISSING,
                Severity::Error,
            ),
            (
                r##"#document("index.html")[#html.script(src: "#missing")]"##,
                ReferenceCategory::Resource,
                RESOURCE_MISSING,
                Severity::Error,
            ),
            (
                r##"#document("index.html", html.html(
  html.head(html.base(href: "/missing/"))
  + html.body(html.a(href: "#intro")[Missing page])
))"##,
                ReferenceCategory::Navigation,
                NAVIGATION_MISSING,
                Severity::Error,
            ),
            (
                r#"#document("index.html")[#html.a(href: "/missing/#fragment")[Missing page]]"#,
                ReferenceCategory::Navigation,
                NAVIGATION_MISSING,
                Severity::Error,
            ),
            (
                r#"#document("index.html")[#html.a(href: "/post/#missing")[Missing fragment]]
#document("post/index.html")[#html.h1(id: "available")[Post]]"#,
                ReferenceCategory::Fragment,
                FRAGMENT_MISSING,
                Severity::Warning,
            ),
        ] {
            let (directory, site_index, _graph, world) = candidate(source).unwrap();
            let mut config = crate::config::tests::load_test_config(directory.path(), "");
            config.build.references.fragments = ReferenceLevel::Warn;
            let index = site_references(&site_index, &SiteUrlMount::root(), &world);
            assert_eq!(index.references().next().unwrap().category(), category);
            let diagnostics =
                diagnostic::diagnostics(&index, &config, &BuildCancellation::new()).unwrap();
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(diagnostics[0].code, code);
            assert_eq!(diagnostics[0].severity, severity);
        }
    }

    #[test]
    fn missing_destination_suggests_its_address() {
        for (mount, href, suggestion) in [
            (SiteUrlMount::root(), "/guide", "/guide/"),
            (
                SiteUrlMount::from_base_path("/docs/").unwrap(),
                "/docs/guide",
                "/docs/guide/",
            ),
            (
                SiteUrlMount::from_base_path("/docs/").unwrap(),
                "/docs/guide?view=card#intro%2520part",
                "/docs/guide/?view=card#intro%2520part",
            ),
            (SiteUrlMount::root(), "/100%25-ready", "/100%25-ready/"),
            (SiteUrlMount::root(), "/a%2541", "/a%2541/"),
        ] {
            let (_directory, site_index, _graph, world) = candidate(&format!(
                r#"#document("index.html")[#html.a(href: "{href}")[Page]]
#document("guide/index.html")[#html.h1(id: "intro%20part")[Guide]]
#document("100%-ready/index.html")[Ready]
#document("a%41/index.html")[Named]"#
            ))
            .unwrap();
            let index = site_references(&site_index, &mount, &world);
            let reference = index.references().next().unwrap();
            let ReferenceResolution::Unresolved { reason } = reference.resolution() else {
                panic!("{href} resolved: {reference:?}");
            };
            let UnresolvedReferenceReason::TargetMissing {
                suggestion: offered,
            } = reason
            else {
                panic!("{href} lost its suggestion: {reason:?}");
            };
            assert_eq!(offered.as_deref(), Some(suggestion), "{href}");
        }
    }

    #[test]
    fn each_missing_fragment_is_reported() {
        let (_directory, site_index, _graph, world) = candidate(
            r#"#document("index.html")[
  #html.a(href: "/post/#missing")[First]
  #html.a(href: "/post/#missing")[Second]
]
#document("post/index.html")[#html.h1(id: "z")[Last] #html.h2(id: "a")[First]]"#,
        )
        .unwrap();
        let index = site_references(&site_index, &SiteUrlMount::root(), &world);
        assert_eq!(index.references().len(), 2);
        for reference in index.references() {
            let ReferenceResolution::Unresolved { reason } = reference.resolution() else {
                panic!("missing fragment resolved");
            };
            let UnresolvedReferenceReason::FragmentMissing {
                fragment,
                available,
            } = reason
            else {
                panic!("missing fragment lost its diagnostic");
            };
            assert_eq!(fragment, "missing");
            assert_eq!(available.as_ref(), ["a", "z"]);
        }
    }

    #[test]
    fn base_href_preserves_resolved_target() {
        for (base, href, query, fragment) in [
            ("/docs/", "target/", None, None),
            (
                "/docs/target/?view=base",
                "#intro",
                Some("view=base"),
                Some("intro"),
            ),
        ] {
            let (_directory, site_index, _graph, world) = candidate(&format!(
                r#"#document("guide/index.html", html.html(
  html.head(html.base(href: "{base}"))
  + html.body(html.a(href: "{href}")[Target])
))
#document("docs/target/index.html")[#html.h1(id: "intro")[Intro]]"#,
            ))
            .unwrap();
            let index = site_references(&site_index, &SiteUrlMount::root(), &world);
            let ReferenceResolution::Found { target } =
                index.references().next().unwrap().resolution()
            else {
                panic!("{href} did not resolve against {base}");
            };
            assert_eq!(target.url().as_str(), "/docs/target/");
            assert_eq!(target.query(), query);
            assert_eq!(target.fragment(), fragment);
        }
    }

    #[test]
    fn mount_rejects_urls_outside_its_prefix() {
        let (_directory, site_index, _graph, world) = candidate(
            r#"#document("index.html")[
  #html.a(href: "/target/")[Outside mount]
  #html.a(href: "/docs/target/")[Mounted]
  #html.a(href: "target/")[Relative]
]
#document("target/index.html")[Target]"#,
        )
        .unwrap();

        let index = site_references(
            &site_index,
            &SiteUrlMount::from_base_path("/docs/").unwrap(),
            &world,
        );

        assert!(matches!(
            index.references().next().unwrap().resolution(),
            ReferenceResolution::Unresolved {
                reason: UnresolvedReferenceReason::OutsideSiteMount { required_prefix, .. }
            } if required_prefix == "/docs/"
        ));
        assert!(matches!(
            index.references().nth(1).unwrap().resolution(),
            ReferenceResolution::Found { .. }
        ));
        assert!(matches!(
            index.references().nth(2).unwrap().resolution(),
            ReferenceResolution::Found { .. }
        ));
    }

    #[test]
    fn img_refuses_document_targets() {
        let (_directory, site_index, _graph, world) = candidate(
            r#"#document("index.html")[
  #html.object(data: "/embedded/")[]
  #html.img(src: "/embedded/")
]
#document("embedded/index.html")[Embedded document]"#,
        )
        .unwrap();

        let index = site_references(&site_index, &SiteUrlMount::root(), &world);

        assert_eq!(
            index.references().next().unwrap().category(),
            ReferenceCategory::Resource
        );
        assert!(matches!(
            index.references().next().unwrap().resolution(),
            ReferenceResolution::Found { .. }
        ));
        assert_eq!(
            index.references().nth(1).unwrap().category(),
            ReferenceCategory::Resource
        );
        assert!(matches!(
            index.references().nth(1).unwrap().resolution(),
            ReferenceResolution::Unresolved {
                reason: UnresolvedReferenceReason::DocumentNotAllowed { document, .. }
            } if document.as_str() == "/embedded/"
        ));
    }

    #[test]
    fn prefetch_never_becomes_navigation() {
        let (_directory, site_index, _graph, world) = candidate(
            r#"#document("index.html", html.html(html.head(
  html.elem("link", attrs: (rel: "PrEfEtCh prefetch", href: "/docs/next/"))
  + html.link(rel: "prefetch", href: "/docs/download.txt")
  + html.link(rel: "prefetch", href: "/docs/missing/")
)))
#document("next/index.html")[Next]
#asset("download.txt", "Download")"#,
        )
        .unwrap();
        let index = site_references(
            &site_index,
            &SiteUrlMount::from_base_path("/docs/").unwrap(),
            &world,
        );
        let references = index.references().collect::<Vec<_>>();
        assert_eq!(references.len(), 3);
        for (reference, expected) in references[..2].iter().zip(["/next/", "/download.txt"]) {
            assert_eq!(reference.category(), ReferenceCategory::Resource);
            let ReferenceResolution::Found { target } = reference.resolution() else {
                panic!("prefetch did not resolve: {reference:?}");
            };
            assert_eq!(target.url().as_str(), expected);
        }
        assert_eq!(references[2].category(), ReferenceCategory::Resource);
        assert!(matches!(
            references[2].resolution(),
            ReferenceResolution::Unresolved {
                reason: UnresolvedReferenceReason::TargetMissing { .. }
            }
        ));

        // A prefetch relation never relaxes the destination each use already requires.
        let (_directory, site_index, _graph, world) = candidate(
            r#"#document("index.html", html.html(html.head(
  html.link(rel: ("stylesheet", "prefetch"), href: "/next/")
  + html.link(rel: ("prefetch", "modulepreload"), href: "/next/")
  + html.link(rel: ("icon", "prefetch"), href: "/next/")
  + html.link(rel: ("preload", "prefetch"), href: "/next/")
  + html.script(src: "/next/")
  + html.script(type: "module", src: "/next/")
) + html.body(html.img(src: "/next/"))))
#document("next/index.html")[Next]"#,
        )
        .unwrap();
        let index = site_references(&site_index, &SiteUrlMount::root(), &world);
        assert_eq!(index.references().len(), 7);
        for reference in index.references() {
            assert_eq!(reference.category(), ReferenceCategory::Resource);
            assert!(
                matches!(
                    reference.resolution(),
                    ReferenceResolution::Unresolved {
                        reason: UnresolvedReferenceReason::DocumentNotAllowed { document, .. }
                    } if document.as_str() == "/next/"
                ),
                "{reference:?}"
            );
        }
    }

    #[test]
    fn typed_uses_report_media_type_mismatches() {
        let (_directory, site_index, _graph, world) = candidate(
            r#"#document("index.html", html.html(
  html.head(
    html.link(rel: "stylesheet", href: "/opaque.bin")
    + html.link(rel: "stylesheet", href: "/module.js")
    + html.script(src: "/site.css")
  )
))
#asset("opaque.bin", "")
#asset("module.js", "")
#asset("site.css", "")"#,
        )
        .unwrap();

        let index = site_references(&site_index, &SiteUrlMount::root(), &world);
        let failures = index
            .references()
            .map(|reference| match reference.resolution() {
                ReferenceResolution::Unresolved {
                    reason:
                        UnresolvedReferenceReason::MediaTypeMismatch {
                            reference_use,
                            required,
                            declared,
                        },
                } => (*reference_use, required.clone(), declared.clone()),
                resolution => panic!("expected media-type mismatch, got {resolution:?}"),
            })
            .collect::<Vec<_>>();

        assert_eq!(
            failures,
            [
                (
                    tola_typst::HtmlReferenceUse::Stylesheet,
                    ResponseMediaType::CSS,
                    ResponseMediaType::OCTET_STREAM,
                ),
                (
                    tola_typst::HtmlReferenceUse::Stylesheet,
                    ResponseMediaType::CSS,
                    ResponseMediaType::JAVASCRIPT,
                ),
                (
                    tola_typst::HtmlReferenceUse::ClassicScript,
                    ResponseMediaType::JAVASCRIPT,
                    ResponseMediaType::CSS,
                ),
            ]
        );
    }

    #[test]
    fn external_uses_need_no_declaration() {
        let (_directory, site_index, _graph, world) = candidate(
            r#"#document("index.html", html.html(
  html.head(
    html.link(rel: "stylesheet", href: "https://cdn.example/site")
    + html.script(type: "module", src: "//cdn.example/module")
  )
))"#,
        )
        .unwrap();

        let index = site_references(&site_index, &SiteUrlMount::root(), &world);

        assert_eq!(index.references().len(), 2);
        assert!(index.references().all(|reference| {
            reference.category() == ReferenceCategory::External
                && reference.resolution() == &ReferenceResolution::External
        }));
    }
}
