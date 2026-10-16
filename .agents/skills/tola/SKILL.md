---
name: tola
description: Build and maintain Tola websites and documentation. Use for site setup, Typst content and templates, navigation, HTML and CSS, assets, media, SEO, hooks, and troubleshooting.
---

# Writing Tola sites

One root Typst program chooses the site's documents and generated files. Tola compiles the Bundle,
combines its outputs with configured assets, and checks the complete site before publication.
Content is Typst; browser structure and appearance use HTML and CSS.

Use this guide for working decisions and connections. The installed executable carries the exact
reference:

```sh
tola --version
tola help "@tola/document" headings
tola help "[build.hooks]"
```

Package pages describe functions and values; configuration pages describe fields and defaults.
Use `tola <command> --help` for command options. Package imports use `0.0.0`, independently of the
executable release. Import members or rename the module: a bare `@tola/document` import shadows
Typst's `document(...)`. `@tola/host` is internal.

## Start with the site

For an existing site, read `tola.toml`, the entry, schema, selection, templates, and hooks before
changing them. Tola uses the nearest `tola.toml` in the current directory or its parents;
`-C path/to/tola.toml` selects another. Its directory is the site root, and configuration paths
are relative to it. `tola config` prints selected resolved paths and settings.

### Create the first page

```sh
tola init my-site --dry-run
tola init my-site
cd my-site
```

Initialization accepts a missing or empty directory. `--force` allows a nonempty directory but
never overwrites scaffold files. Choose a preset, features, or interactive selection with
`tola init --help`. The scaffold's `content/index.typ` is the site's first page:

```typst
#import "@tola/source:0.0.0": tola-meta

// This page's metadata; every allowed field is declared in `site/schema.typ`.
#tola-meta((
  title: "Welcome",
  description: "A starter page for a site built with Tola",
))

#title()

= Start here <start-here>

Write content with *Typst*. Use HTML and CSS for the browser.

- Add another source under `content/`; its path names its route.
- Share layouts and helpers through `site/`.

#link(<start-here>)[Return to this section.]
```

Run `tola dev` and open its printed URL, which includes the deployment mount. Save to rebuild;
the browser follows successful builds. `tola check` checks production output without publishing
files. `tola build` replaces `build.publish-dir` (`public` by default).

In the scaffold, `site.typ` selects and emits pages, `site/schema.typ` declares metadata,
`site/selection.typ` chooses outputs, and `site/page.typ` renders one `(source:, output:)` record
through `page-template(page)`. `site/seo.typ` holds head and selected feed/sitemap recipes.
`site/not-found.typ` emits `404.html`; selected search features also create `site/search.typ`.

Keep shared modules outside `build.content-dir`. `site/` and `static/` are conventions, not special
names. An empty content directory is valid when the root generates pages from data. A site with
no HTML document reports `site.no_pages`; dev and preview show a welcome page. The scaffold's
`404.html` counts as a document.

### Read the bundled reference

```sh
tola help "@tola/source"
tola help "[build.hooks]"
tola help "@tola/schema"
```

`-i`/`--interactive` uses the built-in reader; `TOLA_PAGER` applies only to ordinary help output.

To configure language services for a site, use `tola editor setup --list`, then the chosen editor's
setup command. After upgrading Tola, `tola editor packages` refreshes package files.

## From sources to pages

### Write and declare

Typst starts in markup: `#` introduces code, `{ ... }` is code, and `[ ... ]` is content.
Inside code, expressions need no `#`; inside content they do. `[A *rich* title]` is content;
`"A *rich* title"` is a string. Strings containing HTML remain text.

`import` reads definitions; `include` inserts content. Neither inherits the caller's locals:
pass inputs to functions. Render arrays with loops or `.join()`. Multiline chains at the top
level need `{ ... }` or parentheses; a new `.map(...)` line is markup, not a continuation.
Named arguments use `name: value`, `..values` spreads a collection, and trailing content belongs
after named arguments: `html.ul(class: "topics")[...]`.

Read configured site values through `#import "@tola/site:0.0.0": site`. Define custom template
values in Typst modules and pass them explicitly to the functions that need them.

Call `tola-meta` once in the source's ordinary evaluation. A helper can calculate the dictionary,
but a call written in another file belongs to that file. Keep the call outside deferred `context`
and show rules. Reusing its invisible marker adds no declaration. A declaration already executed
is retained after a later error, but that error still fails the build. Draft sources are evaluated too.

Metadata fields belong to the site. The starter's `title`, `draft`, `permalink`, dates, tags, and
SEO flags are schema and template conventions. Extend the actual `site/schema.typ` before adding
fields: its default is to reject unknown keys.

### Parse in the root and reuse the selection

`all-sources()` lists discovered `.typ` inputs, not published pages. Discovery accepts lowercase
`.typ`, skips hidden names, symlinks, and the entry, and orders complete content paths. Its `file`
is a Typst input path for `include`; `id`/`path` identify the source below `build.content-dir`.
A source can appear in several documents or none.

The root gets settled declarations. Parse the complete set there, then filter, sort, and choose
outputs. `parse-sources` points field failures at literal metadata values when known, otherwise at the
source's declaration, or file start when absent. Notes and nested causes remain diagnostic hints;
it treats absent metadata as an empty dictionary. Returned records keep their identity and order
with parsed `meta`. Neither `all-sources()` nor `tola inspect sources` is changed by that parse.
Pass one declaration, or `source => declaration` to choose by each complete source record.
The same call collects field errors across all selected declarations before reporting them.

The starter already returns `(source:, output:)` records through `select-pages`. Pass those to
page templates, menus, and SEO rather than calculating destinations again. The starter's
`site/schema.typ`, `site/selection.typ`, and `site/navigation.typ` carry that complete connection.

A Typst type such as `str` is already a schema. `optional(S)` accepts an absent key; without a
default the key stays absent. `nullable(S)` accepts explicit `none`. An invalid supplied value
never falls back to a default. `non-empty(trim(str))` trims before checking; reversing the wrappers
can accept whitespace and return an empty string. `unknown: "keep"` preserves undeclared fields
without validating them. `try-parse` returns a value or structured issues; malformed declarations
and callback errors still stop compilation. Exact contracts: `tola help "@tola/schema"`.

Use `date` for calendar fields: a native date or strict `YYYY-MM-DD` string becomes a native
date-only `datetime`. Use `timestamp` for instants: a complete native calendar datetime or strict
RFC 3339 string becomes `(rfc3339:, unix-seconds:, subsecond:)`. Native values use UTC, with
date-only native values at midnight; date-only strings belong to `date`. Both require years
1 through 9999 and reject time-only values; timestamps reject leap seconds. Constraints and
examples: `tola help "@tola/schema" date timestamp`.

The starter parses `published` as a timestamp and `updated` with `union(date, timestamp)`.
This keeps a date-only change date at calendar precision when it reaches the sitemap.

Read data with `json("/static/data/chapters.json")`, `csv(..., row-type: dictionary)`, or `read`.
A string argument names a file; parsing read JSON bytes uses `json(read(input, encoding: none))`.
CSV cells are strings. Reading does not publish the input. `datetime(year: 2026, month: 9, day: 1)`
constructs a date; parse date strings with the schema declarations above.

One content source can feed several documents: its source identity stays the same while contextual
document identity changes. The same JSON data can serve two layouts and a download; a `site/` helper
returns content while `site.typ` owns outputs.

## Documents, addresses, and templates

### Keep the boundaries clear

An input `path` names a file Typst reads. An `output` names a published file, such as
`guide/index.html`. Its decoded site-root route is `/guide/`; a browser URL adds percent-encoding,
the mount, and optionally an origin. `join-route` joins chosen segments, `output-of` chooses the
output filename, and `output-url` creates the browser address. `route-url` accepts a decoded route
and equals `output-url(output-of(route))`; both canonicalize `/guide/index.html` to `/guide/`.

The starter slugs file-layout segments: `index.typ` becomes `/`, `about.typ` and `about/index.typ`
both become `/about/`, and `guide/install.typ` becomes `/guide/install/`. Two sources claiming one
output conflict. A trailing `/` names a directory; `/guide/` maps to `guide/index.html`, while
`/guide` names the file `guide`. A permalink is decoded once and changes the address, not the format.
A PDF still needs a PDF document declaration. The starter uses `@tola/schema`'s `url-path` for
permalinks: its parsed value is already a decoded route, ready for `output-of` or `route-url`.

For deployment at `https://example.com/docs/`, configure `origin = "https://example.com"` and
`base-path = "/docs/"` under `[site]`. Keep outputs unprefixed. `output-url`, `route-url`, `asset-url`, and
native label links apply the mount once. Pass `origin: site.origin` when an absolute URL is needed.
Hand-written site-root URLs must include the mount. `asset-url` takes an unmounted declared URL
and preserves its filename, including `index.html`; escape literal `%` and `#` as `%25` and `%23`.
Slug each segment separately; ASCII Han pronunciation follows the primary language (`ja` selects
Japanese). Literal `%` and `#` may belong to decoded filenames; encoding happens at the URL boundary.

`current-source()` identifies the file containing the call and needs no `context`.
`current-document()` identifies the containing output and needs `context` in a document body;
its `location` scopes native queries. Sources rendered into one document share that document identity.

### Compose HTML and native content

Root outputs are siblings. The starter's `page-template(page)` includes `page.source.file` in a
custom HTML shell. Native `document` metadata does not fill that custom head: write title,
viewport, description, and helper entries there. Give the shell an `html.head`, which also receives
the exporter's equation stylesheet. A head takes one content body; arrays of entries can be looped
or joined. A custom shell's `site/page.typ` shows this complete wrapper.

`#title()` exports as `<h1>`; native section headings start at `<h2>`. Use native headings for
outline, numbering, and queries. A label gets an HTML anchor when a link, reference, or outline
targets it. Use stable explicit labels for shared section URLs and native location links for a TOC.
`link` navigates; `@label` also requires a numbered target. `html.h2` does not create a native heading.

Typed HTML constructors take documented attributes and one body; void elements take none.
Use `html.elem` for custom tags or attributes:

```typst
#html.elem("aside", attrs: (class: "note", "data-kind": "tip"))[A reusable note.]
```

Its attributes are strings. `html.script` and `html.style` also take strings; an external script
is `html.script("", src: "...", defer: true)`. Use CSS for browser layout. Typst page settings,
columns, and spacing do not become CSS. `html.frame` deliberately makes SVG artwork and cannot
contain `set page`. Upstream's default footnote placement is unavailable in a custom `html.html`
shell; retain the default shell when needed. Citations and bibliography work in a custom shell.

For highlighted code, combine the source's `raw.theme`, the renderer, and the head stylesheet:

```typst
#import "@tola/code:0.0.0": render-code, code-stylesheet, code-themes
#set raw(theme: code-themes.github)
#show raw: render-code.with(dark-theme: code-themes.github-dark)
```

Load `code-stylesheet()` in `html.head`. The root's `data-theme="dark"` chooses the dark appearance;
the site supplies its theme control. The scaffold also follows the system when no explicit theme
is set. Style the code box with CSS. Inline backticks have no language; use `raw(..., lang: "rust")`
for inline highlighting. Read `tola help "@tola/code"` for exact helper contracts.

## Navigation and collections

Select visible pages and destinations once, then choose an editorial order with an identity
tie-breaker. Pass the same array to templates, sidebars, neighbors, feeds, and sitemaps.

- A page-local TOC uses contextual `headings`, native locations, and a separate `outlined` filter;
  a `site/` helper holds the reusable function and `site.typ` places it in each page.
- Backlinks select body references and render the derived list outside that body region: query the
  occurrences and choose one row per linking document. A heading label selects the heading, not the
  following chapter text.
- Arrays and field accessors supply collection identity and hierarchy. `key`/`keys` receive members;
  `parent` receives keys, including keys absent from the array. Hierarchy keys are unique and non-`none`,
  and the parent rule must eventually reach `none`. Contracts: `tola help "@tola/collection"`.

### Breadcrumbs and a sidebar

For the starter, add `order: optional(int, default: 0)` to the schema and write an overview at
`content/docs/index.typ`. Sort `select-pages(all-sources())` by `(page.source.meta.order, page.source.id)`
in a code block. This `site/navigation.typ` follows HTML directory outputs and skips absent overview
records while traversing their keys:

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

Change the starter signature to `page-template(page, pages)`, import `docs-navigation`, and insert
`#docs-navigation(pages, page.output)` in the body. Pass both arguments from the root's rendering loop.
Keep the missing-page and selected SEO output declarations. If permalinks should not determine
editorial hierarchy, supply an explicit parent rule instead.

`adjacent(pages, page.output, key: member => member.output)` finds previous/next pages in that order.
A missing anchor returns `none`; boundary neighbors have no `before`/`after` key. Pagination needs
neighbors of the generated listing pages, not the original posts. Group tags with `group-by-keys`,
compute listing outputs once, and handle tags that slug to the same name explicitly. Sort parsed
timestamps with `key: stamp => (stamp.unix-seconds, stamp.subsecond)`: the fractional string keeps
all digits except trailing zeros, so this preserves precision beyond nanoseconds. Use `.rfc3339`
for display and SEO, or native calendar dates for year archives.

## Add what the site needs

### Styles, assets, and browser fonts

`[assets]` maps source files or trees to public site-root URLs. The scaffold maps
`static/web-assets/` to `/assets/`. Load a declared stylesheet in the head:

```typst
#import "@tola/address:0.0.0": asset-url
#html.link(rel: "stylesheet", href: asset-url("/assets/css/site.css"))
```

`asset-url` takes the declared URL, not a source filename. It applies the mount and optional
byte-dependent query without changing the filename; an undeclared URL fails at the call site.
An exact file mapping takes precedence over a tree member's URL. CSS `url(...)` resolves against
the published stylesheet. Tola does not bundle JavaScript or rewrite CSS resource references.
Read `tola help "[assets]"` for complete declarations.

Compiler fonts and browser fonts have separate inputs: `[typst.fonts]` supplies Typst, and
`tola fonts` lists what a build discovered; browsers need published files loaded through CSS
`@font-face`. Setting a Typst font does not install it in the browser. `site.language` supplies the
default language; `site.languages` is template data for a site's own language routing and
switcher. Read `tola help "@tola/site"` for their shape.

If using Tailwind, scan the actual `content/`, `site/`, and entry files. Put its input outside the
published tree and run a finite stylesheet recipe before the build. Choose utilities or ordinary
CSS where each fits; place component rules in a layer when utilities should override them.
The scaffold already wires its dark variant to explicit `data-theme` and the system fallback.

### Images and icons

Use native `image` for compiler-rendered images and figures, `html.img` with a declared `asset-url`
for browser-served originals, or `resize-image` for a generated derivative. Its input is a filesystem
path; its returned URL is already mounted. `image-metadata` inspects without publishing. A mapped
original remains published after resizing; keep inputs outside asset trees when only derivatives
are wanted. Local SVG collections and the configuration, input paths, and final URLs that connect
them are declared in `tola.toml`.

An unlabeled icon is decorative. Informative icons need a label, and surrounding controls still
need their own names. Inline artwork using `currentColor` follows the page's text color; fixed fills
stay fixed. An `icon-url` image does not inherit its surrounding page's color. Complete options:
`tola help "@tola/image"` and `tola help "@tola/icon"`.

### Mathematics

HTML equations use MathML by default. Keep that mathematical structure unless unsupported notation
needs deliberate SVG rendering in the page template:

```typst
#import "@tola/web:0.0.0": math-svg
#show math.equation: math-svg
```

Numbering and references survive. SVG keeps compiled colors; repaint black fill and stroke
selectively so intentional colors remain:

```css
.tola-math-inline [fill="#000000"], .tola-math-block [fill="#000000"] { fill: currentColor; }
.tola-math-inline [stroke="#000000"], .tola-math-block [stroke="#000000"] { stroke: currentColor; }
.tola-math-block { overflow-x: auto; overflow-y: hidden; text-align: center; }
.tola-math-block > svg { margin-inline: auto; }
```

SVG loses MathML's mathematical structure; explain essential formulas in prose or give an explicit
`alt`. `figure(alt:)` and `math.equation(alt:)` do not supply HTML alternatives in the current exporter.
Inspect notation, baseline, colors, and narrow-width overflow. Contract: `tola help "@tola/web" math-svg`.

### Feed and SEO choices

Reuse selected page outputs. The starter implements `feed`/`sitemap` flags and defaults missing or
`none` `feed-content` to the whole exported body. Setting a summary alone therefore does not make
its feed summary-only. Direct `feed` declarations use API fields `summary` and `content`; omit
`content` for a summary-only entry.

Direct Typst content supports portable text, breaks, emphasis, strong, strike, and URL links; it is
not rendered as a document. Styles, deferred values, and unsupported elements are rejected.
`(document: output)` takes the whole body; `(document: output, id: "article-id")` takes one exported
HTML subtree. That ID must exist exactly once and is separate from the entry's subscriber identity.
Use it to exclude menus and sidebars while keeping the article's formatting and images.

Set `site.origin` for absolute SEO URLs and a nonempty feed title. Fixed entry IDs survive a route
move. Feed dates use the timestamp inputs above; sitemap dates also accept date-only strings.
Pass `.rfc3339` from parsed timestamp records. Shared parsing keeps the full fractional precision,
but feed and sitemap timestamp formatting keeps at most nanoseconds, and RSS keeps whole seconds.
Sitemap targets are whole documents, not fragments.

Canonical and social helpers return head-entry arrays. Build absolute page/image URLs with
`output-url(..., origin: site.origin)`, convert rich titles with `plain-text`, and preserve `none`
for omitted descriptions. Declaring social image metadata does not publish the image.
Formats and fields: `tola help "@tola/web" feed sitemap canonical open-graph twitter-card`.

### External runners

Use `before-build` to produce inputs such as CSS, and publish those inputs through assets or Bundle
outputs. Use `generate-outputs` for a tool that reads compiled HTML, such as Pagefind; its declared
outputs join the final site directly and need no asset mapping. Link their files with `output-url`.
`after-publish` consumes committed output and cannot undo publication.

Use the site's locked runner and finite recipes. Tola owns watching; a hook must finish.
`rerun-on` adds literal inputs the site otherwise does not read, including tool manifests and
lockfiles, not globs. `--pure` and `--offline` restrict Tola's reads, not a script's side effects.
Exact declarations and stage directories: `tola help "[build.hooks]"`.

## Check and publish

Choose checks for the changed surface:

- `tola inspect sources` shows declarations; `inspect documents`/`routes` shows emitted outputs.
  Inspection runs no hooks and does not publish the site, so it cannot generate missing hook inputs.
- `tola check` checks the complete production output and references, running the first two hook stages.
  `tola check --no-build` instead only parses the site's own sources: it builds nothing, so it proves
  syntax alone.
- `tola fmt` formats the site's Typst sources in place; `tola fmt --check` reports the sources that
  are not formatted and exits non-zero without writing them. Both read `[formatter]` from
  `tola.toml`, which your editor's format-on-save uses too (`tola help "[formatter]"`).
- `tola dev` watches; `tola preview` builds with production settings and serves without watching.
  Both serve in memory, include the configured mount, and leave disk publication to `build`.
- For rendered changes, exercise the actual route: heading structure, token colors, controls,
  image alternatives, mathematical notation, and narrow layouts as relevant.
- For feeds and sitemaps, inspect serialized entries, exclusions, IDs, dates, content, and URLs.

`build` replaces all of `build.publish-dir`, removing stale files. Failure before publication keeps
the previous tree; an `after-publish` failure does not roll it back. Keep hand-maintained files outside
that directory. `--publish-dir` overrides it for one run; `inspect sources --output` and `skill --output`
instead write those commands' results.

### Freeze dependencies and deploy

Configure `[vendor] path`, then use `tola vendor --dry-run`, `tola vendor`, and `tola build --pure`.
Vendoring runs no hooks and collects selected packages, remote icon collections, and compiler fonts.
`--offline` refuses network reads while allowing host packages, caches, and fonts. `--pure` also
excludes those host dependencies and files physically outside the site. Embedded dependencies remain
available. Options: `tola vendor --help` and `tola help "[vendor]"`.

Set the real `site.origin` and `site.base-path`, preview under them, build, and upload the complete
publish directory. Configure the host's directory routes to serve `index.html` and its missing-page
response to use `404.html`. Publish provider files such as `CNAME` explicitly when needed.

### Diagnose a failure

Start with the named file and next step. Use `tola config` for resolved paths and `tola doctor --json`
for configuration and tools. Correct undeclared assets, local pages, and fragments to target outputs
the site actually publishes. Absolute URLs stay external, even when they spell `site.origin`.
`write.stale` means an input changed during the build; retry after edits settle. Deep native headings
need a shallower hierarchy when they exceed HTML's heading levels.

For an invalid package call, use the installed executable's package help or editor hover.
`-v`/`-vv` and `--log-file PATH` provide detailed diagnostics when needed. Report changed files,
achieved behavior, the executable used, and checks actually run; name the route and viewport for
browser work and state any unperformed build, browser, or publication check.
