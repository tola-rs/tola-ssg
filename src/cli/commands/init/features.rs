//! Scaffold features and their generated files, hooks, and Typst code.

use std::borrow::Cow;
use std::collections::BTreeSet;

/// One piece of scaffold a selection can add.
///
/// Declaration order keeps generated files deterministic regardless of selection order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Feature {
    /// A starter stylesheet at `static/web-assets/css/site.css`, linked from every page.
    StarterStylesheet,
    /// Canonical link entries honoring each page's published address.
    Canonical,
    /// Feed declarations that honor each page's published metadata.
    Feed,
    /// Sitemap declarations that honor each page's metadata.
    Sitemap,
    /// Open Graph head entries naming the page and the site's social image.
    OpenGraph,
    /// Twitter card head entries naming the page and the site's social image.
    TwitterCard,
    /// A Pagefind search index and the search box that reads it.
    Pagefind,
    /// The Deno toolchain files and their install step.
    DenoToolchain,
    /// A Tailwind stylesheet input, its published mapping, and the hook that builds it.
    TailwindCss,
}

/// A selection of features, iterated in [`Feature`] declaration order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct FeatureSet(BTreeSet<Feature>);

impl FeatureSet {
    pub(super) fn new(features: impl IntoIterator<Item = Feature>) -> Self {
        Self(features.into_iter().collect())
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = Feature> + '_ {
        self.0.iter().copied()
    }

    pub(super) fn contains(&self, feature: Feature) -> bool {
        self.0.contains(&feature)
    }

    pub(super) fn insert(&mut self, feature: Feature) {
        self.0.insert(feature);
    }

    pub(super) fn remove(&mut self, feature: Feature) {
        self.0.remove(&feature);
    }
}

/// One output position the scaffold can fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SlotId {
    /// The stylesheet every linked page has, at `static/web-assets/css/site.css`.
    Stylesheet,
    /// The canonical link entries.
    Canonical,
    /// The feed at `feed.xml`.
    Feed,
    /// The sitemap at `sitemap.xml`.
    Sitemap,
    /// The Open Graph head entries.
    OpenGraph,
    /// The Twitter card head entries.
    TwitterCard,
    /// The search index and the search box that reads it.
    Search,
    /// The toolchain used by Tailwind and Pagefind.
    Runner,
}

/// One output position and the features that can fill it.
pub(super) struct Slot {
    pub(super) id: SlotId,
    /// Later providers replace earlier ones in the same slot.
    pub(super) providers: &'static [Feature],
}

/// The output positions the model declares.
pub(super) const SLOTS: &[Slot] = &[
    Slot {
        id: SlotId::Stylesheet,
        providers: &[Feature::StarterStylesheet, Feature::TailwindCss],
    },
    Slot {
        id: SlotId::Canonical,
        providers: &[Feature::Canonical],
    },
    Slot {
        id: SlotId::Feed,
        providers: &[Feature::Feed],
    },
    Slot {
        id: SlotId::Sitemap,
        providers: &[Feature::Sitemap],
    },
    Slot {
        id: SlotId::OpenGraph,
        providers: &[Feature::OpenGraph],
    },
    Slot {
        id: SlotId::TwitterCard,
        providers: &[Feature::TwitterCard],
    },
    Slot {
        id: SlotId::Search,
        providers: &[Feature::Pagefind],
    },
    Slot {
        id: SlotId::Runner,
        providers: &[Feature::DenoToolchain],
    },
];

/// The features filling `slot`, in declaration order.
pub(super) fn providers(slot: SlotId) -> &'static [Feature] {
    SLOTS
        .iter()
        .find(|entry| entry.id == slot)
        .expect("every slot is declared")
        .providers
}

/// A named feature set the prompt and the presets offer.
pub(super) struct Preset {
    pub(super) name: &'static str,
    pub(super) composition: &'static str,
    pub(super) features: &'static [Feature],
    /// Whether the prompt marks the preset as its recommended starting point.
    pub(super) recommended: bool,
}

/// The presets from the smallest scaffold upward; `rich`'s justfile is derived, not selected.
pub(super) const PRESETS: &[Preset] = &[
    Preset {
        name: "minimal",
        composition: "a minimal site",
        features: &[],
        recommended: false,
    },
    Preset {
        name: "medium",
        composition: "minimal + stylesheet, feed, sitemap",
        features: &[Feature::StarterStylesheet, Feature::Feed, Feature::Sitemap],
        recommended: false,
    },
    Preset {
        name: "rich",
        composition: "medium + deno, tailwind, justfile",
        features: &[
            Feature::StarterStylesheet,
            Feature::Feed,
            Feature::Sitemap,
            Feature::DenoToolchain,
            Feature::TailwindCss,
        ],
        recommended: true,
    },
];

/// The preset values the `--preset` flag offers, in declaration order, each as its name and its
/// terse composition.
pub(in crate::cli) fn preset_values() -> impl Iterator<Item = (&'static str, &'static str)> {
    PRESETS
        .iter()
        .map(|preset| (preset.name, preset.composition))
}

/// The presets in the order the interactive surfaces offer them, richest first, so the offered
/// position is the `1`, `2`, `3` key that applies one.
pub(super) fn preset_choices() -> impl Iterator<Item = &'static Preset> {
    PRESETS.iter().rev()
}

/// The preset a name selects; `name` must be a preset name.
pub(super) fn preset(name: &str) -> &'static Preset {
    PRESETS
        .iter()
        .find(|preset| preset.name == name)
        .expect("every preset name selects a preset")
}

/// The features a preset name selects; `name` must be a preset name.
pub(super) fn features(name: &str) -> FeatureSet {
    FeatureSet::new(preset(name).features.iter().copied())
}

/// One feature as the `--features` flag names it.
struct FeatureValue {
    feature: Feature,
    /// The kebab-case name the flag and diagnostics spell.
    name: &'static str,
    /// The one-line summary the flag's help shows.
    summary: &'static str,
}

/// The feature names the `--features` flag offers, in [`Feature`] declaration order; the spellings
/// here are the only ones diagnostics use.
const FEATURE_VALUES: &[FeatureValue] = &[
    FeatureValue {
        feature: Feature::StarterStylesheet,
        name: "starter-stylesheet",
        summary: "the starter stylesheet every page links",
    },
    FeatureValue {
        feature: Feature::Canonical,
        name: "canonical",
        summary: "a canonical link for every page the site addresses",
    },
    FeatureValue {
        feature: Feature::Feed,
        name: "feed",
        summary: "a feed built from each page's published metadata",
    },
    FeatureValue {
        feature: Feature::Sitemap,
        name: "sitemap",
        summary: "a sitemap built from each page's metadata",
    },
    FeatureValue {
        feature: Feature::OpenGraph,
        name: "open-graph",
        summary: "Open Graph entries for each page and the site's social image",
    },
    FeatureValue {
        feature: Feature::TwitterCard,
        name: "twitter-card",
        summary: "a Twitter card for each page and the site's social image",
    },
    FeatureValue {
        feature: Feature::Pagefind,
        name: "pagefind",
        summary: "a search index built from the published pages",
    },
    FeatureValue {
        feature: Feature::DenoToolchain,
        name: "deno-toolchain",
        summary: "the Deno configuration and its install step",
    },
    FeatureValue {
        feature: Feature::TailwindCss,
        name: "tailwind-css",
        summary: "a Tailwind input stylesheet and the build hook",
    },
];

/// The feature values the `--features` flag offers, in declaration order, each as its name and its
/// one-line summary.
pub(in crate::cli) fn feature_values() -> impl Iterator<Item = (&'static str, &'static str)> {
    FEATURE_VALUES
        .iter()
        .map(|value| (value.name, value.summary))
}

/// The feature a name selects; `name` must be a feature name.
pub(super) fn feature(name: &str) -> Feature {
    FEATURE_VALUES
        .iter()
        .find(|value| value.name == name)
        .expect("every feature name selects a feature")
        .feature
}

/// The kebab-case name a diagnostic spells `feature` with.
pub(super) fn feature_name(feature: Feature) -> &'static str {
    FEATURE_VALUES
        .iter()
        .find(|value| value.feature == feature)
        .expect("every feature is named")
        .name
}

/// Every feature the scaffold can select, in [`Feature`] declaration order.
pub(super) fn selectable_features() -> impl Iterator<Item = Feature> {
    FEATURE_VALUES.iter().map(|value| value.feature)
}

/// The name of the preset with the same canonical features as `set`, if any; otherwise the
/// selection is custom.
pub(super) fn preset_name(set: &FeatureSet) -> Option<&'static str> {
    let canonical = canon(set);
    PRESETS
        .iter()
        .find(|preset| canonical == canon(&FeatureSet::new(preset.features.iter().copied())))
        .map(|preset| preset.name)
}

/// How the report names `set`: the preset with the same canonical features and its
/// composition, or `custom` with how many of the selectable features the selection holds.
pub(super) fn selection_label(set: &FeatureSet) -> String {
    let canonical = canon(set);
    match preset_name(&canonical) {
        Some(name) => format!("{name} ({})", preset(name).composition),
        None => format!(
            "custom ({}/{})",
            canonical.iter().count(),
            selectable_count()
        ),
    }
}

/// How many features the scaffold can select.
fn selectable_count() -> usize {
    selectable_features().count()
}

/// One row group the interactive surfaces show; the grouping lives here only.
pub(super) struct Group {
    pub(super) name: &'static str,
    pub(super) slots: &'static [SlotId],
}

pub(super) const GROUPS: &[Group] = &[
    Group {
        name: "Styling",
        slots: &[SlotId::Stylesheet],
    },
    Group {
        name: "SEO",
        slots: &[
            SlotId::Canonical,
            SlotId::Feed,
            SlotId::Sitemap,
            SlotId::OpenGraph,
            SlotId::TwitterCard,
        ],
    },
    Group {
        name: "Search",
        slots: &[SlotId::Search],
    },
    Group {
        name: "Tooling",
        slots: &[SlotId::Runner],
    },
];

/// What one feature adds to `site/seo.typ`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SeoCode {
    /// The sources the feature's code reads from, each with the names it takes from it.
    pub(super) imports: &'static [(&'static str, &'static [&'static str])],
    /// The guarded lines the feature adds inside `head-entries`.
    pub(super) entries: &'static str,
    /// The shared values those lines read; `head-entries` defines each of them once.
    pub(super) prelude: &'static [SeoValue],
    /// The output the feature declares; `None` when it declares none.
    pub(super) output: Option<SeoOutput>,
}

/// One output `site/seo.typ` declares and the root program calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SeoOutput {
    /// The function `site/seo.typ` exports for this output.
    pub(super) export: &'static str,
    /// The output file the root program declares, such as `feed.xml`.
    pub(super) file: &'static str,
    /// The function's own definition.
    pub(super) definition: &'static str,
}

/// The `site/seo.typ` code of a feature that contributes none.
const NO_SEO: SeoCode = SeoCode {
    imports: &[],
    entries: "",
    prelude: &[],
    output: None,
};

/// One value `head-entries` defines for the features that read it, so the page's own values are
/// read once instead of once per feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SeoValue {
    /// The page's absolute URL, when the site names an origin.
    Url,
    /// The site's social image as an absolute URL, when it names one.
    Image,
    /// The page's title as plain text.
    Title,
    /// The page's description as plain text, `none` when it has none.
    Description,
}

impl SeoValue {
    /// The names this value reads, each with the source it comes from. A value and its import
    /// travel together: a feature that asks for the value gets the import without naming it.
    pub(super) fn imports(self) -> &'static [(&'static str, &'static [&'static str])] {
        match self {
            Self::Url => &[("@tola/address:0.0.0", &["output-to-url"])],
            Self::Image => &[],
            Self::Title | Self::Description => &[("@tola/web:0.0.0", &["plain-text"])],
        }
    }

    /// The `let` bindings `head-entries` writes for this value, indented inside its block.
    pub(super) fn bindings(self) -> &'static str {
        match self {
            Self::Url => {
                "    let url = if site.origin == none { none } else { output-to-url(page.output, origin: site.origin) }\n"
            }
            Self::Image => {
                "    let social = site.extra.at(\"social-image\", default: \"\")\n    \
                 let image = if social == \"\" or site.origin == none { none } else if social.starts-with(\"/\") { site.origin + social } else { social }\n"
            }
            Self::Title => {
                "    let meta = page.source.meta\n    \
                 let title = if meta.title == none { \"\" } else { plain-text(meta.title) }\n"
            }
            Self::Description => {
                "    let description = if meta.description == none { none } else { plain-text(meta.description) }\n"
            }
        }
    }
}

/// One `justfile` recipe a feature contributes to the derived site `justfile`.
#[derive(Clone, Copy)]
pub(super) struct Recipe {
    /// The comment above the recipe, without the leading `# `.
    summary: &'static str,
    /// The recipe name, ending with the colon in the file.
    name: &'static str,
    /// The recipe's command lines, indented by the derived text.
    commands: &'static [&'static str],
}

impl Recipe {
    /// This recipe as it appears in the derived `justfile`, without a trailing newline.
    fn text(self) -> String {
        let commands = self
            .commands
            .iter()
            .map(|command| format!("    {command}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("# {}\n{}:\n{commands}", self.summary, self.name)
    }
}

const DENO_SETUP_RECIPE: Recipe = Recipe {
    summary: "Install the site tools.",
    name: "setup",
    commands: &["deno install"],
};

const TAILWIND_CSS_RECIPE: Recipe = Recipe {
    summary: "Build the site's stylesheet; the tailwind before-build hook runs this recipe.",
    name: "css",
    commands: &["deno task css"],
};

/// The preset a non-interactive invocation writes when no name is given: the first the model
/// offers, in display order.
pub(super) const DEFAULT_PRESET: &str = "minimal";

/// The files the selection decides, in the order the Files group lists them: the derived `justfile`
/// (a recipe-contributing feature writes it), then every feature's staged files in declaration
/// order. Files init writes unconditionally (`tola.toml`, the site sources) are not listed: the
/// selection never decides them.
pub(super) fn decided_files() -> Vec<&'static str> {
    let mut files = vec!["justfile"];
    for feature in selectable_features() {
        for (name, _) in feature.files() {
            if !files.contains(name) {
                files.push(name);
            }
        }
    }
    files
}

/// Every feature that can write `file`, in declaration order: the recipe contributors for the
/// derived `justfile`, the features whose staged files hold the path otherwise.
pub(super) fn file_writers(file: &'static str) -> Vec<Feature> {
    selectable_features()
        .filter(|feature| writes(*feature, file))
        .collect()
}

/// The names of the selected features that write `file`, in declaration order.
pub(super) fn writers(file: &'static str, canonical: &FeatureSet) -> Vec<&'static str> {
    file_writers(file)
        .into_iter()
        .filter(|feature| canonical.contains(*feature))
        .map(feature_name)
        .collect()
}

/// `selected` has already resolved competing providers.
pub(super) fn file_selected(file: &'static str, selected: &FeatureSet) -> bool {
    selected.iter().any(|feature| writes(feature, file))
}

/// Whether `feature` writes `file`: a recipe contributor for the derived `justfile`, a staged file
/// with the path otherwise.
fn writes(feature: Feature, file: &'static str) -> bool {
    if file == "justfile" {
        !feature.recipes().is_empty()
    } else {
        feature.files().iter().any(|(name, _)| *name == file)
    }
}

/// The site `justfile` the selected features' recipes derive, if any contributes one.
fn justfile_source(recipes: &[Recipe]) -> Option<String> {
    if recipes.is_empty() {
        return None;
    }
    let blocks = recipes
        .iter()
        .map(|recipe| recipe.text())
        .collect::<Vec<_>>();
    Some(format!("{}\n", blocks.join("\n\n")))
}

/// Typst code one feature adds to the page templates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HeadFragment {
    /// The code added to the template's import block; empty when the fragment needs no import.
    pub(super) import: &'static str,
    /// The code added to the template's head; empty when the fragment contributes no entry.
    pub(super) head: &'static str,
}

/// The starter stylesheet every linked page has: `asset-url` imported and the published URL
/// linked.
const STARTER_STYLESHEET_HEAD: HeadFragment = HeadFragment {
    import: "#import \"@tola/address:0.0.0\": asset-url",
    head: "#html.link(rel: \"stylesheet\", href: asset-url(\"/assets/css/site.css\"))",
};

/// The Tailwind stylesheet every linked page has: `asset-url` imported and the published URL
/// linked.
const TAILWIND_STYLESHEET_HEAD: HeadFragment = HeadFragment {
    import: "#import \"@tola/address:0.0.0\": asset-url",
    head: "#html.link(rel: \"stylesheet\", href: asset-url(\"/assets/tailwind-output/site.css\"))",
};

/// The canonical link one page has.
const CANONICAL_ENTRIES: &str = r#"
    if url != none {
      entries += canonical(url)
    }"#;

/// The Open Graph entries one page has, silent without a social image or a page title.
const OPEN_GRAPH_ENTRIES: &str = r#"
    if url != none and image != none and title != "" {
      entries += open-graph(
        title: title,
        kind: "website",
        url: url,
        description: description,
        site-name: site.title,
        locale: site.language.tag,
        images: ((url: image, alt: title),),
      )
    }"#;

/// The Twitter card entries one page has, silent under the same conditions as Open Graph.
const TWITTER_CARD_ENTRIES: &str = r#"
    if url != none and image != none and title != "" {
      entries += twitter-card(
        card: "summary_large_image",
        title: title,
        description: description,
        image: (url: image, alt: title),
      )
    }"#;

/// The search entries each page has; the search box itself belongs to one page.
const SEARCH_ENTRIES: &str = r#"
    entries += search-head()"#;

/// What one feature stages and changes.
struct Effect {
    /// Files this feature stages, each as its path and contents.
    files: &'static [(&'static str, &'static str)],
    /// Lines this feature adds to `.gitignore` and `.ignore`.
    ignore: &'static [&'static str],
    /// The page-template code this feature injects.
    head: &'static [HeadFragment],
    /// Whether published asset URLs get a content-derived `?h=…`.
    cache_busting: bool,
    /// The hooks this feature declares.
    hooks: &'static [Hook],
    /// The site `justfile` recipes this feature contributes.
    recipes: &'static [Recipe],
    deno: Option<DenoTask>,
    /// The `site/seo.typ` code this feature contributes.
    seo: SeoCode,
    /// Steps the author must run once in a new site.
    next_steps: &'static [&'static str],
}

struct DenoTask {
    name: &'static str,
    command: &'static str,
    imports: &'static [(&'static str, &'static str)],
}

/// The build stage one hook declaration runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HookStage {
    /// Generates declared site inputs before source discovery.
    BeforeBuild,
    /// Produces declared final output trees after the site program compiles.
    GenerateOutputs,
}

/// One hook, as the configuration declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Hook {
    pub(super) stage: HookStage,
    pub(super) name: &'static str,
    pub(super) command: &'static [&'static str],
    pub(super) rerun_on: &'static [&'static str],
    /// The stage's declared outputs: files or directories a `before-build` command writes, or
    /// final output trees a `generate-outputs` command adds to the graph.
    pub(super) outputs: &'static [&'static str],
}

impl Feature {
    /// The files this feature's effect writes, in declaration order.
    pub(super) fn files(self) -> &'static [(&'static str, &'static str)] {
        self.effect().files
    }

    /// The site `justfile` recipes this feature contributes, in declaration order.
    pub(super) fn recipes(self) -> &'static [Recipe] {
        self.effect().recipes
    }

    fn effect(self) -> &'static Effect {
        match self {
            Feature::StarterStylesheet => &STARTER_STYLESHEET,
            Feature::Canonical => &CANONICAL,
            Feature::Feed => &FEED,
            Feature::Sitemap => &SITEMAP,
            Feature::OpenGraph => &OPEN_GRAPH,
            Feature::TwitterCard => &TWITTER_CARD,
            Feature::Pagefind => &PAGEFIND,
            Feature::TailwindCss => &TAILWIND_CSS,
            Feature::DenoToolchain => &DENO_TOOLCHAIN,
        }
    }

    /// The slot this feature fills; every selectable feature fills exactly one.
    pub(super) fn slot(self) -> SlotId {
        SLOTS
            .iter()
            .find(|slot| slot.providers.contains(&self))
            .expect("every selectable feature fills a slot")
            .id
    }
}

/// The effect of a feature that stages nothing and changes nothing; each feature declares only the
/// fields it fills.
const NO_EFFECT: Effect = Effect {
    files: &[],
    ignore: &[],
    head: &[],
    cache_busting: false,
    hooks: &[],
    recipes: &[],
    deno: None,
    seo: NO_SEO,
    next_steps: &[],
};

const STARTER_STYLESHEET: Effect = Effect {
    files: &[(
        "static/web-assets/css/site.css",
        include_str!("templates/stylesheet.css"),
    )],
    head: &[STARTER_STYLESHEET_HEAD],
    next_steps: &["Edit the starter stylesheet `static/web-assets/css/site.css`."],
    ..NO_EFFECT
};

const CANONICAL: Effect = Effect {
    seo: SeoCode {
        imports: &[("@tola/web:0.0.0", &["canonical"])],
        entries: CANONICAL_ENTRIES,
        prelude: &[SeoValue::Url],
        ..NO_SEO
    },
    next_steps: &["Set `site.origin` for the canonical link on every page."],
    ..NO_EFFECT
};

const FEED_OUTPUT: SeoOutput = SeoOutput {
    export: "feed-output",
    file: "feed.xml",
    definition: r#"/// Declare the feed output `output` for `pages`.
///
/// The feed publishes once `site.origin` and `site.title` name the site; a site without them
/// declares no feed. Entries keep source order; ordering is the site's to decide.
///
/// Each entry publishes the page's whole document unless the page sets `feed-content`;
/// `feed-summary` fills the entry's summary.
///
/// - output (string): the output file the feed is published at.
/// - pages (array): the `(source:, output:)` records `select-pages` returns.
/// -> content
#let feed-output(output, pages) = {
  if site.origin == none or site.title == "" { return }
  feed(
    output: output,
    entries: pages
      .filter(page => page.source.meta.published != none and page.source.meta.feed)
      .map(page => (
        id: page.source.meta.id,
        target: page.output,
        published: page.source.meta.published,
        updated: page.source.meta.updated,
        summary: page.source.meta.feed-summary,
        content: if page.source.meta.feed-content == none {
          (document: page.output)
        } else {
          page.source.meta.feed-content
        },
      )),
  )
}"#,
};

const FEED: Effect = Effect {
    seo: SeoCode {
        imports: &[("@tola/web:0.0.0", &["feed"])],
        output: Some(FEED_OUTPUT),
        ..NO_SEO
    },
    next_steps: &["Set `site.origin` and `site.title` for `feed.xml`."],
    ..NO_EFFECT
};

const SITEMAP_OUTPUT: SeoOutput = SeoOutput {
    export: "sitemap-output",
    file: "sitemap.xml",
    definition: r#"/// Declare the sitemap output `output` for `pages`.
///
/// The sitemap needs `site.origin`. Targets keep source order, dated by `updated` where a page
/// sets one.
///
/// - output (string): the output file the sitemap is published at.
/// - pages (array): the `(source:, output:)` records `select-pages` returns.
/// -> content
#let sitemap-output(output, pages) = {
  if site.origin == none { return }
  sitemap(
    output: output,
    targets: pages
      .filter(page => page.source.meta.sitemap)
      .map(page => (target: page.output, lastmod: page.source.meta.updated)),
  )
}"#,
};

const SITEMAP: Effect = Effect {
    seo: SeoCode {
        imports: &[("@tola/web:0.0.0", &["sitemap"])],
        output: Some(SITEMAP_OUTPUT),
        ..NO_SEO
    },
    next_steps: &["Set `site.origin` for `sitemap.xml`."],
    ..NO_EFFECT
};

/// The values a card's entries read from the page and the site.
const CARD_VALUES: &[SeoValue] = &[
    SeoValue::Url,
    SeoValue::Image,
    SeoValue::Title,
    SeoValue::Description,
];

const OPEN_GRAPH: Effect = Effect {
    seo: SeoCode {
        imports: &[("@tola/web:0.0.0", &["open-graph"])],
        entries: OPEN_GRAPH_ENTRIES,
        prelude: CARD_VALUES,
        ..NO_SEO
    },
    next_steps: &["Set `site.origin` and `site.extra.social-image` for the cards."],
    ..NO_EFFECT
};

const TWITTER_CARD: Effect = Effect {
    seo: SeoCode {
        imports: &[("@tola/web:0.0.0", &["twitter-card"])],
        entries: TWITTER_CARD_ENTRIES,
        prelude: CARD_VALUES,
        ..NO_SEO
    },
    next_steps: &["Set `site.origin` and `site.extra.social-image` for the cards."],
    ..NO_EFFECT
};

const PAGEFIND_RECIPE: Recipe = Recipe {
    summary: "Build the site's search index; the search generate-outputs hook runs this recipe.",
    name: "search",
    commands: &["deno task search"],
};

const PAGEFIND: Effect = Effect {
    files: &[
        ("site/search.typ", include_str!("templates/search.typ")),
        (
            "content/search.typ",
            include_str!("templates/search-page.typ"),
        ),
    ],
    hooks: &[Hook {
        stage: HookStage::GenerateOutputs,
        name: "search",
        command: &["just", "search"],
        rerun_on: &["deno.json", "deno.lock", "justfile"],
        outputs: &["assets/pagefind-search"],
    }],
    recipes: &[PAGEFIND_RECIPE],
    deno: Some(DenoTask {
        name: "search",
        command: "deno run -A npm:pagefind@1.5.2 --site \"$TOLA_HOOK_INPUT_DIR\" --output-path \"$TOLA_HOOK_OUTPUT_DIR/assets/pagefind-search\"",
        imports: &[("pagefind", "npm:pagefind@1.5.2")],
    }),
    seo: SeoCode {
        imports: &[("search.typ", &["search-head"])],
        entries: SEARCH_ENTRIES,
        ..NO_SEO
    },
    next_steps: &["Every build runs `just search` to update the search index."],
    ..NO_EFFECT
};

const DENO_TOOLCHAIN: Effect = Effect {
    files: &[("deno.json", include_str!("templates/deno.json"))],
    ignore: &["/node_modules/"],
    recipes: &[DENO_SETUP_RECIPE],
    next_steps: &["Run `just setup` before the first build."],
    ..NO_EFFECT
};

const TAILWIND_CSS: Effect = Effect {
    files: &[(
        "static/tailwind-sources/site.css",
        include_str!("templates/tailwind-input.css"),
    )],
    head: &[TAILWIND_STYLESHEET_HEAD],
    ignore: &["/static/web-assets/tailwind-output/"],
    cache_busting: true,
    hooks: &[Hook {
        stage: HookStage::BeforeBuild,
        name: "tailwind",
        command: &["just", "css"],
        rerun_on: &[
            "static/tailwind-sources",
            "deno.json",
            "deno.lock",
            "justfile",
        ],
        outputs: &["static/web-assets/tailwind-output/site.css"],
    }],
    recipes: &[TAILWIND_CSS_RECIPE],
    deno: Some(DenoTask {
        name: "css",
        command: "deno run -A npm:@tailwindcss/cli@4.3.3 -i static/tailwind-sources/site.css -o static/web-assets/tailwind-output/site.css --minify",
        imports: &[
            ("@tailwindcss/cli", "npm:@tailwindcss/cli@4.3.3"),
            ("tailwindcss", "npm:tailwindcss@4.3.3"),
        ],
    }),
    next_steps: &["Edit `static/tailwind-sources/site.css`; every build runs `just css`."],
    ..NO_EFFECT
};

/// The effects a selection sums to.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Effects {
    pub(super) files: Vec<(&'static str, Cow<'static, str>)>,
    pub(super) ignore: Vec<&'static str>,
    /// The `site/seo.typ` code the surviving features contribute, in declaration order.
    pub(super) seo: Vec<SeoCode>,
    pub(super) head: Vec<HeadFragment>,
    pub(super) cache_busting: bool,
    pub(super) hooks: Vec<Hook>,
    pub(super) next_steps: Vec<&'static str>,
}

impl Effects {
    pub(super) fn combine(set: &FeatureSet) -> Self {
        let canonical = canon(set);
        let mut effects = Self::default();
        let mut recipes = Vec::new();
        for feature in canonical.iter() {
            let effect = feature.effect();
            effects.files.extend(
                effect
                    .files
                    .iter()
                    .map(|(path, contents)| (*path, Cow::Borrowed(*contents))),
            );
            effects.ignore.extend_from_slice(effect.ignore);
            effects.seo.push(effect.seo);
            effects.head.extend_from_slice(effect.head);
            effects.cache_busting |= effect.cache_busting;
            effects.hooks.extend_from_slice(effect.hooks);
            recipes.extend_from_slice(effect.recipes);
            for step in effect.next_steps {
                if !effects.next_steps.contains(step) {
                    effects.next_steps.push(step);
                }
            }
        }
        if let Some(source) = justfile_source(&recipes) {
            effects.files.push(("justfile", Cow::Owned(source)));
        }
        if canonical.contains(Feature::DenoToolchain) {
            let contents = &mut effects
                .files
                .iter_mut()
                .find(|(path, _)| *path == "deno.json")
                .expect("Deno writes its configuration")
                .1;
            *contents = Cow::Owned(deno_source(&canonical));
        }
        effects
    }

    pub(super) fn outputs(&self) -> Vec<SeoOutput> {
        self.seo.iter().filter_map(|code| code.output).collect()
    }
}

fn deno_source(selected: &FeatureSet) -> String {
    let mut config: serde_json::Value = serde_json::from_str(include_str!("templates/deno.json"))
        .expect("the Deno template is valid JSON");
    let mut imports = serde_json::Map::new();
    let mut tasks = serde_json::Map::new();
    for feature in selected.iter() {
        if let Some(task) = &feature.effect().deno {
            imports.extend(
                task.imports
                    .iter()
                    .map(|(name, source)| ((*name).into(), (*source).into())),
            );
            tasks.insert(task.name.into(), task.command.into());
        }
    }
    if !tasks.is_empty() {
        config["imports"] = imports.into();
        config["tasks"] = tasks.into();
    }
    format!(
        "{}\n",
        serde_json::to_string_pretty(&config).expect("Deno configuration is serializable")
    )
}

/// Each slot keeps its last selected provider. Resolving a selection never renders files.
pub(super) fn canon(set: &FeatureSet) -> FeatureSet {
    FeatureSet::new(SLOTS.iter().filter_map(|slot| {
        slot.providers
            .iter()
            .rev()
            .copied()
            .find(|provider| set.contains(*provider))
    }))
}

pub(super) fn replacement(set: &FeatureSet, feature: Feature) -> Option<Feature> {
    providers(feature.slot())
        .iter()
        .rev()
        .take_while(|provider| **provider != feature)
        .copied()
        .find(|provider| set.contains(*provider))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(features: &[Feature]) -> FeatureSet {
        FeatureSet::new(features.iter().copied())
    }

    /// Every selectable feature; the match fails to compile when a variant is added.
    fn all_features() -> Vec<Feature> {
        [
            Feature::StarterStylesheet,
            Feature::Canonical,
            Feature::Feed,
            Feature::Sitemap,
            Feature::OpenGraph,
            Feature::TwitterCard,
            Feature::Pagefind,
            Feature::DenoToolchain,
            Feature::TailwindCss,
        ]
        .into_iter()
        .map(|feature| match feature {
            Feature::StarterStylesheet
            | Feature::Canonical
            | Feature::Feed
            | Feature::Sitemap
            | Feature::OpenGraph
            | Feature::TwitterCard
            | Feature::Pagefind
            | Feature::DenoToolchain
            | Feature::TailwindCss => feature,
        })
        .collect()
    }
    /// Every subset of the selectable features.
    fn subsets() -> Vec<FeatureSet> {
        let features = all_features();
        (0..(1u32 << features.len()))
            .map(|mask| {
                FeatureSet::new(
                    features
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| mask & (1 << *index) != 0)
                        .map(|(_, feature)| *feature),
                )
            })
            .collect()
    }

    #[test]
    fn feature_set_iterates_in_declaration_order() {
        let forward = FeatureSet::new([
            Feature::StarterStylesheet,
            Feature::Canonical,
            Feature::Feed,
            Feature::Sitemap,
            Feature::OpenGraph,
            Feature::TwitterCard,
            Feature::Pagefind,
            Feature::TailwindCss,
            Feature::DenoToolchain,
        ]);
        let backward = FeatureSet::new([
            Feature::DenoToolchain,
            Feature::TailwindCss,
            Feature::Pagefind,
            Feature::TwitterCard,
            Feature::OpenGraph,
            Feature::Sitemap,
            Feature::Feed,
            Feature::Canonical,
            Feature::StarterStylesheet,
        ]);
        let expected = [
            Feature::StarterStylesheet,
            Feature::Canonical,
            Feature::Feed,
            Feature::Sitemap,
            Feature::OpenGraph,
            Feature::TwitterCard,
            Feature::Pagefind,
            Feature::DenoToolchain,
            Feature::TailwindCss,
        ];

        assert_eq!(forward.iter().collect::<Vec<_>>(), expected);
        assert_eq!(backward.iter().collect::<Vec<_>>(), expected);
        assert_eq!(forward, backward);
    }

    #[test]
    fn feature_values_match_every_feature() {
        let values = feature_values().collect::<Vec<_>>();
        assert_eq!(
            values
                .iter()
                .map(|(name, _)| feature(name))
                .collect::<Vec<_>>(),
            all_features(),
            "the values cover every feature in declaration order"
        );
        for (name, summary) in &values {
            assert_eq!(feature_name(feature(name)), *name);
            assert!(!summary.is_empty(), "{name}");
        }
    }

    #[test]
    fn preset_name_matches_only_the_exact_set() {
        for preset in PRESETS {
            assert_eq!(preset_name(&set(preset.features)), Some(preset.name));
            for extra in all_features()
                .into_iter()
                .filter(|feature| !preset.features.contains(feature))
            {
                let mut extended = preset.features.to_vec();
                extended.push(extra);
                assert_eq!(preset_name(&set(&extended)), None, "{}", preset.name);
            }
        }
    }

    #[test]
    fn canon_drops_the_replaced_provider() {
        assert_eq!(
            canon(&set(&[Feature::StarterStylesheet, Feature::TailwindCss])),
            set(&[Feature::TailwindCss])
        );
    }

    #[test]
    fn canon_is_idempotent() {
        for set in subsets() {
            let canonical = canon(&set);
            assert_eq!(canon(&canonical), canonical);
        }
    }

    #[test]
    fn file_marks_match_generated_files() {
        for set in subsets() {
            let selected = canon(&set);
            let effects = Effects::combine(&selected);
            for path in decided_files() {
                assert_eq!(
                    file_selected(path, &selected),
                    effects.files.iter().any(|(file, _)| *file == path),
                    "{selected:?}: {path}"
                );
            }
        }
    }

    #[test]
    fn canon_only_keeps_selected_features() {
        for set in subsets() {
            for feature in canon(&set).iter() {
                assert!(set.contains(feature), "{feature:?}");
            }
        }
    }

    #[test]
    fn slot_providers_are_declared_features() {
        let declared = all_features();
        for slot in SLOTS {
            assert!(!slot.providers.is_empty(), "{:?} has no providers", slot.id);
            for provider in slot.providers {
                assert!(
                    declared.contains(provider),
                    "{provider:?} provides {:?}",
                    slot.id
                );
            }
        }
    }

    #[test]
    fn features_have_one_slot() {
        for feature in all_features() {
            let slots = SLOTS
                .iter()
                .filter(|slot| slot.providers.contains(&feature))
                .count();
            assert_eq!(slots, 1, "{feature:?}");
        }
    }

    #[test]
    fn feature_groups_partition_the_selectable_features() {
        let mut seen = Vec::new();
        for group in GROUPS {
            for slot in group.slots {
                for provider in providers(*slot) {
                    assert!(!seen.contains(provider), "{provider:?} appears twice");
                    seen.push(*provider);
                }
            }
        }
        for feature in all_features() {
            assert!(seen.contains(&feature), "{feature:?} is in no group");
        }
    }

    #[test]
    fn every_slot_appears_in_one_group() {
        for slot in SLOTS {
            let groups = GROUPS
                .iter()
                .filter(|group| group.slots.contains(&slot.id))
                .count();
            assert_eq!(groups, 1, "{:?}", slot.id);
        }
    }

    #[test]
    fn presets_declare_feed_and_sitemap_outputs() {
        for name in ["medium", "rich"] {
            let effects = Effects::combine(&features(name));
            let outputs = effects.outputs();
            assert_eq!(
                outputs
                    .iter()
                    .map(|output| (output.export, output.file))
                    .collect::<Vec<_>>(),
                [
                    ("feed-output", "feed.xml"),
                    ("sitemap-output", "sitemap.xml"),
                ],
                "{name}"
            );
        }
        assert!(Effects::combine(&features("minimal")).outputs().is_empty());
    }

    #[test]
    fn canon_keeps_at_most_one_provider_per_slot() {
        for set in subsets() {
            let canonical = canon(&set);
            for slot in SLOTS {
                let surviving = slot
                    .providers
                    .iter()
                    .filter(|provider| canonical.contains(**provider))
                    .count();
                assert!(surviving <= 1, "{:?} keeps {surviving} providers", slot.id);
            }
        }
    }

    #[test]
    fn replacement_names_the_surviving_provider() {
        let tailwind = set(&[Feature::TailwindCss, Feature::DenoToolchain]);
        assert_eq!(
            replacement(&tailwind, Feature::StarterStylesheet),
            Some(Feature::TailwindCss)
        );
        assert_eq!(replacement(&tailwind, Feature::TailwindCss), None);
        assert_eq!(replacement(&set(&[]), Feature::StarterStylesheet), None);
        assert_eq!(replacement(&set(&[]), Feature::TailwindCss), None);
        // An earlier provider never displaces a later one.
        assert_eq!(
            replacement(&set(&[Feature::StarterStylesheet]), Feature::TailwindCss),
            None
        );
        assert_eq!(
            replacement(
                &set(&[Feature::StarterStylesheet, Feature::TailwindCss]),
                Feature::StarterStylesheet
            ),
            Some(Feature::TailwindCss)
        );
    }

    #[test]
    fn feed_and_sitemap_outputs_declared_once() {
        let effects = Effects::combine(&set(&[Feature::Feed, Feature::Sitemap]));
        let outputs = effects.outputs();
        assert_eq!(
            outputs
                .iter()
                .map(|output| (output.export, output.file))
                .collect::<Vec<_>>(),
            [
                ("feed-output", "feed.xml"),
                ("sitemap-output", "sitemap.xml"),
            ]
        );
        for output in &outputs {
            assert!(
                output
                    .definition
                    .contains(&format!("#let {}(", output.export)),
                "{}",
                output.definition
            );
            assert!(
                output.definition.contains("site.origin == none"),
                "{}",
                output.definition
            );
        }
    }

    #[test]
    fn pagefind_contributes_search_hook() {
        let effects = Effects::combine(&set(&[Feature::Pagefind, Feature::DenoToolchain]));
        let hook = effects
            .hooks
            .iter()
            .find(|hook| hook.name == "search")
            .expect("pagefind declares the search hook");
        assert_eq!(hook.stage, HookStage::GenerateOutputs);
        assert_eq!(hook.command, ["just", "search"]);
        assert_eq!(hook.outputs, ["assets/pagefind-search"]);
        assert_eq!(
            Feature::Pagefind.recipes()[0].commands,
            ["deno task search"]
        );
        assert!(
            effects
                .files
                .iter()
                .any(|(path, _)| *path == "site/search.typ"),
            "{:?}",
            effects.files
        );
    }
    #[test]
    fn deno_installs_selected_tools() {
        for (features, expected) in [
            (vec![Feature::DenoToolchain], vec![]),
            (
                vec![Feature::DenoToolchain, Feature::TailwindCss],
                vec!["css"],
            ),
            (
                vec![Feature::DenoToolchain, Feature::Pagefind],
                vec!["search"],
            ),
            (
                vec![
                    Feature::DenoToolchain,
                    Feature::TailwindCss,
                    Feature::Pagefind,
                ],
                vec!["css", "search"],
            ),
        ] {
            let effects = Effects::combine(&FeatureSet::new(features));
            let source = &effects
                .files
                .iter()
                .find(|(path, _)| *path == "deno.json")
                .unwrap()
                .1;
            let config: serde_json::Value = serde_json::from_str(source).unwrap();
            let tasks = config.get("tasks").and_then(|tasks| tasks.as_object());
            assert_eq!(tasks.map_or(0, |tasks| tasks.len()), expected.len());
            for task in expected {
                assert!(tasks.unwrap().contains_key(task));
            }
            assert_eq!(
                config["imports"].get("pagefind").is_some(),
                config["tasks"].get("search").is_some()
            );
            assert_eq!(
                config["imports"].get("tailwindcss").is_some(),
                config["tasks"].get("css").is_some()
            );
        }
    }
}
