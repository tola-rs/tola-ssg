# Tola

Read by task, not cover to cover. One root Typst program selects the sources and writes the
site's documents; a source is an input, not necessarily a page. Templates and metadata rules
belong to the site. HTML and CSS control browser structure and appearance.

Start by reading the site's configuration, selected entry, and the templates involved in the
change. Reuse its selected output paths for links, navigation, and feeds.

```sh
tola skill headings
tola skill headings levels
tola skill math numbering
tola skill scripts arguments
tola skill --list
```

A topic gives the common answer and its subsection commands. A subsection gives only its own
text and deeper choices. `--list` lists the selected heading tree; `--all` includes its entire
body. With no selection, `--all` prints this complete document. `--output FILE` writes the
same selected text to a file without overwriting it: `tola skill math numbering -o math.md`,
or `tola skill --all -o TOLA-SKILL.md` for the complete guide. Without the CLI, read the
matching heading below rather than loading the whole document.

Check `tola --version`. Use `tola <command> --help` for flags and `tola help` for installed
configuration and package contracts. Package imports use `0.0.0`, not the executable version.
Import members or rename the module so `@tola/document` does not shadow native `document(...)`.

Validate site changes with `tola check` and inspect affected routes for browser changes.
Report which command and browser checks actually ran; syntax alone does not prove rendering.

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
scaffold files. Use `tola init --help` to choose features rather than assuming CSS, feeds, or
search are present. Open the URL `dev` prints; save a source and check that it rebuilds.

### Locate

Tola finds the nearest `tola.toml` in the current directory or its parents. A command's
`-C path/to/tola.toml` selects another site. Paths in the configuration are relative to it.

In the scaffold, `content/` holds content sources; `site.typ` connects `site/selection.typ`
and `site/page.typ`. `site/schema.typ` owns metadata fields, `site/seo.typ` owns head and SEO
outputs, and `site/not-found.typ` writes the missing page. These are editable conventions,
not reserved names. Keep shared helpers outside the content directory.

### Editors

Use `tola editor setup --list`, then the setup command for your editor. After upgrading Tola,
`tola editor packages` refreshes its editor package files. Read the site's `[formatter]`
settings before formatting; the CLI and language service share them.

## Syntax

Typst is not Markdown or LaTeX: `=` introduces headings, `$` introduces math, and `#`
enters code from markup. Use native content for prose and HTML/CSS for browser layout.

### Modes

```typst
#let name = "Tola"
#let greeting = [Hello, *#name*!]
#greeting
```

`[...]` is content; `{...}` is code. Inside code, omit `#`; inside nested content, use it
again. `[A *rich* title]` carries markup; `"A *rich* title"` is a string. An HTML string is
text, not an element. Render arrays with a loop or `.join()`.

### Functions

```typst
#let note(body, kind: "note") = html.aside(class: kind, body)
#note(kind: "tip")[Keep the body as content.]
```

Named arguments use `name: value`; `..values` spreads arguments. Trailing content follows
named arguments. Put a multiline expression or method chain inside parentheses or a code
block: a new top-level `.map(...)` line is markup, not a continuation.

### Imports

`import` reads definitions; `include` inserts a file's content. Imported files do not inherit
the caller's locals: pass inputs to their functions. Import the members you use, or use
`as` to choose the module name.

Project `/...` paths start at the site root; inside a package they start at that package.
Pass a discovered source's `file` directly to `include`. For project data read by a theme,
see [Modules](#modules). Reading a file does not publish it.

### Context

Use `context` for locations, counters, and document queries, inside the document body that
owns the result. `all-sources()` and a source's `tola-meta(...)` declaration run in ordinary
code. Deferring the declaration into `context` does not declare source metadata.

## Headings

Write native headings for document structure and queries. `#title()` is the document title;
`= Section` starts its sections. Style their exported HTML with CSS instead of changing the
logical hierarchy to get a font size.

### Levels

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

### Numbering

```typst
#set heading(numbering: "1.1")
= Installation <guide-install>
See @guide-install.
```

Numbering is separate from the outline. `link(<guide-install>)[Installation]` needs no
numbering, while `@guide-install` does. Labels and stable fragments belong to
`tola skill links fragments`.

### Contents

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

### Inclusion

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

## Math

Use native Typst equations. `$x^2$` is inline; spaces at both ends, as in `$ x^2 $`,
make a displayed equation. HTML uses MathML by default; keep that structure unless a
specific formula needs a different rendering.

### Notation

```typst
Inline: $x_i^2 + alpha$.

$ (a + b) / c = frac(a + b, c) $
$ sum_(i=1)^n i = n(n+1) / 2 $
$ mat(1, 0; 0, 1) $
```

Use `_` and `^` for attachments and parentheses to group their bodies. Functions such as
`frac` and `mat` are Typst math, not LaTeX commands. Multi-letter identifiers name symbols,
functions, or variables; quote ordinary words, for example `$ x "where" x > 0 $`.

### Numbering

```typst
#set math.equation(numbering: "(1)")
$ E = m c^2 $ <energy-equation>
See @energy-equation.
```

Numbering applies to displayed equations. Keep labels unique across the site. For numbering
that restarts per document, put `#counter(math.equation).update(0)` at that document's start.
Use a native reference rather than manually repeating its number.

### Rendering

Test in the browser at narrow widths. A successful compile does not prove that long
formulas fit, that the intended math font loaded, or that light and dark themes are legible.
Explain essential formulas in prose as well.

#### Fonts

MathML uses browser fonts. Publish the font file and load it with CSS; a Typst compiler
font setting does not install it in the browser. SVG equations instead use the fonts
available to the compiler. See `tola skill assets fonts`.

#### SVG

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

## Links

Link a native target with `link(label-or-location)`. For a selected page, use
`output-url(page.output)`. Source paths are files to read, not browser destinations.

### Routes

An output such as `guide/index.html` publishes at route `/guide/`. `/guide` instead names
a file without an extension. `output-of` turns a route into an output; `output-url` turns
an output into a browser URL. Use `route-url` when you already have a decoded route.

The scaffold slugs each source's route segments; `about.typ` and `about/index.typ` therefore
claim the same output. Its `permalink` field changes the address, not the document format.
Choose outputs once in selection and pass them to every consumer.

### Mount

For a site served at `https://example.com/docs/`, configure:

```toml
[site]
origin = "https://example.com"
base-path = "/docs/"
```

Keep outputs unprefixed. `output-url`, `route-url`, and `asset-url` apply the mount once;
use their answers directly. Handwritten site-root URLs must include the mount. Pass
`origin: site.origin` for an absolute URL. Encode at the URL boundary, not in output names.

### Fragments

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

### Check

Use `tola check --json` to read the routes, outputs and reference results from that
check. Fields for unfinished stages are null; a completed stage with no rows returns an empty array. Absolute URLs are external even when they spell `site.origin`.
Exact conversions: `tola help "@tola/address" output-of output-url route-url asset-url`.

## Metadata

Declare metadata with `tola-meta`; validate it in the root program with the site's schema.
Tola does not assign built-in meaning to fields such as `draft` or `permalink`. The scaffold
implements those choices, and a theme may choose different fields.

### Declaration

```typst
#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: "Hello", description: "A short introduction"))
```

Call once during ordinary source evaluation, directly or through an imported function.
A deferred `context` or selector show rule does not declare metadata. Imported files' own
top-level declarations belong to those files. Draft sources still evaluate and can fail.
`tola inspect sources` discovers file identities without evaluating metadata. Return
selected metadata through a model query with an explicit JSON projection.

### Fields

Read the site's schema before adding a field. The scaffold's `site/schema.typ` rejects
unknown fields. Add the field there, then make its consumer use the parsed value.

```typst
#import "@tola/schema:0.0.0": schema, optional, parse
#let fields = schema((title: str, order: optional(int, default: 0)))
#let meta = parse((title: "Guide"), fields)
#assert.eq(meta.order, 0)
```

`optional` accepts an absent key; `nullable` accepts explicit `none`. A supplied invalid
value does not fall back to a default. For source records use `parse-sources` rather than
parsing each dictionary by hand: it reports the source whose metadata failed.

### Dates

Choose calendar precision deliberately: `date` is a complete day, `timestamp` is an instant,
and `partial-date` retains year, month, or day precision without inventing missing fields.
The scaffold uses `timestamp` for `published` and `union(date, timestamp)` for `updated`.

```typst
#import "@tola/schema:0.0.0" as s
#let published = s.parse("2026-09-01T09:30:00+08:00", s.timestamp)
#let period = s.parse("2026-09", s.partial-date)
#assert.eq(period.precision, "month")
```

Parsed timestamps carry `rfc3339`, `unix-seconds`, and `subsecond`. Pass `.rfc3339` to
feed and sitemap helpers. Exact accepted inputs: `tola help "@tola/schema" date timestamp partial-date`.

## Pages

Select sources and destinations once in the root program. Pass those same records to page
templates, menus, and SEO outputs. A source can appear in several documents or none; its
file path alone does not publish it.

### Selection

The scaffold's `select-pages(all-sources())` parses every source with `page-schema`, filters
drafts, and returns `(source:, output:)` records. Edit `site/selection.typ` to change that
choice; keep rendering in `site/page.typ` and connect them in `site.typ`.

`all-sources()` discovers lowercase `.typ` files beneath the configured content directory,
excluding hidden names, symlinks, and the site entry. Its order is by complete source path,
not editorial order. Validate and sort explicitly before using the records in navigation.

### Rendering

Pass `page.source.file` directly to `include`. It is a native input path, not a URL.
`document(page.output, format: "html", ...)[...]` emits the page; choose a PDF output and
format explicitly for PDF. A permalink changes an address, not its format.

`current-source()` belongs to the content source where the call is written; `current-document()`
belongs to the containing output and needs `context`. A call to `current-source()` in a shared
helper does not identify its caller. Pass the source to helpers instead.

### Data

A site may generate documents from `json` or `csv` without a content source per page.
Validate generated metadata with `parse` and the same schema used by other pages.
Keep data reads, output selection, and rendering separate; reading a file does not publish it.
Compare the paths from `tola inspect sources` with the pages in `tola check --json`
when a source exists but a page is missing. `tola check --no-hooks --json` uses existing
inputs without running generators; missing generated inputs remain errors.

## HTML

Use native Typst content for text, headings, equations, and links. Use typed `html.*`
constructors for browser structure and CSS for layout. Typst page settings are not CSS.

### Elements

```typst
#html.aside(class: "note")[A reusable note.]
#html.elem("aside", attrs: ("data-kind": "tip"))[A note with a custom attribute.]
```

Typed constructors accept their documented attributes and one content body; void elements
have none. Use `html.elem` for a tag or attribute the typed constructor does not provide.
Use native `link` for Typst targets; `html.a` is useful for raw attributes such as `target`
and `rel`. Strings containing HTML stay text.

### Shell

A custom `html.html` shell owns its head: document metadata does not fill it automatically.
Use the site's `head-entries` helper or supply the title, viewport, description, and styles.
`html.head` takes content; loop over entry arrays or `.join()` them.

Keep an actual `html.head` so the exporter can add equation styles. With a custom shell,
check footnotes explicitly: upstream's default placement is not available there; retain the
default shell or supply a footnote show rule. Test citations and bibliography in the same shell.

### CSS

Load declared stylesheets through `asset-url`; see `tola skill assets files`. Use CSS for
spacing, columns, responsive layout, and browser typography. `html.frame` makes SVG artwork,
not an HTML layout container, and cannot contain `set page`.

`html.script` and `html.style` take strings. An external script uses an empty body:
`html.script("", src: url, defer: true)`. Inspect the actual DOM and narrow viewport before
claiming the layout is correct.

## Code

For highlighting, apply `render-code` as a show rule and load `code-stylesheet()` in the
head. Calling the renderer directly skips Typst's preparation of a raw element.

### Highlighting

```typst
#import "@tola/code:0.0.0": render-code, code-themes
#set raw(theme: code-themes.github)
#show raw: render-code.with(dark-theme: code-themes.github-dark)
#raw("let answer = 42;", lang: "rust", block: true)
```

Set `raw.theme` where the code is written. Inline backticks have no language;
`raw(..., lang: "rust")` enables inline highlighting. Browser token styles also require
`code-stylesheet` imported from `@tola/code:0.0.0` and called in the document head.

### Themes

The dark appearance applies while the root element has `data-theme="dark"`. The site owns
the toggle and system preference behavior. Style the wrapper with CSS; inspect both theme
states. Exact options: `tola help "@tola/code" render-code code-stylesheet`.

## Navigation

Build menus from the selected page records, not a fresh interpretation of source paths.
Choose a stable editorial order and use each record's output for its link.

### Ordering

Add an `order` field to the site's schema before sorting by it. Break ties by source identity,
for example `(page.source.meta.order, page.source.id)`. Pass the ordered records to both the
page template and navigation. A previous/next link is a neighbor in this order, not a filename
guess. For paginated lists, find neighbors among the generated listing pages.

### Hierarchy

Use `@tola/collection` for lineage and descendants once the site's parent rule is explicit.
A permalink is an address, not necessarily a parent relationship. Keys must be unique and
non-`none`; the parent rule must reach `none` and may traverse keys absent from the array.
Exact examples: `tola help "@tola/collection" lineage descendants adjacent`.

### Backlinks

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
For a page TOC use `tola skill headings contents`; recursive relations can use
`tola help "@tola/datalog" database query` without replacing rendered heading queries.

## Assets

Distinguish a compiler input path from a browser URL. Reading a file does not publish its
original. Declare browser-served files under `[assets]`; use image helpers when a generated
derivative, rather than the original, should be published.

### Files

The stylesheet URL is the URL declared by the site's asset mapping, not its input filename:

```typst
#import "@tola/address:0.0.0": asset-url
#html.link(rel: "stylesheet", href: asset-url("/assets/css/site.css"))
```

Place this in the head after declaring that URL. `asset-url` validates the declaration and
adds the deployment mount. CSS `url(...)` is relative to the published stylesheet; Tola does
not bundle JavaScript or rewrite CSS resource references. See `tola help "[assets]"`.

### Fonts

Compiler fonts and browser fonts are separate inputs. `[typst.fonts]` supplies Typst;
`tola inspect fonts` lists its fonts. Browsers need published font files loaded through CSS
`@font-face`. A Typst font setting alone does not load a web font.
Keep compiler fonts in the site's configured paths for pure builds; host system fonts
cannot be used with `--pure`.

### Images

Use native `image` for compiler-rendered content, `html.img(src: asset-url(...))` for a
published original, or `@tola/image` for resized derivatives and responsive sets.

Pass an input path or raw bytes to image helpers, not an asset URL. Resolve a path with
`path(...)` before passing it to another file's function. Returned URLs already include
the mount. Supply `sizes` for responsive images that are not full-width.

`resize-image` needs an explicit raster format to rasterize SVG; responsive helpers refuse
SVG. Keep vector artwork as vector when resizing is unnecessary. A declared original stays
published even when a helper also creates derivatives. Put private originals outside asset
trees. Inspect alternate text, dimensions, and narrow layouts. See `tola help "@tola/image"`.

### Icons

Use collections declared under `[icons.collections]` and their actual `namespace:name` IDs.
An unlabeled icon is decorative; informative icons need a label and controls need accessible
names. Inline `currentColor` artwork follows its surrounding color; an `icon-url` image
does not. Exact helpers: `tola help "@tola/icon"`.

## SEO

Reuse selected page outputs for canonical URLs, feeds, and sitemaps. Configure the real
`site.origin` and deployment path before judging their addresses. Social image metadata
does not publish the image: declare that resource too.

### Head

The scaffold's `site/seo.typ` composes head entries. Canonical and social helpers return
arrays; render them in the custom head. Build absolute URLs with
`output-url(page.output, origin: site.origin)`. Use `plain-text` for rich titles.
See `tola help "@tola/web" canonical open-graph twitter-card`.

### Feeds

A feed needs an origin and a non-blank title. In the scaffold, absent `feed-content` selects
the exported body; setting a summary alone does not make an excerpt-only feed. For that,
change the recipe's `content:` to `none`.

A `(document: output, id: "article")` selection keeps one exported subtree, whose HTML ID
must occur exactly once. Use it to exclude navigation. Keep entry IDs stable when routes
move, and pass parsed timestamps as `.rfc3339`. Inspect the serialized feed's actual content
and URLs. Exact inputs: `tola help "@tola/web" feed`.

### Sitemap

Sitemaps name whole documents, not fragments. They accept calendar dates as well as
timestamps for updates. The scaffold's selected sitemap feature skips its output without
an origin. Check the serialized entries and exclusions after `tola check` or a build.
Exact inputs: `tola help "@tola/web" sitemap`.

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

Direct `tola nu` discovers site configuration and its Nu libraries without building the site
or running hooks. Select another configuration with `tola nu --config FILE ENTRY`; a managed
module command also passes its selected configuration through `TOLA_CONFIG_PATH`. Disk scripts
keep the caller's working directory; a public `@tola-modules/ALIAS` entry runs at the site root.
Inherited `TOLA_MODULE_ROOT` affects relative script entries, so use an absolute entry when
invoking a different script from a module command. Reusable libraries receive options as
arguments: importing a library does not select its module as the current command. Keep output
paths in the caller's intended project location, separately from installed script resources.

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
not silently fall back to a different binary. Test the consumer workflow without extra Nu/Deno/Node on
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

## Modules

Remote modules use an HTTPS repository, one revision selector and an optional repository subdirectory:

```toml
[modules.notes]
repository = "https://github.com/example/notebook"
tag = "v1.0.0"
subdir = "theme"
```

Use a real repository URL and replace `tag` with `branch` or a full-commit `rev` when appropriate.
Run `tola module fetch notes`; omit the alias to fetch all remote modules. Fetch uses gix,
without a system Git command, and records the exact commit in `tola.lock`. Keep that lock
file with the site. Ordinary builds and tools use prepared `.tola/modules` sources without
network access; run fetch explicitly to refresh a tag or branch. The selected directory
contains `tola-module.toml`; it need not be the repository root. Remote init is not supported
yet; a module initializer still takes a local directory.

A local theme can supply the site's root program and public writing API. Read its manifest
and exported schema before changing its options; do not assume scaffold fields or filenames
apply to a theme.

### Theme

```toml
[theme]
module = "notes"
[modules.notes]
path = "theme"
```

The module directory has `tola-module.toml`: `[typst].entry` exports its writing API and
`[site].entry` supplies the site program. Content imports `@tola-modules/notes:0.0.0`.
Selecting a theme and explicitly setting `[build].entry` together is an error.

### Options

Project options live under `[modules.notes.options]`; slot overrides live under
`[modules.notes.overrides]`. Inside the module, `get-module()` from `@tola/module` reads
validated options, slots, and assets; project code selects `get-module("notes")`.
Use the returned `assets.urls` directly, without adding the mount again.

### Paths

Package `/...` paths point to the package root. Preserve `source.file` when including project
content. For a known project data path, use `project-path` from `@tola/site` before reading
it. Creating a path does not run a generator. Tools and hooks depend on the module's actual
manifest: use `tola help "[modules]"` for current declarations rather than guessing an
external script's environment or assuming a query runs its hooks.

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

Built-in libraries use `use tola NAME`. Image operations accept bytes or file paths:

```nu
use tola image
image metadata-file photo.png
image resize-file photo.png thumb.webp --width 640 --op fit-width --format webp
```

The file command is `tola nu @tola/image resize photo.png thumb.webp --width 640 --op fit-width
--format webp`. `image metadata` returns dimensions and format. Raster resize does not provide
SVG rendering or publish a site asset; use the site's existing output hooks for generated files.
The destination directory must exist and existing files are not overwritten. `resize-file`
returns a small metadata record; it does not return the encoded image through Nu. The byte
pipeline remains available as `open --raw photo.png | image resize --width 640 --op fit-width
| save --raw thumb.webp`. Resizing still holds decoded pixels; choose a modest explicit
`par-each --threads` value when processing large images concurrently.
Unknown `@tola/NAME` or `@tola-modules/ALIAS` entries fail directly. Use `./@file.nu` for an
ordinary filename beginning with `@`. `tola nu --lsp` serves these same Nu libraries to editors;
`tola lsp` supplies Typst and declared entity semantics.

### Commands

Register a Nu tool through the module's existing argv command, in `tola-module.toml`:

```toml
schema-version = 1

[commands.greet]
kind = "argv"
argv = ["{tola}", "nu", "tools/greet.nu"]
```

Put the Scripts greeting example at `tools/greet.nu`. With `[modules.notes] path = "theme"`
in the project configuration, run `tola notes greet Ada --upper`. The site chooses the alias;
renaming it changes the command namespace. `{tola}` selects the current executable, and
the relative script entry resolves under the module root while cwd stays at the project.
Other user arguments pass through unchanged and Nu binds them using the script's typed `main`
signature. Write the command description as a comment above `main`; define options, types and
defaults there once. `tola --help`, `tola notes --help`, and `tola notes greet --help` derive
help from that signature without running the script. A manifest `help` field is optional and can
override the description. For other argv tools, use `help` to describe their usage. Direct
`tola nu /path/to/tools/greet.nu --help` uses Nu's own help.

Registered tools receive `TOLA_PROJECT_ROOT`, `TOLA_CONFIG_PATH`, `TOLA_CONTENT_ROOT`,
`TOLA_MODULE_ROOT`, `TOLA_MODULE_NAME`, `TOLA_MODULE_MANIFEST`, `TOLA_EXECUTABLE` and
`TOLA_INPUT_SCOPE`. These are external-command context, not Nu-only APIs. Module hooks
also receive `TOLA_MODULE_OPTIONS` as a JSON-file path; ordinary argv tools do not.

#### Reports

A registered argv tool or a hook at any stage can submit diagnostics separately from its ordinary output:

```nu
use tola diagnostic
[
  (diagnostic issue "Some entries have no summary" --code missing-summary
    --hints ["Add a summary to the listed entries."])
] | diagnostic submit
```

The default severity is `warning`; `error` makes an otherwise successful command fail.
Submit the complete list once, including `[]` when it is empty. The host supplies
`TOLA_REPORT_PATH` and identifies the command or hook; the script does not choose its producer.
An absent report is distinct from a submitted empty list. A malformed report fails the operation;
a nonzero process exit remains a failure even if a report was submitted. Commands and hooks have
no supplied source snapshot, so submit issues without locations. A file path is not a source ID.
This report interface does not yet support independent checks or snapshots of unsaved editor text.

An independently run script can use `diagnostic submit --path report.json` to write the same
report, but writing a file does not display it as Tola diagnostics. Other languages can write the
same schemaVersion 1 JSON batch.

#### Queries

Use public Tola queries rather than building a second site model in a script. The Nu command
uses the current Tola executable and input scope, and inherits the selected configuration.
For a module that declares `queries.index`, a registered tool can use:

```nu
use tola
tola query $"($env.TOLA_MODULE_NAME).index" | to json
```

Replace `index` with a query the module actually declares. The Nu command returns the query
value directly; this example serializes it for its own caller. Use `--config FILE` to override
the inherited configuration and `--report` for the complete report. `use tola query` imports
the same command as bare `query`; importing a library never fills in its module alias.
Private queries run no build hooks and publish nothing; missing generated inputs remain
missing. Check against a nondefault config and renamed alias before shipping the tool.

`tola query --help` lists configured query names. `tola query notes.index` returns
only the query value as JSON; add `--report` for the versioned envelope, diagnostics
and input evidence. This query envelope is separate from a command or hook diagnostic batch. `tola query ./tools/model.typ --label model` reads one explicit
Typst entry without requiring a module declaration.
For a file query, the report's `moduleManifest` is null.

Use `tola query notes.index -i` to select a row, or add `--report` to return it inside
the envelope. A declared query command's fixed `projection` applies after selection.
Use ordinary Nu `where` and `select` for filtering and field selection. Nonterminal
output and `--output FILE` return the ordinary result without opening the browser;
leaving the browser writes nothing. A report's `resultDigest` identifies returned data,
including the selection, and is not an editing revision.

#### Selection

For a module's existing `index` query with `format = "entities"`, this `tools/find.nu` keeps
each entity's complete record through search and selection. Register it as an argv command
with `argv = ["{tola}", "nu", "tools/find.nu"]` and run `tola notes find`:

```nu
use tola [query cli ui]

def main [] {
  let entities = query $"($env.TOLA_MODULE_NAME).index"
  if ($entities | is-empty) { return }
  let choice = ($entities | ui choose --columns [id label] --search-columns [label] --fuzzy)
  if $choice.action != "submit" { return }
  let entity = $choice.selected
  if $entity.source == null or $entity.source.root != "project" {
    error make {msg: "This entity has no project source to read"}
  }
  let reply = cli capture [edit --config $env.TOLA_CONFIG_PATH read -- $entity.source.path]
  if $reply.stderr != "" { print --stderr --no-newline $reply.stderr }
  if $reply.exit_code != 0 { exit $reply.exit_code }
  print --no-newline $reply.stdout
}
```

The final command returns source text and its current digest as JSON. It takes the project's
native source path from the entity record, without deriving it from the title or ID. Package
sources and entities without sources remain separate cases. The original ID, source, URL and
detail survive the table's display-column selection. For a module action instead, pass
`$entity.id` as one argument to that module's declared command, retaining its configuration
and scope. Only `submit` may invoke the action; quitting has no side effect.

An entities query uses `id`, `label`, `detail`, `source`, and `url`; `source` is `null` or a
record with `root` and `path`. An ordinary JSON query defines its own fields and types.
The interactive `find` tool above requires a terminal; keep the plain query available for
pipelines and output files. Existing project files are read again at action time, so their
digest is an editing precondition only for that read, not for the earlier query.

`cli capture` takes a list of arguments and returns Nu's `stdout`, `stderr`, and `exit_code`.
It uses the current Tola executable and input scope; place `--config` where the selected CLI
command expects it. Pipeline input follows Nu external-command rules. Use this for finite
output; interactive editors and continuously running commands use ordinary external calls.

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
run no hooks. `rerun-on` names literal site-relative paths, not globs. For stage directories,
outputs, development reuse, and environment variables read `tola help "[build.hooks]"`.
Pure and offline modes restrict Tola's reads, not an external command's side effects.
For a Nu hook, use the finite scripts and explicit status handling in `tola skill scripts`;
keep its writes in the directories assigned by the hook stage.

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
