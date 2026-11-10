# Tola authoring guide

Read the part that matches your work. Writing content or using a theme does not require
learning how to build one.

| You want to | Start with |
| --- | --- |
| Create a site, find its files, or set up an editor | `tola skill start` |
| Write content with the site's existing theme | `tola skill writing` |
| Install or update a theme or module | `tola skill modules use` |
| Customize a theme | `tola skill modules theme customization` |
| Choose pages, routes, layout, navigation, assets, or SEO | `tola skill site` |
| Create a theme | `tola skill modules theme --all` |
| Create a reusable library or author tool | `tola skill modules create` |
| Write a Nu script using Tola's libraries | `tola skill scripts` |
| Generate files during a build or act after publication | `tola skill hooks` |
| Check, preview, or publish a site | `tola skill build` |

A topic prints its own guidance and subsection commands. Select a deeper task when needed:

```sh
tola skill writing math numbering
tola skill site assets files
tola skill hooks generate-outputs
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

For an existing site, start with `tola.toml` and the theme's public guide. Read the selected
`build.entry` when changing page selection or templates. For a module, start with
`tola-module.toml`. Check site changes with `tola check`; use a browser to judge appearance.

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

### Workflow

| Task | Command |
| --- | --- |
| See the site's settings | `tola inspect config` |
| Write and preview with live rebuilding | `tola dev` |
| Validate the site without replacing published files | `tola check` |
| Preview production settings locally | `tola preview` |
| Write the complete site to `build.publish-dir` | `tola build` |

`check` and `preview` still run input and output generators; see `tola skill hooks`.
Use `tola check --no-hooks` when checking with already generated inputs.
For a theme's extra commands and guide, run `tola ALIAS --help` with its configured alias.

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
For pure builds, keep compiler fonts in configured site paths or available module inputs.
Host system fonts cannot be used with `--pure`.

Modules can declare compiler font directories with `fonts = ["typst-fonts"]` in the
`[typst]` table of `tola-module.toml`. Select their aliases in `[typst.fonts].modules`.
To use two versions of the same family, give one module's family a new compiler name.

```toml
[typst.fonts]
modules = [
  "theme",
  { module = "cjk", rename = { "IBM Plex Sans" = "CJK Plex Sans" } },
]
```

All matching faces from `cjk` in this entry move to `CJK Plex Sans`. Use that name in
Typst and check the registered names and module aliases with `tola inspect fonts --json`.

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
extensions. Runtime generators belong to `tola skill hooks`; finish a module with **Releases**.

### Use

Choose an alias in the site's `tola.toml`; the examples use `notes`. For a local theme:

```toml
[build]
entry = { module = "notes" }
[modules.notes]
path = "theme"
```

Content imports `@tola-modules/notes:0.0.0`. Selecting the module as `build.entry` uses
its site program; adding a library or tool does not require changing the entry.
Read the module's guide for its public functions and `[modules.notes.options]` settings.
For theme changes, see `tola skill modules theme customization`.

Choose exactly one source: `path`, `tarball`, or `git`. Local paths are relative to
`tola.toml`. Keep `tola.lock` with the site so other builds use the same remote inputs.

#### Archives

Published themes can be read from an HTTPS download or a local archive file:

```toml
[modules.notes]
tarball = { url = "https://github.com/example/notebook/releases/download/v1.0.0/theme.tar.zst" }
```

Use `tarball = { path = "downloads/theme.zip" }` for a local file. Choose one of `url`
or `path`; optional `sha256` checks the archive's expected SHA-256 digest.
Supported archives are tar.gz, tar.zst, and ZIP, with one outer directory containing
`tola-module.toml` directly inside it. Use the author's prepared release, since downloading
does not run author build tools. For a private release, download it separately or use Git.

A GitHub branch archive also works when the repository root contains the complete module.

```toml
[modules.notes]
tarball = { url = "https://github.com/example/notebook/archive/refs/heads/preview.tar.gz" }
```

This expects `notebook-preview/tola-module.toml` inside the archive. For a module under
`theme/`, use `git.subdir` or publish a separate archive of the module directory.

#### Git

Use Git when the selected directory already contains the module's runtime files:

```toml
[modules.notes]
git = { url = "https://github.com/example/notebook", tag = "v1.0.0", subdir = "theme" }
```

`subdir` selects the directory containing `tola-module.toml`; omit it for the repository
root. Choose one of `tag`, `branch`, or a full-commit `rev`; omitting all three selects
the default branch. HTTPS uses your Git credential helper. SSH URLs and `git@host:path`
use your SSH configuration and agent.

To follow `preview` when the module lives in `theme/`, use this declaration.

```toml
[modules.notes]
git = { url = "https://github.com/example/notebook", branch = "preview", subdir = "theme" }
```

#### Updates

Ordinary builds reuse the lock entry. Run `tola update notes` to refresh a branch or
archive, even if its name or URL is unchanged. To switch to another version, edit the
selector or URL first. If you follow a moving archive, omit `sha256` so the update can
record the new digest in `tola.lock`. If you set `sha256`, update it to match the new archive.
Check the site after updating. If it uses vendor copies, run `tola vendor --refresh`.

| Build option | Use it when |
| --- | --- |
| `--locked` | The build must keep `tola.lock` unchanged; locked inputs may still be fetched |
| `--offline` | Tola must use inputs already available without network access |
| `--pure` | Tola must use locked project inputs and vendor copies, without host caches |

Use `tola skill build vendor` to prepare an offline delivery. A lock records selected
inputs; it does not contain their files. These flags govern Tola's input loading, not
what a hook's external tools can access.

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
The entry runs with the consuming site's content and modules. The label must select
exactly one metadata value. Reuse the theme's page selection so queries agree with the site.

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

Queries do not build the site or run its hooks. Prepare generated inputs before querying
them. Keep query entries focused on the data the tool needs.

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

Each declaration selects a parameter of a public function exported by `[typst].entry`.
`positional` is zero-based; `named` selects a named parameter, not a positional parameter
with that name. Use a dot-separated export such as `content.page-link` for a nested member.

A query with at least one reference or definition must return an array of records with
these fields:

| Field | Requirement | Meaning |
| --- | --- | --- |
| `id` | Nonempty string, unique within this query | The value the public API accepts as a reference |
| `name` | String | The item's human-readable name |
| `description` | Optional string or `none` | Explanatory text for completion and hover |
| `source` | Optional native Typst path or `none` | The source file used for definition lookup |
| `url` | Optional string or `none` | The item's web address |

Extra fields are rejected. Use a separate ordinary query when a tool needs another data
shape. An ordinary query's consumer validates the fields it needs.

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

Use the ID the public function accepts. It is local to this module alias and query.
The query's `label` selects metadata; a record's `name` is display text. Keep native
`source` paths separate from web URLs; JSON represents a source as `{ root, path }`.

These declarations give consumers completion, hover, definition lookup, and reference
search for the selected arguments. Rename needs a writable literal definition and
writable references; computed IDs may prevent it. Check these features in a consumer
site through the module's public imports.

#### Calling

For a module that declares `queries.index`, a registered Nu tool can use:

```nu
use tola [query get-module]
query $"((get-module).name).index" | to json
```

`query` returns a Nu value directly; this example serializes it. It uses the calling site's
configuration unless given `--config FILE`. Always include the instance alias in a named
query; importing a library does not supply it. Check a renamed alias before shipping.

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

This reads project text using the selected record's source path, not a filename guessed
from its title or ID. Hidden table fields remain in the selected record. Only `submit`
starts the action; quitting has no side effect. Keep the plain query available for use
without a terminal.

`cli capture` takes an argument list and returns `stdout`, `stderr`, and `exit_code` from
the current Tola executable. Check the status before using stdout. Use ordinary external
calls for interactive editors or continuing commands such as `tola dev`.

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

Tola registers the initializer's original source under the selected alias, preserving that
module's generated options and other settings. A local initializer becomes a local `path`;
a remote initializer retains its Git selector or archive location. Temporary download
directories are used only while running and validating the initializer.

Git initializers accept the same selectors as Git modules. For a module in `theme/` on
the `preview` branch, initialize a site with:

```sh
tola init my-site --from https://github.com/example/notebook --branch preview --subdir theme --alias notes
```

Choose at most one of `--branch`, `--tag`, and `--rev`; `--rev` requires a full commit ID.
Omitting all three selects the default branch. HTTPS and SSH use the same authentication
as ordinary Git modules.

An HTTPS URL or local file ending in `.tar.gz`, `.tar.zst`, or `.zip` selects a module
archive. It must contain one outer directory with `tola-module.toml` directly inside.
Git-specific flags, including `--subdir`, apply only to Git initializers.

```sh
tola init my-site --from https://example.com/theme-v1.tar.zst --alias notes
tola init my-site --from ./theme-v1.zip --alias notes
```

The generated configuration can also declare other Git, tarball, or local modules.
Initialization resolves and validates these inputs before writing the site, records them
in `tola.lock`, and retains downloaded module snapshots in the new site's cache. The
initializer's own lock entry identifies the exact snapshot that ran, even if its branch
advances during initialization. `--dry-run` performs preparation in temporary storage and
shows the proposed site without writing it. Input restrictions such as `--offline` and
`--locked` also apply during preparation.

From an existing site, `tola notes init new-site` uses the selected `notes` module and its
locked snapshot. To change the new site's source later, edit its module declaration and
run `tola update notes`.

`get-site` and `get-module` are unavailable here; use `(get-init).module` for source details.
An unattended initializer must not prompt; `(get-init).interactive` indicates explicit
interactive use.

### Releases

Prepare a directory containing everything a consumer needs, then distribute it through
an archive or Git. The examples package `theme/` into `dist/theme/`; the directory names
and repository layout are your choice.

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

Patterns are module-relative. A directory includes its descendants, exclusions win,
and the manifest is always included. Keep entries, imported files, and declared assets
in the release; author-only sources may be excluded.

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

Pack runs before-pack hooks in the source module, then replaces the destination with
the selected release files. A failed preparation leaves the previous release intact.
The delivered manifest omits `pack` and `hooks.before-pack`, so author scripts may be
excluded. Consumers receive the prepared files and never run before-pack themselves.
This stage has no consuming site: use `get-hook`, not `get-site` or `get-module`.

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

If an author instead commits pack output to Git, consumers can select its directory
with `git.subdir`. Branch names and source/output directory names are author choices.

## Scripts

Use `tola nu` for author tools, structured data processing, and build scripts.
It includes Nushell, so consumers do not need a separate Nu installation. Save this as
`greet.nu` and run `tola nu greet.nu "Tola author" --upper`:

```nu
# Greet someone by name.
def main [name: string, --upper] {
  let greeting = $"Hello, ($name)!"
  if $upper { $greeting | str upcase } else { $greeting }
}
```

Use `tola skill modules commands` to expose it as a module command, or
`tola skill hooks` to run it during a build. Browser code still needs JavaScript.

### Runtime

`tola nu` runs a script file or a public `@tola-scripts/NAME` or `@tola-modules/ALIAS`
entry. Put Tola options before the entry and script arguments after it. It does not accept
the standalone Nu CLI's `-c` flag. Use `tola nu --help` to list available libraries.

The standard library is available, for example `use std/assert`. Personal Nu startup
files and plugins are not loaded. Run `version` or `help COMMAND` inside a script to
inspect the runtime you are using.

Pass process input with `--stdin`. For `upper.nu`:

```nu
def main [] { $in | from json | get name | str upcase }
```

`printf '%s' '{"name":"Tola"}' | tola nu --stdin upper.nu` returns `TOLA`.
Decode process input explicitly; Nu records are not passed between processes as records.

### Libraries

Import the Tola helpers you need:

| Import | Use |
| --- | --- |
| `use tola get-site` | Site settings, options, and paths |
| `use tola get-module` | The calling module's alias and options, or a named instance |
| `use tola query` | A declared module query or an explicit Typst query file |
| `use tola get-hook` | Directories and stage of the current hook |
| `use tola get-init` | The current initializer's source and destination |
| `use tola cli` | Capture a finite Tola command's output and exit status |
| `use tola diagnostic` | Report warnings and errors; see `tola skill modules commands reports` |
| `use tola ui` | Interactive selection and confirmation |
| `use tola image` | Read image metadata and produce resized images |
| `use tola-modules notes` | Public Nu exports of the configured `notes` module |

`get-hook` and `get-init` require their respective invocation. `get-module notes` reads
an explicit alias; calling it without an alias requires a module command, hook, or public
entry. Importing a library does not change the calling instance. Pass settings to shared
library functions explicitly.

For example, resize an image without installing an external image tool:

```nu
use tola image
image resize photo.png --output thumb.webp --width 640 --format webp
```

One dimension preserves the aspect ratio; two fill the rectangle and may crop.
The destination directory must exist, and an existing file is not overwritten.
Creating the image does not publish it: declare it as an asset or hook output.
Use `tola help "@tola-scripts/image"` for image operations and their options.

### Values

Nu pipes records, lists, and tables. Keep values structured until writing a file or
calling another process:

```nu
let pages = [
  {title: "Guide", draft: false}
  {title: "Notes", draft: true}
]
$pages | where draft == false | select title | to json
```

A table is a list of records. `get` extracts a field; `select` keeps columns;
`update` and `upsert` return changed records. Use `describe` to inspect a value.
`$row.description?` reads an optional field. Decode JSON with `from json` and serialize
with `to json`; a rendered terminal table is not a data format.

### Pipelines

Pipeline input and positional parameters are separate:

```nu
def add [amount: int]: int -> int { $in + $amount }
2 | add 3
```

This returns `5`. Use `$in` for pipeline input; piping into a closure does not fill its
first parameter. Use `each` for transformed results and `for` for ordered side effects.
Only the last expression is returned. Use `print --stderr` for progress beside structured
stdout, rather than inserting messages into the returned data.

### Commands

Typed `def main` parameters provide argument parsing and `--help`. A switch such as
`--upper` is boolean; a value flag has a type, such as `--limit: int = 10`.
Nu invokes `main` after top-level code; do not call it again at the bottom.
Keep top-level code free of writes so asking for script help has no side effects.

Put reusable definitions in `lib.nu`:

```nu
export def greeting [name: string]: nothing -> string {
  $"Hello, ($name)!"
}
```

Import them from `hello.nu`:

```nu
use ./lib.nu greeting
def main [name: string] { greeting $name }
```

Run `tola nu hello.nu Ada`. `use` imports exports; `source` executes in the current scope.
Import paths must be literal or constant, not runtime `let` values. To publish a library
through a module, see `tola skill modules nu-libraries`.

### Arguments

Pass one value per argument. Spread a list instead of joining it into a shell command:

```nu
def main [file: string, ...words: string] {
  let program = $env.TOLA_EXECUTABLE
  ^$program nu $file ...$words
}
```

`^` selects an external executable and `...$words` preserves argument boundaries.
Use `--` for a positional value that starts with a dash, for example
`tola nu greet.nu -- -Ada`. Nu interpolation is `$"Hello, ($name)"`; single-quoted
strings are literal. Serialize JSON instead of assembling it with string interpolation.

### Paths

Disk scripts keep the caller's working directory and use a nearby `tola.toml` when one
exists. Select another site with `tola nu --config FILE ENTRY`. A public module entry
and a registered module command run at the site root. These invocations do not build
the site or run hooks; prepare any generated inputs before querying them.

Use `path join` for filesystem paths. `$env.PWD` is the working directory;
`$env.FILE_PWD` locates resources beside a script. A relative `open` uses the working
directory, while relative `use` paths follow the importing file.

```nu
use tola get-site
let data = ((get-site).root | path join "data" "pages.json")
open $data
```

This script needs a configured site. For reusable libraries, use `path self` for their
own resources and pass site data as arguments.

### Files

`open` decodes known formats; `open --raw` preserves text or bytes. Use raw reads for
Typst templates and text editing. To write selected records from JSON:

```nu
def main [source: string, destination: string] {
  open $source | select title output | to json | save --raw $destination
}
```

`to json` chooses the format; `save --raw` writes it without another conversion.
Use `save --force` only for intentional replacement. For tools editing existing project
text, `tola edit --help` describes writes that check whether the input has changed.

### Errors

Let unexpected errors fail the command. Use `try/catch` for a specific recovery.
When capturing an external command, inspect its status before using its output:

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

`complete` returns `stdout`, `stderr`, and `exit_code`. `cli capture` provides the same
shape for a finite Tola command and uses the current executable. See
`tola skill modules queries selection` for an example using it with a query.

### TUI

Use `ui choose` for an interactive author command:

```nu
use tola ui
let rows = [{name: Guide, file: "content/guide.typ"} {name: Notes, file: "content/notes.typ"}]
let choice = ($rows | ui choose --columns [name])
if $choice.action == "submit" { $choice.selected }
```

The selected value is the original record, including fields hidden from the table.
Only a submitted choice should start an editor or write a file. Keep previews read-only
and hooks unattended. For filtering query results, see `tola skill modules queries selection`.

### Testing

Run scripts with the `tola nu` executable consumers will use. A small `test.nu` for
`lib.nu` from Commands can use the standard library:

```nu
use std/assert
use ./lib.nu greeting
assert equal (greeting "世界") "Hello, 世界!"
```

Run `tola nu test.nu`. Use a temporary site for file-changing tools; check the result,
error handling, and cancellation. A parse check with `nu-check --debug` helps find syntax
errors, but does not exercise the tool's behavior.

## Hooks

Choose a hook by what it produces. Hooks finish after one invocation; Tola handles watching.

| Work | Stage | Read |
| --- | --- | --- |
| Prepare files that Typst or asset declarations will read | `before-build` | `tola skill hooks before-build` |
| Add final files, such as a search index, to the site | `generate-outputs` | `tola skill hooks generate-outputs` |
| Upload or otherwise use the published site | `after-publish` | `tola skill hooks after-publish` |
| Prepare a module's release files for its consumers | Module `before-pack` | `tola skill modules releases preparation` |

`build` runs all three runtime stages. `check` and `preview` run the first two without
replacing published files. `dev` follows each hook's development setting. Ordinary queries,
Nu scripts, inspection, and vendoring do not run build hooks.

### Declaration

Declare runtime hooks in `tola.toml` or `tola-module.toml`, under `[[hooks.STAGE]]`.
Each has a name and exactly one action: `nu = "tools/task.nu"`, with optional `args`,
or `argv = ["tool", "argument", ...]`. `argv` does not expand shell variables or globs.
Nu script paths are relative to the declaring site or module; runtime hooks run at the
site root.

Use `enable = false` to disable a hook. `enabled-by = "search"` reads a boolean in the
site's `[options]` or the calling module's effective options. Module hooks can use `{name}`
in generated paths and input patterns to distinguish consumer aliases.

Nu hooks use `get-site`, `get-module`, and `get-hook`. External tools receive `TOLA_*`
environment variables; their stage-specific reference is `tola help "[hooks]"`.

### Before build

For an existing `tools/data.nu` that writes `generated/data.json`, add to `tola.toml`:

```toml
[[hooks.before-build]]
name = "local-data"
nu = "tools/data.nu"
rerun-on = { include = ["data/**/*.json"] }
generates = ["generated/data.json"]
```

`generates` names files or directories relative to the site root. They must exist when
the hook finishes. Read them from Typst, or declare them under `[assets]` if browsers
need the files themselves. Generating an input does not publish it.

This stage writes project inputs directly; it has no hook input or output directory.
Keep generated files separate from their source inputs and outside `build.publish-dir`.

### Generate outputs

This stage reads compiled pages from `(get-hook).input-dir` and writes final files beneath
`(get-hook).output-dir`. Declare each file with `{ file = "search.json" }`, or a whole
directory with `{ tree = "search" }`. Paths are relative to the published site, without
`site.base-path`. Files must match the declarations and cannot overwrite other site outputs.

For a module with the `pages` query from `tola skill modules queries`, add this to
`tola-module.toml` and create `runtime/search.nu`:

```toml
[[hooks.generate-outputs]]
name = "search"
nu = "runtime/search.nu"
outputs = [{ file = "_tola-modules/{name}/search.json" }]
```

```nu
use tola [get-hook get-module query]

def main [] {
  let module = get-module
  let destination = ((get-hook).output-dir
    | path join "_tola-modules" $module.name "search.json")
  mkdir ($destination | path dirname)
  query $"($module.name).pages" | to json | save --raw $destination
}
```

Link this file from the theme with `output-url("_tola-modules/" + get-module().name
+ "/search.json")`, importing `output-url` from `@tola/address` and `get-module` from
`@tola/module`. It applies the site's mount. Hook outputs need no `[assets]` entry and
do not appear in `assets.urls`; the generator owns any minification or hashed filenames.

Each output hook reads the same compiled site and writes into its own empty directory.
If several steps depend on each other's results, put them in one script. Use
`(get-hook).cache-dir` for reusable tool data and `temp-dir` for temporary work; a running
hook must still write its declared output files.

### After publish

Use `(get-hook).input-dir` to read the just-published files, for example from an upload
script. This stage does not add files to the site. `tola build` waits for it to finish;
a failure reports an error but leaves the publication in place.

Development skips this stage by default. With `dev = "run"`, it follows the latest
published development site without delaying browser updates; intermediate revisions may
be skipped. Use it for a consumer of the latest site, not a record of every build.

### Development

`before-build` and `generate-outputs` default to `dev = "run"`; `after-publish` defaults
to `"skip"`. These settings affect `tola dev`, not production builds.

Use `rerun-on = { include = [...], exclude = [...] }` for additional files a tool reads.
Patterns are relative to the declaring site or module; `{ project = "content/**/*.typ" }`
selects project inputs from a module. The Nu entry script is watched automatically.
Include imported helpers and tool configuration when they affect the result.

Only `generate-outputs` accepts `dev = "reuse"`: unchanged inputs can reuse its outputs
and diagnostics. Use it for generators determined by the compiled site and declared inputs,
not by current time or network results. Production still runs the hook.

For warnings or errors from a hook, use `tola skill modules commands reports`.

## Build

Use `tola check` to build and validate without replacing the published tree. Use `tola dev`
for watching and `tola preview` for a production preview. `tola build` publishes to disk.
Hook stages and examples are in `tola skill hooks`.

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
