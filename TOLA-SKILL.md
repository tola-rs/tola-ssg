# Tola authoring guide

Read by task. Content authors write Typst sources; site authors choose documents, routes,
and templates; module authors package reusable APIs, themes, and tools. One root Typst
program produces the site's documents. A discovered source can appear in several documents
or none.

| Task | Read |
| --- | --- |
| Create a site or find its configuration | `tola skill start` |
| Write prose, headings, equations, links, or code | `tola skill writing` |
| Select pages and implement layout, navigation, assets, or SEO | `tola skill site` |
| Use a theme | `tola skill modules use` |
| Create a library, theme, query, or author tool | `tola skill modules create` |
| Write Nushell scripts | `tola skill scripts` |
| Run hooks, check, develop, or publish | `tola skill build` |

A topic prints its own guidance and subsection commands. Select a deeper task when needed:

```sh
tola skill writing math numbering
tola skill site assets files
tola skill modules theme --all
tola skill --list
```

`--list` lists the selected heading tree; `--all` includes all its descendants.
Without a selection, `--all` prints the complete guide. `--output FILE` writes the selected
text without overwriting a file, for example
`tola skill modules theme --all -o theme-guide.md`. Without the CLI, read the matching
heading below.

Check `tola --version`. Use `tola <command> --help` for flags and `tola help` for installed
configuration and package contracts. Typst package imports use `0.0.0`, not the executable
version. Import members or rename a package so `@tola/document` does not shadow native
`document(...)`.

For an existing site, first read `tola.toml`, its `build.entry`, and the relevant public
template API. For a module, start with `tola-module.toml` and its declared entries.
Keep input files, document outputs, and browser URLs distinct. Reuse selected outputs
for navigation and feeds.

Validate site changes with `tola check` and inspect affected routes for browser changes.
For modules, check the manifest and build a consumer of the released files. Report the
checks actually run; compilation alone does not establish browser appearance.

## Start

Read `tola.toml`, the selected site entry, and the files it imports before editing. Run
`tola inspect config` to see resolved settings. A theme can supply the entry instead of a project file.

### Create

```sh
tola init my-site --dry-run
tola init my-site
cd my-site
tola dev
```

`init` takes a missing or empty directory. `--force` permits other files but does not overwrite
scaffold files. It writes `tola.toml`, `site.typ`, `.gitignore`, `.ignore`, and an empty
`content/`; `site.typ` publishes every source at the route its file layout gives it and serves
`404.html`. Open the URL `dev` prints; add a source, save it, and check that it rebuilds.

### Locate

Tola finds the nearest `tola.toml` in the current directory or its parents. A command's
`-C path/to/tola.toml` selects another site. Paths in the configuration are relative to it.

Read `[build] entry` to find the root program: a path selects a project file, while
`{ module = "notes" }` selects that instance's `[build] entry` in `tola-module.toml`.
The configured content directory holds inputs; shared helpers belong outside it.
`content/` and `site.typ` are defaults, not reserved names.

### Editors

Use `tola editor setup --list`, then the setup command for your editor. After upgrading Tola,
`tola editor packages` refreshes its editor package files. Read the site's `[formatter]`
settings before formatting; the CLI and language service share them.

## Writing

Use the site's existing metadata wrapper and theme API when writing content. The selected
entry decides whether a source becomes a page. A theme user usually needs
`@tola-modules/ALIAS:0.0.0` and the theme's documented options; configuring the site program
belongs to `tola skill site`.

Typst syntax and document structure are below. Metadata declarations and their schema
are explained together in `tola skill site metadata`. Browser-served images and fonts
belong to `tola skill site assets`.

### Syntax

Typst is not Markdown or LaTeX: `=` introduces headings, `$` introduces math, and `#`
enters code from markup. Use native content for prose and HTML/CSS for browser layout.

#### Modes

```typst
#let name = "Tola"
#let greeting = [Hello, *#name*!]
#greeting
```

`[...]` is content; `{...}` is code. Inside code, omit `#`; inside nested content, use it
again. `[A *rich* title]` carries markup; `"A *rich* title"` is a string. An HTML string is
text, not an element. Render arrays with a loop or `.join()`.

#### Functions

```typst
#let note(body, kind: "note") = html.aside(class: kind, body)
#note(kind: "tip")[Keep the body as content.]
```

Named arguments use `name: value`; `..values` spreads arguments. Trailing content follows
named arguments. Put a multiline expression or method chain inside parentheses or a code
block: a new top-level `.map(...)` line is markup, not a continuation.

#### Imports

`import` reads definitions; `include` inserts a file's content. Imported files do not inherit
the caller's locals: pass inputs to their functions. Import the members you use, or use
`as` to choose the module name.

Project `/...` paths start at the site root; inside a package they start at that package.
Pass a discovered source's `file` directly to `include`. For project data read by a theme,
see [Modules](#modules). Reading a file does not publish it.

#### Context

Use `context` for locations, counters, and document queries, inside the document body that
owns the result. `all-sources()` and a source's `tola-meta(...)` declaration run in ordinary
code. Deferring the declaration into `context` does not declare source metadata.

### Headings

Write native headings for document structure and queries. `#title()` is the document title;
`= Section` starts its sections. Style their exported HTML with CSS instead of changing the
logical hierarchy to get a font size.

#### Levels

```typst
#title[Guide]

= Install
== Requirements
=== Platforms
```

With the default heading rules, title exports to `h1`, `=` to `h2`, `==` to `h3`,
and `===` to `h4`. Typst level 1 is a section, not the page title. Keep nesting shallow
enough for HTML's six heading levels and avoid skipped levels.

For programmatic headings use `heading(depth: 2)[Requirements]`. `depth` is relative;
`offset + depth` gives the resolved level unless `level` is explicitly set.

#### Numbering

```typst
#set heading(numbering: "1.1")
= Installation <guide-install>
See @guide-install.
```

Numbering is separate from the outline. `link(<guide-install>)[Installation]` needs no
numbering, while `@guide-install` does. Labels and stable fragments belong to
`tola skill writing links fragments`.

#### Contents

```typst
#import "@tola/document:0.0.0": headings
#context {
  let sections = headings(depth: 2).filter(section => section.outlined)
  html.nav(aria-label: "On this page")[
    #for section in sections [#link(section.location, section.body) \
    ]
  ]
}
```

Place this in the page template's document body. `headings()` is document-scoped;
`query(heading)` without a scope can see other documents in the same site. `level` is the
resolved heading level; `nesting` is its depth in the outline tree. Filter `outlined`
explicitly. Use native locations rather than guessing fragment strings.

#### Inclusion

When nesting included content, adjust its headings locally:

```typst
#let subsection(body) = context {
  set heading(offset: heading.offset + 1)
  body
}
#subsection[= Included section]
```

Counters are shared across a Bundle. Put `#counter(heading).update(0)` at a document's
start if its numbering must restart. Check the exported heading order and TOC links, not
just their visual sizes. Exact query fields: `tola help "@tola/document" headings`.

### Math

Use native Typst equations. `$x^2$` is inline; spaces at both ends, as in `$ x^2 $`,
make a displayed equation. HTML uses MathML by default; keep that structure unless a
specific formula needs a different rendering.

#### Notation

```typst
Inline: $x_i^2 + alpha$.

$ (a + b) / c = frac(a + b, c) $
$ sum_(i=1)^n i = n(n+1) / 2 $
$ mat(1, 0; 0, 1) $
```

Use `_` and `^` for attachments and parentheses to group their bodies. Functions such as
`frac` and `mat` are Typst math, not LaTeX commands. Multi-letter identifiers name symbols,
functions, or variables; quote ordinary words, for example `$ x "where" x > 0 $`.

#### Numbering

```typst
#set math.equation(numbering: "(1)")
$ E = m c^2 $ <energy-equation>
See @energy-equation.
```

Numbering applies to displayed equations. Keep labels unique across the site. For numbering
that restarts per document, put `#counter(math.equation).update(0)` at that document's start.
Use a native reference rather than manually repeating its number.

#### Rendering

Test in the browser at narrow widths. A successful compile does not prove that long
formulas fit, that the intended math font loaded, or that light and dark themes are legible.
Explain essential formulas in prose as well.

##### Fonts

MathML uses browser fonts. Publish the font file and load it with CSS; a Typst compiler
font setting does not install it in the browser. SVG equations instead use the fonts
available to the compiler. See `tola skill site assets fonts`.

##### SVG

For a formula that deliberately needs compiler-rendered artwork:

```typst
#let math-svg(equation) = if equation.at("block", default: false) {
  block(html.frame(equation))
} else {
  box(html.frame(equation))
}
#show math.equation: math-svg
```

Use `box` for inline placement and `block` for display placement. SVG loses MathML's
selectable mathematical structure; verify equation references and accessibility after
applying a custom show rule.

### Code

For highlighting, apply `render-code` as a show rule and load `code-stylesheet()` in the
head. Calling the renderer directly skips Typst's preparation of a raw element.

#### Highlighting

```typst
#import "@tola/code:0.0.0": render-code, code-themes
#set raw(theme: code-themes.github)
#show raw: render-code.with(dark-theme: code-themes.github-dark)
#raw("let answer = 42;", lang: "rust", block: true)
```

Set `raw.theme` where the code is written. Inline backticks have no language;
`raw(..., lang: "rust")` enables inline highlighting. Browser token styles also require
`code-stylesheet` imported from `@tola/code:0.0.0` and called in the document head.

#### Themes

The dark appearance applies while the root element has `data-theme="dark"`. The site owns
the toggle and system preference behavior. Style the wrapper with CSS; inspect both theme
states. Exact options: `tola help "@tola/code" render-code code-stylesheet`.

### Links

Link a native target with `link(label-or-location)`. For a selected page, use
`output-url(page.output)`. Source paths are files to read, not browser destinations.

#### Routes

An output such as `guide/index.html` publishes at route `/guide/`. `/guide` instead names
a file without an extension. `output-of` turns a route into an output; `output-url` turns
an output into a browser URL. Use `route-url` when you already have a decoded route.

The file layout gives each source its route segments; `slugify` them when the address should be
lowercase and hyphenated. `about.typ` and `about/index.typ` claim the same output. A metadata
field your site defines, such as `permalink`, can replace that address; it changes the route,
not the document format. Choose outputs once in selection and pass them to every consumer.

#### Mount

For a site served at `https://example.com/docs/`, configure:

```toml
[site]
origin = "https://example.com"
base-path = "/docs/"
```

Keep outputs unprefixed. `output-url`, `route-url`, and `asset-url` apply the mount once;
use their answers directly. Handwritten site-root URLs must include the mount. Pass
`origin: site.origin` for an absolute URL. Encode at the URL boundary, not in output names.

#### Fragments

```typst
= Installation <guide-install>
#link(<guide-install>)[Jump to installation]
```

Linking a native heading creates an anchor: its label when present, otherwise a generated
`loc-N`. A label alone does not guarantee an exported ID when nothing links to it. Keep
labels unique across the Bundle; a native link cannot choose between duplicate targets.

For a permanent, externally linked fragment independent of native links, write an explicit
HTML ID, for example `#html.h2(id: "installation")[Installation]`. A raw HTML heading is not
a native heading and does not join `headings()` or the outline. See [Headings](#headings)
when both navigation and native heading semantics matter.

#### Check

Use `tola check --json` to read the routes, outputs and reference results from that
check. Fields for unfinished stages are null; a completed stage with no rows returns an empty array. Absolute URLs are external even when they spell `site.origin`.
Exact conversions: `tola help "@tola/address" output-of output-url route-url asset-url`.

## Site

The root program chooses sources, parses their metadata, assigns outputs, and renders
documents. Keep that selection in a shared function so layouts, navigation, queries,
and SEO agree. HTML and CSS control browser structure and appearance.

With a module-selected entry, configure its options and public customization functions;
see `tola skill modules use` and `tola skill modules theme customization`. With a project
entry, read the topics below before changing its source selection or templates.

### Metadata

Declare metadata with `tola-meta`; validate it in the root program with a schema your site
declares. Tola does not assign built-in meaning to fields such as `draft` or `permalink`; your
site's schema does.

#### Declaration

```typst
#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: "Hello", description: "A short introduction"))
```

Call once during ordinary source evaluation, directly or through an imported function.
A deferred `context` or selector show rule does not declare metadata. Imported files' own
top-level declarations belong to those files. Draft sources still evaluate and can fail.
`tola inspect sources` discovers file identities without evaluating metadata. Return
selected metadata through a model query with an explicit JSON projection.

#### Fields

Declare the metadata fields your site accepts. A schema rejects unknown fields unless declared
with `unknown: "keep"`. Add the field there, then make its consumer use the parsed value.

```typst
#import "@tola/schema:0.0.0": schema, optional, parse
#let fields = schema((title: str, order: optional(int, default: 0)))
#let meta = parse((title: "Guide"), fields)
#assert.eq(meta.order, 0)
```

`optional` accepts an absent key; `nullable` accepts explicit `none`. A supplied invalid
value does not fall back to a default. For source records use `parse-sources` rather than
parsing each dictionary by hand: it reports the source whose metadata failed.

#### Dates

Choose calendar precision deliberately: `date` is a complete day, `timestamp` is an instant,
and `partial-date` retains year, month, or day precision without inventing missing fields.
Choose `timestamp` for `published` and `union(date, timestamp)` for `updated` when an instant
and a calendar date must both parse.

```typst
#import "@tola/schema:0.0.0" as s
#let published = s.parse("2026-09-01T09:30:00+08:00", s.timestamp)
#let period = s.parse("2026-09", s.partial-date)
#assert.eq(period.precision, "month")
```

Parsed timestamps carry `rfc3339`, `unix-seconds`, and `subsecond`. Pass `.rfc3339` to
feed and sitemap helpers. Exact accepted inputs: `tola help "@tola/schema" date timestamp partial-date`.

### Pages

Select sources and destinations once in the root program. Pass those same records to page
templates, menus, and SEO outputs. A source can appear in several documents or none; its
file path alone does not publish it.

#### Selection

A site that selects pages parses every source with its schema, filters drafts, and returns
`(source:, output:)` records. Keep that choice in one function and pass the records to every
consumer.

`all-sources()` discovers lowercase `.typ` files beneath the configured content directory,
excluding hidden names, symlinks, and the site entry. Its order is by complete source path,
not editorial order. Validate and sort explicitly before using the records in navigation.

#### Rendering

Pass `page.source.file` directly to `include`. It is a native input path, not a URL.
`document(page.output, format: "html", ...)[...]` emits the page; choose a PDF output and
format explicitly for PDF. A permalink changes an address, not its format.

`current-source()` belongs to the content source where the call is written; `current-document()`
belongs to the containing output and needs `context`. A call to `current-source()` in a shared
helper does not identify its caller. Pass the source to helpers instead.

#### Data

A site may generate documents from `json` or `csv` without a content source per page.
Validate generated metadata with `parse` and the same schema used by other pages.
Keep data reads, output selection, and rendering separate; reading a file does not publish it.
Compare the paths from `tola inspect sources` with the pages in `tola check --json`
when a source exists but a page is missing. `tola check --no-hooks --json` uses existing
inputs without running generators; missing generated inputs remain errors.

### HTML

Use native Typst content for text, headings, equations, and links. Use typed `html.*`
constructors for browser structure and CSS for layout. Typst page settings are not CSS.

#### Elements

```typst
#html.aside(class: "note")[A reusable note.]
#html.elem("aside", attrs: ("data-kind": "tip"))[A note with a custom attribute.]
```

Typed constructors accept their documented attributes and one content body; void elements
have none. Use `html.elem` for a tag or attribute the typed constructor does not provide.
Use native `link` for Typst targets; `html.a` is useful for raw attributes such as `target`
and `rel`. Strings containing HTML stay text.

#### Shell

A custom `html.html` shell owns its head: document metadata does not fill it automatically.
Use the site's `head-entries` helper or supply the title, viewport, description, and styles.
`html.head` takes content; loop over entry arrays or `.join()` them.

Keep an actual `html.head` so the exporter can add equation styles. With a custom shell,
check footnotes explicitly: upstream's default placement is not available there; retain the
default shell or supply a footnote show rule. Test citations and bibliography in the same shell.

#### CSS

Load declared stylesheets through `asset-url`; see `tola skill site assets files`. Use CSS for
spacing, columns, responsive layout, and browser typography. `html.frame` makes SVG artwork,
not an HTML layout container, and cannot contain `set page`.

`html.script` and `html.style` take strings. An external script uses an empty body:
`html.script("", src: url, defer: true)`. Inspect the actual DOM and narrow viewport before
claiming the layout is correct.

### Navigation

Build menus from the selected page records, not a fresh interpretation of source paths.
Choose a stable editorial order and use each record's output for its link.

#### Ordering

Add an `order` field to the site's schema before sorting by it. Break ties by source identity,
for example `(page.source.meta.order, page.source.id)`. Pass the ordered records to both the
page template and navigation. A previous/next link is a neighbor in this order, not a filename
guess. For paginated lists, find neighbors among the generated listing pages.

#### Hierarchy

Use `@tola/collection` for lineage and descendants once the site's parent rule is explicit.
A permalink is an address, not necessarily a parent relationship. Keys must be unique and
non-`none`; the parent rule must reach `none` and may traverse keys absent from the array.
Exact examples: `tola help "@tola/collection" lineage descendants adjacent`.

#### Backlinks

```typst
#import "@tola/document:0.0.0": references
#context {
  let seen = ()
  for reference in references(to: auto, from-within: <article-body>) {
    let output = reference.document.output
    if output in seen { continue }
    seen.push(output)
    link(reference.document.location)[#reference.document.route]
  }
}
```

Label the containing article region `<article-body>` and put this list outside that region.
A heading label selects the heading, not the text following it. The query returns reference
occurrences; deduplicate by document output when the UI needs one row per linking page.
For a page TOC use `tola skill writing headings contents`; recursive relations can use
`tola help "@tola/datalog" database query` without replacing rendered heading queries.

### Assets

Distinguish a compiler input path from a browser URL. Reading a file does not publish its
original. Declare browser-served files under `[assets]`; use image helpers when a generated
derivative, rather than the original, should be published.

#### Files

Use `files` for one concrete source and `trees` for a directory. A tree can select members
with `include` and `exclude` patterns relative to the declaring site or module root.

```toml
[assets]
trees = [{ source = "static", url-prefix = "/assets", include = ["static/**/*.css"], exclude = ["static/drafts"] }]
files = [{ source = "static/robots.txt", url = "/robots.txt" }]
```

Only matches inside `source` are eligible. A matched `static/css/site.css` is published at
`/assets/css/site.css`, with the path within `source` appended to `url-prefix`. Omitting
`include` uses `["."]`, which covers the whole source tree. `include = []` selects nothing,
and `exclude` defaults to empty. An exact file declaration owns its source even when an
option disables it. Tree filters apply only to tree members. An instance replacement can
substitute the bytes of a selected asset.

The stylesheet URL is the URL declared by the site's asset mapping, not its input filename:

```typst
#import "@tola/address:0.0.0": asset-url
#html.link(rel: "stylesheet", href: asset-url("/assets/css/site.css"))
```

Place this in the head after declaring that URL. `asset-url` validates the declaration and
adds the deployment mount. CSS `url(...)` is relative to the published stylesheet; Tola does
not bundle JavaScript or rewrite CSS resource references. See `tola help "[assets]"`.

#### Fonts

Compiler fonts and browser fonts are separate inputs. `[typst.fonts]` supplies Typst;
`tola inspect fonts` lists its fonts. Browsers need published font files loaded through CSS
`@font-face`. A Typst font setting alone does not load a web font.
Keep compiler fonts in the site's configured paths for pure builds; host system fonts
cannot be used with `--pure`.

#### Images

Use native `image` for compiler-rendered content, `html.img(src: asset-url(...))` for a
published original, or `@tola/image` for resized derivatives and responsive sets.

Pass an input path or raw bytes to image helpers, not an asset URL. Resolve a path with
`path(...)` before passing it to another file's function. Returned URLs already include
the mount. Supply `sizes` for responsive images that are not full-width.

`resize-image` needs an explicit raster format to rasterize SVG; responsive helpers refuse
SVG. Keep vector artwork as vector when resizing is unnecessary. A declared original stays
published even when a helper also creates derivatives. Put private originals outside asset
trees. Inspect alternate text, dimensions, and narrow layouts. See `tola help "@tola/image"`.

#### Icons

Use collections declared under `[icons.collections]` and their actual `namespace:name` IDs.
An unlabeled icon is decorative; informative icons need a label and controls need accessible
names. Inline `currentColor` artwork follows its surrounding color; an `icon-url` image
does not. Exact helpers: `tola help "@tola/icon"`.

### SEO

Reuse selected page outputs for canonical URLs, feeds, and sitemaps. Configure the real
`site.origin` and deployment path before judging their addresses. Social image metadata
does not publish the image: declare that resource too.

#### Head

Compose head entries in the page template. Canonical and social helpers return arrays; render
them in the custom head. Build absolute URLs with `output-url(page.output, origin: site.origin)`.
Use `plain-text` for rich titles.
See `tola help "@tola/web" canonical open-graph twitter-card`.

#### Feeds

A feed needs an origin and a non-blank title. An entry without `content` publishes no full-text
body, so a summary alone makes an excerpt-only entry. Pass a document selection as `content` to
publish the whole page.

A `(document: output, id: "article")` selection keeps one exported subtree, whose HTML ID
must occur exactly once. Use it to exclude navigation. Keep entry IDs stable when routes
move, and pass parsed timestamps as `.rfc3339`. Inspect the serialized feed's actual content
and URLs. Exact inputs: `tola help "@tola/web" feed`.

#### Sitemap

Sitemaps name whole documents, not fragments. They accept calendar dates as well as timestamps
for updates, and every listed URL is absolute, so the site needs `site.origin`; skip the output
when it is absent. Check the serialized entries and exclusions after `tola check` or a build.
Exact inputs: `tola help "@tola/web" sitemap`.

## Modules

A module is a directory containing `tola-module.toml`. It can provide a Typst library,
a Nu library, a site entry, assets, queries, commands, or an initializer. Declare only the
capabilities it provides. A theme is a module that supplies a site entry and writing API.

The site chooses the instance alias in `[modules.ALIAS]`. Keep it out of module code:
several aliases can refer to the same source with different options. The manifest needs
no package name, version, authors, or description; describe the project in its README.

Use **Use** to configure a module, **Create** for a small library, and **Theme** for a
complete author-and-consumer example. Queries, commands, and initialization are optional
extensions. Finish with **Releases**.

### Use

```toml
[build]
entry = { module = "notes" }
[modules.notes]
path = "theme"
```

The module directory has `tola-module.toml`: `[typst].entry` exports its writing API and
`[build].entry` supplies the site program. Content imports `@tola-modules/notes:0.0.0`.
Set the site's `build.entry` to a file path to use a project program instead.

Published themes can be read from an HTTPS download or a local archive file:

```toml
[modules.notes]
tarball = { url = "https://github.com/example/notebook/releases/download/v1.0.0/theme.tar.zst" }
```

Use `tarball = { path = "downloads/theme.zip" }` for a local file. `url` and `path` are
mutually exclusive inside `tarball`; an optional 64-digit hexadecimal `sha256` checks the expected compressed
file before the first extraction. `path`, `git`, and `tarball` are mutually exclusive
module sources. Local file paths are relative to the site's `tola.toml`.

Tarballs support tar.gz, tar.zst, and ZIP (stored or deflate). Each contains one outer
directory with `tola-module.toml` directly inside it. Tola removes that directory layer;
its name is not a module identifier. There is no tarball `subdir` or recursive manifest
search. Download URLs must use HTTPS without embedded credentials or fragments. Private
Release authentication is not provided; download such a file separately and use
`tarball.path`, or use an authenticated Git source.

Git remains available for a repository directory that already contains the files a
consumer needs:

```toml
[modules.notes]
git = { url = "https://github.com/example/notebook", tag = "v1.0.0", subdir = "theme" }
```

`git.subdir` chooses a module within the repository; omitting it selects the repository
root. In either case Tola reads `tola-module.toml` directly from the selected directory.

`git.url` accepts an `https://` address, an `ssh://` address, or the scp form `git@host:path`;
`http://`, `git://`, and `file://` are rejected. A private HTTPS repository authenticates
through your configured Git credential helper; a private SSH repository uses the system `ssh`,
so your SSH agent, `~/.ssh/config`, and `known_hosts` apply. Tola stores no credentials in
`tola.lock`.

Replace `git.tag` with `git.branch` or a full-commit `git.rev` when appropriate;
omitting all three selects the default branch. Git locks record the resolved commit and
module directory digest. Tarball locks record the location, `tarball_sha256` of the file,
and `tree_sha256` of the expanded module. Commit `tola.lock` with the site.

A normal build reuses existing lock entries and verifies their contents. Missing entries
may be recorded when the invocation permits it. `--locked` forbids lock changes but can
fetch missing bytes at the locked commit or checksum. `--offline` forbids network access;
local archives and already available inputs can still be read and recorded. `--pure`
implies `--locked`, refuses host caches and network access, and uses project sources and
vendor copies. A project-local tarball can be expanded afresh in pure mode; a remote
archive needs a matching vendor copy.

Change a Git selector or archive location, then run `tola update notes` to record the new
selection. Explicit updates can refresh an archive's digest, but must still honor any
configured `tarball.sha256`. A checksum failure during an ordinary build never changes
the lock. Run `tola vendor --refresh` after updating to replace old frozen copies.

The cache keeps expanded module directories, not downloaded archive files. Git entries
are keyed by commit and subdirectory; tarball entries by compressed-file digest. Aliases
can share a cached directory. Cache loss may require a download; a lock is not a backup.
Use `tola vendor` when the site must remain buildable without the original download.
Fetching and extracting modules never executes their before-pack hooks.

A site and its module can share a repository. The author's site can use `path = "theme"`
while consumers use a published archive or a ready-to-use Git directory. Remote
initialization is not implemented; initializers still take a local module directory.

Read the selected module's public API and options before changing its files.

### Create

Create `theme/tola-module.toml` and `theme/lib.typ`. A Typst library needs just one
manifest entry:

```toml
[typst]
entry = "lib.typ"
```

`theme/lib.typ`:

```typst
#let note(body) = html.aside(class: "note", body)
```

Register the directory as `[modules.notes] path = "theme"` in a site's `tola.toml`.
Content can then use `#import "@tola-modules/notes:0.0.0": note` and
`#note[Remember this.]`. This library does not replace the site's entry.

| Declaration | Purpose |
| --- | --- |
| `[typst] entry` | Public Typst definitions imported through `@tola-modules/ALIAS:0.0.0` |
| `[build] entry` | Site program selected by the consumer's `build.entry = { module = "ALIAS" }` |
| `[nu] entry` | Public Nu exports imported with `use tola-modules ALIAS` |

These entries are independent. A module can be only a library or only a tool. A project
entry is an ordinary project file; it does not become an implicit module.
`tola module check ./theme` checks the declaration and file, without compiling the library.
Use it from a real site to validate its behavior.

### Theme

This example publishes non-draft sources, loads a module stylesheet, and exposes an article
function for customization. Under a site directory, create `theme/` and `content/`.
The four module files below form a complete theme. To extend the library in **Create**,
replace its manifest and `lib.typ` with these versions.

`theme/tola-module.toml`:

```toml
[typst]
entry = "lib.typ"

[build]
entry = "site.typ"

[options]
show-title = true
extra-styles = []

[assets]
files = [{ source = "assets/theme.css", url = "/theme.css", minify = false }]
```

`theme/lib.typ`:

```typst
#import "@tola/address:0.0.0": asset-url, join-route, output-of
#import "@tola/module:0.0.0": get-module
#import "@tola/schema:0.0.0": optional, schema
#import "@tola/site:0.0.0": site
#import "@tola/source:0.0.0": all-sources, parse-sources, tola-meta

#let note(..fields) = tola-meta(fields.named())
#let fields = schema((title: str, draft: optional(bool, default: false)))

#let pages() = {
  parse-sources(all-sources(), fields)
    .filter(source => not source.meta.draft)
    .map(source => (
      source: source,
      output: output-of(join-route(source.route-segments)),
    ))
}

#let article(source) = [
  #if get-module().options.show-title { title(source.meta.title) }
  #include source.file
]

#let render-site(article: article) = {
  let module = get-module()
  set text(
    lang: site.language.lang,
    script: site.language.script,
    region: site.language.region,
  )
  for page in pages() {
    document(page.output, format: "html", title: page.source.meta.title)[
      #html.html[
        #html.head[
          #html.title(page.source.meta.title)
          #html.meta(name: "viewport", content: "width=device-width, initial-scale=1")
          #html.link(rel: "stylesheet", href: module.assets.urls.at("theme.css"))
          #for url in module.options.extra-styles {
            html.link(rel: "stylesheet", href: asset-url(url))
          }
        ]
        #html.body[
          #html.main(class: "page", article(page.source))
        ]
      ]
    ]
  }
}
```

`pages()` validates before filtering, and retains the native `source.file` for inclusion.
Its records can also feed navigation and model queries. Complete-set metadata validation
belongs here, not in the `note` function that individual sources call.

`theme/site.typ` delegates to that same public function:

```typst
#import "lib.typ": render-site
#render-site()
```

`theme/assets/theme.css`:

```css
body { margin: 0; font-family: system-ui, sans-serif; line-height: 1.6; }
.page { max-width: 48rem; margin: auto; padding: 2rem 1rem; }
```

This custom shell supplies its own head. Add the site's SEO, code stylesheet, and footnote
handling when it needs them; see `tola skill site html shell` and `tola skill site seo`.

#### Consumer

Create `tola.toml` beside `theme/`:

```toml
[site]
base-path = "/docs/"

[build]
entry = { module = "notes" }

[modules.notes]
path = "theme"

[modules.notes.options]
show-title = false
```

`content/index.typ`:

```typst
#import "@tola-modules/notes:0.0.0": note
#note(title: "Welcome")

= First section

A page supplied by the theme.
```

Run `tola check`, then `tola build` or `tola dev` from this site. The output is
`index.html`, served at `/docs/`. The stylesheet URL is
`/docs/_tola-modules/notes/theme.css`, with any enabled cache-busting suffix.
Renaming the alias requires changing the site's entry selection and content imports,
not editing the theme's source.

#### Customization

Keep simple choices in options. For added site CSS, declare the file in the site's
`[assets]` and add its declared URL to `[modules.notes.options] extra-styles`.
The example theme loads those styles after its own stylesheet.

For structural changes, set the site's `[build] entry = "site.typ"` and create that
project entry:

```typst
#import "@tola-modules/notes:0.0.0": article, render-site

#let with-banner(source) = [
  #html.aside(class: "notice")[Work in progress.]
  #article(source)
]

#render-site(article: with-banner)
```

The project calls the same function as the module's default entry. Expose ordinary
functions and content parameters for the variations a theme supports; document which
are public. Additional modules compose through their exported Typst or Nu APIs.
A named `get-module` call reads instance settings; it does not import that instance's API.

### Options

Set defaults in the module manifest's `[options]` table. The site's
`[modules.notes.options]` overrides them. Tables merge recursively, and an explicit array
or scalar replaces its default. A module's own public API can validate the fields it uses.
Use the site's top-level `[options]` for site-owned data, read as `site.options` in Typst
and `(get-site).options` in Nu.

From module Typst source, import `get-module` from `@tola/module:0.0.0` and call
`get-module()` to read the instance's `name`, `options`, and `assets`. Project code
selects the instance with `get-module("notes")`. Use `assets.urls` directly because the
URLs already contain the site mount. Pass layout customizations through the theme's
public API.

Typst chooses the current instance from the package containing the call. In Nu, the
current instance belongs to the calling module command, hook, or public Nu entry:
importing another Nu library does not change it. Pass options explicitly to shared Nu
functions. Inside module code, use the current instance instead of hard-coding the example
alias.

Options are ordinary TOML data. Validate the fields your API consumes and treat paths or
URLs according to that API's documented meaning. Layout callbacks belong in Typst
function parameters, not in the options table.

### Paths

Package `/...` paths point to the package root. Preserve `source.file` when including project
content. For a known project data path, use `project-path` from `@tola/site` before reading
it. Creating a path does not run a generator. Tools and hooks depend on the module's actual
manifest: use `tola help "[modules]"` for current declarations rather than guessing an
external script's environment or assuming a query runs its hooks.

### Assets

Use the shared `files/source/url` and `trees/source/url-prefix` declarations; the full
selection rules are in `tola skill site assets files`. In a module manifest, both
`source` and tree `include/exclude` patterns are relative to the module root.

For the theme's `source = "assets/theme.css"` and `url = "/theme.css"`:

| Value | Meaning |
| --- | --- |
| `assets/theme.css` | File read inside the module |
| `theme.css` | Resource key in `get-module().assets.files` and `.urls` |
| `/docs/_tola-modules/notes/theme.css` | Consumer URL with base-path and alias, before cache busting |

Use the URL returned by `assets.urls` directly; it already includes the mount and any
cache-busting identity. `assets.files` provides native Typst input paths. The maps describe
selected static assets; they are not a catalogue of every file a hook might generate.
A normal Nu command or query does not prepare the build's final asset bytes.

For a feature with optional CSS, merge its default into the existing `[options]` table
and append its file declaration:

```toml
[options]
search = true

[[assets.files]]
source = "assets/search.css"
url = "/search.css"
enabled-by = "search"
```

Supply the file even when the default is false: it is part of the module's declared
capabilities. Use the same boolean for the public UI and its runtime hook. Site overrides
go in `[modules.notes.options]`. An exact file declaration owns its source before the
option is checked, so disabling it cannot republish it through an overlapping tree.

A site can replace an already selected resource:

```toml
[modules.notes.assets.replace]
"theme.css" = "styles/alternate.css"
```

The replacement path is site-relative and keeps the resource's module URL. It cannot add
a tree member excluded by its patterns. `[modules.notes.assets] enable = false` disables
the module's assets; a theme that unconditionally looks up its CSS needs a corresponding
template customization.

The site owns `[assets] cache-busting` and `[build.minify]`. A declaration's
`minify = false` preserves its bytes. Keep CSS resource references relative to the
published stylesheet; Tola does not rewrite those references or bundle JavaScript.

### Hooks

Declare both site and module hooks under top-level `[hooks]`. Choose the stage by the
files it needs and produces:

| Work | Stage |
| --- | --- |
| Compile fixed theme CSS or generate reusable module data before delivery | `hooks.before-pack` |
| Generate inputs that depend on the consuming site's content or options | `hooks.before-build` |
| Read the compiled site's files and add final outputs | `hooks.generate-outputs` |
| Use the committed publication, such as uploading it | `hooks.after-publish` |

A fixed-class stylesheet can be compiled once by the author and included in the release.
Configure its compiler as a before-pack `argv` or `nu` action: `tola module pack` runs
it automatically. A stylesheet whose class selection depends on consumer content needs
consumer-time generation and its required compiler. Decide this from the inputs, including
whether dynamic class names need to be discovered.

Module runtime hooks run at the project root. Their `nu` path is module-relative;
`rerun-on` strings select module inputs and `{ project = "..." }` selects project inputs.
Before-build `generates` names project paths; use `{name}` to avoid collisions between
instances. Generate-outputs `outputs` names final output paths, not module source files.

For example, a module that supplies `runtime/search.nu` and the `search` option can declare:

```toml
[[hooks.generate-outputs]]
name = "search"
nu = "runtime/search.nu"
enabled-by = "search"
dev = "reuse"
rerun-on = { include = ["runtime/data/**/*.json", { project = "content/**/*.typ" }], exclude = [{ project = "content/drafts" }] }
outputs = [{ tree = "_tola-modules/{name}/search" }]
```

With the `pages` query from `tola skill modules queries`, `runtime/search.nu` can write
a title-and-URL index:

```nu
use tola [get-hook get-module query]

def main [] {
  let module = get-module
  let destination = ((get-hook).output-dir
    | path join "_tola-modules" $module.name "search" "index.json")
  query $"($module.name).pages" | to json | save --raw --force $destination
}
```

Write below `(get-hook).output-dir` at the declared output paths, using the actual alias.
If the hook needs compiled HTML, read it through `(get-hook).input-dir`. To link an output
like this index, use `route-url` from `@tola/address` with
`"/_tola-modules/" + get-module().name + "/search/index.json"`; it applies the site's mount.
The output is declared by the hook and does not appear in the static `assets.urls` map.

For ordinary runtime stages, site settings and the calling instance are available.
Before-pack has no consuming site: use `get-hook` for `input-dir` and `temp-dir`;
`get-site` and `get-module` are unavailable there. A hook must finish; Tola owns watching.
See `tola skill build hooks` for rerun selection, development reuse, and reports.

### Nu libraries

List built-in and configured Nu libraries with `tola nu --help`. A module can expose a library
alongside its Typst entry and commands by adding this to `tola-module.toml`:

```toml
[nu]
entry = "tools/mod.nu"
```

The library uses standard Nu exports. For example, `tools/mod.nu` can contain:

```nu
export def title [text: string] { $text | str trim }
export def main [text: string] { title $text }
```

With the module selected as `notes`, another script can use `use tola-modules notes` and
call `notes title " A title "`. Run the public entry with
`tola nu @tola-modules/notes " A title "`; append `--help` for its argument help. Libraries
need no `main` unless directly executed. Use `path self` for resources belonging to the
library, and pass site options explicitly when sharing a library across aliases.

Unknown `@tola-modules/ALIAS` entries fail directly. `tola nu --lsp` supplies Nu
libraries to editors; `tola lsp` supplies Typst and declared entity semantics.
Built-in Nu libraries are described in `tola skill scripts libraries`.

### Queries

A named query selects a module-relative Typst `entry` and a metadata `label`.
The official compiler evaluates the entry in the consuming site's source and package
environment. The label must select exactly one metadata value across the compiled Bundle.
The entry can use private helpers independently of the site's page template.

```toml
[queries.pages]
entry = "queries/pages.typ"
label = "page-list"
```

`theme/queries/pages.typ` can reuse the example theme's page selection:

```typst
#import "../lib.typ": pages
#import "@tola/address:0.0.0": output-url

#metadata(pages().map(page => (
  title: page.source.meta.title,
  output: page.output,
  url: output-url(page.output),
))) <page-list>
```

Run `tola query notes.pages --json` from the consumer. Ordinary queries accept scalars,
arrays, and dictionaries that can be converted to JSON. Native Typst paths are encoded as
`{ root, path }`, preserving their project or package root. Project content and functions
to explicit data fields before returning them.

Queries run with the current source inputs. They discover sources and compile their selected
entry independently of the site template. Prepare generated inputs before querying them.
Several queries can select different labels from the same entry. The complete entry is
compiled before labels are selected, so those queries share its computations and failures.

#### References and definitions

Declare editor support under the query that supplies the IDs. The following example assumes
that `lib.typ` exports `page-link(id)`, `page-card(id: ...)`, and `page(..., id: ...)`:

```toml
[typst]
entry = "lib.typ"

[queries.index]
entry = "queries/index.typ"
label = "page-index"

[[queries.index.references]]
export = "page-link"
parameter = { positional = 0 }

[[queries.index.references]]
export = "page-card"
parameter = { named = "id" }

[[queries.index.definitions]]
export = "page"
parameter = { named = "id" }

[commands.list-pages]
query = "index"
```

Each array entry selects one public function parameter. Add entries when more functions or
parameters use the same query. `positional` is zero-based. `named` selects a Typst named
parameter, so a positional parameter called `id` still uses its positional index.
Dot-separated exports such as `content.page-link` traverse the public entry's members.
Query helpers can be imported from private module files; the declared functions must be
available from `[typst].entry`.

A query with at least one reference or definition must return an array of records with
these fields:

| Field | Requirement | Meaning |
| --- | --- | --- |
| `id` | Nonempty string, unique within this query | The value the public API accepts as a reference |
| `name` | String | The item's human-readable name |
| `description` | Optional string or `none` | Explanatory text for completion and hover |
| `source` | Optional native Typst path or `none` | The source file used for definition lookup |
| `url` | Optional string or `none` | The item's web address |

Extra fields are rejected. Use an ordinary query for other data shapes. Tola infers record
validation from the reference and definition declarations, and applies it to CLI and LSP
calls alike. A query with neither declaration uses ordinary data conversion; tools that
consume such a query validate their own required fields.

`theme/queries/index.typ` can project the existing page model into these records:

```typst
#import "../lib.typ": pages
#import "@tola/address:0.0.0": output-url

#metadata(pages().map(page => (
  id: page.source.id,
  name: page.source.meta.title,
  source: page.source.file,
  url: output-url(page.output),
))) <page-index>
```

The query generates this list from the site's model. Reuse the ID that the public reference
function already accepts. An ID belongs to its configured module alias and query name;
different queries may contain the same ID. This example uses the content source path.
The query's `label` selects metadata, while each record's `name` supplies display text.
Keep the native `source` path and the web `url` separate. JSON output represents a source
as `{ root, path }`.

For `page-link("intro")`, the server uses the declared query to complete IDs and display
names, descriptions, and URLs. Definition lookup opens the declared source and selects the
unique matching definition argument when present. References follow actual function values,
including import aliases, instead of matching function names as text. A declaration of a
definition identifies the ID's source position; the query still computes which items exist.

Entity operations select explicit string arguments. Rename requires one writable literal
definition in the declared source and a complete set of writable references. Proposed edits
are checked by evaluating the query again and verifying the affected entity identities.
Computed arguments, shared definitions, or changes to other query identities can prevent a
rename. Invalid exports, missing parameters, and conflicting declarations identify the
module and query declaration that needs correction.

The language server keeps one query session per module instance and entry. Labels from the
same entry share its compilation within an editor revision. Later revisions reuse the
session's dependency-based source analysis, while answers remain tied to current unsaved
text. Configuration, module declaration, or input-scope changes replace the sessions.

Migrate old manifests by moving each `[[semantics]]` entry into the corresponding query's
`references` or `definitions` array. Remove its `role` and `query` fields and the query's
`returns` field. Rename record fields `label` to `name` and `detail` to `description`.
Keep the metadata selector `label`. Old field names are rejected.

#### Calling

The Nu query command uses the invocation's input scope and selected configuration. For a
module that declares `queries.index`, a registered tool can use:

```nu
use tola [query get-module]
query $"((get-module).name).index" | to json
```

Replace `index` with a query the module actually declares. The Nu command returns the query
value directly; this example serializes it for its own caller. Use `--config FILE` to override
the inherited configuration. `use tola query` imports
the same command as bare `query`; importing a library never fills in its module alias.
Private queries run no build hooks and publish nothing; missing generated inputs remain
missing. Check against a nondefault config and renamed alias before shipping the tool.

`tola query --help` lists configured query names. `tola query notes.index` prints a
human-readable table; `--json` returns the query value as compact JSON, and
`--json --pretty` indents it. `tola query ./tools/model.typ --label model` reads one explicit
Typst entry without requiring a module declaration.

Use `tola query notes.index -i` to select a row. A declared query command's fixed
`projection` applies after selection. Use ordinary Nu `where` and `select` for filtering
or field selection. Nonterminal output and `--json --output FILE` return the ordinary
result without opening the browser; leaving the browser writes nothing.

#### Selection

For the module's `index` query above, this `tools/find.nu` keeps each
record through search and selection. Register it with
`nu = "tools/find.nu"` under `[commands.find]` and run `tola notes find`:

```nu
use tola [query cli ui get-module get-site]

def main [] {
  let entities = query $"((get-module).name).index"
  if ($entities | is-empty) { return }
  let choice = ($entities | ui choose --columns [id name] --search-columns [name] --fuzzy)
  if $choice.action != "submit" { return }
  let entity = $choice.selected
  if $entity.source == null or $entity.source.root != "project" {
    error make {msg: "This entity has no project source to read"}
  }
  let reply = cli capture [edit --config (get-site).config-path read -- $entity.source.path]
  if $reply.stderr != "" { print --stderr --no-newline $reply.stderr }
  if $reply.exit_code != 0 { exit $reply.exit_code }
  print --no-newline $reply.stdout
}
```

The final command returns source text and its current digest as JSON. It takes the project's
native source path from the entity record, without deriving it from the title or ID. Package
sources and entities without sources remain separate cases. The original ID, source, URL and
description survive the table's display-column selection. For a module action instead, pass
`$entity.id` as one argument to that module's declared command, retaining its configuration
and scope. Only `submit` may invoke the action; quitting has no side effect.

An editor query uses `id`, `name`, `description`, `source`, and `url`; `source` is `null` or a
record with `root` and `path`. An ordinary query defines its own fields and types.
The interactive `find` tool above requires a terminal; keep the plain query available for
pipelines and output files. Existing project files are read again at action time, so their
digest is an editing precondition only for that read, not for the earlier query.

`cli capture` takes a list of arguments and returns Nu's `stdout`, `stderr`, and `exit_code`.
It uses the current Tola executable and input scope; place `--config` where the selected CLI
command expects it. Pipeline input follows Nu external-command rules. Use this for finite
output; interactive editors and continuously running commands use ordinary external calls.

### Commands

Register a Nu tool in `tola-module.toml`:

```toml
[commands.greet]
nu = "tools/greet.nu"
```

Use the `greet.nu` example from `tola skill scripts` as `tools/greet.nu`.
With the module configured as `notes`, run `tola notes greet Ada --upper`. The script
path is module-relative and cwd is the project root. Optional `args` precede the user
arguments. Nu binds the arguments using the script's typed `main` signature. Write the
description above `main`. Command help reads the signature without running the script,
and a manifest `help` field can override the description.

Embedded Nu commands read the calling module instance with `get-module`, another instance
with `get-module notes`, and site settings with `get-site`. Importing a Nu library does not
change the calling instance. Use `argv` for external tools. They receive the project and
module paths through `TOLA_PROJECT_ROOT`, `TOLA_CONFIG_PATH`, `TOLA_CONTENT_ROOT` and
`TOLA_MODULE_*`. `TOLA_MODULE_OPTIONS` points to the effective options JSON for the call.

An action declares exactly one of `nu`, `argv`, `query`, or `skill`. A query action
exposes a declared query without a wrapper script; `projection` selects its output fields:

```toml
[commands.pages]
query = "pages"
projection = ["title", "url"]

[commands.guide]
skill = "GUIDE.md"
```

Create `GUIDE.md` with the module's authoring instructions before declaring the skill
action. Consumers read it with `tola notes guide` and its heading selectors. A command's
description belongs in its Nu signature or `help` field; the manifest needs no general
project-description fields.

#### Reports

A registered tool or hook can submit diagnostics separately from its ordinary output:

```nu
use tola diagnostic
[
  (diagnostic issue "Some entries have no summary" --code missing-summary
    --hints ["Add a summary to the listed entries."])
] | diagnostic submit
```

The default severity is `warning`. An `error` fails the operation. Submit the complete list
once, using `[]` for an empty batch. Embedded Nu submits directly to the host. External
tools use the file named by `TOLA_REPORT_PATH` and the version 1 JSON batch. Submit issues
without locations because these commands and hooks have no source snapshot.

For an independent check, pipe the list to `diagnostic render` to print it. A location
for rendering uses a file `path` and optional one-based `line` and `column`. The script
chooses its own exit status after rendering.

### Initializer

An optional initializer creates a consumer before a site exists. Add to the manifest:

```toml
[init]
nu = "tools/init.nu"
```

`theme/tools/init.nu` creates a site using this theme's `note` API:

```nu
use tola get-init

def main [] {
  let init = get-init
  let config = {
    build: {entry: {module: $init.module.name}}
    modules: {($init.module.name): {path: $init.module.root}}
  }
  $config | to toml | save --raw ($init.output-dir | path join "tola.toml")
  mkdir ($init.output-dir | path join "content")
  let source = [
    $'#import "@tola-modules/($init.module.name):0.0.0": note'
    '#note(title: "Welcome")'
    ''
    'Your first page.'
  ] | str join (char nl)
  $source | save --raw ($init.output-dir | path join "content" "index.typ")
}
```

Run `tola init my-site --from ./theme --alias notes --dry-run`, then the same command
without `--dry-run`. Tola runs the generator in a disposable output directory even for
dry-run, validates the candidate, and applies its proposed files to the target.
Write only to `(get-init).output-dir`. `target-dir` is the final destination for calculating
relative paths, not the directory to populate during generation.

The generated configuration must register the source module under the selected alias.
This example records its absolute source path for local development; it does not copy or
vendor the module. A relative registration must be correct from the final target.
`get-site` and `get-module` are unavailable here; use `(get-init).module` for source details.
An unattended initializer must not prompt; `(get-init).interactive` indicates explicit
interactive use.

### Releases

A common theme workflow keeps author source in Git and distributes prepared files as a
Release attachment. For example, the repository can hold a site beside `theme/`; pack
writes `dist/theme/`, which may stay untracked. The source tag identifies the author
revision, while the attached archive contains its prepared module. Separate publication
branches and committed generated directories are not required.

This is one example, not a required repository layout. A module directory that already
contains its runtime files can also be consumed directly through `path` or `git`.

Publish the files a consumer needs, including entries, imported helpers, runtime scripts,
query models, and declared assets. Author-only sources and build tools can stay outside
the release. Add `[pack]` to the example theme:

```toml
[pack]
include = ["lib.typ", "site.typ", "assets", "queries", "tools"]
exclude = ["assets/**/*.map"]
```

This selection assumes the optional Queries and Initializer examples are present.
For the four-file theme alone, use `include = ["lib.typ", "site.typ", "assets"]`.
Include additional runtime files when adding other capabilities.

Patterns are module-relative, as with asset trees and module hook inputs. Matching is
case-sensitive; `*` stays within one path component and `**` crosses directories.
A directory match includes its descendants, exclusions win, and a glob can match nothing.
Pack requires each non-excluded literal include to exist. The manifest is always included.

#### Preparation

For generated CSS, put the author's source at `theme/styles/theme.css` and declare:

```toml
[[hooks.before-pack]]
name = "compile-assets"
nu = "author/assets.nu"
```

This runnable `theme/author/assets.nu` copies the source CSS; replace its processing
step with the compiler or data transformation the module needs:

```nu
use tola get-hook

def main [] {
  let root = (get-hook).input-dir
  mkdir ($root | path join "assets")
  open --raw ($root | path join "styles" "theme.css")
    | save --raw --force ($root | path join "assets" "theme.css")
}
```

Use `argv = ["tool", "argument", ...]` instead when the whole action is an external
compiler call. Supply real arguments for the chosen tool; no implicit shell or wildcard
expansion occurs. Install author dependencies in the author environment. Consumers need
only dependencies of the runtime hooks they execute.

Run pack from beside `theme/`; the destination's parent must exist:

```sh
mkdir -p dist
tola module pack ./theme --output ./dist/theme
```

Pack validates the selection, runs before-pack hooks in the source directory, selects
files, checks the release, and replaces the destination directory as a complete tree.
Files omitted from the new release disappear; a failed preparation leaves the previous
release intact. The destination must be absent or an ordinary directory. It does not watch.
The delivered manifest removes `pack` and `hooks.before-pack`, so `author/` and `styles/`
can be omitted. The retained init/query/runtime declarations must still name included files.
Ordinary builds read module directories directly and never run before-pack hooks.
The published directory must include any generated files consumers need.

#### Verification

`tola module check ./theme` validates the manifest and explicitly declared files without
compiling programs or executing hooks. Run it after preparing generated files, or check
the already prepared release:

```sh
tola module check ./dist/theme
```

Configure a consumer to use the staged `dist/theme` directory and run its `tola check`
and `tola build`. Set `path` relative to that consumer's `tola.toml`.
Exercise its declared queries and commands there. Verify an overridden option, a renamed
alias, and a non-root `site.base-path` so accidental hard-coded assumptions are visible.
For initializer modules, run `tola init` against the release and build the resulting site.

A successful pack is evidence about the delivery files, not the theme's appearance.
Inspect the published CSS URLs and browser layout separately. A precompiled theme should
build in its consumer environment without the author's CSS compiler.

#### Publication

After checking the prepared directory, compress the directory itself so the archive has
one outer directory with the manifest directly inside it. For example:

```sh
tar -czf dist/theme-v1.0.0.tar.gz -C dist theme
```

A tar.zst or ZIP with the same layout also works. Pack does not invoke a compressor or
upload files. The author or CI attaches the archive to a release; the Git tag can point
to the source commit. GitHub's automatically generated source-code archives do not run
pack and are not substitutes for a prepared module when generated files are needed.

Give consumers the actual attachment download address:

```toml
[modules.notes]
tarball = { url = "https://github.com/example/notebook/releases/download/v1.0.0/theme-v1.0.0.tar.gz" }

[build]
entry = { module = "notes" }
```

`tola build` downloads, checks, extracts, and caches the module. It records the file and
module digests in `tola.lock`; subsequent reads must match. Consumers do not run the
author's pack step or CSS compiler. Use versioned download addresses instead of a
moving latest-release URL. Changing the URL and running `tola update notes` upgrades it.

Archives are limited to 128 MiB compressed, 512 MiB expanded, and 10,000 members. Links,
special files, escaping paths, and conflicting or nonportable filename spellings
are rejected. ZIP encryption and compression methods other than stored/deflate are not accepted. Zstandard windows are limited to 128 MiB; dictionaries
outside the archive are not used. All module paths remain relative to the expanded root.

If an author instead commits pack output to Git, consumers can select its directory
with `git.subdir`. Branch names and source/output directory names are author choices.

## Scripts

Use `tola nu` for author tools, structured data processing, and finite build scripts.
It runs standard Nushell without requiring a separate Nu installation. Save this as
`greet.nu` and run `tola nu greet.nu "Tola author" --upper`:

```nu
# Greet someone by name.
def main [name: string, --upper] {
  let greeting = $"Hello, ($name)!"
  if $upper { $greeting | str upcase } else { $greeting }
}
```

Return values for pipelines; use `print --stderr` for progress beside machine-readable
output. Keep data structured until a file or process boundary. Browser scripts still need
a JavaScript host. For module registration use `tola skill modules commands`; for hook
stages use `tola skill build hooks`.

### Runtime

Check the runtime inside the Tola executable you will deploy. Save as `runtime.nu` and
run `tola nu runtime.nu`:

```nu
{
  version: (version | get version)
  tui: ("tui debug" in (scope commands | get name))
} | to json
```

These recipes target embedded Nu 0.116.1. A separately installed `nu --version` describes
that executable, not Tola's runtime; Nu 0.115.1 does not provide the TUI recipe below.
Run `help COMMAND` or inspect `scope commands` inside a script when flags or capabilities
matter. Tola's runner accepts files, `--stdin`, and `--lsp`; it is not the standalone
Nu CLI, so do not pass standalone `-c` or `--ide-check` flags to it.

The standard library is available through imports such as `use std/assert`. Personal
Nu startup files and plugin registrations are not loaded; the parent process environment
is still inherited. Plugins such as Polars and `query web` are not implied by the language
version; inspect availability rather than
adding an installation step to a theme's default workflow.

Pass stdin explicitly, before the script filename. Save this as `upper.nu`:

```nu
def main [] { $in | from json | get name | str upcase }
```

`printf '%s' '{"name":"Tola"}' | tola nu --stdin upper.nu` returns `TOLA`. This process
boundary carries bytes, so decode JSON explicitly; it does not transport an in-memory Nu record.

### Libraries

Built-in libraries use `use tola NAME`. Image operations accept bytes or file paths:

```nu
use tola image
image metadata photo.png
image resize photo.png --output thumb.webp --width 640 --format webp
```

Use `tola nu @tola-scripts/image resize photo.png --output photo.webp --format webp` to keep the
source dimensions and change the encoding. With no explicit `--op`, one dimension
preserves the aspect ratio and two dimensions fill the requested rectangle, cropping
if needed. `image metadata` returns dimensions and format. Raster resize does not provide
SVG rendering or publish a site asset; use the site's existing output hooks for generated files.
With `--output`, the destination directory must exist, existing files are not overwritten,
and the command returns a small metadata record. Without `--output`, it returns encoded bytes.
For example, `open --raw photo.png | image resize --width 640
| save --raw thumb.webp`. Resizing still holds decoded pixels; choose a modest explicit
`par-each --threads` value when processing large images concurrently.
Unknown `@tola-scripts/NAME` entries fail directly. Use `./@file.nu` for an ordinary
filename beginning with `@`. These are executable script entries; Typst imports continue
to use `@tola/NAME:0.0.0`.

### Values

Nu pipes records, lists, tables, and streams, not just lines. Transform values before
serializing them for another process:

```nu
let pages = [
  {title: "Guide", draft: false, tags: [help]}
  {title: "Notes", draft: true, tags: []}
]
$pages | where draft == false | select title tags | to json
```

A table is a list of records. `get` extracts values; `select` keeps named columns;
`update`, `insert` and `upsert` return changed records. Inspect unfamiliar values with
`describe`. Access a genuinely optional field with `$row.description?`; missing required
fields should remain errors. `default (expression)` evaluates that argument eagerly. In
Nu 0.116.1, `default { expression }` computes a fallback only when needed; use that closure
or an explicit branch for fallible or expensive work.

At a process boundary use `to json` or `to nuon`, not the rendered terminal table. Decode
external JSON with `from json` before selecting fields. Test empty inputs and absent fields
as well as a nonempty table.

### Pipelines

Pipeline input and positional parameters are separate channels:

```nu
def add [amount: int]: int -> int { $in + $amount }
2 | add 3
```

This returns `5`. Likewise, `5 | do { $in * 2 }` uses pipeline input, while
`do {|n| $n * 2} 5` passes a parameter. Piping into a closure does not fill its first
positional parameter. Capture `$in` once with `let rows = $in` when it must be reused.

Use `each` to build transformed results, `where` to filter, and `reduce` to accumulate.
Use `for` for ordered side effects; it does not collect a list of return values. Closures
cannot capture an outer `mut` binding. `each` normally drops null results;
`--keep-empty` retains null, not an empty string. Inspect the result's shape before replacing
an explicit loop. Parallel mapping is only appropriate when effects and ordering allow it.

Only the last expression's value is returned. `echo` returns a value too; it is not a
printing side effect. Use `print` deliberately, and keep diagnostic messages off JSON stdout.

### Commands

Use typed `def main` parameters for a script's arguments and automatic `--help`. A switch
such as `--upper` is boolean; a value flag has a type, for example `--limit: int = 10`.
A rest parameter such as `...names: string` collects additional values.

Put reusable definitions in `lib.nu`:

```nu
export def greeting [name: string]: nothing -> string {
  $"Hello, ($name)!"
}
```

Then `hello.nu` can import them without copying the implementation:

```nu
use ./lib.nu greeting
def main [name: string] { greeting $name }
```

Run `tola nu hello.nu Ada` or `tola nu hello.nu --help`. Nu invokes `main` after top-level
code; do not call it again at the bottom. Script subcommands such as `def "main list"`
still need a `main` definition. Keep reusable modules free of unrelated top-level work;
even direct script `--help` can evaluate top-level code before displaying main help.

`use` imports exported definitions; `source` executes code in the current scope;
`run` executes a script as an isolated pipeline stage. Their file targets are resolved
during parsing, so use literal or constant paths rather than runtime `let` values.
Use `def --env` only when changes to the caller's environment are part of the command.

### Arguments

Pass one value per argument, including paths with spaces. A list needs explicit spreading.
This `forward.nu` invokes a chosen script through the same Tola executable:

```nu
def main [file: string, ...words: string] {
  let program = $env.TOLA_EXECUTABLE
  ^$program nu $file ...$words
}
```

`^` selects an external executable; `...$words` preserves individual arguments. Do not
join them into a command string or interpolate user text into `nu -c` or `sh -c`.
Use `--` before a positional value that starts with a dash when calling a script, such as
`tola nu greet.nu -- -Ada`. Test the actual boundary with spaces, quotes, Unicode,
newlines, leading dashes, and numeric-looking text such as `007`.

Nu interpolation is `$"Hello, ($name)"`, not Bash variable expansion. Single-quoted strings
are literal; quoted `'*.typ'` is text, while an unquoted glob may expand. Backticks are not
Bash command substitution or a universal literal escape. Keep strings as data; serialize
JSON instead of constructing it by string interpolation.

### Paths

Distinguish the process working directory from the script's directory. Run this file from
a different directory to see both:

```nu
{
  working: $env.PWD
  script: $env.CURRENT_FILE
  resources: ($env.FILE_PWD | path join "templates" "note.typ")
} | to json
```

Use `path join` for paths, not slash concatenation. A relative `open` uses the working
directory; use `FILE_PWD` for resources beside the script. Relative `use` paths follow Nu's
module resolution, not a runtime `cd`. Module commands keep the project working directory
while locating their script entry under the module; see `tola skill modules commands`.

Direct `tola nu` loads site configuration and Nu libraries without building the site or
running hooks. Select another configuration with `tola nu --config FILE ENTRY`. Disk scripts
keep the caller's working directory, and a public `@tola-modules/ALIAS` entry runs at the
site root. Import `use tola get-site` to read `(get-site).config-path`. Pass
options explicitly to reusable libraries because importing a library leaves the calling
module instance unchanged.

### Files

`open` decodes known formats; `open --raw` reads their original text or bytes. Use raw reads
for Markdown/Typst templates and exact-text transformations. For example, save this as
`select-pages.nu` and give it a JSON array of records with `title` and `output`:

```nu
def main [source: string, destination: string] {
  open $source | select title output | to json | save --raw $destination
}
```

Here `to json` chooses the format and `save --raw` writes it without extension-driven
conversion. Use `save --force` only for an intentional replacement; in Nu 0.116.1 it also
creates missing parent directories. Check the resulting bytes or decoded values, not just
the command's exit status.

Default `save` refuses a file it finds already present, but Nu 0.116.1 checks existence
separately from creating the file. It does **not** guarantee atomic exclusive creation.
A preceding `path exists` check does not fix that race. A successful save also does not
promise atomic replacement. For author operations requiring no-clobber under concurrent
writers, use an operation with that explicit guarantee; if unavailable, report the gap
instead of weakening the contract.

Use a format-aware parser for HTML or namespace-sensitive XML. Nu's `from xml` does not
retain namespace identity for attributes with the same local name; it is not a faithful
round-trip representation. HTML DOM editing is not a regex or line-splitting task. Check
whether an appropriate parser is actually available before migrating that part of a tool.

### Errors

Let unexpected Nu errors propagate. Use `try/catch` for a specific recovery or added context,
not to turn a failed operation into an empty success. Parsing happens before execution;
`try` does not make invalid syntax safe to run.

For an external process whose output and status matter, capture them explicitly. This
`capture.nu` accepts an executable and its arguments:

```nu
def main [program: string, ...arguments: string] {
  let child = (^$program ...$arguments | complete)
  if $child.exit_code != 0 {
    print --stderr $child.stderr
    exit $child.exit_code
  }
  print --no-newline $child.stdout
}
```

`complete` turns an external nonzero status into data, so inspect `exit_code` before using
stdout. Command-not-found can still throw. Do not infer every upstream status from the last
stage of a pipeline. For structured stdout, decode only after accepting the exit status.

Use `finally` to release temporary resources owned by the operation. Normal Nu 0.116.1
`exit` unwinds it; `exit --abort` and forced process termination do not. Keep cleanup from
silently replacing the original failure. Preview and selection should not modify files;
perform the write only after a successful decision.

### Testing

Parse first in the intended runtime. Save this checker as `check.nu`:

```nu
use std/assert
def main [file: string] { assert (nu-check --debug $file) }
```

Run `tola nu check.nu hello.nu`. Plain `nu-check` can return `false` without failing its
process; assert its result or use `--debug`. `source` is execution, not a syntax checker.
Then test behavior. For `lib.nu` from Commands, this `test.nu` needs no test framework:

```nu
use std/assert
use ./lib.nu greeting
assert equal (greeting "世界") "Hello, 世界!"
```

Run `tola nu test.nu`. Use isolated directories for file-changing examples; assert exact
content, preserved existing files, failure status, and no writes on cancel. Keep independent
expected values and existing behavior tests during migrations, rather than comparing only
two implementations built from the same assumptions.

A supported official `nu` can be a development runner. Final acceptance must use the actual
`tola nu` consumers will run. Keep the tested executable path explicit in the harness; do
not silently fall back to a different binary. Test the consumer workflow without extra Nu/Node on
PATH. Neither a parse check nor a headless UI test proves the full workflow.

### TUI

Use a TUI only for interactive author commands, not unattended hooks. Read/query the model
once, then pass the resulting rows into `tui table` and `tui run`. Movement and previews
should use those rows instead of recompiling the model on each event.

The 0.116.1 widgets can be exercised without a terminal:

```nu
use std/assert
let rows = [{name: first} {name: second}]
let chosen = ($rows | tui table | tui debug --keys [down enter])
assert equal $chosen.selected.name "second"
let cancelled = ($rows | tui table | tui debug --keys [esc])
assert equal $cancelled.action "quit"
assert equal $cancelled.selected null
```

For the real interaction, use `let choice = ($rows | tui table | tui run)` and check its
`action` before acting on `selected`. A quit is not a request to edit the highlighted row.
Only a submitted choice may start an editor or write. Headless tests establish selection
logic; separately exercise actual terminal cancellation and restoration before shipping.

## Build

Use `tola check` to build and validate without replacing the published tree. Use `tola dev`
for watching and `tola preview` for a production preview. `tola build` publishes to disk.

### Development events

`tola dev --json` keeps the development server running and writes one versioned JSON event per
stdout line. Human logs and hook output go to stderr. `server-ready` gives the actual URL;
`build-finished` distinguishes success, failure and cancellation, whether this build published,
and the revision still being served. A failed rebuild can leave the previous page available.
`hook-finished` reports asynchronous after-publish work separately; `hook: null` means its
shared inputs could not be prepared. `stopped` reports orderly shutdown or a fatal error;
consumers must also observe process exit because forced termination cannot send an event.

The VS Code extension consumes these events for preview readiness and build status. A Nu
script can use `$env.TOLA_EXECUTABLE` as an external command and pipe its stdout through
`lines | each {|line| $line | from json }`. Do not use `cli capture` for a continuing dev
process. These events observe publication; generators whose output belongs to that publication
remain hooks.

### Hooks

Hooks must finish; Tola owns watching. `before-build` writes inputs read by the site,
`generate-outputs` reads compiled content and contributes final files, and `after-publish`
consumes the committed publication. An after-publish failure cannot undo publication.
All three stages may submit unlocated diagnostic reports; see `tola skill modules commands reports`.
When a development `generate-outputs` result is reused, its report is reused with its outputs.

`check` and `preview` run the first two stages, not after-publish. Inspection and vendoring
run no hooks. Declare a stage under the top-level `[hooks]` table with a `nu` or `argv`
action.

```toml
[[hooks.before-build]]
name = "local-data"
nu = "tools/data.nu"
rerun-on = { include = ["data/**/*.json", "tool.lock"], exclude = ["data/scratch"] }
generates = ["generated/data.json"]
```

Omit `rerun-on` for no extra inputs. When present, `include` is required and `exclude`
defaults to empty. Strings are relative to the declaring site or module. Either list can
use `{ project = "content/**/*.typ" }` to select relative to the site root. A module
hook can use `{name}` in either list to substitute its instance alias. The Nu script is
observed independently of these lists. A literal input can be missing at first and added
later. An observed change triggers a full development build.

With `dev = "reuse"`, a `generate-outputs` hook can reuse its outputs and report when its
observed inputs, upstream site, effective options, action, and output declarations agree.
Production runs the hook. Embedded Nu reads stage directories through `get-hook`.
For stage contracts and external environment variables, see `tola help "[hooks]"`.

### Checks

- `tola inspect sources` lists discovered `path` and `contentPath` identities without evaluating metadata.
- `tola inspect icons`, `fonts`, and `config` read those resources or resolved settings.
- `tola query alias.name` reads a model's explicit JSON projection without running hooks or publishing.
- `tola check` builds and validates; `--json` returns that operation's pages, routes, outputs and references.
  Add `--no-hooks` to use existing inputs without running generators.
- `tola check --no-build` parses owned source syntax only; it cannot validate imports or outputs.
- `tola fmt --check` reports formatting without writing; `tola fmt` edits sources.

For failures, start at the named source and its suggested action. `tola inspect config` shows
resolved settings; `tola doctor --json` checks configuration and local tools. Use `-v` or
`--log-file` for more detail. A `write.stale` diagnostic means an input changed during the
build; retry once edits stop. `tola build --json` reports the actual publication.
An after-publish failure can have `success: false` and `published: true`; unfinished
report fields are null rather than empty successful results.

### Publish

`tola build` replaces all of `build.publish-dir`, removing stale files. Keep hand-maintained
files outside it. Preview with the real origin and mount, then upload the complete published
tree. Configure the host to serve directory `index.html` files and use `404.html` for missing
pages. Publish provider files explicitly if the host needs them.

### Vendor

Configure `[vendor] path`, inspect `tola vendor --dry-run`, then run `tola vendor` and
`tola build --pure`. Vendoring freezes selected packages and remote icon collections, not
compiler fonts. Keep fonts under configured site paths. `--offline` refuses network access;
`--pure` also excludes host packages, caches, and outside-site source files. Exact behavior:
`tola vendor --help` and `tola help "[vendor]"`.
