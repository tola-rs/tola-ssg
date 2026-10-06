---
name: tola
description: Build and maintain Tola websites and documentation. Use for site setup and configuration, Typst content and templates, HTML and CSS, routes and navigation, assets and images, icons, mathematics, feeds and SEO, hooks, editor setup, and site troubleshooting.
---

# Writing Tola sites

Tola builds static websites from Typst Bundles: one root Typst program emits the site's documents, and Tola compiles it with the official Typst compiler, validates the complete output, and publishes it.

Write content in Typst, share layouts through ordinary Typst modules, and use HTML and CSS for the browser. The site's own Typst code chooses its pages, navigation, and generated files.

This guide has three parts. Part 1 follows the work: start a site, write pages, give them addresses, compose HTML, navigate, style, add media, and publish feeds. Part 2 is reference material for lookup during a task: bundled packages, commands, configuration, hooks, and deployment. Part 3 verifies what you built and explains common failures.

Exact signatures, parameter defaults, and complete contracts live in `tola help` (for example `tola help "@tola/web" feed`), in editor hover and completion, and in the generated `tola.toml` comments. This guide states what each piece is for and how the pieces fit.

| Task | Read |
| --- | --- |
| Start a site, run it, set up an editor | [Start or open a site](#start-or-open-a-site) |
| Write a page, declare metadata, validate fields | [Write pages and metadata](#write-pages-and-metadata) |
| Choose routes, links, and a deployment mount | [Addresses, links, and mounts](#addresses-links-and-mounts) |
| Build page structure in HTML | [Templates and HTML](#templates-and-html) |
| Add sidebars, archives, tags, breadcrumbs | [Navigation and collections](#navigation-and-collections) |
| Add CSS, a stylesheet toolchain, or fonts | [Stylesheets and assets](#stylesheets-and-assets) |
| Add images, resized variants, or icons | [Images and icons](#images-and-icons) |
| Write formulas | [Mathematics](#mathematics) |
| Add feeds, a sitemap, or social previews | [Feeds, sitemap, and social metadata](#feeds-sitemap-and-social-metadata) |
| Look up a package, command, or setting | [Bundled packages](#bundled-packages), [Commands](#commands), [Configuration](#configuration), [Hooks](#hooks) |
| Diagnose a problem or verify a change | [Check the actual site](#part-3-check-the-actual-site) |

Two habits make everything else easier:

- Check the actual executable and site before relying on memory. Run `tola --version` and `tola help`; for an existing site read its `tola.toml`, entry file, templates, schema, assets, and hooks, then extend that site's conventions rather than replacing them with a new scaffold.
- Verify against the running site, not the source alone. `tola check` and a browser show more than reasoning about the source; Part 3 says what to exercise.

## Part 1: Build a site

### Start or open a site

#### Use the intended site and executable

Tola uses the nearest `tola.toml` in the current directory or its parents. Select another configuration with `tola build --config path/to/tola.toml` (`-C` on any site command). The directory containing that configuration is the site root, and configured paths are relative to it. `tola config` prints selected resolved paths and settings as JSON; it is not an inventory of every setting.

The guide bundled with a binary is the one that binary was built with. `tola help` reads no site, configuration, or network: use it for command help, configuration tables, and package documentation.

#### Create a site

```sh
tola init my-site --dry-run   # preview the files and configuration
tola init my-site             # create them
cd my-site
```

Initialization accepts a missing or empty directory. `--force` permits a nonempty directory but never overwrites scaffold files; it is not an update command. `--preset` chooses minimal (the default), medium, or rich. `--features starter-stylesheet,feed,sitemap` chooses individual features instead; it cannot be combined with `--preset`. Explicit feature lists must include dependencies: `tailwind-css` and `pagefind` need `deno-toolchain`.

`--interactive` asks for the directory, then shows `Styling`, `SEO`, `Search`, `Tooling`, and optional `Files`. `Space` toggles a feature and updates its dependencies; file rows identify their writers and can enable a sole writer. `/` filters; `1`/`2`/`3` apply rich, medium, or minimal. `y` accepts; `q`, `Esc`, or Ctrl-C cancel. Named features open with their dependencies selected. The command then asks for editors and confirms before writing. A terminal that cannot draw uses numbered presets or the completed named selection. `--editor vscode,helix` supplies editors directly; `tola editor setup` configures them later.

The medium and rich stylesheets share browser typography and system light/dark colors. Root `data-theme="light"` or `data-theme="dark"` overrides the system, including code highlighting and Tailwind `dark:` utilities. The feed requires `site.origin` and `site.title`; the sitemap requires only `site.origin`. Deno installs only the selected tools.

Initialization leaves `content/` empty. Create `content/index.typ`:

```typst
#import "@tola/source:0.0.0": tola-meta

#tola-meta((
  title: "Welcome",
  description: "A first page written with Typst and Tola",
))

#title()

= Start here <start-here>

This site uses *Typst* for content, templates, and $a^2 + b^2 = c^2$.

- Add another page under `content/`.
- Share page structure through `site/`.
- Use CSS for colors, spacing, and responsive layout.

#link(<start-here>)[Return to this section.]
```

Then run `tola dev` and open the address it prints (it includes the configured mount). Save a file to rebuild; the browser follows only successful revisions. Stop the server with Ctrl-C. `tola check` checks the complete site and `tola build` writes the output directory when it is time to publish.

A site that produces no HTML document at all reports `site.no_pages`, and dev/preview serve a welcome page at the mount root. Any HTML document counts, including `404.html`; an empty content directory is valid when the entry program generates pages from data.

#### The starter site

The scaffold is editable Typst, not a fixed layout:

| File or directory | Purpose |
| --- | --- |
| `tola.toml` | Site settings, inputs, assets, and published directory |
| `site.typ` | Root Bundle: selects the pages, emits each page, the `404.html` document, and the feed and sitemap outputs the selection declares |
| `site/selection.typ` | `source-route(source)` and `select-pages(sources)`: metadata validation, drafts, output paths |
| `site/schema.typ` | The page metadata fields and defaults |
| `site/page.typ` | `page-template(page)`: page structure, head entries, and content placement |
| `site/not-found.typ` | `not-found-template()`: the `404.html` document the root program emits |
| `site/seo.typ` | `head-entries(title, description:, page:)`, `feed-output(output, pages)`, and `sitemap-output(output, pages)`: the head's search and sharing entries and the output declarations |
| `site/search.typ` | The Pagefind search box and its head entries, written only when the `pagefind` feature is selected |
| `content/` | Page bodies and their metadata |
| `static/web-assets/` | Published below `/assets/` by the scaffold's `[assets]` mapping; `images/`, `fonts/`, `css/`, and `scripts/` are its conventions |
| `static/typst-fonts/` | Fonts for Typst compilation only; not published |
| `vendor/` | External dependencies kept with the site (`tola vendor`) |
| `.tola/` | Disposable local work: logs, caches, editor package mirrors |

Prefer purpose-specific directories under `static/` for site-owned resources and processing inputs — for example `static/tailwind-sources/`, `static/site-data/`, `static/image-sources/`, and `static/icon-sources/` — even when only their processed results are published. Keep shared Typst modules outside `content/` so discovery does not treat them as pages; `site/`, `utils/`, and `components/` are conventions, not special names. `tola init --dry-run` shows the complete file list and the default configuration.

#### Editors

```sh
tola editor setup --list          # supported editors and settings other LSP clients need
tola editor setup vscode --dry-run
tola editor setup                 # choose interactively
tola editor packages              # refresh editor package files after upgrading Tola
```

VS Code (with the Tola extension) and Helix settings are merged into site files; Neovim, Emacs, Zed, and Sublime Text receive instructions or snippets. Review conflicts instead of replacing unrelated settings. `tola lsp` serves diagnostics and language queries over stdio; a folder without a `tola.toml` is still served as the documents it holds.

### Write pages and metadata

#### Typst you need

Typst starts in markup mode. `#` introduces code; `{ ... }` is a code block and `[ ... ]` is content. Inside a code block, write code without another `#`; inside content, use `#` for expressions.

| Syntax | Meaning |
| --- | --- |
| `= Heading`, `== Subheading` | Native headings |
| `*strong*`, `_emphasis_` | Semantic emphasis |
| `- item`, `+ item`, `/ term: definition` | Lists |
| `$x^2$`, `$ x^2 $` | Inline and display mathematics |
| `<install>`, `#link(<install>)[Install]` | A label attached to preceding content, and a link to it |
| `@equation-label` | A numbered reference, when its target supports numbering |
| `#let name = value`, `#if`, `#for` | A binding, conditional content, repeated content |

```typst
#let greeting(reader-name) = [Hello, *#reader-name*!]
#greeting("visitor")

#let chapters = ("Install", "Write", "Publish")
#for chapter in chapters [
  - #chapter
]
```

A function returns the value of its body. Values stay distinct: `[A *rich* title]` is content, `"A *rich* title"` is a string of literal asterisks; `none`, `auto`, `false`, zero, and `""` differ; `.at("summary", default: none)` reads an optional key. Named arguments use `name: value`, `.with(...)` binds arguments for a later call, and `..values` spreads an array or dictionary into a call. Prefer names that do not shadow built-ins such as `title`, `text`, `label`, or `query`; a shadowed function stays reachable through `std`, as in `std.title()`.

Displaying an array prints its representation; render content with a loop or `.join()`:

```typst
#let topics = ("Content", "Templates", "Routes")
#html.ul[
  #for topic in topics [#html.li[#topic]]
]
```

Strings join into strings and content joins into content; convert numbers with `str` first. A filesystem's order is not an editorial order: sort explicitly, and use `.dedup(key: ...)` when identity matters.

Modules work as in Typst: `#import "templates/note.typ": note` imports definitions; `#include "chapter.typ"` inserts content. An imported file does not inherit the caller's variables; pass template inputs explicitly. `set` changes settable properties for following content, `show` changes how selected content renders. Browser appearance belongs to CSS: Typst text fonts, fills, margins, and page layout settings do not automatically become web styles.

`context` computes a value where its content is placed; operations on document identity, queries, state, and counters often need it:

```typst
#context {
  let section-count = query(heading).len()
  [Sections: #section-count]
}
```

Query native elements (`heading.where(level: 1)`); region selectors such as `within(...)` work in queries — scope one to the containing document with `current-document().location` — but not in show rules. A label attaches to actual content (`= Install <install>`, `[Introduction #label("introduction")]`) and becomes an HTML anchor once something targets it. State updates take effect where their returned content is placed; closures cannot mutate captured locals — use a loop, `fold`, or state.

Read ordinary site data with `json`, `yaml`, `toml`, `csv`, `xml`, or `read`:

```typst
#let chapters = json("/static/site-data/chapters.json")
#let rows = csv("/static/site-data/topics.csv", row-type: dictionary)
#let introduction = read("/static/site-data/introduction.txt")
```

A string argument names a file, so `json(read(path))` treats the read text as another filename; use `json("/path")`, `json(read(path, encoding: none))`, or `json(bytes("..."))`. CSV fields are strings; convert before calculating. A read or include does not publish the file.

`datetime(year: 2026, month: 9, day: 1)` builds a date and `.display("[month repr:long] [day], [year]")` formats one; `datetime` does not parse a date string, and `datetime.today()` is unavailable — supply dates explicitly. Feed timestamps may instead be RFC 3339 strings with a timezone.

A few Typst traps:

- A new line starting with `.map(...)` at the top level is markup, not a continuation of the previous expression. In a content source this errors; inside an imported module it silently becomes module text and the variable keeps only the first line's value, surfacing later as an unrelated "missing key" error. Wrap multiline expressions in `{ }` or parentheses, or keep the operator at the end of the previous line. Chains inside argument lists may break lines safely.
- A label cannot be added with `+`: `content + label("x")` is an error. Write `[#content#label("x")]`, or attach the label to content directly as in `= Heading <heading>`.
- `html.script` and `html.style` take strings; an external script is `html.script("", src: "…", defer: true)`.
- A positional argument cannot follow a named one. Use the element's trailing content block: `html.ul(class: "x")[…]`.
- Typed constructors accept only their documented attributes and values; an unsupported value is an error rather than a silently dropped attribute. Render a different element in a branch when the state differs.
- `html.frame` cannot contain a `set page(...)` rule ("page configuration is not allowed inside of containers"). For fixed-column artwork, use ordinary content with CSS grid.
- A backticked inline `` `code` `` holds no language and no highlighting. Give inline code a language with `raw("…", lang: "rust")`; block code takes the fence language.
- A closure's parameter list and a destructuring pattern differ by one pair of parentheses: `.map((pair,) => …)` takes one parameter, while `.map(((key, value)) => …)` destructures a pair; the wrong form fails with a type error such as `expected string, found array`.

#### Author a page

Write the body in markup: headings, paragraphs, lists, quotes, tables, figures, and mathematics are native Typst, and they keep their semantics, queries, numbering, and references. Use `html.nav`, `html.main`, `html.section`, `html.article`, `html.aside`, and `html.elem` when an element needs a class, an attribute, or structure generated by code — article lists, tag clouds, and tables of contents. A string containing HTML is text, not a native node.

`#title()` is the document's own title and exports as `<h1>`; `=` is a section heading and exports as `<h2>` (native levels 1–5 become `<h2>`–`<h6>`; deeper levels warn and use `role="heading"` with a higher `aria-level`, so restructure very deep sections). Give reusable elements semantic class names in addition to utility classes, so CSS, scripts, and tests can select them reliably.

A page that can live as a source under `content/` is easier to write, edit, and feed than one generated by code. Keep generated aggregation only for what a source cannot express.

#### Declare metadata

Declare metadata once, eagerly, in the content source itself. `#tola-meta(dictionary)` is the simplest form; a helper called eagerly from the same file may compute the dictionary:

```typst
#import "@tola/source:0.0.0": tola-meta
#let page-meta = (title: [Field *notes*], tags: ("typst",))
#tola-meta(page-meta)

#title()
= Observations
These notes keep a rich-text title.
```

The call is the declaration: a build reads it whether or not the rest of the source succeeds. Keep it out of deferred `context` and out of imported helpers — those do not declare the caller's metadata. A `metadata(...)` call holding a `<tola-meta>` label registers nothing and is reported once as `source.declaration_deprecated`. Only one outermost declaration exists per source; a second call is a duplicate.

Draft pages are still evaluated: fix metadata and Typst errors in a draft, because exclusion does not skip the source.

The starter schema accepts these fields (all read by the site's own templates; extend `site/schema.typ` before adding another, since unknown fields are errors by default):

| Field | Meaning |
| --- | --- |
| `title`, `description` | Page title and summary; string or rich content; default to the site's values |
| `authors` | A name, an `{name, email, url}` dictionary, or an array; defaults to `site.authors` |
| `tags` | String or array of strings; default `()` |
| `draft` | Keep the page out of the published site |
| `permalink` | Explicit URL path; `/about/` is a directory and `/about.html` is a file |
| `feed`, `sitemap` | Whether the page's feed or sitemap recipe includes it |
| `id` | Stable feed identity; `auto` derives one from the address |
| `published`, `updated` | A `datetime` or an RFC 3339 string with timezone |
| `summary`, `content` | Feed summary and feed body |

Metadata is analyzed with Eval, not Layout: it retains native Typst values — content, styles, functions, modules, deferred `context` — and Tola does not restrict field names or convert strings to content. A declaration may read other sources; successive evaluation rounds let such metadata settle. Budget exhaustion and a repeated source state are reported separately.

#### Validate metadata with schemas

`parse-sources(sources, schema)` from `@tola/source` resolves selected source records against a schema, reports every failure at the source's own declaration (file start when it has none), and returns the same records with `meta` replaced. `all-sources()` itself is unchanged. A source that declares nothing is validated as an empty dictionary, and a parsed result that is not a dictionary is an error.

Where the call runs decides what it can validate:

- In the root program (`site.typ` and modules evaluated from it), validate the whole set. This is the right place to report missing or invalid fields for every page.
- In a content source, `all-sources()` lists every other source, but the source being evaluated has not registered its own declaration yet — it appears with `meta: none`. Validating `all-sources()` wholesale here mis-reports that source as missing every required field, at the file start. Validate a subset that excludes the source itself, or read raw metadata without reporting.
- A shared module should expose functions (for example `site-model(sources)`, `program-model()`, `page-model()`) rather than a module-level value that calls `parse-sources`. A content source that imports such a module would otherwise run a full-set validation during source analysis. A page-side view can default missing fields with `try-parse` and leave an invalid value unchanged; the root program's reporting pass still fails the build, so the permissive view never reaches publication.

The starter's `select-pages(sources)` validates all sources before filtering drafts, then pairs each published source with the output file its route names. Reuse those `(source:, output:)` records for navigation, feeds, and sitemaps instead of recomputing routes.

Schemas come from `@tola/schema`. A native Typst type (`str`, `int`, `bool`, `content`, `datetime`) is already a schema:

```typst
#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": array-of, non-empty, optional, schema, trim

#let page-schema = schema((
  title: non-empty(trim(str)),
  tags: optional(array-of(str), default: ()),
  draft: optional(bool, default: false),
))
// The root program validates the whole set; a content source validates a subset.
#let sources = parse-sources(all-sources(), page-schema)
```

Only an absent key is missing: `optional(S)` without a default omits the key, and `nullable(S)` accepts an explicit `none`. An invalid supplied value never falls back to a default. Wrappers run from the inside out, so `non-empty(trim(str))` rejects whitespace-only strings while `trim(non-empty(str))` checks before trimming. `parse` returns the value or reports the issues; `try-parse` returns `(ok: true, value: ...)` or `(ok: false, issues: ...)`. Use `format-issues` for custom reports rather than flattening errors to strings.

The combinators cover presence, literals and enumerations, typed collections, tuples and recursive schemas, unions and variants, string and number bounds, format checks, conversions and checks, and structured results; `tola help "@tola/schema"` lists each with its parameters and contracts. Malformed declarations and callback errors stop compilation; they need a code fix, not a fallback value.

### Addresses, links, and mounts

#### Sources and pages are different things

`all-sources()` returns discovered `.typ` files under `build.content-dir`, in discovery order — not the list of published pages. Each record has:

| Field | Meaning |
| --- | --- |
| `id`, `path` | The source's path below `build.content-dir`, extension included (`posts/deep.typ`) |
| `file` | The same file addressed from the site root, for `include` and `read`; an input path, never a URL |
| `filename` | The last path component |
| `route-segments` | The identity segments the file layout gives: `()`, `("about",)`, `("posts", "deep")` |
| `meta` | The declaration as seen in this evaluation round, or `none` |

Discovery accepts lowercase `.typ` only, skips hidden names and symbolic links, and names a directory index after its directory (`about/index.typ` gives `("about",)`, like `about.typ`). The entry file is not a discovered source. A source may be omitted, included in a page, or used in several pages; files colocated with a source are ordinary inputs, not a page-owned bundle.

`current-source()` returns the source containing the call, with the same identity fields and no `meta`, available during ordinary evaluation. Call it in the content source and pass the value on; a call written in a shared module names the module. `current-document()` needs document context and identifies the page containing the call: inside `context` it gives the output path (`guide/index.html`), the route (`/guide/`), and the location, which scopes queries such as `query(selector(figure).within(current-document().location))`.

#### Starter routes and permalinks

The starter turns each source's `route-segments` into a directory route, unless the source sets `permalink`:

| Content file | Starter route | Output path |
| --- | --- | --- |
| `index.typ` | `/` | `index.html` |
| `about.typ` or `about/index.typ` | `/about/` | `about/index.html` |
| `guide/install.typ` | `/guide/install/` | `guide/install/index.html` |
| any source with `permalink: "/manual.pdf"` | `/manual.pdf` | `manual.pdf` |

A `permalink` is a URL path, so it is decoded once with `decode-url-path`; it changes the path, not the format — a PDF needs a PDF document declaration. Only a trailing `/` names a directory: `/guide/` becomes `guide/index.html`, while `/guide` becomes the file `guide`. Publishing two sources at one output path is a conflict, not a priority rule.

#### Names, routes, and URLs

| Function | Meaning |
| --- | --- |
| `slugify(text, mode: "unicode", case: "lower", separator: "-", language: "zh")` | One slug from text; each slug names one path segment, so `route` joins them and nothing here builds a URL |
| `route(segments)` | A decoded site-root route from already chosen segments; always ends with `/`; `()` is the site root |
| `route-to-output(route)` | The logical output file a route names; `/guide/` becomes `guide/index.html`, `/guide` becomes `guide` |
| `output-to-route(output)` | The canonical route of an output; `guide/index.html` becomes `/guide/` |
| `output-to-url(output, base-path: auto, origin: none)` | The browser URL of a decoded output path; an omitted base path applies the site's own mount |
| `decode-url-path(url-path)` | Decode a site-root URL path exactly once; no origin, query, or fragment |
| `asset-url(declared-url)` | The browser URL of a declared asset URL; applies the mount and appends `?h=…` when cache busting is on |

Keep the domains distinct: `route-to-output` takes a decoded route, `output-to-url` takes a decoded relative output path. `slugify` returns one name, so slashes become separators; modes are `unicode` and `ascii`, case is `lower`, `upper`, `capitalize`, or `preserve`, and separators are `-` or `_`. Pass `language: site.language.lang` when names should follow the site's language (`ascii` mode takes Han pronunciation: a primary `ja` subtag selects Japanese, other tags select Chinese; region subtags do not choose a reading). Text with nothing nameable is an error; choose an explicit name or route for symbol-only titles.

A literal `%` or `#` is an ordinary filename character in a route. Percent-encoding happens once at the URL boundary: decode an incoming URL path with `decode-url-path`, and never decode again.

#### Labels, links, and anchors

Prefer native label links to reach pages and sections:

```typst
#document("index.html", format: "html", title: [Home])[
  Visit the #link(<guide-page>)[guide].
] <home-page>

#document("guide/index.html", format: "html", title: [Guide])[
  #title()
  = Install <guide-install>
  Begin here.
  #link(<home-page>)[Home]
] <guide-page>
```

A label becomes an HTML anchor only when something targets it — a `link`, a reference, or an outline — and a label uses letters, digits, `-`, and `_`. Use explicit, stable heading labels for shared links; unlabelled targeted headings get positional anchors that move with the section. Use `link` to navigate; `@label` additionally requires a numbered target. A heading written as `html.h2` is not a native heading and creates no numbering or outline entry.

Because `slugify` keeps `.` (it is filename-safe), a name derived from a filename such as `about.typ` is not a valid label. Strip the extension before slugging when you derive anchors from filenames.

#### Page-local headings and references

`headings(depth: none)` from `@tola/document` lists the current document's headings in document order — including unlabelled ones and those with `outlined: false`. Each has `level`, `nesting`, `number`, `text`, `label`, `location`, and `outlined`; `depth` keeps headings up to that declared level. Link by `location` so labelled and unlabelled headings both work, and filter `outlined` when following outline visibility:

```typst
#import "@tola/document:0.0.0": headings
#context {
  html.nav(aria-label: "On this page")[
    #for section in headings(depth: 2).filter(s => s.outlined) [
      #link(section.location, section.text) \
    ]
  ]
}
```

`references(from: none, from-within: none, to: none, to-within: none)` from `@tola/document`
returns the outermost native `link` and `ref` occurrences of each document, in Bundle order.
Every supplied filter applies to those occurrences. `from` selects their containing documents;
`from-within` selects ancestor regions of the references. `to` selects native document or asset
outputs, or exact target elements; `to-within` selects ancestor regions of known native targets.
Both region filters use strict descendants, excluding the region itself. Label a containing
`html.section` for chapter content; a heading label names only the heading.

All four filters accept `none`, labels, locations, locatable element functions, and native
locatable selectors. Compose selections with `selector(...).or(...)` or `.and(...)`.
`from` and `to` also accept `auto` for the current document. Labels select all matching elements;
a selection with no matches returns an empty array. `from` must select content belonging to a
published native document. Render derived links outside the region `from-within` reads.

Each record has `element`, `document`, `destination`, `resolution` (`found`, `external`, or
`unresolved`), `reason`, and `target`. An unresolved `reason` carries a stable `tag`, a `message`,
and a `help` naming the fix. A target carries `kind` (`output`, `element`, or `url`), `output`,
`route`, `query`, `fragment`, and `location`. Only native targets carry their actual location.
URL targets carry output and address information, so they match output-level `to` selections;
element-level `to` and `to-within` require native target identity. URL fragments are decoded for
display, while native anchors retain their assigned spelling. `found` means a native output
exists; final HTML reference validity belongs to `tola check`.

Select references in A's body to native targets inside B's chapter:

```typst
#import "@tola/document:0.0.0": references
#document("a.html")[
  #html.main[#link(<detail>)[Detail]] <a-body>
  #context {
    assert.eq(references(
      from: <a>, from-within: <a-body>,
      to: <b>, to-within: <chapter>,
    ).len(), 1)
  }
] <a>
#document("b.html")[
  #html.section[
    = Detail <detail>
  ] <chapter>
] <b>
```

#### Deployment mounts

For a site published at `https://example.com/docs/`, set `site.origin = "https://example.com"` and `site.base-path = "/docs/"`. Keep document output paths such as `guide/index.html` unprefixed, and let `output-to-url`, `asset-url`, or a native label link apply the mount once. An omitted `base-path` applies the site's own; an explicit string overrides it (`base-path: "/"` selects the host root), and `origin` must have no deployment path. Pass `origin: site.origin` only when an absolute URL is needed, such as canonical links and feeds.

### Templates and HTML

#### The entry program and the starter shell

The entry emits sibling `document(...)` outputs — an HTML page template combines its head, body, and content, and native `asset(path, bytes)` publishes a generated file such as a download. Outputs are siblings, never nested inside another page.

The scaffold's `site/page.typ` is a working shell to edit: `page-template(page)` takes one
`(source:, output:)` record from `select-pages` and emits one
`document(page.output, format: "html", title:, description:, author:, keywords:)` whose body
includes `page.source.file`. The starter shell writes its own `html.html`, `html.head`, and
`html.body`, so it also writes its own head entries; `site/seo.typ` owns them:
`head-entries(title, description:, page:)` returns `head-metadata(site, …)` entries (the character
set, the viewport, the title, the description) plus whatever the selected SEO features add for a
page, and a document that is not a source — `404.html` — passes no `page` and holds only the
metadata entries. Native `document(...)` metadata (`title`, `description`, `author`, `keywords`)
is separate; it does not fill a custom head. A head takes one content body:

```typst
#import "@tola/web:0.0.0": head-metadata
#html.head(head-metadata(site, title: page-title).join())
```

Give every page template an `html.head`: when a document uses mathematics, the exporter injects its
equation stylesheet into the head. The starter loads `code-stylesheet()` and the page's stylesheet
there too (see [Code blocks](#code-blocks)).

A complete root program for a small isolated site, with empty content:

```typst
#let web-page(output, page-title, body) = document(
  output, format: "html", title: page-title,
)[
  #html.html(lang: "en")[
    #html.head[
      #html.meta(charset: "utf-8")
      #html.meta(name: "viewport", content: "width=device-width, initial-scale=1")
      #html.title(page-title)
    ]
    #html.body[
      #html.nav(aria-label: "Primary")[
        #link(<home>)[Home]
        #link(<guide>)[Guide]
      ]
      #html.main[
        #title()
        #body
      ] <page>
    ]
  ]
]
#web-page("index.html", [Home])[
  Read the #link(<guide>)[guide].
] <home>
#web-page("guide/index.html", [Guide])[
  = Install <install>
  Begin here.
  #link(<install>)[Return to installation.]
] <guide>
#document("404.html", format: "html", title: [Page not found])[
  #html.h1[Page not found]
  Return to the #link(<home>)[home page].
]
```

#### Native structure and custom HTML

| Typst content | Browser structure |
| --- | --- |
| `#title()` | The document title as `<h1>` |
| `=`, `==`, `===` | `<h2>`, `<h3>`, `<h4>` |
| Strong, emphasis, lists, terms, quote | Semantic text, list, and quotation elements |
| Raw code | Inline code, or `<pre><code>` for blocks |
| `table` with `table.header` | A table with header and body |
| `figure(..., caption: ...)` | A figure with a caption |
| Native references and bibliography | Linked citations and a bibliography |
| Native mathematics | MathML by default |
| `image(..., alt: ...)` | An image with alternative text |

Typed constructors (`html.nav`, `html.section`, `html.details`, …) accept their documented attributes, including `class`, `id`, `role`, and ARIA attributes; they do not take a generic attribute dictionary. For a custom tag, a `data-*` attribute, or an unsupported attribute, use `html.elem`, whose attribute values are strings:

```typst
#html.elem("aside", attrs: (class: "note", "data-kind": "tip"))[
  A reusable note.
]
```

Most elements take one content body; void elements such as `html.meta`, `html.link`, `html.img`, and `html.input` take none.

HTML is not a PDF page drawn in the browser: use CSS instead of `set page`, `pagebreak`, `place`, `columns`, `grid`, `stack`, or spacing primitives for web layout. Use `html.frame` intentionally for paged artwork that becomes SVG, not for the whole site.

Custom HTML shells follow upstream Typst's current restriction: its default footnote placement is unavailable inside a custom `html.html` tree, including the starter's shell. A document that needs upstream's default footnotes must retain Typst's default shell; Tola supplies no replacement renderer. Bibliography and citations work with a custom shell.

#### Code blocks

The starter applies `render-code` with `#show raw: render-code` and loads `code-stylesheet()`. The helper keeps Typst's own highlighting; it does not choose the theme. `raw.theme` selects each block's appearance before Typst highlights it — `auto` keeps Typst's default, a path or bytes uses that theme, and `none` disables highlighting — so set it where the block is written:

```typst
#import "@tola/code:0.0.0": render-code, code-stylesheet, code-themes
#show raw: render-code

#[
  #set raw(theme: code-themes.github)
  #raw("let x = 1", lang: "rust", block: true)
]
```

`code-themes` names the themes Tola ships, as values such as `code-themes.tokyo-night`; `tola help "@tola/code" code-themes` lists them. Pair the appearances where the block is written, so the light and dark themes come from one family:

```typst
#set raw(theme: code-themes.github)
#show raw: render-code.with(dark-theme: code-themes.github-dark)
```

The helper holds both appearances on every run; `code-stylesheet()` consumes the dark one while
the site's root element holds `data-theme="dark"`. The site decides what sets that attribute: a
small script that mirrors `prefers-color-scheme`, a toggle, or nothing at all. A site that passes no
`dark-theme` has one appearance and never switches. The scaffold's own stylesheet follows the
system with a `@media (prefers-color-scheme: dark)` block guarded by `:not([data-theme])`, so the
default works without JavaScript and an explicit `data-theme` still wins in both directions.

Block code renders as `<pre class="tola-code"><code data-lang="rust">`, inline code as `<code class="tola-code" data-lang="rust">`. `attrs` passes string attributes such as `class`, `id`, or `data-*` to that element, where `class` and `style` extend the helper's own; `data-lang` belongs to the language and is refused in `attrs`. Colors and font styles are final CSS consumed at zero specificity, so your own rule with an ordinary selector wins, with no `!important`; style the box in your CSS. Every build publishes the shared stylesheet at `_tola/code-stylesheet.css`, minified, whether or not the build renders code.

#### Accessibility, language, and fonts

- Give each page a meaningful title and description, one clear `<h1>`, and a consistent heading hierarchy.
- Set `[site] language` to the real language, such as `"zh-Hans-CN"`. A custom shell uses `site.language.tag` for `html.html(lang: …)` and `site.language.lang`/`.script`/`.region` for Typst text.
- Use semantic landmarks and named controls; an icon's label does not name its surrounding button.
- Give informative images an `alt`, and decorative images an empty one. A figure caption is not alternative text.
- Describe important equations in surrounding text; `figure(alt:)` and `math.equation(alt:)` do not supply HTML alternatives in the current exporter.
- Check keyboard focus, contrast, wrapping, zoom, and narrow screens in a browser.

Fonts split into two jobs: Typst compilation reads `[typst.fonts].paths` (and system fonts when enabled); browsers read published files loaded with CSS `@font-face` from an asset tree such as `static/web-assets/fonts/`. A Typst font setting alone does not install a browser font.

### Navigation and collections

#### Select once, reuse everywhere

Select visible sources once, calculate their output paths once, and use that same ordered list for sidebars, previous/next links, archives, feeds, and sitemaps. Pass it to templates explicitly; an included file does not inherit the caller's variables. The starter's `select-pages` already returns `(source:, output:)` records in discovery order. For documentation, add fields such as `section` and `order` to the schema first, then sort by `(meta.order, source.id)` for a deterministic editorial order.

#### Collection helpers

`@tola/collection` operates on your own values through accessor functions you pass; it assumes no field name, order, or hierarchy convention. `key` identifies a member, `keys` gives the memberships of one member, and `parent` maps a member key to its container key (receiving keys, including ones absent from the array).

| Function | Use it for |
| --- | --- |
| `pick(fields, keys)` | Copy selected dictionary keys in the requested order, keeping `none` |
| `index-by(members, key:)` | Look members up by unique string keys |
| `group-by(members, key:)` | Group members under one string key each |
| `group-by-keys(members, keys:)` | Group members under several memberships |
| `select-members(members, wanted, keys:, match: "any")` | Filter by membership; `"all"` requires every wanted value |
| `adjacent(members, at, key:)` | The `(before:, after:)` neighbors of one key, `none` if absent |
| `window(members, at, key:, before:, after:)` | A span around one key, excluding the anchor |
| `children(members, of, parent, key:)` | Direct children of an identity |
| `descendants(members, of, parent, key:)` | All descendants |
| `ancestors(members, at, parent, key:)` | Existing ancestors, root to parent |
| `lineage(members, at, parent, key:)` | Breadcrumbs, root to self |
| `siblings(members, at, parent, key:)` | Other members under the same parent |

Grouping and selection preserve input order, and group keys first appear in member order. Hierarchy keys must be unique and non-`none`; `parent` must be deterministic and reach `none` after finitely many steps. `adjacent` omits a neighbor key that does not exist (test key presence at boundaries); `window` counts are nonnegative. A missing ancestor record is skipped without ending the walk, but the parent rule can still answer through it. Read `tola help "@tola/collection"` for exact edge behavior.

#### Documentation navigation

Add `order: optional(int, default: 0)` to the page schema, give documentation pages titles, and create an overview at `content/docs/index.typ`. Sort the existing `(source:, output:)` array in `site.typ` first. Then a `site/navigation.typ` module can derive breadcrumbs and a sidebar from the same records:

```typst
#import "@tola/address:0.0.0": output-to-url
#import "@tola/collection:0.0.0": descendants, lineage

#let parent-output(output) = {
  let segments = output.split("/")
  let directory = segments.slice(0, segments.len() - 1)
  if segments.last() == "index.html" {
    if directory.len() == 0 { return none }
    directory = directory.slice(0, directory.len() - 1)
  }
  (directory + ("index.html",)).join("/")
}

#let docs-navigation(pages, output) = {
  let url-of(selected) = output-to-url(selected.output)
  let trail = lineage(pages, output, parent-output, key: selected => selected.output)
  let docs-pages = descendants(pages, "docs/index.html", parent-output, key: selected => selected.output)
  html.nav(aria-label: "Breadcrumbs")[
    #if trail != none {
      for crumb in trail [#link(url-of(crumb))[#crumb.source.meta.title] / ]
    }
  ]
  html.nav(aria-label: "Documentation")[
    #for selected in docs-pages [
      #link(url-of(selected))[#selected.source.meta.title] \
    ]
  ]
}
```

Give `site/page.typ`'s `page-template` a fourth `pages` parameter, import `docs-navigation` from `"navigation.typ"`, place `#docs-navigation(pages, output)` in the body, and pass `pages` from `site.typ`. A permalink can change the derived hierarchy; use an explicit parent rule when the documentation structure should not follow URLs. Previous/next links use `adjacent(pages, output, key: selected => selected.output)` on the chosen order, guarding missing anchors and neighbors.

#### Tags, archives, and paginated listings

Put generators in the root program after selecting page outputs, outside page bodies, and reuse the site's layout where possible. This minimal recipe uses the starter's `pages` records and needs homogeneous `datetime` publication values; normalize string timestamps before sorting:

```typst
#import "@tola/site:0.0.0": site
#import "@tola/address:0.0.0": route, route-to-output, output-to-url, slugify
#import "@tola/collection:0.0.0": group-by, group-by-keys

#let listing(output, list-title, selected-pages) = document(
  output, format: "html", title: list-title,
)[
  #title[#list-title]
  #html.ul[
    #for selected in selected-pages [
      #html.li[
        #link(output-to-url(selected.output))[#selected.source.meta.title]
      ]
    ]
  ]
]

#let dated-posts = {
  pages
    .filter(selected => selected.source.path.starts-with("posts/") and selected.source.meta.published != none)
    .sorted(key: selected => (selected.source.meta.published, selected.source.id))
    .rev()
}

#for (tag-name, tagged) in group-by-keys(dated-posts, keys: selected => selected.source.meta.tags) {
  listing(route-to-output(route(("tags", slugify(tag-name, language: site.language.lang)))), [Tag: #tag-name], tagged)
}

#for (year, year-posts) in group-by(dated-posts, key: selected => selected.source.meta.published.display("[year]")) {
  listing(route-to-output(route(("archive", year))), [Archive: #year], year-posts)
}

#let listing-pages = dated-posts.chunks(10).enumerate().map(((index, batch)) => (
  output: route-to-output(route(if index == 0 { ("browse",) } else { ("browse", str(index + 1)) })),
  title: [Posts #(index + 1)],
  members: batch,
))
#for listed in listing-pages {
  listing(listed.output, listed.title, listed.members)
}
```

`chunks` with a positive size keeps the final partial batch; empty input produces no listing pages. Compute destinations once and navigate with `adjacent` or `window` over `listing-pages`, not the original posts. Two tags that slug to one name, or a content page claiming a listing's path, need an explicit routing decision — one logical output has one owner.

### Stylesheets and assets

#### How a stylesheet reaches the browser

Tola generates no CSS. A site produces its stylesheet with an external tool (or a hand-written file) and publishes it like any other asset, so the tool's own configuration owns utilities, resets, and theme tokens. The pipeline is: an input outside the published tree → a `before-build` hook or a hand edit → a file under a declared asset tree → `asset-url(...)` in the template's head.

`[assets]` is a top-level table (not `[build.assets]`) that maps files or trees to site-root URLs:

```toml
[assets]
cache-busting = false
trees = [{ source = "static/web-assets", url-prefix = "/assets" }]
files = []
```

The scaffold already writes this mapping. `cache-busting = true` appends a content-derived `?h=…` to URLs returned by `asset-url`; published filenames stay unchanged. An exact `files` entry replaces a member's tree URL, so a tree can publish many files while one generated stylesheet keeps a stable URL such as `/assets/css/site.css`.

```typst
#import "@tola/address:0.0.0": asset-url
#html.link(rel: "stylesheet", href: asset-url("/assets/css/site.css"))
```

`asset-url` takes the URL the declaration spells, not a source filename, and a URL no declaration publishes is an error at the call site — that catches stale references at build time. CSS `url(...)` resolves relative to the published stylesheet URL; Tola does not bundle CSS or JavaScript or rewrite their references. `build.minify.css` and `build.minify.javascript` apply to declared assets and to stylesheets and scripts a page writes inline; a tool that already minifies can leave them off. Tola's CSS minification may drop quotes from attribute selectors (`[data-width="fixed"]` becomes `[data-width=fixed]`); write selectors and tests that survive both spellings.

#### Build Tailwind with a hook

Use a finite `before-build` hook when the author has chosen Tailwind or another toolchain. Keep the tool's inputs private and its output inside the published tree:

```toml
[[build.hooks.before-build]]
name = "tailwind"
command = ["bunx", "@tailwindcss/cli", "-i", "static/tailwind-sources/site.css", "-o", "static/web-assets/tailwind-output/site.css"]
generates = ["static/web-assets/tailwind-output"]
rerun-on = ["static/tailwind-sources"]
```

`generates` declares the generated tree so dev attributes those writes and nothing else claims them; publish it through the existing `[assets]` tree (`/assets/tailwind-output/site.css`) or an exact `files` mapping. `rerun-on` adds literal paths Tola does not otherwise observe (the CSS entry directory, the package manifest or lockfile). Do not use `--watch`: Tola waits for a finite command and owns the watching. Consider adding a tree the hook rewrites on every build to the site's `.gitignore` and `.ignore`, so the generated bytes stay out of history; The Tailwind scaffold already ignores its generated stylesheet directory.

Tailwind's scanner does not read `.typ` files by default, so point it at the template and content files whose class names it must see; the site's own templates are otherwise not scanned:

```css
@import "tailwindcss" source(none);
@source "../../content/**/*.typ";
@source "../../templates/**/*.typ";
@source "../../site.typ";
```

`source(none)` with explicit `@source` keeps only the classes the site actually uses. Write the site's own rules in Tailwind's syntax — `@apply`, `@utility`, theme values, variants — not plain declarations: `@apply` compiles the named utilities into the rule, so `text-lg` becomes `var(--text-lg)` and `dark:text-red-500` the variant selector, where a hand-written declaration hardcodes the value and cannot express the variant at all. For the site's dark state, bind the variant to the attribute the site sets: `@custom-variant dark (&:where([data-theme="dark"], [data-theme="dark"] *));`. What may override a rule follows from where it sits, not how it is written: unlayered CSS outranks every layer, so a rule left outside a layer beats the utilities a page applies — keep the site's own classes in `@layer components`, or declare one with `@utility`, and lower specificity inside the layer with `:where()` (`.prose :where(h2)`).

Pin the toolchain in the site's own runner (`deno.json` tasks with `deno install`, or `package.json` with npm), run it once by hand to verify the recipe, and keep the lockfile under `rerun-on` when Tola does not observe it.

### Images and icons

#### Images and resized variants

Native `image(...)` reads an input and embeds its representation in the HTML; it is convenient for small images. For a browser-served file, publish it through `[assets]` and point `html.img` at its `asset-url`. For a resized derivative, use `resize-image` from `@tola/image`:

```typst
#import "@tola/image:0.0.0": resize-image
#let thumbnail = resize-image(
  path("/static/image-sources/cover.png"),
  width: 640, op: "fit-width", format: "webp",
)
#html.img(
  src: thumbnail.url, width: thumbnail.width, height: thumbnail.height,
  alt: "A mountain path leading toward the horizon",
)
```

| `op` | Needs | Result |
| --- | --- | --- |
| `fill` (default) | width and height | Center crop to that size |
| `scale` | width and height | Resize to both; may distort |
| `fit-width` / `fit-height` | one side | Preserve ratio; may enlarge |
| `fit` | width and height | Fit inside the bounds; never enlarges |

`path` is resolved against the call itself (or an already resolved `path(...)` value); it names an input file, never a browser URL. Formats are `auto` (JPEG for lossy sources without alpha, PNG otherwise), `jpg`, `png`, and `webp`; AVIF reports an error rather than silently choosing another format. JPEG quality is 1–100 (default 75) and requires an opaque `background` when the source has alpha; WebP is lossless unless a quality 0–100 is given; PNG is always lossless. `filter` selects the resampling kernel (`lanczos3` default, `nearest`, `triangle`, `catmull-rom`, `gaussian`). Output dimensions stay within 65,535 pixels per axis for JPEG and 16,383 for WebP. Animated images and SVG are not resized; re-export CMYK JPEG and unsupported HDR PNG first.

The returned dictionary holds `url`, `width`, `height`, `original-width`, and `original-height`; the URL is already mounted. Calling `resize-image` requests the derivative even if the result is discarded, and a build publishes every derivative its converged evaluation requested — a check publishes none. Resizing a file that an asset tree also publishes leaves the original published; to publish only derivatives, keep originals outside mapped trees.

`image-metadata(path)` inspects without producing output and returns `width`, `height`, `format`, `mime`, `has-alpha`, and `is-lossy`; `has-alpha` describes the source's alpha, not a pixel scan. Reading an image publishes nothing.

#### Configured icons

Define collections in `tola.toml`, then use them from `@tola/icon`:

```toml
[icons.collections.brand]
source-type = "local-svg-dir"
path = "static/icon-sources/brand"
```

```typst
#import "@tola/icon:0.0.0": icon
#icon("brand:mark", label: "Brand")
```

`icon(id, label: none, attrs: (:))` inserts inline SVG; without a label it is decorative, and a nonempty label names an informative image. `attrs` holds string-valued root SVG attributes such as `class` and `style`; the attributes the label owns (`aria-label`, `aria-hidden`, `role`, and friends) are refused. A control still needs its own name. `icon-bytes(id)` returns normalized SVG bytes for other compositions, and `icon-url(id)` publishes the icon and returns its mounted URL; identical bytes share one published file.

Icons keep their own colors. One drawn with `currentColor` follows the `color` of the page around it, so a theme or a CSS class recolors it; fixed fills and strokes stay as drawn. Prefer artwork already drawn that way — every icon of the `lucide` collection is, so the preset follows the theme by itself. A published icon (`icon-url`) cannot follow the page — a file of its own inherits nothing — so use `icon()` for an icon that should.

Collections can also be `local-json` (with a path) or `remote-json` (with a preset such as `lucide`). `tola inspect icons` lists configured namespaces and `tola inspect icons lucide` lists one namespace's names; remote collections download and cache exactly as a build does, so `--offline` applies. Keep SVGs self-contained and respect collection licences.

### Mathematics

#### Write Typst mathematics

Use Typst syntax rather than LaTeX commands:

```typst
The sequence term is $a_n$.
$ sum_(i=1)^n i = (n (n + 1)) / 2 $
$ frac(a + b, c) + sqrt(x) $
$ mat(1, 0; 0, 1) vec(x, y) $
$ x > 0 quad "for positive inputs" $
```

Subscripts use `_`, superscripts `^`, and parentheses delimit their scope; use `frac(...)` for complex fractions. Matrix commas separate columns and semicolons rows; `&` aligns and `\` breaks multiline equations. A single letter is a symbol and `#value` inserts a code variable. Common functions include `sqrt`, `frac`, `binom`, `cases`, `abs`, `norm`, `floor`, `ceil`, `limits`, `hat`, `tilde`, `arrow`; operators include `sum`, `product`, `integral`, `lim`, `NN`, `ZZ`, `QQ`, `RR`, `oo`. Use exact symbol names such as `times.o` and `subset.eq` rather than guessed LaTeX spellings.

Separate a baseline argument from a subscript and make fraction boundaries explicit, since some scope mistakes compile:

```typst
$ alpha_c (N) $
$ cal(S)_("loc")[x] $
$ frac(cal(Z)(tilde(S)), cal(Z)(0)) $
```

For numbered references, set a numbering and label the equation:

```typst
#set math.equation(numbering: "(1)", supplement: [Eq.])
$ a^2 + b^2 = c^2 $ <pythagoras>
See @pythagoras.
```

Numbers are not automatically page-local; choose the intended scope.

#### MathML and deliberate SVG rendering

HTML equations use MathML by default. Keep it unless a formula needs another form: the exporter ignores mathematical `overline`, `underline`, `cancel`, and skewed fractions with a warning, and SVG is how those keep their notation.

To render equations as SVG, apply the helper in the page template:

```typst
#import "@tola/web:0.0.0": math-svg
#show math.equation: math-svg
```

One rule covers inline and display math, following each equation's own placement; outside HTML export the equation is unchanged, and references and numbering survive. `alt` names the wrapper for assistive technology.

Framed math is selected by `tola-math-inline` or `tola-math-block`. SVG keeps the colors the equation was compiled with, so a stylesheet repaints the shapes to follow the text color — under a dark color scheme, for instance. The starter stylesheet repaints black fills and strokes separately:

```css
.tola-math-inline [fill="#000000"], .tola-math-block [fill="#000000"] {
  fill: currentColor;
}
.tola-math-inline [stroke="#000000"], .tola-math-block [stroke="#000000"] {
  stroke: currentColor;
}
.tola-math-block {
  margin-block: 1em;
  overflow-x: auto;
  overflow-y: hidden;
  text-align: center;
}
.tola-math-block > svg {
  margin-inline: auto;
}
```

SVG loses MathML's mathematical structure for assistive technology, so give important formulas a prose explanation or an `alt`.

Check superscripts, subscripts, fraction boundaries, font size, baseline, color, and narrow-width overflow in a browser; a syntactically valid expression does not prove the page displays the intended mathematics.

### Feeds, sitemap, and social metadata

#### Feeds

Declare feeds in the root program, next to the documents they describe. Selection and order stay the author's, and a site can declare several feeds — for example RSS and JSON Feed — each with its own `output`:

```typst
#import "@tola/web:0.0.0": feed, sitemap

#feed(entries: ((
  target: "notes/index.html",
  published: datetime(year: 2026, month: 9, day: 1),
),))
#sitemap(targets: ("notes/index.html",))
```

`feed(output: auto, format: "rss", id: auto, title: auto, description: auto, language: auto, authors: auto, entries: ())` supports RSS, Atom, and JSON Feed; default outputs are `feed.xml`, `atom.xml`, and `feed.json`. It needs `site.origin`, a nonempty title (site or explicit), and publication dates. Omitted feed `title`, `description`, `language`, and `authors` come from the site's values; an omitted `id` is the feed's own absolute URL.

An entry requires `target` and `published`, and may set `id`, `title`, `updated`, `summary`, `content`, and `authors`. A `target` is a logical output path, an `(output:, fragment:)` dictionary, a label, or a location. An element target needs a real exported anchor: something in the site must link to it (`#link(<label>)`, `@label`, or `#outline()`), because Typst gives the element an `id` only then; a whole-document label needs no body anchor. An automatic entry `id` is the resolved target URL and an automatic `title` is the target document's title — set independent stable IDs when subscriber identity must survive a move. Atom IDs must be absolute URIs and Atom needs authors on the feed or every entry. Entry IDs are unique within a feed.

`summary` and `content` are independent values. Strings are plain text; directly written content supports text, breaks, emphasis, strong, strike, and URL links. For full page content, select compiled HTML: `content: (document: "notes/index.html")`, optionally with `id:` to select one element. A body selection keeps body attributes in a `div` and includes the head's styles and stylesheet links (not scripts), so the fragment renders under its own dependencies rather than the page's full environment.

The starter's `feed` and `sitemap` metadata flags are conventions your program implements — a hand-written declaration does not read them. For sitemap targets, use the selected records:

```typst
#import "@tola/web:0.0.0": sitemap
#sitemap(targets: pages
  .filter(entry => entry.source.meta == none or entry.source.meta.at("sitemap", default: true))
  .map(entry => entry.output))
```

#### Sitemap

`sitemap(output: "sitemap.xml", targets: ())` lists whole documents. A target that resolves to an element — a label or location inside a document, or a `fragment` — is an error rather than a silently shortened address; so is a repeated target. Attach a modification date with `(target: ..., lastmod: ...)`. Listed URLs are absolute, so the site needs `site.origin`. Both feed and sitemap output paths stay outside Tola's reserved `_tola` namespace; conflicting output paths fail the build. Inspect the serialized files after building.

#### Canonical and social metadata

`canonical(href)`, `open-graph(...)`, and `twitter-card(...)` return arrays of native head entries, composed into a head exactly like `head-metadata`. They validate explicit arguments and infer nothing: URLs must be absolute HTTP(S), Open Graph requires a title, a kind, a URL, and at least one image `(url:, alt:)`, and a `summary_large_image` card requires an image. Setting site metadata does not create or publish the image.

```typst
#import "@tola/site:0.0.0": site
#import "@tola/address:0.0.0": output-to-url
#import "@tola/web:0.0.0": canonical, open-graph, twitter-card

#let url = output-to-url(output, origin: site.origin)
#let social-image = (
  url: output-to-url("assets/social-card.png", origin: site.origin),
  alt: "A description of the site's social card",
)
#let social-head = (
  ..canonical(url),
  ..open-graph(
    title: page-title, kind: "article", url: url,
    description: description, site-name: site.title, images: (social-image,),
  ),
  ..twitter-card(
    card: "summary_large_image", title: page-title,
    description: description, image: social-image,
  ),
)
```

SEO text fields require strings; convert a rich title with `plain-text` from `@tola/web` when needed. JSON-LD, preload hints, and other head content are ordinary head entries you write.

## Part 2: Reference

### Bundled packages

Import members or rename the module; the fixed import version is `0.0.0`, independent of the Tola executable's version. `@tola/host` is internal. Full signatures, parameter types, defaults, and examples: `tola help "@tola/<name>"`, or `tola help "@tola/address" slugify output-to-url` for selected exports.

| Package | What it gives you |
| --- | --- |
| `@tola/site` | The `site` dictionary: `origin`, `base-path`, `url`, `title`, `authors`, `description`, `language` (`tag`, `lang`, `script`, `region`), `copyright`, `extra` |
| `@tola/source` | `tola-meta`, `all-sources`, `current-source`, `parse-sources` — source metadata and selection |
| `@tola/address` | `slugify`, `route`, `route-to-output`, `output-to-route`, `output-to-url`, `decode-url-path`, `asset-url` — names, routes, outputs, and URLs |
| `@tola/document` | `current-document`, `headings`, `references` — the containing page, its headings, and the links you wrote |
| `@tola/collection` | Selection, grouping, hierarchies, and neighbors over your own arrays |
| `@tola/schema` | Value and metadata schemas, `parse`, `try-parse`, and structured issues |
| `@tola/code` | `render-code`, `code-themes`, `code-stylesheet`, `code-stylesheet-url` |
| `@tola/web` | `plain-text`, `head-metadata`, `canonical`, `open-graph`, `twitter-card`, `math-svg`, `feed`, `sitemap` |
| `@tola/icon` | `icon`, `icon-bytes`, `icon-url` |
| `@tola/image` | `resize-image`, `image-metadata` |

A bare `import "@tola/document:0.0.0"` binds a module named `document`, shadowing Typst's `document(...)`; prefer member imports or `import "@tola/document:0.0.0" as doc`. External packages use their own versions (`@preview/name:version`).

### Commands

| Command | Effect |
| --- | --- |
| `tola init [DIR]` | Create a scaffold in a new or empty directory; `--preset`, `--features`, `--interactive`, `--dry-run`, `--force`, `--editor` |
| `tola dev` | Build, serve locally, watch saved files, and update the browser after each successful revision |
| `tola preview` | Build once with production settings and serve locally; no watching or browser reload |
| `tola check` | Check a complete production build without writing the output directory |
| `tola build` | Build and write the configured output directory |
| `tola inspect sources` | Declared source metadata as JSON, without building; `--fields`, `--raw`, `--pretty`, `--filter-empty`, `--output` |
| `tola inspect icons [NAMESPACE]` | Configured collections, or one namespace's icon names |
| `tola inspect documents`, `routes`, `outputs`, `references` | Build without publishing and project HTML documents, URLs, outputs and producers, or link resolution as JSON |
| `tola config` | Selected resolved paths and settings as JSON |
| `tola doctor`, `tola doctor --json` | Check configuration, paths, and local tools; produce an issue report |
| `tola vendor` | Freeze the packages, fonts, and icons the build used into `[vendor] path`; `--refresh`, `--dry-run` |
| `tola editor setup`, `packages`, `template` | Configure editors, refresh package files, or print one editor's settings |
| `tola lsp` | Language server over stdio for editors |
| `tola skill` | Print this guide; `--output DIR` exports `tola/SKILL.md` |
| `tola completions <shell>`, `tola manpage` | Shell completions or a roff manual page |
| `tola help` | Configuration-table or bundled-package documentation; needs no site |

`tola help` with no target lists the commands, a help-topic index, the configuration-table selectors, and the bundled-package selectors. Quote table headers and package names: `tola help "[assets]"`, `tola help "@tola/web"`, `tola help "@tola/address" slugify output-to-url`. Package pages show a copyable import, an export index, then declarations, contracts, examples, and any related targets; a displayed value is not a function. `tola help` writes its documentation pages in English or Simplified Chinese (`--lang en|zh`, or the locale environment); command help and diagnostics stay English. `tola --version` prints the Tola and Typst releases.

Global options work on every command: `--color auto|always|never`, `--no-pager` (write `tola help` documentation directly), `-q/--quiet` (keep warnings and errors), `-v/--verbose` (repeat for trace detail), `--log-file PATH` and `--no-log-file` (JSONL session logs), and `--offline` / `--pure` (see [Vendoring, offline builds, and deployment](#vendoring-offline-builds-and-deployment)).

Notable behavior:

- `dev` and `preview` serve immutable in-memory revisions; they never replace the disk output. A failed build keeps the last good revision and, in dev, serves the diagnostics until a fix builds; the printed serving URL includes the mount.
- `check` and `preview` run `before-build` and `generate-outputs` with production settings but never `after-publish`; `inspect` runs no hook at all and does not write output.
- `build` replaces the entire configured output directory, removing stale files; a failed or cancelled build leaves the previous output intact. Keep hand-maintained files out of it. Disjoint `--output` destinations keep separate ownership.
- Build overrides apply to one invocation: `--output`, `--minify[=true|false]`, `--origin`, `--base-path`; `tola dev` and `tola preview` also accept `--interface` and `--port`, and `tola dev` additionally `--watch[=true|false]`. Port 0 chooses an available port; bind `0.0.0.0` or `::` only when LAN access is intended.
- `tola skill` needs no site and never overwrites: export elsewhere and diff before replacing an edited installation.
- `tola doctor --json` and `--log-file` are for issue reports; review logs for private material before sharing.

### Configuration

`tola.toml` is read before anything else. `tola init` generates a documented starting point — use it as a starting point, not as an inventory — and `tola help "<table>"` prints every field of a table with its documentation and defaults, for example `"[site]"`, `"[build]"`, `"[build.minify]"`, `"[build.references]"`, `"[build.hooks]"`, `"[assets]"`, `"[typst]"`, `"[typst.fonts]"`, `"[icons]"`, `"[vendor]"`, `"[server]"`, `"[dev]"`, and `"[diagnostics]"`.

The settings a site most often edits:

| Setting | Meaning |
| --- | --- |
| `[site] title`, `description`, `authors`, `copyright` | Values templates read; authors are `{name, email, url}` records |
| `[site] language` | A tag such as `"zh-Hans-CN"`, or `{lang, script, region}`; Typst reads `tag`, `lang`, `script`, `region` |
| `[site] origin`, `base-path` | Public origin without a path, and the deployment mount (`/docs/`) |
| `[site.extra]` | Custom values, available as `site.extra`; TOML dates arrive as strings |
| `[build] entry`, `content`, `output` | Root program (`site.typ`), content root (`content`), owned output (`public`) |
| `[build.minify] html`, `css`, `javascript` | Independent minification, all enabled by default |
| `[build.references] navigation`, `resources`, `fragments` | Missing-reference severity: `error` (default) or `warn` |
| `[assets] trees`, `files`, `cache-busting` | Published files and their site-root URLs |
| `[typst.fonts] paths`, `system` | Compiler fonts; `system = false` keeps host fonts out |
| `[icons.collections.<name>]` | A `local-svg-dir`, `local-json`, or `remote-json` collection |
| `[vendor] path` | Where vendored dependencies live |
| `[server] interface`, `port` | Dev/preview listener; `127.0.0.1:5277` by default |
| `[dev] watch` | Rebuild and browser reload in `tola dev` |
| `[diagnostics] max_errors`, `max_warnings` | Terminal display limits per batch (default 3), not loss of diagnostics |
`tola.toml` is read before anything else. `tola init` generates a documented starting point — use it as a starting point, not as an inventory — and `tola help "<table>"` prints every field of a table with its documentation and defaults, for example `"[site]"`, `"[build]"`, `"[build.minify]"`, `"[build.references]"`, `"[build.hooks]"`, `"[assets]"`, `"[typst]"`, `"[typst.fonts]"`, `"[icons]"`, `"[vendor]"`, `"[server]"`, `"[dev]"`, and `"[diagnostics]"`.
`site.url` is origin plus mount, or `none` without an origin; absolute SEO URLs need it. `site.extra` keeps author-chosen keys, and a TOML date or datetime is delivered as a string, so parse it or keep separate fields. Changing title or extras does not choose routes.

A link is checked when its destination is relative or site-root (`/guide/`), because only the
site's own outputs can answer it. A destination naming its own scheme and host is never checked —
`https://example.com/guide/` stays external even when it spells `site.origin` — since another
site may serve it, and Tola cannot observe what that site publishes. Write such a link as you
would any other; it neither fails the build nor needs a declaration.

### Hooks

Hooks are trusted site scripts with the caller's permissions — not sandboxed, and their side effects are not rolled back. Review them before running an unfamiliar site. `--offline` and `--pure` restrict Tola's own reads, not a hook's.

| Stage | Purpose |
| --- | --- |
| `before-build` | Generate declared site inputs (CSS, data, resized images) before discovery; runs from the site root and declares `generates` as site-relative files or directories |
| `generate-outputs` | Produce declared final outputs (search indexes, extra files); read the upstream snapshot at `TOLA_HOOK_INPUT_DIR` and write below `TOLA_HOOK_OUTPUT_DIR`, declaring `{file = "…"}` or `{tree = "…"}` outputs relative to the site output |
| `after-publish` | Consume the committed revision through a read-only view; failures cannot undo publication |

Every entry supports `enable` (default true), `name` (one word, unique in its stage), `command` (argv array, no implicit shell, finite), `dev` (`"run"`/`"skip"`; defaults to run before publication, skip for after-publish), and `rerun-on` (literal site-root paths whose edits trigger a dev build). A `before-build` entry declares `generates` — the paths it produces for the build to read — and a `generate-outputs` entry declares `outputs`, the final site outputs it adds. Declaring either lets dev attribute the hook's writes and check that they exist; neither publishes anything by itself: publish it through `[assets]` or a Bundle `asset(...)`.

Every stage receives `TOLA_HOOK_STAGE`, `TOLA_BUILD_MODE` (`dev` under `tola dev`; `prod` under `build`, `check`, and `preview` — production semantics, not a deployed site), `TOLA_HOOK_CACHE_DIR` (persistent per site/stage/name for the command's own results), and `TOLA_HOOK_TEMP_DIR` (this invocation's private directory; `TMPDIR`, `TMP`, and `TEMP` name the same one). `TOLA_HOOK_INPUT_DIR` names what the stage reads; only `generate-outputs` also gets `TOLA_HOOK_OUTPUT_DIR`. A variable a stage does not define is absent, never inherited. `tola check` runs the first two stages with production settings; `tola preview` too, never `after-publish`; `tola inspect` and `tola vendor` run no hook.

`rerun-on` paths are literals, not globs: a directory is observed recursively. An edit triggers a whole site build, and every participating command of its stage runs again; command results are not cached. Tola already observes content, templates actually read, assets, and icons; add a path only for a file or directory Tola does not otherwise observe, such as a CSS entry file or a scanner directory.

#### A search index

Pagefind reads the compiled HTML and writes its own index, so it belongs to `generate-outputs`: the
index joins the same candidate as the pages it indexes, passes the same reference checks, and
publishes with them. The scaffold's `pagefind` feature writes this hook, and `site/search.typ`
holds the box and its head entries:

```toml
[[build.hooks.generate-outputs]]
name = "search"
command = ["just", "search"]
rerun-on = ["justfile"]
outputs = [{ tree = "assets/pagefind-search" }]
```

```just
search:
    deno task search
```

```json
{
  "imports": { "pagefind": "npm:pagefind@1.5.2" },
  "tasks": {
    "search": "deno run -A npm:pagefind@1.5.2 --site \"$TOLA_HOOK_INPUT_DIR\" --output-path \"$TOLA_HOOK_OUTPUT_DIR/assets/pagefind-search\""
  }
}
```

The declared tree needs no `[assets]` entry — it joins the output graph at
`assets/pagefind-search/...`, and only this hook may own that path. Link it with
`output-to-url("assets/pagefind-search/pagefind-ui.js")`: `asset-url` answers for `[assets]`
declarations alone. Cache busting does not reach a hook's bytes either; the command names them
itself, and Pagefind hashes its own data files. Run the CLI through `deno run`, not `deno x`: the
task must use the version `deno install` resolved into `deno.lock`, or a build on another machine —
or an offline one — installs Pagefind while the hook runs. `generate-outputs` runs in development
too, so the box works under `tola dev`.

### Vendoring, offline builds, and deployment

```sh
tola vendor --dry-run   # prepare and validate; may fetch, keeps the current vendor
tola vendor             # replace the vendored packages, fonts, and icons
tola build --pure       # prove the site builds from its sources and frozen dependencies
```

Vendoring needs a configured `[vendor] path`; it collects the Typst packages, remote icon collections, and compiler fonts the build selects, runs no hooks, and never publishes the site. `--refresh` re-selects without reading the installed copy. A dry run still takes the site lock and may download or cache.

`--offline` refuses Tola's network requests but permits host package roots, system fonts, and local caches. `--pure` additionally excludes host packages, host caches, system fonts, and source files physically outside the site. Embedded packages and fonts and recomputable image caches stay available. Neither flag is a hook sandbox, so a machine-dependent hook can still make output differ.

To deploy: configure the real `site.origin` and `site.base-path`, preview under them, run `tola build`, and upload the complete configured output directory to a static host. Preserve generated subdirectories, configure the host to serve `index.html` for directory routes, and set its missing-page response to the site's `404.html`. Provider files such as `CNAME` are not created automatically; publish them explicitly when required.

## Part 3: Check the actual site

Use the smallest check that proves the changed behavior, then inspect the result:

1. Save the changed sources. `tola inspect sources` shows metadata as JSON and `tola inspect routes` shows the document and asset URLs; inspection runs no hooks.
2. Run `tola check` for a complete-site check, including configured input and output generation. Fix missing pages, resources, fragments, and metadata rather than lowering reference errors to warnings.
3. Run `tola preview` for production behavior, or `tola dev` while editing, and open the printed mount URL — not an assumed host-root path.
4. In a browser, exercise the changed surface. A successful compilation proves nothing about appearance or interaction.
5. Run `tola build` only when writing the configured output directory is intended, then inspect generated feeds, sitemap, downloads, URLs, and page HTML.

A useful pass over any site:

- Heading hierarchy: exactly one `<h1>` per page, the first heading is the `<h1>`, and levels do not skip. A page whose title uses `=` instead of `#title()` has no `<h1>`.
- Code highlighting: for each `pre.tola-code`, check `getComputedStyle` of the token runs — a wrapper attribute alone proves nothing — to confirm the intended theme is applied.
- Site toggles (width, theme): verify both the computed style change and that `localStorage` (or whatever the site uses) remembers it.
- Feed and sitemap exclusions: check drafts, `feed: false`, and `sitemap: false` one by one against the serialized files, not entry counts alone.
- Asset URLs: confirm `?h=` changes when bytes change and that served `Content-Type` is right (`application/rss+xml`, `image/svg+xml`, and so on).
- Mounted links and assets: everything renders when served under `site.base-path`, with no doubled mount.

Common failures and what they mean:

- `a site input changed while the site was being built` (`write.stale`): an input changed during the build, so the candidate was refused. Normal while editing in dev; run again once edits stop.
- `` `/lab/` is not a page this site publishes ``: a link, including a hand-written `html.a(href:)`, points at a page the complete output graph does not contain. The message lists the pages holding the reference; fix or remove the link. Only relative and site-root destinations are checked, so an absolute URL never produces this.
- `asset-url` fails at the call site: the URL is not published by any `[assets]` declaration. Add the declaration or fix the spelling; a rename never silently drops references.
- An unknown call or missing signature: read `tola help "@tola/<package>" <export>` and the editor hover for the intended executable; the same name may differ between versions.
- A lost paged layout: HTML is not a page. Rebuild the layout with CSS, or use `html.frame` deliberately for SVG artwork.
- `heading of level N was transformed to <div role="heading" …>`: the section hierarchy is deeper than HTML's `<h2>`–`<h6>` range; restructure it.

For a problem, start with `tola config` (selected resolved settings) and `tola doctor --json` (configuration, paths, and local tools). `--quiet` keeps warnings and errors; `-v` and `-vv` add debug and trace detail; `--log-file PATH` records a JSONL session log, and `--no-log-file` disables the automatic one. Review logs and issue reports for private material before sharing.

When reporting a change, name the files you changed, the behavior achieved, the Tola executable used, and the commands actually run. For browser work, give the exercised route and viewport. State any unperformed build, browser, or deployment check plainly.
