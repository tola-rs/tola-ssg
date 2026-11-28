# Tola authoring guide

| You want to | Read |
| --- | --- |
| Create a site, locate files, or set up an editor | `tola skill start` |
| Write content in Typst (headings, math, code, links) | `tola skill writing` |
| Configure metadata, pages, HTML/CSS, navigation, assets, or SEO | `tola skill site` |
| Install or update a theme or module | `tola skill modules use` |
| Use official modules (`resize-images`, `tailwind`) | `tola skill modules official` |
| Customize a theme | `tola skill modules theme customization` |
| Create a theme, library, query, or author command | `tola skill modules create` |
| Write a Nu script or interactive author tool | `tola skill scripts` |
| Run build hooks or generate extra outputs | `tola skill hooks` |
| Check, preview, publish, or vendor a site | `tola skill build` |

Browse sections on demand from any directory:

```sh
tola skill --list
tola skill writing math
tola skill site assets files
tola skill modules theme --all -o theme-guide.md
```

- `--list` prints available section paths.
- `--all` includes every subsection (or the complete guide with no section name).
- `-o FILE` writes the selected text without overwriting an existing file.
- Use `tola <command> --help` for CLI flags and `tola help` for full configuration, package, and script references.

When working on an existing site, read `tola.toml` and its selected `build.entry` first (or the theme's guide via `tola ALIAS --help`). For a module, start with `tola-module.toml`. Verify changes with `tola check`, and inspect rendered layout in a browser.

## Start

Read `tola.toml`, the selected site entry, and any files it imports before editing. Run `tola inspect config` to see the active configuration.

### Create

```sh
tola init my-site --dry-run
tola init my-site
cd my-site
printf '%s\n' '= Hello' > content/index.typ
tola dev
```

`init` requires a missing or empty directory (`--force` allows other existing files without overwriting scaffold files). It creates `tola.toml`, `site.typ`, `.gitignore`, `.ignore`, and `content/`. Open the URL printed by `tola dev` and save edits to see live rebuilds.

To scaffold from a theme or starter module, see `tola skill modules initializer`.

### Layout

Tola searches upward from the current directory for `tola.toml`; pass `-C path/to/tola.toml` to target another site. Paths in configuration are relative to `tola.toml`.

- `[build] entry` chooses the root site program: a project file path (default `site.typ`) or `{ module = "ALIAS" }` to use a module's entry.
- The configured content directory (default `content/`) holds source pages; keep shared templates and helpers outside it. Neither `content/` nor `site.typ` is a reserved name.
- To split large configurations across files, put a top-level `include = ["config/base.toml"]` before the first `[table]` header in `tola.toml`. Include paths resolve from the file that declares them, while site paths still resolve from the site root. Later values override earlier ones; asset and hook arrays append. See `tola help include`.

### Workflow

| Task | Command |
| --- | --- |
| Live preview with file watching | `tola dev` |
| Build and validate without touching published files | `tola check` |
| Preview production settings locally | `tola preview` |
| Publish the site to `build.publish-dir` | `tola build` |
| Inspect active settings, sources, fonts, or icons | `tola inspect <config\|sources\|fonts\|icons>` |
| Check configuration and local tools | `tola doctor` |

`check` and `preview` run build hooks (`before-build` and `generate-outputs`) without replacing published files; add `--no-hooks` to `tola check` to reuse already generated inputs. Run `tola ALIAS --help` to see a configured module's commands and guide.

### Editors

Run `tola editor setup --list`, then run the setup command for your editor. After upgrading Tola, run `tola editor packages` to refresh editor package files. Both `tola fmt` and `tola lsp` share the site's `[formatter]` settings.

## Writing

If the site uses a theme, follow its guide for metadata helpers, components, and options, importing from `@tola-modules/ALIAS:0.0.0`. Import Tola's built-in Typst packages with version `0.0.0` (for example `@tola/site:0.0.0`).

### Syntax

Typst is not Markdown or LaTeX: `=` starts headings, `$` enters math, and `#` enters code from markup. Write native Typst content for prose and `html.*` elements with CSS for web layout.

- **Modes**: `[...]` is content; `{...}` is code. Inside code, omit `#`; inside nested content, use `#` again. `[A *rich* title]` is markup, whereas `"A *rich* title"` is a plain string. Strings containing HTML render as text, not elements. Render arrays of elements with a loop or `.join()`.
- **Functions and chaining**: Named arguments use `name: value`, trailing `[...]` passes content, and `..values` spreads arguments. Wrap multiline expressions or method chains in parentheses or `{...}` — a top-level `.map(...)` on a new line is parsed as markup text.
- **Imports vs includes**: `import` loads definitions without inheriting the caller's locals; `include` inserts a file's content. Project `/...` paths start at the site root; inside a module or package, `/...` starts at that package root. Reading or including a file does not publish it.
- **Context**: Use `context` inside a document body for locations, counters, and document queries (`headings()`, `references()`, `current-document()`). Keep `all-sources()` and `tola-meta(...)` in ordinary code — placing `tola-meta(...)` inside `context` or a show rule will not declare source metadata.

### Headings

Use `#title[...]` for the page title and `= Section` for sections; style sizes with CSS rather than skipping heading levels.

```typst
#title[Guide]

#set heading(numbering: "1.1")
= Install <guide-install>
== Requirements

See @guide-install or #link(<guide-install>)[Install].
```

By default, `#title` exports to `<h1>`, `=` to `<h2>`, `==` to `<h3>`, and so on. `@guide-install` requires numbering, while `link(<guide-install>)[...]` does not.

To build an on-page table of contents inside a template's `document(...)` body, import `headings` from `@tola/document:0.0.0` (import specific members or alias the package so it does not shadow Typst's `document(...)`):

```typst
#import "@tola/document:0.0.0": headings
#context {
  let sections = headings(depth: 2).filter(h => h.outlined)
  html.nav(aria-label: "On this page")[
    #for section in sections [
      #link(section.location, section.body) \
    ]
  ]
}
```

When nesting included content, shift its headings locally with `#set heading(offset: heading.offset + 1)`. Because counters are shared across a site, place `#counter(heading).update(0)` at the start of a document if numbering should restart per page. See `tola help "@tola/document" headings`.

### Math

Write native Typst equations: `$x^2$` is inline, and spaces inside the delimiters (`$ x^2 $`) make a displayed equation. Quote ordinary prose words inside math (`$ x "where" x > 0 $`).

```typst
#set math.equation(numbering: "(1)")
$ E = m c^2 $ <energy-eq>
See @energy-eq.
```

Equation numbering applies to displayed equations; keep labels unique across the site, and reset per page with `#counter(math.equation).update(0)` if needed.

HTML export uses MathML by default:

- **MathML fonts**: Browsers render MathML with web fonts, not compiler fonts. Publish the math font file and load it via CSS `@font-face` (`tola skill site assets fonts`). Test long formulas at narrow widths and in both light and dark themes.
- **SVG fallback**: When a specific formula needs compiler-rendered artwork instead of MathML, wrap it in `html.frame`:

```typst
#show math.equation: eq => if eq.at("block", default: false) {
  block(html.frame(eq))
} else {
  box(html.frame(eq))
}
```

SVG equations use compiler fonts (`[typst.fonts]`) and give up selectable MathML structure, so re-check references and accessibility if you apply this rule.

### Code

Apply `render-code` as a `raw` show rule and include `code-stylesheet()` in the HTML `<head>` so token colors load in the browser:

```typst
#import "@tola/code:0.0.0": code-stylesheet, code-themes, render-code

#set raw(theme: code-themes.github)
#show raw: render-code.with(dark-theme: code-themes.github-dark)
```

- Set `raw.theme` where code blocks are rendered. Inline backticks have no language; use `raw("x + 1", lang: "rust")` for highlighted inline code.
- Dark token colors activate when the root element has `data-theme="dark"`. Your site owns the theme toggle and wrapper CSS. See `tola help "@tola/code" render-code code-stylesheet`.

### Links

Keep output paths, routes, and browser URLs distinct:

- `guide/index.html` is an **output path**; its **route** is `/guide/`. The route `/guide` refers to a file named `guide`.
- Pass a route to `output-of` (`@tola/address:0.0.0`) for `document(output, ...)`. Pass an output path to `output-url` (or a route to `route-url`) to produce a browser URL.
- By default, `about.typ` and `about/index.typ` suggest the same route `/about/`. Use `slugify` for lowercase hyphenated segments. To preserve URLs when moving files, read a custom metadata field such as `permalink` and reuse the resulting output path across templates, navigation, and feeds.

**Subpath deployment**: Configure `[site]` in `tola.toml`:

```toml
[site]
origin = "https://example.com"
base-path = "/docs/"
```

Keep output paths relative to the publish directory (`guide/index.html`). `output-url`, `route-url`, and `asset-url` prepend `base-path` automatically; pass `origin: site.origin` for an absolute URL. Handwritten HTML links must include the mount prefix themselves.

**Fragments**: Linking a native heading with `link(<guide-install>)[...]` emits its label as the HTML ID (or a generated `loc-N` if unlabeled). A label alone does not emit an ID unless something links to it. Keep labels unique across the site. For a stable external anchor that never depends on internal links, emit an explicit HTML ID (for example `#html.h2(id: "install")[Install]`), noting that raw `html.h2` elements do not join native `headings()`.

Run `tola check` to catch broken internal links (`tola check --json` prints structured routes and references). Absolute URLs are treated as external, including those starting with `site.origin`.

## Site

The site entry selects which sources to publish, maps each to an output path, and renders it through templates. Reuse the same validated list of page records for menus, feeds, and sitemaps so they never drift.

If `[build] entry` points to a theme module, configure it through options or a wrapper entry instead (`tola skill modules theme customization`).

### Metadata

Declare per-page metadata once near the top of a source file with `tola-meta`, outside `context` and show rules:

```typst
#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: "Hello", draft: false, published: "2026-09-01"))
```

Tola assigns no built-in meaning to fields like `draft` or `permalink`; your site entry defines and validates them with `@tola/schema:0.0.0`:

```typst
#import "@tola/schema:0.0.0": date, optional, schema
#import "@tola/source:0.0.0": all-sources, parse-sources

#let page-schema = schema((
  title: str,
  draft: optional(bool, default: false),
  published: optional(date),
))
#let sources = parse-sources(all-sources(), page-schema)
```

- Schemas reject unknown fields unless created with `unknown: "keep"`.
- `optional` accepts a missing key; `nullable` accepts explicit `none`. An invalid supplied value fails instead of falling back to a default.
- Prefer `parse-sources` over manual loops so errors report the offending source file.
- Use `date` (calendar day), `timestamp` (date and time; pass `.rfc3339` to feeds/sitemaps), `partial-date` (year or month), or `union(date, timestamp)`. See `tola help "@tola/schema" date timestamp partial-date`.
- Note that Tola still evaluates drafts during discovery, so syntax or runtime errors in drafts fail builds.

### Pages

`all-sources()` discovers lowercase `.typ` files under the content directory (excluding hidden names, symlinks, and the site entry), ordered by source path. Validate, filter, and sort explicitly:

```typst
#import "@tola/address:0.0.0": join-route, output-of
#import "@tola/site:0.0.0": site

#let pages = sources
  .filter(s => not s.meta.draft)
  .map(s => (source: s, output: output-of(join-route(s.route-segments))))

#for page in pages {
  document(page.output, format: "html", title: page.source.meta.title)[
    #include page.source.file
  ]
}
```

- Pass `page.source.file` directly to `include`. One source can feed multiple outputs (for example HTML and `format: "pdf"`), or be skipped.
- Inside an article, `current-source()` (`@tola/source:0.0.0`) returns its source path; inside `context`, `current-document()` (`@tola/document:0.0.0`) returns the current output path and route.
- **Build mode**: `site.build-mode` is `"dev"` under `tola dev`, `"prod"` under `tola build`/`check`/`preview`, and `none` in the editor, `tola query`, and `tola inspect`. Use it to trim work while writing (for example rendering only recent posts in `"dev"`) or toggle production-only scripts.
- **Languages**: Configure `language` (and optional `languages`) under `[site]` in `tola.toml`. `site.language` provides `tag`, `lang`, `script`, and `region` for `#set text(...)`; your site entry decides how to route and link localized pages.
- **Data-driven pages**: Read `json(...)` or `csv(...)` files and validate rows with `parse(...)` using the same schema. If a page is missing, compare `tola inspect sources` against `tola check --json`.

### HTML

Use typed `html.*` constructors for structure and CSS for layout (Typst page settings do not control CSS layout):

```typst
#html.aside(class: "note")[A reusable note.]
#html.elem("aside", attrs: ("data-kind": "tip"))[Custom attributes.]
```

- Typed constructors take documented attributes and one content body (void elements take none). Use `html.elem` for custom tags or attributes. Use `html.a` when you need raw attributes like `target` or `rel`.
- **Custom document shell**: Writing `#html.html[...]` gives you full control over `<head>` — document metadata is no longer inserted automatically. Always keep an explicit `html.head[...]` (so equation styles can attach) and supply title, viewport, stylesheets, and any SEO tags. Check footnotes, citations, and bibliographies in your custom shell, since a custom shell requires your own footnote placement rule.
- **Scripts and styles**: `html.script` and `html.style` take strings; an external script uses an empty string body: `html.script("", src: url, defer: true)`.
- `html.frame(...)` produces SVG artwork, not an HTML layout container, and cannot contain `set page`.

### Navigation

Build menus, prev/next links, and trees from your selected page records — never by re-scanning paths separately.

- **Ordering**: Add an `order` (or date) field to your schema and sort with a deterministic tie-breaker such as `(p.source.meta.order, p.source.id)`. Compute prev/next neighbors from this same ordered array (or from generated pagination records).
- **Hierarchy**: Use `@tola/collection:0.0.0` (`children`, `descendants`, `ancestors`, `lineage`, `adjacent`) when your site defines explicit parent keys. Keys must be unique and non-`none`, and the parent rule must terminate at `none`. See `tola help "@tola/collection" lineage descendants adjacent`.
- **Backlinks**: Label the article body `<article-body>` and render backlinks outside it using `references()` from `@tola/document:0.0.0`:

```typst
#import "@tola/document:0.0.0": references
#context {
  let seen = ()
  for r in references(to: auto, from-within: <article-body>) {
    if r.document.output in seen { continue }
    seen.push(r.document.output)
    link(r.document.location)[#r.document.route]
  }
}
```

For multi-step or recursive graph queries across site data, see `tola help "@tola/datalog" database`.

### Assets

Compiler input paths and browser URLs are separate: reading a file in Typst does not publish it. Declare browser-served files under `[assets]`, or generate derivatives with image helpers or hooks.

#### Files

Use `files` for single files and `trees` for directories in `tola.toml`:

```toml
[assets]
trees = [{ source = "static", url-prefix = "/assets", include = ["static/**/*.css"], exclude = ["static/drafts"] }]
files = [{ source = "static/robots.txt", url = "/robots.txt" }]
```

- `include` defaults to `["."]` (the whole `source` tree); `include = []` selects nothing; `exclude` defaults to empty. Patterns are relative to the site (or module) root and apply only within `source`.
- A matched `static/css/site.css` publishes at `/assets/css/site.css`. Set `follow-file-symlinks = true` on a tree entry to publish regular-file symlinks at the link's path.
- Reference declared assets with `asset-url` (`@tola/address:0.0.0`), passing the declared URL (not the input filename):

```typst
#import "@tola/address:0.0.0": asset-url
#html.link(rel: "stylesheet", href: asset-url("/assets/css/site.css"))
```

`asset-url` validates the declaration and prepends `site.base-path` (plus cache-busting if enabled). Tola does not bundle JavaScript or rewrite `url(...)` inside CSS; keep CSS resource paths relative to the published stylesheet. See `tola help "[assets]"`.

#### Fonts

Compiler fonts and browser fonts are configured separately:

- **Compiler fonts** (used for PDFs, `html.frame`, and SVG rasterization) come from `[typst.fonts]`; inspect them with `tola inspect fonts --json`. Pure builds (`--pure`) cannot use host system fonts, so keep required fonts inside the site or a module.
- **Module compiler fonts**: A module can ship fonts via `[typst] fonts = ["typst-fonts"]` in `tola-module.toml`. Sites enable them in `[typst.fonts].modules` and can rename a module's font family if two modules ship conflicting versions. See `tola help "[typst.fonts]"`.
- **Browser fonts** must be published as static assets (or hook outputs) and loaded with CSS `@font-face`.

#### Images

Choose the right path for each image:

- **Compiler-rendered content** (PDF or inline Typst graphics): native `image("...")`.
- **Published original**: declare under `[assets]` and render `html.img(src: asset-url(...))`.
- **On-demand derivatives and responsive `<picture>`/`srcset`**: use `@tola/image:0.0.0`. Pass an input path or raw bytes (never an asset URL); resolve relative paths with `path(...)` before handing them to helpers in another file. Returned URLs already include `base-path`. Supply `sizes` for non-full-width responsive images. `resize-image` requires an explicit raster format for SVG inputs; responsive helpers refuse SVG. See `tola help "@tola/image"`.
- **Batch resizing of `[assets]` images**: use the official `resize-images` module (`tola skill modules official`).

Keep private originals outside `[assets]` trees, since declared originals are always published even if you also create resized variants.

#### Icons

Declare icon collections under `[icons.collections]` in `tola.toml` (or in a module's `tola-module.toml`), inspect available IDs with `tola inspect icons`, and render via `@tola/icon:0.0.0`:

```typst
#import "@tola/icon:0.0.0": icon, icon-url
#icon("lucide:rocket")
#icon("lucide:rocket", label: "Launch site")
```

- Omit `label` for decorative icons; supply `label` for informative icons (screen readers).
- Inline SVG drawn with `currentColor` inherits surrounding CSS color; `icon-url(...)` images do not.
- Inside a module that declares its own icon collections, bind `icon.with(module: get-module().name)` so lookups target that module instance. See `tola help "[icons]"` and `tola help "@tola/icon"`.

### SEO

Reuse your selected page list for canonical links, feeds, and sitemaps, and configure the real `site.origin` and `site.base-path` before verifying output URLs.

- **Head tags**: `@tola/head:0.0.0` helpers (`head-metadata`, `canonical`, `open-graph`, `twitter-card`) return arrays of elements to render inside `html.head`. Pass `output-url(page.output, origin: site.origin)` for canonical URLs and `plain-text` for rich titles. Social preview images must also be published as assets or derivatives. See `tola help "@tola/head" canonical open-graph twitter-card`.
- **Feeds**: `@tola/feed:0.0.0` requires `site.origin` and a non-blank title. Pass `content: (document: page.output, id: "article")` to embed one unique HTML subtree (excluding site chrome); omitting `content` creates an excerpt-only entry. Keep entry IDs stable across route moves and format dates with `.rfc3339`. See `tola help "@tola/feed" feed`.
- **Sitemap**: `@tola/sitemap:0.0.0` lists whole documents with absolute URLs (skip generating it when `site.origin` is unset) and accepts calendar dates or timestamps. See `tola help "@tola/sitemap" sitemap`.

## Modules

A module is a directory with `tola-module.toml`. It can supply any combination of a Typst library, a site entry (theme), a Nu library, static assets, icons, compiler fonts, queries, CLI commands, hooks, or an initializer.

Each site assigns an instance alias under `[modules.ALIAS]`. Never hard-code an alias inside module code — one module can be mounted under multiple aliases with different options.

Start with `tola skill modules use` to install a module, `tola skill modules official` for official modules, `tola skill modules create` to write a library, or `tola skill modules theme --all` to build a theme.

### Use

Register each module under `[modules.ALIAS]` in `tola.toml` using `path`, `tarball`, or `git` as its sole source.

```toml
[build]
entry = { module = "notes" } # optional: only when the module supplies the site entry

[modules.notes]
path = "theme"
```

Content and templates import its Typst library from `@tola-modules/notes:0.0.0`, and configure it under `[modules.notes.options]`.

- **`path`**: Relative to `tola.toml` (or absolute; `--pure` requires paths inside the site).
- **`tarball`**: `{ url = "https://..." }` or `{ path = "downloads/theme.zip" }`, with optional 64-hex `sha256`. Supports `.tar.gz`, `.tar.zst`, and `.zip` with one outer directory containing `tola-module.toml` directly inside. Downloading does not run module build scripts, so use prepared release archives.
- **`git`**: `{ url = "...", tag | branch | rev, subdir = "..." }`. Pick at most one of `tag`, `branch`, or full-commit `rev` (omitting all selects the default branch). HTTPS uses your Git credential helper; SSH / `git@host:path` uses your SSH config and agent.

Keep `tola.lock` committed so builds use the same resolved inputs. Run `tola update ALIAS` after changing a source or to refresh a moving branch/archive (and `tola vendor --refresh` if vendoring).

| Flag | Effect |
| --- | --- |
| `--locked` | Keep `tola.lock` unchanged; locked remote inputs may still be fetched |
| `--offline` | Disallow network access; use already cached or local inputs |
| `--pure` | Use only locked site-local inputs and vendored copies; no host caches or system fonts |

See `tola help "[modules]"`.

### Official

Load the official modules from `tola-rs/modules` with `git`. Use `subdir` to select a module and commit `tola.lock` to pin the fetched revision.

#### Resize images

Resizes JPEG, PNG, and WebP images declared in `[assets]` and caches each size separately:

```toml
[assets]
trees = [{ source = "photos", url-prefix = "/gallery" }]

[modules.resize-images]
git = { url = "https://github.com/tola-rs/modules.git", subdir = "resize-images" }

[modules.resize-images.options]
include = ["photos/**"]
exclude = ["photos/drafts/**"]
widths = [480, 960] # format = "webp" (or "png", "jpg"), quality, upscale, jobs, dev

[modules.resize-images.options.overrides."photos/cover.jpg"]
widths = [1200]
```

Link generated variants with `output-url("images/<alias>/<source>/w<width>.<format>")` (for example `output-url("images/resize-images/photos/cover.jpg/w1200.webp")`). To resize a module's assets, set `source = { module = "notes" }`.

#### Tailwind

Compiles Tailwind CSS v4 from the HTML pages Tola renders:

```toml
[modules.tailwind]
git = { url = "https://github.com/tola-rs/modules.git", subdir = "tailwind" }

[modules.tailwind.options]
runner = "auto" # or e.g. ["bunx", "--no-install", "@tailwindcss/cli"]
minify = false
dev = "run"     # "run", "reuse", or "skip"
```

Install the Tailwind CSS v4 CLI. Automatic lookup checks the declaring module's `node_modules/.bin`, the site's `node_modules/.bin`, and `PATH`, in that order. Link the generated stylesheet in `<head>` with `output-url("_tola-modules/tailwind/tailwind.css")` using your module alias. Add `styles/tailwind.css` for custom CSS or `@source` directives. Set `toolchain` to identify dependencies that a lockfile does not cover.

### Create

A Typst library module needs only `tola-module.toml` and one file:

```toml
# theme/tola-module.toml
[typst]
entry = "lib.typ"
```

```typst
// theme/lib.typ
#let note(body) = html.aside(class: "note", body)
```

Register `[modules.notes] path = "theme"` in `tola.toml`, then import `#import "@tola-modules/notes:0.0.0": note`.

| Manifest entry | Purpose |
| --- | --- |
| `[typst] entry` | Public Typst library imported via `@tola-modules/ALIAS:0.0.0` |
| `[build] entry` | Site program selected by `build.entry = { module = "ALIAS" }` |
| `[nu] entry` | Public Nu library imported via `use tola-modules ALIAS` or run with `tola nu @tola-modules/ALIAS` |

These entries are independent. Validate a manifest and its declared files with `tola module check ./theme`, then test behavior from a real site.

### Theme

A complete minimal theme combines a manifest, a reusable Typst API (`lib.typ`), a thin site entry (`site.typ`), and a stylesheet:

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
    .filter(s => not s.meta.draft)
    .map(s => (source: s, output: output-of(join-route(s.route-segments))))
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

`theme/site.typ`:

```typst
#import "lib.typ": render-site
#render-site()
```

`theme/assets/theme.css`:

```css
body { margin: 0; font-family: system-ui, sans-serif; line-height: 1.6; }
.page { max-width: 48rem; margin: auto; padding: 2rem 1rem; }
```

A consuming site points `[build] entry = { module = "notes" }` at `theme/`, optionally overrides `[modules.notes.options]`, and writes pages with `#import "@tola-modules/notes:0.0.0": note`. Note that a custom `html.html` shell needs explicit SEO, code stylesheet, and footnote handling (`tola skill site html`, `tola skill site seo`).

#### Customization

- **Options and styles**: Expose everyday toggles in `[options]`. Sites can declare extra CSS in `[assets]` and pass its declared URL via `extra-styles`.
- **Structural overrides**: Switch `tola.toml` to `[build] entry = "site.typ"` and wrap the theme's public functions:

```typst
#import "@tola-modules/notes:0.0.0": article, render-site

#let with-banner(source) = [
  #html.aside(class: "notice")[Work in progress.]
  #article(source)
]

#render-site(article: with-banner)
```

Expose layout hooks as Typst function parameters rather than putting code or markup callbacks in TOML options.

### Options and paths

- **Merging**: `[options]` in `tola-module.toml` sets defaults; `[modules.ALIAS.options]` in `tola.toml` overrides them (tables merge recursively; arrays and scalars replace). Site-wide data belongs in top-level `[options]`, read as `site.options` (Typst) or `(get-site).options` (Nu).
- **Reading in Typst**: `get-module()` (`@tola/module:0.0.0`) inside module code reads the calling instance's `name`, `options`, and `assets`; project code passes an explicit alias (`get-module("notes")`).
- **Reading in Nu**: `get-module` returns the calling module instance in module commands, hooks, and public Nu entries (importing another Nu library does not change the calling instance; pass options explicitly to shared functions).
- **Manifest option references**: Fields in `tola-module.toml` can bind directly to merged module options using `{ option = "key" }`:

```toml
[options]
dev = "reuse"
watch = { include = ["styles/**"] }

[[hooks.generate-outputs]]
name = "build-css"
nu = "compile.nu"
dev = { option = "dev" }
rerun-on = { option = "watch" }
outputs = [{ file = "_tola-modules/{name}/site.css" }]
```

Option references work on hook `dev`, `order`, `rerun-on`, `args`, `generates`, `outputs`; asset `minify`, `include`, `exclude`; and command `args`, `projection`. The resolved value is still validated by the field's normal rules.

- **Paths**: Inside a module, `/...` resolves from the module root. Preserve `source.file` when including site content. To read a known site file (such as JSON produced by a hook) from module Typst code, use `project-path("data/file.json")` from `@tola/site:0.0.0`.

### Assets and icons

Module `[assets]` use the same `files` and `trees` rules as sites (`tola skill site assets files`), with paths relative to the module root.

For `source = "assets/theme.css"` and `url = "/theme.css"` under alias `notes` (with `base-path = "/docs/"`):

- Key in `get-module().assets.files` and `.urls`: `"theme.css"`
- Published URL: `/docs/_tola-modules/notes/theme.css` (with cache-busting query if enabled)

Use `assets.urls` for browser links and `assets.files` for Typst reads. To link a file generated by a module hook instead of `[assets]`, use `output-url(...)`.

- **Conditional assets**: Add `enabled-by = "search"` (referencing a boolean option) to an asset declaration. Ship the file even when the default is `false`; disabling an exact file entry never lets an overlapping tree republish it.
- **Site overrides**: A site can replace any selected module asset with a site-relative file while keeping its module URL, or disable all module assets:

```toml
[modules.notes.assets.replace]
"theme.css" = "styles/alternate.css"
# Or: enable = false
```

- **Module icons**: Declare `[icons.collections]` in `tola-module.toml` (local paths are module-relative) and bind `#let icon = icon.with(module: get-module().name)` in Typst so icon names stay scoped to that module instance.

### Queries

A named query evaluates a module-relative Typst file against the consuming site and reads one unique `<label>` metadata value:

```toml
[queries.pages]
entry = "queries/pages.typ"
label = "page-list"
```

```typst
// theme/queries/pages.typ
#import "../lib.typ": pages
#import "@tola/address:0.0.0": output-url

#metadata(pages().map(p => (
  title: p.source.meta.title,
  output: p.output,
  url: output-url(p.output),
))) <page-list>
```

Run `tola query notes.pages` (human table), `--json` / `--json --pretty`, or `-i` (interactive picker). You can also query any standalone file with `tola query ./model.typ --label model`. From Nu, call `use tola [query get-module]; query $"((get-module).name).pages"`. Queries do not run build hooks; prepare generated inputs beforehand.

**Editor support (completion, hover, go-to-definition, rename)**: Attach `references` and `definitions` to a query whose metadata returns an array of records with only these fields: `id` (unique nonempty string), `name` (string), and optional `description`, `source` (native Typst path), `url`:

```toml
[queries.index]
entry = "queries/index.typ"
label = "page-index"

[[queries.index.references]]
export = "page-link"
parameter = { positional = 0 }

[[queries.index.definitions]]
export = "page"
parameter = { named = "id" }
```

`export` names a function exported by `[typst] entry` (dot-separated for nested members); `positional` is zero-based and `named` matches a named parameter.

### Commands

Expose author tools under `[commands.NAME]` in `tola-module.toml`, invoked as `tola ALIAS NAME ...`. Each command declares exactly one of `nu`, `argv`, `query`, or `skill`:

```toml
[commands.greet]
nu = "tools/greet.nu"

[commands.pages]
query = "pages"
projection = ["title", "url"]

[commands.guide]
skill = "GUIDE.md"
help = "Read the theme guide."
```

- `nu` / `argv` commands run at the site root; optional `args` prepend user arguments and support `{module}`, `{project}`, and `{tola}` placeholders. Nu command help comes from comments above `def main` (overridable via `help`).
- `skill` serves a module Markdown guide using the same section navigation (`--list`, `--all`, `-o`) as `tola skill`.
- External `argv` tools can read invocation paths and effective `module.options` via `"$TOLA_EXECUTABLE" nu @tola-scripts/context --json` (see `tola help @tola-scripts/context`).

**Diagnostics and reports**:

- **From Typst** (`@tola/diagnostic:0.0.0`): Report non-fatal warnings or errors during compilation without aborting evaluation immediately:

```typst
#import "@tola/diagnostic:0.0.0": report
#report("Missing summary.", severity: "warning", code: "missing-summary",
  hints: ("Add a `summary` field to metadata.",))
```

- **From Nu hooks and commands** (`use tola diagnostic`): Submit one batch per invocation (`[]` counts as a submission; an `error` fails the Tola command after the script exits; omit `location` in build hooks and module commands):

```nu
use tola diagnostic
[
  (diagnostic make "Some entries have no summary" --code missing-summary
    --hints ["Add a summary to the listed entries."])
] | diagnostic submit
```

Standalone scripts can render diagnostics with `| diagnostic render` (with optional file `location`), or pipe JSON to `tola nu --stdin @tola-scripts/diagnostic render`. External hooks/commands write version 1 JSON to `reportPath` from context. See `tola help "@tola/diagnostic" report` and `tola help @tola-scripts/diagnostic`.

### Initializer

Declare `[init] nu = "tools/init.nu"` (or `argv`) to let users scaffold sites with `tola init my-site --from <path|git-url|archive> --alias notes [-- --script-args]` (or `tola notes init new-site` from an existing site):

```nu
use tola get-init

def main [] {
  let init = get-init
  {
    build: {entry: {module: $init.module.name}}
    modules: {($init.module.name): {path: $init.module.root}}
  } | to toml | save --raw ($init.output-dir | path join "tola.toml")
  mkdir ($init.output-dir | path join "content")
  [
    $'#import "@tola-modules/($init.module.name):0.0.0": note'
    '#note(title: "Welcome")'
  ] | str join (char nl) | save --raw ($init.output-dir | path join "content" "index.typ")
}
```

- Write all generated files under `(get-init).output-dir` (a disposable staging directory, even for `--dry-run`); use `target-dir` only when computing relative paths.
- Tola rewrites the initializer's own module declaration to preserve the original local path, Git selector, or archive URL and locks all generated remote dependencies into `tola.lock`.
- `get-site` and `get-module` are unavailable during initialization; inspect `(get-init).interactive` before prompting. Remote `--from` flags (`--branch`, `--tag`, `--rev`, `--subdir`, `--offline`, `--locked`) follow the same rules as `tola skill modules use`.

### Releases

Prepare a clean release directory with `[pack]` and optional `[[hooks.before-pack]]` in `tola-module.toml`:

```toml
[pack]
include = ["lib.typ", "site.typ", "assets", "queries", "tools"]
exclude = ["assets/**/*.map"]

[[hooks.before-pack]]
name = "compile-assets"
nu = "author/assets.nu"
```

In `before-pack`, read the module source directory via `get-context | get hook.inputDir` and write compiled assets into the module before packing. Then pack, check, test, and archive:

```sh
mkdir -p dist
tola module pack ./theme --output ./dist/theme
tola module check ./dist/theme
tar -czf dist/theme-v1.0.0.tar.gz -C dist theme
```

- `tola module pack` runs `before-pack`, copies selected files (always including the manifest, stripping `[pack]` and `before-pack` so consumers never run author build tools), and replaces the output directory atomically on success.
- Before publishing, test `dist/theme` from a real site with an overridden option, a renamed alias, and a non-root `site.base-path` (and without author-only tools installed).
- Compress the directory itself so the archive has one outer directory containing `tola-module.toml` (`.tar.gz`, `.tar.zst`, or `.zip`), and attach it to a versioned release URL (or commit the packed directory and point consumers at `git.subdir`).

## Scripts

Tola bundles Nushell so author tools, data scripts, and build hooks run identically without a separate Nu installation. Browser interactivity still uses JavaScript.

```nu
# Greet someone by name.
def main [name: string, --upper] {
  let msg = $"Hello, ($name)!"
  if $upper { $msg | str upcase } else { $msg }
}
```

Run it with `tola nu greet.nu "Tola author" --upper`, expose it as a module command (`tola skill modules commands`), or run it during a build (`tola skill hooks`).

### Runtime

`tola nu` runs a `.nu` file, a built-in `@tola-scripts/NAME` entry, or a module's `@tola-modules/ALIAS` entry (when `[nu] entry` is declared in `tola-module.toml`).

- Place Tola flags (`--config FILE`, `--stdin`) **before** the entry, and script arguments **after** it. (`tola nu` does not support standalone Nu's `-c` flag.)
- Personal Nu startup configs and external Nu plugins are not loaded; the Nu standard library (`use std/assert`) is available. Run `tola nu --help` to list available libraries.
- Pass piped stdin with `--stdin` and decode explicitly (`$in | from json`).
- Disk scripts keep the caller's working directory and discover nearby `tola.toml`; module commands and `@tola-modules/ALIAS` entries run at the site root. None of these run build hooks.
- A module with `[nu] entry = "tools/mod.nu"` exports functions imported via `use tola-modules ALIAS` (add `export def main` only if it should also be directly runnable via `tola nu @tola-modules/ALIAS`).

### Libraries

| Import | Purpose |
| --- | --- |
| `use tola get-site` | Site settings, options, and paths |
| `use tola get-module` | Current module instance (in module contexts) or `get-module ALIAS` |
| `use tola query` | Evaluate a module query (`alias.name`) or standalone Typst query file |
| `use tola get-context` | Full invocation record (paths, options, stage, `reportPath`) |
| `use tola get-hook` | Current hook's stage/directories, or `get-hook --rerun-on-digest` |
| `use tola get-assets` | Configured asset snapshot in post-discovery hooks (`--module`, `--include`, `--exclude`) |
| `use tola get-init` | Initializer source and output/target directories |
| `use tola diagnostic` | `make`, `submit`, and `render` diagnostics |
| `use tola terminal` | Interactive table picker (`pick`) |
| `use tola syntax-highlight` | Terminal syntax highlighting for previews |
| `use tola image` | Image `metadata` and `resize` (supports raster and SVG inputs) |
| `use tola-modules ALIAS` | Public Nu exports of a configured module |

For example, resize or rasterize an image without external tools (`tola help "@tola-scripts/image"`):

```nu
use tola image
image resize photo.png --output thumb.webp --width 640 --format webp
```

One dimension preserves aspect ratio; two fill and crop (`--anchor` controls crop). The output directory must exist, existing files are not overwritten, and creating a file does not publish it unless declared as an asset or hook output.

### Conventions

Keep Tola Nu scripts clean, portable, and side-effect-safe:

- **Structured data & files**: Keep values as records/tables until serialization (`to json` / `to toml`). Use `open --raw` for Typst/text templates and `to json | save --raw $dest` to write exact bytes without double conversion; use `save --force` only for intentional overwrites.
- **Arguments & `main`**: Typed `def main` parameters generate `--help` automatically. Keep top-level code free of writes so `--help` has no side effects, and never call `main` manually. Send progress messages with `print --stderr` so stdout stays clean data. Pass `--` before positional values starting with `-`.
- **Subprocesses & errors**: Invoke external commands with `^$program ...$args` (never string-joined shell lines). Inspect exit status via `let r = (^$prog ...$args | complete)` before using `$r.stdout`. Call Tola recursively through `$env.TOLA_EXECUTABLE`, forwarding `--config` and matching `--offline`/`--pure` when `$env.TOLA_INPUT_SCOPE` is `offline` or `pure`.
- **Paths & modules**: Build paths with `path join`. Relative `open` uses `$env.PWD`; relative `use` follows the importing file (import paths must be constant). Use `$env.FILE_PWD` or `path self` for resources bundled beside a script or library, and `(get-site).root` for site files.
- **Testing**: Test reusable Nu helpers with `use std/assert` and `tola nu test.nu` against the same `tola` binary your users run. Use a temporary site directory when testing file-modifying scripts.

### TUI

Use `terminal pick` for interactive author commands (never inside unattended build hooks):

```nu
use tola [terminal syntax-highlight]

let rows = [
  {name: "Guide", file: "content/guide.typ"}
  {name: "Notes", file: "content/notes.typ"}
]

let picked = (
  $rows
  | terminal pick --columns [name] --fuzzy
      --preview {|row| open --raw $row.file | syntax-highlight typst }
)
if ($picked | is-not-empty) { $picked | first }
```

`pick` returns a list of the original records (preserving hidden fields like `source`), or `[]` when cancelled or empty. Pass `--multi` to select multiple rows with Space. Keep previews strictly read-only, and keep a non-interactive query command available for scripts.

## Hooks

Hooks run external tools or Nu scripts at fixed stages of a build. Every hook runs once and exits; Tola owns file watching and rebuilds.

### Overview

| Stage | Purpose | Runs in `build` | Runs in `check`/`preview` | Default in `dev` |
| --- | --- | --- | --- | --- |
| `before-build` | Generate Typst sources, data, or asset inputs | Yes | Yes | `"run"` |
| `generate-outputs` | Read compiled pages/assets and add final outputs | Yes | Yes | `"run"` |
| `after-publish` | Act on the published directory (e.g. deploy) | Yes | No | `"skip"` |

Module release preparation uses `before-pack` (`tola skill modules releases`). Queries, `tola nu`, `tola inspect`, and `tola vendor` never run build hooks.

Declare hooks under `[[hooks.STAGE]]` in `tola.toml` or `tola-module.toml` with a `name` and exactly one action: `nu = "path/to/script.nu"` (plus optional `args`) or `argv = ["tool", "arg", ...]`. `argv` runs directly with no shell or glob expansion. Runtime hooks run with cwd at the site root; Nu script paths resolve from the declaring site or module.

- **Toggling**: Set `enable = false`, or `enabled-by = "search.enable"` to read a boolean from site `[options]` or effective module options (quote keys containing dots, e.g. `'"search.enable"'`). Module hooks also support `{ option = "key" }` field bindings (`tola skill modules options-and-paths`), `{name}` in generated/output/rerun paths, and `{module}`/`{project}`/`{tola}` in arguments.
- **Ordering & concurrency**: Within each site or module instance, lower `order` runs first (default `0`); equal orders may run concurrently (limit with `[build] hook-jobs` or `par-hooks = false`). `before-build` runs site hooks before module hooks; `generate-outputs` and `after-publish` run module hooks before site hooks. Inspect effective order and reuse status with `tola inspect config --mode dev --json --pretty`.
- **Context & diagnostics**: Nu hooks read invocation data with `use tola [get-context get-hook]` and report issues via `diagnostic submit`. External tools call `"$TOLA_EXECUTABLE" nu @tola-scripts/context --json`. See `tola help "[hooks]"` and `tola help @tola-scripts/context`.

### Before build

Generate files that Typst or `[assets]` will read (outside `build.publish-dir`), and list them in `generates`:

```toml
[[hooks.before-build]]
name = "local-data"
nu = "tools/data.nu"
dev = "reuse"
rerun-on = { include = ["data/**/*.json"] }
generates = ["generated/data.json"]
```

Paths in `generates` are relative to the site root and must exist when the hook completes. Generating a file makes it available to Typst or `[assets]`; it does not publish it automatically. See `tola help "[[hooks.before-build]]"`.

### Generate outputs

Read compiled pages and assets from `hook.inputDir` (or `(get-hook).input-dir`) and write declared final files into the empty `hook.outputDir` (`(get-hook).output-dir`):

```toml
# In a module's tola-module.toml
[[hooks.generate-outputs]]
name = "search"
nu = "runtime/search.nu"
outputs = [{ file = "_tola-modules/{name}/search.json" }]
```

```nu
use tola [get-hook get-module query]

def main [] {
  let mod = get-module
  let dest = (get-hook).output-dir | path join "_tola-modules" $mod.name "search.json"
  mkdir ($dest | path dirname)
  query $"($mod.name).pages" | to json | save --raw $dest
}
```

- Declare each output as `{ file = "path" }` or `{ tree = "dir" }` (relative to the publish root, without `base-path`). Every declared path must be written, undeclared files are rejected, and outputs cannot overwrite other site outputs.
- Link hook outputs from Typst with `output-url(...)` (`@tola/address:0.0.0`); they do not need `[assets]` entries and are not minified or hashed automatically.
- All `generate-outputs` hooks read the same compiled snapshot. If step B depends on step A's output, put both in one script.
- Use `use tola get-assets` to inspect declared static assets (`source`, `output`, `media-type`, filterable by `--module`, `--include`, `--exclude`), `hook.cacheDir` for persistent caches, and `hook.tempDir` for disposable scratch files. See `tola help "[[hooks.generate-outputs]]"`.

### After publish

Read the freshly published site from `hook.inputDir` (`(get-hook).input-dir`), for example to sync or validate deployed files. This stage cannot add files to the site. `tola build` waits for it to finish; an error reports failure while leaving the published files intact.

With `dev = "run"`, `after-publish` runs asynchronously against the latest dev output without blocking browser reloads (intermediate edits may be coalesced). See `tola help "[[hooks.after-publish]]"`.

### Development

Production builds always run enabled hooks. In `tola dev`, each hook obeys its `dev` setting (`"run"`, `"reuse"`, or `"skip"`):

- **`rerun-on`**: Tola tracks a hook's `nu` entry script automatically. Declare every other file it reads under `rerun-on = { include = [...], exclude = [...] }`. Patterns resolve from the declaring site or module; module hooks match site files with `{ project = "content/**/*.typ" }`. Broad patterns do not match VCS directories — list `.git` or `.jj` paths explicitly if needed. For a file symlink, name the link path directly; for a directory symlink, list the target directory.
- **`dev = "reuse"`**: Supported on `before-build` (with nonempty `generates`) and `generate-outputs`. Within a `tola dev` session, Tola skips rerunning the hook and keeps its last diagnostics while tracked inputs, config, and outputs remain unchanged. Use `dev = "run"` if a hook depends on time, network, environment variables, or untracked external state. Pass `tola dev --no-reuse` to force reruns.
- **Cross-run caching**: Nu hooks can call `get-hook --rerun-on-digest` to include a digest of declared `rerun-on` inputs in their own `hook.cacheDir` cache key (returns `null` if an untrackable path is matched).

## Build

Use `tola check` to validate without overwriting published files, `tola dev` for live editing, `tola preview` for a local production preview, and `tola build` to publish.

### Checks

- `tola check`: Full build and link/output validation. Add `--json` for structured pages, routes, outputs, and references; `--no-hooks` to reuse existing inputs; `--no-build` for fast syntax-only parsing of owned sources.
- `tola inspect <config|sources|fonts|icons>`: Read-only inspection (supports `--json`).
- `tola query alias.name`: Run a module query without running hooks or publishing.
- `tola fmt --check` / `tola fmt`: Verify or apply formatting.
- `tola doctor`: Verify configuration, hooks, and external tools. Add `-v` or `--log-file FILE` for detailed diagnostics.

If a build reports that a file changed while running, finish saving edits and re-run.

### Development events

Run `tola dev --json` when automating against live builds. Stdout emits one JSON object per line:

- `server-ready`: local preview URL
- `build-finished`: build outcome (a failed rebuild may keep serving the previous successful site)
- `hook-finished`: result of `after-publish` work

In Nu, parse the stream with `lines | each {|line| $line | from json }`.

### Publish

`tola build` atomically replaces `build.publish-dir`, removing stale outputs — keep hand-maintained files elsewhere. Verify with your real `site.origin` and `site.base-path` via `tola preview`, then deploy the contents of `build.publish-dir`. Configure your web host to serve `index.html` for directories and `404.html` for missing pages.

### Vendor

To make a site buildable with zero network access or host caches:

1. Configure `[vendor] path` in `tola.toml`.
2. Preview with `tola vendor --dry-run`, then run `tola vendor` (refresh later with `tola vendor --refresh`).
3. Verify with `tola build --pure`.

Vendoring freezes selected packages and remote icon collections, not compiler fonts — keep required fonts inside the site or a module. See `tola vendor --help` and `tola help "[vendor]"`.
