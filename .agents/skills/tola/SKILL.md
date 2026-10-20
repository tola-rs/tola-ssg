---
name: tola
description: Build and maintain Tola websites. Use for site setup, Typst content and templates, routes and addresses, navigation, HTML and CSS, assets and media, SEO, build hooks, publishing, and troubleshooting.
---

# Writing Tola sites

A Tola site is one root Typst program. It reads the sources under `content/`, chooses the documents
to publish, and names every output file. Tola compiles the program, adds the files that `[assets]`
publishes and hooks generate, checks the complete site, then publishes it. Content is Typst; browser
structure and appearance are HTML and CSS.

The work has four steps:

1. **Discover** — `all-sources()` lists the `.typ` files under `build.content-dir`.
2. **Select** — `parse-sources(sources, schema)` validates metadata; the root program turns the
   records it keeps into `(source:, output:)` pairs.
3. **Render** — `document(output, ...)` writes each pair; a template composes its HTML shell.
4. **Collect** — Tola adds assets and generated outputs, checks references, then publishes.

Most of the work lives in `site/selection.typ` and `site/page.typ`; the root program `site.typ`
connects them. These names are scaffold conventions, not requirements.

The executable carries the exact reference. Use this guide for decisions and connections.

```sh
tola --version
tola help "@tola/document" headings
tola help "[build.hooks]"
```

`tola <command> --help` lists command options. `tola help -i` browses the bundled documentation;
`TOLA_PAGER` applies to ordinary help output. Package pages describe functions and values;
configuration pages describe fields and defaults.

Package imports use `0.0.0`, independently of the executable release. Import members, or rename the
module: a bare `@tola/document` import shadows Typst's `document(...)`. `@tola/host` is internal.

| Word | Means |
| --- | --- |
| source | a `.typ` file under `build.content-dir`; an input, not a published page |
| document | one output file; `document(output)` writes it |
| output | the file path under `build.publish-dir`, such as `guide/index.html` |
| route | the decoded site-root address, such as `/guide/` |
| URL | the browser address: a route or output plus the mount and encoding |
| mount | `site.base-path`, the path the site is served below |
| scaffold | the files `tola init` writes |

## Start with the site

### Read an existing site first

Read `tola.toml`, the root program and the `site/` files it imports, and any `[build.hooks]`
entries before changing them. The scaffold table under *Create a site* names what each file owns.

Tola uses the nearest `tola.toml` in the current directory or its parents. `-C path/to/tola.toml`
is a per-command option that selects another. The configuration file's directory is the site root;
configuration paths are relative to it. `tola config` prints the resolved paths and settings as
JSON.

### Create a site

```sh
tola init my-site --dry-run
tola init my-site
cd my-site
tola dev
```

`init` accepts a missing or empty directory. `--force` allows a non-empty directory but never
overwrites scaffold files. Choose a preset, individual features, or an interactive selection with
`tola init --help`. `--dry-run` prints the file tree and configuration without writing them.

`tola dev` builds, serves the URL it prints (including the mount), and rebuilds on save; the browser
follows successful builds. `tola check` runs production checks without writing published files.
`tola build` replaces `build.publish-dir` (`public` by default).

| Path | Role |
| --- | --- |
| `content/index.typ` | the first page; its path gives its default route |
| `site.typ` | root program: `select-pages(all-sources())`, then one template call per record |
| `site/schema.typ` | metadata fields; unknown fields are errors until added here |
| `site/selection.typ` | `select-pages`: parse metadata, drop drafts, choose outputs |
| `site/page.typ` | `page-template(page)`: one record becomes one HTML document |
| `site/seo.typ` | `head-entries`; feed and sitemap outputs when those features are selected |
| `site/not-found.typ` | emits `404.html` |
| `static/web-assets/` | published below `/assets/` by the scaffold's `[assets]` tree |
| `vendor/` | frozen packages and icon collections for `--pure` builds |

Keep shared modules outside `build.content-dir`; `site/` and `static/` are conventions, not special
names. An empty content directory is valid when the root program generates pages from data.

A site that publishes no HTML document gets `site.no_pages`; `dev` and `preview` serve a welcome
page instead. The scaffold's `404.html` counts as a document, and a build without one warns
`site.not_found_missing`.

For language services, run `tola editor setup --list`, then the chosen editor's setup command.
After upgrading Tola, `tola editor packages` refreshes the package files the editor reads.

You have a working site when `tola dev` serves the starter page at the mount and a save rebuilds it.

## Write and select pages

### Typst rules that bite here

Typst starts in markup: `#` introduces code, `{ ... }` is a code block, and `[ ... ]` is content.
Inside code, expressions need no `#`; inside content they do. `[A *rich* title]` is content;
`"A *rich* title"` is a string. Strings containing HTML remain text; use content or the HTML
constructors to emit markup.

`import` reads definitions; `include` inserts content. Neither inherits the caller's locals: pass
inputs to functions. Render arrays with loops or `.join()`. A multiline chain at the top level needs
`{ ... }` or parentheses; a new `.map(...)` line is markup, not a continuation. Named arguments use
`name: value`, `..values` spreads a collection, and trailing content comes after named arguments:
`html.ul(class: "topics")[...]`.

Read configured values through `#import "@tola/site:0.0.0": site`. Define custom values in Typst
modules and pass them explicitly to the functions that need them.

### Declare metadata

A source declares its metadata with one `tola-meta` call in its ordinary evaluation:

```typst
#import "@tola/source:0.0.0": tola-meta

// This page's metadata; every allowed field is declared in `site/schema.typ`.
#tola-meta((
  title: "Welcome",
  description: "A starter page for a site built with Tola",
))
```

- Call it once, outside `context` and outside show rules. A call made by a helper in another file
  belongs to that file and declares nothing for the source that called the helper.
- A declaration whose call already ran survives a later error, but the error still fails the build.
  Draft sources are evaluated too.
- Metadata fields belong to the site. The scaffold's schema covers title, description, authors,
  tags, draft, permalink, feed and sitemap flags, publication dates, and feed bodies.
- Extend `site/schema.typ` before using a new field: its default rejects unknown keys.

A Typst type such as `str` is already a schema. `optional(S)` accepts an absent key; without a
default the key stays absent. `nullable(S)` accepts explicit `none`. An invalid supplied value never
falls back to a default. `non-empty(trim(str))` trims before checking; reversing the wrappers can
accept whitespace and return an empty string. `unknown: "keep"` preserves undeclared fields without
validating them. `try-parse` returns `(ok: true, value:)` or `(ok: false, issues:)`; malformed
declarations and callback errors still stop compilation. Exact contracts:
`tola help "@tola/schema"`.

Use `date` for calendar fields: a native date or strict `YYYY-MM-DD` string becomes a native
date-only `datetime`. Use `timestamp` for instants: a complete native calendar datetime or strict
RFC 3339 string becomes `(rfc3339:, unix-seconds:, subsecond:)`. Native values use UTC, with
date-only native values at midnight; date-only strings belong to `date`. Both require years 1
through 9999 and reject time-only values; timestamps reject leap seconds. Examples:
`tola help "@tola/schema" date timestamp`.

The scaffold parses `published` as a timestamp and `updated` with `union(date, timestamp)`, so a
date-only change date stays at calendar precision in the sitemap.

Read data with `json("/static/data/chapters.json")`, `csv(..., row-type: dictionary)`, or `read`. A
string argument names a file; parse JSON bytes read from a file with
`json(read("data.json", encoding: none))`. CSV cells are strings. Reading does not publish the
input. `datetime(year: 2026, month: 9, day: 1)` constructs a date; parse date strings with the
schema above.

### Parse and select in the root

`all-sources()` lists discovered `.typ` inputs, not published pages. Discovery accepts lowercase
`.typ`, skips hidden names, symlinks, and the entry, and orders records by complete content path.
Each record carries `id`/`path`, `file` (a Typst input path for `include` or `read`),
`route-segments` (the default identity from the file layout), and `meta` (the declaration).

The root program receives settled declarations. Parse the complete set there, then filter, sort,
and choose outputs. `parse-sources` points field failures at the literal metadata value when its
location is known, otherwise at the `tola-meta` call, or the file start when absent. Notes and
nested causes remain separate hints. It treats absent metadata as an empty dictionary and keeps
each record's identity and order, replacing only `meta`. Pass one declaration, or
`source => declaration` to choose by each complete source record; the call resolves every record
before reporting field issues, so one run collects all field errors. Neither `all-sources()` nor
`tola inspect sources` is changed by that parse.

The scaffold's `select-pages` uses `parse-sources`, drops drafts, and returns `(source:, output:)`
records. Pass those records to page templates, menus, feeds, and sitemaps instead of recomputing
destinations; `site/selection.typ` owns the choice and `site.typ` connects it to rendering.

A source can appear in several documents or none: its source identity stays the same while the
containing document identity changes. The same JSON data can serve two layouts and a download; a
`site/` helper returns content while the root program owns outputs.

Check it: `tola inspect sources` shows the declared metadata, and `tola inspect documents` shows
the outputs the selected records produce.

## Choose documents and addresses

### Path, output, route, and URL

These are four different values:

- An input `path` names a file Typst reads.
- An `output` names a published file, such as `guide/index.html`.
- The decoded site-root `route` is `/guide/`.
- The browser `URL` adds percent-encoding, the mount, and optionally an origin.

`join-route` joins chosen segments, `output-of` chooses the output filename, `route-of` reverses
that, and `output-url` builds the browser address. `route-url(route)` equals
`output-url(output-of(route))`; both canonicalize `/guide/index.html` to `/guide/`. Exact
contracts: `tola help "@tola/address"`.

The scaffold slugs each file-layout segment: `index.typ` becomes `/`, `about.typ` and
`about/index.typ` both become `/about/`, and `guide/install.typ` becomes `/guide/install/`. Two
sources claiming one output conflict.

A trailing `/` names a directory: `/guide/` is `guide/index.html`, while `/guide` names the file
`guide`. A permalink is decoded once and changes the address, not the format, so a PDF still needs
a PDF document declaration. The scaffold parses `permalink` with `@tola/schema`'s `url-path`, whose
value is already a decoded route ready for `output-of` or `route-url`.

### Slugs, permalinks, and the mount

Slug each segment separately. `slugify` defaults to `mode: "unicode"`; `mode: "ascii"` transcribes
Han text, and `language: site.language.lang` selects the reading (`ja` for Japanese, anything else
Chinese). A literal `%` or `#` may belong to a decoded filename; encoding happens at the URL
boundary.

For deployment at `https://example.com/docs/`, set `origin = "https://example.com"` and
`base-path = "/docs/"` under `[site]`. Keep outputs unprefixed. `output-url`, `route-url`, and
`asset-url` apply the mount once; native links to exported targets are written relative to the
containing document, so they stay under the mount. Pass `origin: site.origin` when an absolute URL
is needed.

Hand-written site-root URLs must include the mount; a link to `/guide/` under `/docs/` fails and
names the mount. `asset-url` takes an unmounted declared URL and preserves its exact filename
(including `index.html`); write a literal `%` or `#` in a filename as `%25` or `%23`.

`current-source()` identifies the source containing the call and needs no `context`.
`current-document()` identifies the containing output and needs `context` in a document body; its
`location` scopes native queries. Sources rendered into one document share that document identity.

Check it: `tola inspect routes` lists each document output and the browser URL it publishes at.

## Compose the page

### Use typed HTML by default

Default to Typst's typed HTML constructors: `html.div`, `html.span`, `html.a`, `html.section`,
`html.nav`, `html.article`, and the rest. They take each element's real HTML attributes as named
arguments, escape text, and reject an attribute the element does not have.

Use `html.elem` only when the typed API lacks the tag or attribute, and say which one is missing:

```typst
#html.aside(class: "note")[A reusable note.]

// `data-kind` has no typed parameter, so this one needs the escape hatch.
#html.elem("aside", attrs: (class: "note", "data-kind": "tip"))[A reusable note.]
```

Typed constructors take documented attributes and one body; void elements take none. For a Typst
target, use native `link`; reach for `html.a` only when you need raw anchor attributes such as
`target` or `rel`.

### The HTML shell

The scaffold's `page-template(page)` wraps `page.source.file` in a custom `html.html` shell. A
custom shell owns its head: `document()` metadata does not fill it. Write the title, viewport,
description, and helper entries there, and give the shell an `html.head`. The head takes one
content body; loop or `.join()` entry arrays. The exporter appends the equation stylesheet to that
head. The scaffold's `site/page.typ` shows the complete wrapper.

### Headings and labels

`#title()` exports as `<h1>`; native section headings start at `<h2>`. Use native headings for the
outline, numbering, and queries. A label gets an HTML anchor when a link, reference, or outline
targets it. Prefer stable explicit labels for section URLs; use native locations for a table of
contents. `link` navigates; an `@label` reference also requires a numbered target. `html.h2` does
not create a native heading.

`html.script` and `html.style` take strings; an external script is
`html.script("", src: "...", defer: true)`. Use CSS for browser layout: Typst page settings,
columns, and spacing do not become CSS. `html.frame` deliberately makes SVG artwork and cannot
contain `set page`. Upstream's default footnote placement is unavailable in a custom `html.html`
shell; use a custom footnote show rule, or retain the default shell. Citations and bibliography
work in a custom shell.

### Code blocks

For highlighted code, combine the source's `raw.theme`, the renderer, and the head stylesheet:

```typst
#import "@tola/code:0.0.0": render-code, code-stylesheet, code-themes
#set raw(theme: code-themes.github)
#show raw: render-code.with(dark-theme: code-themes.github-dark)
```

Load `code-stylesheet()` in `html.head`. `data-theme="dark"` on the root chooses the dark
appearance; the site supplies the theme control. The scaffold follows the system when no explicit
theme is set. Style the code wrapper with CSS. Inline backticks have no language; use
`raw(..., lang: "rust")` for inline highlighting. Read `tola help "@tola/code"` for exact contracts.

Check it: render the route and inspect the head, the heading outline, and the code colors.

## Navigate and collect

Select visible pages and destinations once, then choose an editorial order with an identity
tie-breaker. Pass the same records to templates, sidebars, neighbors, feeds, and sitemaps;
recomputing destinations is how links drift.

- **Page TOC** — use contextual `headings`, native locations, and a separate `outlined` filter. Put
  the helper in `site/` and place it from the page template.
- **Backlinks** — query body references, choose one row per linking document, and render the list
  outside the queried region so it does not include itself. A heading label selects the heading,
  not the chapter text after it; label a containing element to select that text.
- **Hierarchy** — `key`/`keys` receive members, and `parent` receives keys, including keys absent
  from the array. Hierarchy keys must be unique and non-`none`, and the parent rule must reach
  `none`. Contracts: `tola help "@tola/collection"`.

### Breadcrumbs and a section sidebar

For the scaffold, add `order: optional(int, default: 0)` to `site/schema.typ` and write an overview
at `content/docs/index.typ`. Sort the selected pages by `(page.source.meta.order, page.source.id)`
in a code block:

```typst
#let pages = {
  select-pages(all-sources())
    .sorted(key: page => (page.source.meta.order, page.source.id))
}
```

This `site/navigation.typ` follows HTML directory outputs and tolerates missing overview records
while traversing their keys:

```typst
#import "@tola/address:0.0.0": output-url
#import "@tola/collection:0.0.0": lineage, descendants

#let parent-output(output) = {
  let parts = output.split("/")
  let directory = parts.slice(0, parts.len() - 1)
  if parts.last() == "index.html" {
    if directory.len() == 0 { return none }
    directory = directory.slice(0, directory.len() - 1)
  }
  (directory + ("index.html",)).join("/")
}

#let docs-navigation(pages, output) = {
  let key = page => page.output
  let trail = lineage(pages, output, parent-output, key: key)
  let members = descendants(pages, "docs/index.html", parent-output, key: key)
  html.nav(aria-label: "Breadcrumbs")[
    #if trail != none {
      for page in trail [#link(output-url(page.output))[#page.source.meta.title] / ]
    }
  ]
  html.nav(aria-label: "Documentation")[
    #for page in members [
      #link(output-url(page.output))[#page.source.meta.title] \
    ]
  ]
}
```

Then change `page-template(page)` to `page-template(page, pages)`, import `docs-navigation`, and
insert `#docs-navigation(pages, page.output)` in the body. Pass both arguments from the root's
rendering loop, and keep the missing-page and selected SEO output declarations.

`parent-output` is the part to change: it turns each output into the directory overview that
contains it. Permalinks are addresses, not hierarchy; supply an explicit parent rule when they
should not determine the tree.

`adjacent(pages, page.output, key: member => member.output)` finds previous and next pages in that
order. A missing anchor returns `none`; boundary neighbors have no `before`/`after` key. Pagination
needs the generated listing pages' neighbors, not the original posts. Group tags with
`group-by-keys`, compute listing outputs once, and handle tags that slug to the same name
explicitly. Sort parsed timestamps by `(stamp.unix-seconds, stamp.subsecond)`; the fractional
string keeps all digits except trailing zeros, preserving precision beyond nanoseconds. Use
`.rfc3339` for display and SEO, or native calendar dates for year archives.

Check it: `tola check` resolves every menu link, and the rendered page shows the breadcrumb trail
and section list you expect.

## Feeds, sitemaps, and head metadata

Reuse the selected page records for feeds and sitemaps. Both need `site.origin`; a feed also needs a
non-blank title from `title` or `site.title`. The scaffold's `feed-output` and `sitemap-output` skip
their output when those values are absent.

The scaffold implements `feed` and `sitemap` flags and defaults a missing or `none` `feed-content`
to the whole exported body. Setting only `feed-summary` therefore still publishes full-text RSS; in
`site/seo.typ`, make the `content:` value `none` for an excerpt-only entry. Direct `feed` entries
use `summary` and `content`; omit `content` for a summary-only entry.

Direct Typst content supports portable text, breaks, emphasis, strong, strike, and URL links; it is
not rendered as a document. Styles, deferred values, and unsupported elements are rejected.
`(document: output)` takes the whole body; `(document: output, id: "article-id")` takes one
exported HTML subtree. That ID must exist exactly once and is separate from the entry's subscriber
identity. Use it to exclude menus and sidebars while keeping the article's formatting and images.

Set fixed entry IDs when a route may move. Feed dates use the timestamp inputs above; sitemap dates
also accept date-only strings. Pass `.rfc3339` from parsed timestamp records: parsing keeps the full
fractional precision, while feed and sitemap formatting keeps at most nanoseconds and RSS keeps
whole seconds. Sitemap targets are whole documents, not fragments.

Canonical and social helpers return head-entry arrays. Build absolute page and image URLs with
`output-url(..., origin: site.origin)`, convert rich titles with `plain-text`, and preserve `none`
for omitted descriptions. Declaring social image metadata does not publish the image; put the image
under `[assets]` or publish it with `icon-url`. Formats and fields:
`tola help "@tola/web" feed sitemap canonical open-graph twitter-card`.

Check it: inspect the serialized `feed.xml` and `sitemap.xml` for entries, exclusions, IDs, dates,
content, and absolute URLs.

## Add assets and media

### Assets and browser fonts

`[assets]` maps source files or trees to public site-root URLs. The scaffold maps
`static/web-assets/` to `/assets/`. Load a declared stylesheet in the head:

```typst
#import "@tola/address:0.0.0": asset-url
#html.link(rel: "stylesheet", href: asset-url("/assets/css/site.css"))
```

`asset-url` takes the declared URL, not a source filename. It applies the mount and the optional
byte-dependent query without changing the filename; an undeclared URL fails at the call site. An
exact file mapping takes precedence over a tree member's URL. CSS `url(...)` resolves against the
published stylesheet. Tola does not bundle JavaScript or rewrite CSS resource references.
`tola help "[assets]"` has the complete declarations.

Compiler fonts and browser fonts have separate inputs. `[typst.fonts]` supplies Typst, and
`tola fonts` lists what a build discovered. Browsers need published files loaded through CSS
`@font-face`; setting a Typst font does not install a browser font. Fonts are site inputs: keep them
under `[typst.fonts] paths`. `system = true` with `--pure` is an error, and `--font-path` is refused
under `--pure`. `site.language` supplies the default language; `site.languages` is template data
for the site's own routing and switcher. Read `tola help "@tola/site"` for their shape.

If you use Tailwind, scan the actual `content/`, `site/`, and entry files. Keep its input outside
the published tree and run a finite stylesheet recipe before the build; the scaffold's
`tailwind-css` feature wires the hook and the `data-theme` dark variant. Put component rules in a
layer when utilities should override them.

### Images and icons

Use native `image` for compiler-rendered images and figures, `html.img` with a declared `asset-url`
for a browser-served original, or a `@tola/image` helper for a generated derivative. `img` places
one image at several widths as a responsive element; `image-srcset` returns the same values
without markup (`url`, `srcset`, and the widest derivative's dimensions); `resize-image` derives
one deliberate size.
Their input is a filesystem path or raw image bytes (`read(path, encoding: none)` for a binary);
their returned URL is already mounted.

`resize-image` rasterizes an SVG only with an explicit raster `format`; `image-srcset` and `img`
refuse SVG. Show a vector with `image()` instead. `image-metadata` inspects without publishing.
Pass `sizes` yourself whenever the element is not full-width. A mapped original remains published
after resizing; keep inputs outside asset trees when only derivatives are wanted.

Icon collections, their sources, and the `namespace:name` ids come from `[icons.collections]` in
`tola.toml`. An unlabeled icon is decorative. Informative icons need a label, and surrounding
controls still need their own names. Inline artwork using `currentColor` follows the page's text
color; fixed fills stay fixed. An `icon-url` image does not inherit its surrounding page's color.
Complete options: `tola help "@tola/image"` and `tola help "@tola/icon"`.

### Mathematics

HTML equations use MathML by default. Keep it unless notation is unsupported.

For deliberate SVG, wrap `html.frame` in a site component:

```typst
#let math-svg(equation) = html.frame(equation)
#show math.equation: math-svg
```

Numbering and references survive. SVG keeps compiled colors; repaint black fill and stroke
selectively so intentional colors remain. If the component uses `tola-math-inline` and
`tola-math-block`, these rules apply:

```css
.tola-math-inline [fill="#000000"], .tola-math-block [fill="#000000"] { fill: currentColor; }
.tola-math-inline [stroke="#000000"], .tola-math-block [stroke="#000000"] { stroke: currentColor; }
.tola-math-block { overflow-x: auto; overflow-y: hidden; text-align: center; }
.tola-math-block > svg { margin-inline: auto; }
```

SVG loses MathML's structure. Explain essential formulas in prose, or add `aria-label` in the
component. `figure(alt:)` and `math.equation(alt:)` do not supply HTML alternatives in the current
exporter. Check notation, baseline, colors, and narrow-width overflow.

Check it: `tola check` builds the declared assets, and the rendered route shows fonts, image
alternatives, icon labels, and equation layout at narrow widths.

## Run tools around a build

Hooks are external commands grouped by what they produce. Tola waits for each stage.

| Stage | Runs | Produces |
| --- | --- | --- |
| `before-build` | before source discovery | site inputs, such as generated CSS |
| `generate-outputs` | after the Bundle compiles | final files added to the candidate, such as a Pagefind index |
| `after-publish` | after publication | consumers of the committed site; cannot change or undo it |

`tola check` and `tola preview` run the first two stages and skip `after-publish`; `tola build` runs
all three. `tola inspect` and `tola vendor` run none.

Use the site's own runner with finite recipes; Tola owns watching, so a hook must finish. A
`before-build` command writes inputs the site then reads; publish them through `[assets]`, or have
the site read them and emit the result. Use `generate-outputs` for tools that read compiled HTML;
their declared outputs join the final site directly and need no asset mapping. Link those files
with `output-url`.

`rerun-on` adds literal site-relative paths the site otherwise does not read, including tool
manifests and lockfiles; it takes no globs. `--pure` and `--offline` restrict Tola's own reads, not
a script's side effects. Exact declarations, stage directories, and environment variables:
`tola help "[build.hooks]"`.

`generate-outputs` commands also accept `dev = "reuse"`: in development, Tola runs the command only
when the compiled site or a `rerun-on` path changed, and otherwise reuses the previous outputs.
Use it only for a command whose output depends solely on the compiled site, its `rerun-on` paths,
its command, and its declared `outputs`; a command that reads time, the network, environment state,
or files outside the site keeps the default `dev = "run"`. With `reuse`, list every site file the
command reads in `rerun-on`; an omitted file can leave stale output. `dev = "skip"` keeps a command
out of development, and `tola dev --no-reuse` runs every reusable command for that session.
Production builds always run the hooks. The rebuild summary names the source of each change
(`search: 9 changes`), and `tola dev -v` lists every changed output path.

The scaffold can wire a Tailwind `before-build` hook and a Pagefind `generate-outputs` hook; select
those features at `tola init`. The search feature builds the index; the search UI is yours to add.

Check it: `tola check` runs the first two stages and fails on a hook error or an output conflict.

## Check, publish, and diagnose

### Check the changed surface

- `tola inspect sources` shows declarations; `tola inspect documents`, `routes`, `outputs`, and
  `references` build without publishing and show the resulting outputs. Inspection runs no hooks,
  so it cannot generate missing hook inputs.
- `tola check` builds the complete production site and checks the complete output and its
  references.
- `tola check --no-build` only parses the site's own sources. It builds nothing, so it proves
  syntax alone.
- `tola fmt` formats the site's Typst sources in place; `tola fmt --check` reports what is not
  formatted and exits non-zero without writing. Both read `[formatter]` from `tola.toml`; the
  language service uses the same settings for the file an editor works in.
- `tola dev` watches and serves in memory; `tola preview` builds once with production settings and
  serves without watching. Both apply the mount and leave disk publication to `build`.
- Rendered changes: exercise the actual route for heading structure, token colors, controls, image
  alternatives, mathematical notation, and narrow layouts.
- Feeds and sitemaps: inspect serialized entries, exclusions, IDs, dates, content, and URLs.

A change is ready when the smallest relevant command above passes and the route behaves as
intended.

### Publish and deploy

`tola build` replaces all of `build.publish-dir`, removing stale files. Failure before publication
keeps the previous tree; an `after-publish` failure does not roll it back. Keep hand-maintained
files outside that directory. `--publish-dir` overrides the directory for one run. `inspect sources
--output` and `skill --output` write those commands' results to a path; they publish no site.

Set the real `site.origin` and `site.base-path`, preview under them, build, and upload the complete
publish directory. Configure the host's directory routes to serve `index.html` and its missing-page
response to use `404.html`. Publish provider files such as `CNAME` explicitly when needed.

### Freeze dependencies

Configure `[vendor] path`, then:

```sh
tola vendor --dry-run
tola vendor
tola build --pure
```

Vendoring runs no hooks and freezes the packages and remote icon collections a production build
selected. Compiler fonts are never vendored; keep site fonts under `[typst.fonts] paths`. Its
verification build proves the frozen inputs resolve and compile under `--pure`; it skips hooks and
link checks, which belong to `tola check`. `--offline` refuses network reads but allows host
packages, caches, and system fonts when enabled; `--pure` also excludes those host inputs and files
outside the site. Embedded dependencies remain available. Options: `tola vendor --help` and
`tola help "[vendor]"`.

### Diagnose a failure

Start with the named file and the suggested next step.

- `tola config` shows resolved paths and settings; `tola doctor --json` reports configuration and
  local tools.
- Undeclared assets, local pages, and fragments: point them at outputs the site actually publishes.
  Absolute URLs stay external, even when they spell `site.origin`.
- `write.stale` means an input changed during the build; run the build again once edits stop.
- Deep native headings need a shallower hierarchy when they exceed HTML's heading levels.
- An invalid package call: read the installed executable's package help or the editor hover.
- `-v`/`-vv` and `--log-file PATH` record detailed diagnostics.

When you report a result, name the changed files, the behavior achieved, the executable used, and
the checks actually run. For browser work, name the route and viewport. State any build, browser,
or publication check you did not perform.
