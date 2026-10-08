# tola-ssg
[ **English** | [中文](./README-zh.md) ]

A static site generator built on Typst Bundles. Write your content, templates, and site program in Typst; produce HTML pages, PDF/SVG/PNG documents, and generated files from one site.

```sh
tola init my-blog
cd my-blog
tola build
tola dev
```

The bundled guide and runnable examples follow the installed executable:

```sh
tola help -i
tola help package source
tola help package document headings
tola help config build
tola help demo backlinks --preview
tola help demo sources --export ./sources-site
```

The export directory must not already exist, and its parent must exist. Add `--edit` to open the exported source with `TOLA_EDITOR`, `VISUAL`, or `EDITOR`; use `--editor` for one invocation. `tola skill` prints the site-authoring guide.

## Table of Contents

- [Showcase](#showcase)
- [Features](#features)
- [Usage](#usage)
- [Installation](#installation)
- [Community](#community)
- [Note](#note)
- [Acknowledgements](#acknowledgements)

## Showcase

> Yeah, my blog is also built with `tola`.

| Site | Description |
|------|-------------|
| [kawayww.com](https://kawayww.com) | Author's personal blog |
| [example-sites](https://tola-rs.github.io/example-sites/) | Official example collection |

**My site ([kawayww.com](https://kawayww.com))**

| | |
|:---:|:---:|
| <img src="screenshots/home-0.webp" width="100%"> | <img src="screenshots/home-1.webp" width="100%"> |
| <img src="screenshots/home-2.webp" width="100%"> | <img src="screenshots/home-3.webp" width="100%"> |

**Starter Template** ([example-sites/starter](https://tola-rs.github.io/example-sites/starter))

| | |
|:---:|:---:|
| <img src="screenshots/starter-0.webp" width="100%"> | <img src="screenshots/starter-1.webp" width="100%"> |

## Features

- **One site program** — A root Typst Bundle emits every document and asset. Sources can appear in several documents, and native queries can read across the Bundle.
- **Typed source metadata** — Declare metadata with `tola-meta(...)`, keep Typst content, dates, and functions, and validate your site's fields with `@tola/schema`.
- **Document navigation** — Read the current document, headings, and references to build tables of contents and backlinks.
- **Assets and media** — Publish declared files and trees, resize images, use icon collections, and configure fonts.
- **Development feedback** — `tola dev` watches site inputs and updates the browser. Changes that cannot be patched safely trigger navigation.
- **Site tools** — Check and inspect sources, documents, routes, outputs, and references; use the bundled package guide and editor integration.
- **Editable templates** — Scaffold metadata selection, custom routes, feeds, sitemaps, canonical links, Open Graph, and Twitter Cards in ordinary Typst code.
- **Build integration** — Run before-build, output-generation, and after-publish hooks; select Tailwind CSS or Pagefind scaffolding when creating a site. Optional SPA navigation uses DOM morphing and view transitions.
- **Offline inputs** — Freeze build-selected packages, fonts, and icon data with `tola vendor`, then verify the site under `--pure`.

## Usage

Run `tola --help` or `tola <command> --help` for command options. Tola finds `tola.toml` by searching the current and parent directories.

### Sources and documents

A site starts at `build.entry`, normally `site.typ`. Tola discovers `.typ` sources under `build.content-dir`, normally `content/`, and collects their declarations. The root program chooses which sources to include and which output paths to publish.

```text
.
├── tola.toml
├── site.typ                 # Root Bundle program
├── content/                 # Content sources
│   ├── index.typ
│   └── posts/hello.typ
├── site/                    # Editable templates, metadata schema, and selection
│   ├── page.typ
│   ├── schema.typ
│   └── selection.typ
└── static/                  # Files used by the site or declared as assets
```

Declare metadata in a content source:

```typst
#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: [Hello *Typst*], date: datetime(year: 2026, month: 10, day: 8)))

= Hello
This is the source's body.
```

A small `site.typ` can publish the discovered sources:

```typst
#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": route, route-to-output

#for source in all-sources() {
  let output = route-to-output(route(source.route-segments))
  document(output, format: "html", title: source.meta.title)[
    #include source.file
  ]
}
```

The scaffold adds metadata validation, draft selection, and slugged routes in `site/selection.typ`. Fields such as `title`, `draft`, and `permalink` follow that editable site's schema. A source is an input; `document(...)` decides what is published. The same source can produce several pages, appear in a PDF, or be left out of the output.

Use `asset("data.json", bytes(json.encode(data)))` in the root program to emit a generated file. Document output paths determine routes: `notes/index.html` is reached at `/notes/`; `site.base-path` adds a deployment prefix to browser URLs. Native `query(...)` keeps its Bundle-wide meaning; `@tola/document` supplies queries scoped to a document.

### Bundled packages

| Package | Use it for |
|---------|------------|
| `@tola/source:0.0.0` | `all-sources()`, `current-source()`, `tola-meta(...)`, `parse-sources(...)` |
| `@tola/document:0.0.0` | `current-document()`, `headings(...)`, `references(...)`, inside `context` |
| `@tola/site:0.0.0` | The resolved `site` configuration |
| `@tola/address:0.0.0` | Routes, output paths, browser URLs, slugging, and `asset-url(...)` |
| `@tola/schema:0.0.0` | Validate and resolve your metadata values |
| `@tola/collection:0.0.0` | Select, group, index, and navigate arrays using your own keys and hierarchy |
| `@tola/web:0.0.0` | Head metadata, canonical links, social cards, feeds, sitemaps, and SVG math |
| `@tola/icon:0.0.0` | Inline icons and published icon URLs |
| `@tola/image:0.0.0` | Image metadata and resized image outputs |
| `@tola/code:0.0.0` | Code highlighting and stylesheets |

`tola help package <name>` gives the current signatures and examples. `tola help demo` lists complete sites covering sources, backlinks, headings, media, feeds, and multiple outputs.

### Configuration

```toml
[site]
title = "My Blog"
origin = "https://example.com"
base-path = "/"
language = "en"

[build]
entry = "site.typ"
content-dir = "content"
publish-dir = "public"

[assets]
trees = [{ source = "static/web", url-prefix = "/assets" }]

[typst.fonts]
paths = []
system = false

[vendor]
path = "vendor"
```

Create `static/web` before declaring that asset tree. Fonts from declared paths and the bundled fonts remain available when system font discovery is disabled. `tola init --dry-run` shows the scaffold without writing it; `tola help config` describes the configuration tables.

Shared templates and helpers can live anywhere inside the site, outside `content/`. Tola observes their imports and file reads. A source or helper change can affect several documents through imports or site-wide queries.

### Build, check, and develop

```sh
tola build                  # Publish the complete site
tola check                  # Check without publishing
tola inspect sources        # Declared source metadata as JSON
tola inspect documents      # Built HTML documents as JSON
tola inspect references     # Links and resource resolution as JSON
tola dev                    # Rebuild and serve while editing
tola preview                # Build once and serve a preview
```

The production build checks the complete output set before replacing `public/`. A failed build leaves the previously published site intact. Keep hand-maintained files in declared asset inputs rather than the published directory.

Long-running development reuses source and Typst computation caches. Image derivatives can be reused from disk. These caches preserve the outputs and diagnostics of the complete Bundle compilation.

### Offline and vendored inputs

```sh
tola build --offline
tola vendor --dry-run
tola vendor
tola build --pure
```

`--offline` disables Tola's network access while allowing configured host inputs and caches. `--pure` also excludes host package roots, system fonts, and file reads outside the site, including symlinks that lead outside it.

`tola vendor` freezes the dependencies the build selects and verifies the prepared inputs with a pure build before replacing the vendor tree. Vendoring skips build hooks. Existing vendored packages take precedence over host package roots; `tola vendor --refresh` selects them again from the other available roots. `--dry-run` verifies without replacing the vendor tree.

## Installation

### Cargo

```sh
cargo install --locked tola
```

### Binary Release

Download from the [release page](https://github.com/tola-rs/tola-ssg/releases).

### Nix Flake

The flake builds Tola for Linux and macOS. Add it to your flake inputs:

```nix
inputs.tola = {
  url = "github:tola-rs/tola-ssg";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

Install `inputs.tola.packages.${pkgs.system}.default`; Linux also provides `.static`. The [Cachix cache](https://tola.cachix.org) can reuse available builds:

```nix
nix.settings = {
  substituters = [ "https://tola.cachix.org" ];
  trusted-public-keys = [ "tola.cachix.org-1:5hMwVpNfWcOlq0MyYuU9QOoNr6bRcRzXBMt/Ua2NbgA=" ];
};
environment.systemPackages = [ inputs.tola.packages.${pkgs.system}.default ];
```

For Typst packages inside a Nix sandbox:

```nix
inputs.tola.packages.${pkgs.system}.default.withPackages (ps: [ ps.metalogo ])
```

This supplies `TYPST_PACKAGE_CACHE_PATH`, a host package cache usable offline. A pure site build uses site-owned or vendored inputs. Tola embeds the Typst compiler and does not require the Typst CLI.

## Community

- Matrix: [`#tola:matrix.org`](https://matrix.to/#/#tola:matrix.org)
- QQ: `1065579014`

## Note

> **Early development & experimental HTML export**

`tola` is usable but evolving — expect breaking changes and rough edges. Feedback and contributions are welcome!

HTML and paged documents use different output models. Page geometry and positioned layout do not automatically carry over to HTML; use HTML elements and CSS for browser layout, or embed a rendered frame where you need a paged result. `tola help package web math-svg` explains SVG math for HTML, and `tola help demo multiple-outputs` shows several outputs from one source. Typst's HTML and Bundle targets are experimental.

## Documentation

- Run `tola --help` and `tola <command> --help` for CLI usage
- See [tola-rs/example-sites](https://github.com/tola-rs/example-sites) for examples and source code
- Open an issue if you have any question

# Acknowledgements

- [typsite](https://github.com/Glomzzz/typsite): Static site generator(SSG) for typst
- [kodama](https://github.com/kokic/kodama): A Typst-friendly static Zettelkästen site generator.
- [tinymist](https://github.com/Myriad-Dreamin/tinymist): parts of `tola-lsp` are adapted from it (Apache-2.0)

## License

MIT
