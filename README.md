# tola-ssg

A static site generator for Typst-based blogs.  \
There would be huge change in v0.7.x: vdom diff, live reloading, custom search function, interactive and useful cli

## Table of Contents

- [Showcase](#showcase)
- [Features](#features)
- [Philosophy](#philosophy)
- [Installation](#installation)
- [Usage](#usage)
- [Roadmap](#roadmap-v070)
- [Note](#note)

## Showcase

> Yeah, my blog is also built with `tola`.  \
> There are example site collection: https://tola-ssg.github.io/example-sites

[my blog](https://kawayww.com):
![home](/screenshots/home.avif)

[example site collection](https://tola-ssg.github.io/example-sites/):
![example](/screenshots/example.avif)

Thanks to `typst`, `tola` offers writing flexibility.  \
e.g.: Implement `Recent Posts` easily and customizable (as shown in the image above):

```typst
// Data Architecture: Tola provides virtual JSON files for site metadata.
// Why? This gives you full control over how to list and filter content.
// Available virtual files:
// - "/_data/pages.json": List of all pages with their metadata (url, title, date, etc.)
// - "/_data/tags.json": Map of tags to the pages that use them
// - ...more in the future!

#import "/utils/helpers.typ" as utils

#let pages = json("/_data/pages.json")
#let posts = (pages
  .filter(p => "/posts/" in p.url)
  .filter(p => p.at("draft", default: false) == false)
  .sorted(key: p => p.date)
  .rev())

#html.div(class: "space-y-6")[
  #for post in posts.slice(0, calc.min(posts.len(), 5)) {
    utils.post-card(post)
  }
]
```

## Features

### Performance
(There would be a huge change/refactor in future v0.7.x)

- **font caching** — Fonts loaded once at startup, shared across all compilations
- **file caching** — Only re-read changed files, zero-cost for unchanged files
- **parallel compilation** — Build pages and assets concurrently using rayon
- **incremental rebuilds** — Intelligent dependency tracking for sub-second hot reload
  - **content**: Direct rebuild of changed file
  - **deps (templates/utils)**: Reverse-lookup of dependencies to rebuild only affected pages
  - **config**: Full rebuild to ensure consistency across the site

When a template or shared file changes, `tola` uses its in-memory dependency graph to determine the minimal rebuild set:

```text
DependencyGraph
├── forward: content.typ → {template.typ, utils/a.typ, ...}
└── reverse: template.typ → {page1.typ, page2.typ, ...}
```

This ensures that editing a utility function used by 5 pages only rebuilds those 5 pages, not the entire site.

### Development Experience

- **smart watch mode** — Robust file watching strategy:
  - **debouncing (300ms)**: Batches rapid file events (e.g. "Save All") into single builds
  - **cooldown**: Prevents build thrashing during compilation
  - **error recovery**: Server stays alive on build failures; just fix and save to recover
- **local server** — Built-in HTTP server with directory listing and clean URLs
- **auto config discovery** — Run `tola` from any subdirectory; it finds `tola.toml` automatically
- **graceful error handling** — Human-readable diagnostic messages from Typst

### Content Processing

- **svg extraction & optimization** — Extract inline SVGs, adjust viewBox, compress to SVGZ
- **dark mode svg adaptation** — Auto-inject CSS for SVG theme adaptation (enabled by default)
- **html/xml minification** — Optional minification for production builds
- **url slugification** — Configurable slug modes (full, safe, ascii, no) with case options
- **typst package support** — Uses standard Typst package registry with shared cache

### Site Generation

- **rss-2.0 support** — Auto-generate `feed.xml` from page metadata
- **sitemap support** — Auto-generate `sitemap.xml` for search engines
- **tailwind-css support** — Built-in support, out of the box
- **github pages deployment** — One-command deploy (or use GitHub Actions)

## Philosophy

> **Keep your focus on the content itself.**

`tola` is built around three core principles:

### Minimal Abstraction

`tola` provides a thin layer over Typst — just enough to handle the boring stuff (routing, live reload, local server, incremental rebuild smartly) without locking you into a rigid framework. Your Typst code stays portable.

### Typst First

If Typst can do something easily, use Typst. `tola` doesn't reinvent the wheel — it leverages Typst's powerful markup and scripting capabilities.

### Developer Joy

- **zero config to start** — `tola init <SITE-NAME>` gets you running in seconds
- **fast feedback loop** — Incremental rebuilds keep iteration snappy
- **escape hatches** — Full access to HTML/CSS/JS when you need it
- **predictable output** — What you write is what you get

## Installation

### Cargo

```sh
cargo install tola
```

### Binary Release

Download from the [release page](https://github.com/tola-ssg/tola-ssg/releases).

### Nix Flake

A `flake.nix` is provided in the repo. Pre-built binaries are available at [tola.cachix.org](https://tola.cachix.org).

**Step 1**: Add tola as an input in your `flake.nix`:

```nix
{
  inputs = {
    tola.url = "github:tola-ssg/tola-ssg/v0.6.5";
    # ...
  };
}
```

**Step 2**: Configure cachix in your `configuration.nix`:

```nix
{ config, pkgs, inputs, ... }:

{
  nix.settings = {
    substituters = [ "https://tola.cachix.org" ];
    trusted-public-keys = [ "tola.cachix.org-1:5hMwVpNfWcOlq0MyYuU9QOoNr6bRcRzXBMt/Ua2NbgA=" ];
  };

  environment.systemPackages = [
    # 1. Native build (recommended if you want to build from source)
    # inputs.tola.packages.${pkgs.system}.default

    # 2. Pre-built binaries (recommended for fast CI/CD)
    # Choose the one matching your system:
    inputs.tola.packages.${pkgs.system}.aarch64-darwin        # macOS (Apple Silicon)
    # inputs.tola.packages.${pkgs.system}.x86_64-linux        # Linux (x86_64)
    # inputs.tola.packages.${pkgs.system}.aarch64-linux       # Linux (ARM64)
    # inputs.tola.packages.${pkgs.system}.x86_64-windows      # Windows (x86_64)

    # 3. Static Binaries (Linux only)
    # inputs.tola.packages.${pkgs.system}.x86_64-linux-static
    # inputs.tola.packages.${pkgs.system}.aarch64-linux-static
  ];
}
```

> **Note**: The `default` package builds natively for your system. If a pre-built binary is not available in the cache for `default`, Nix will build it from source. The specific architecture packages (e.g., `aarch64-darwin`) are explicit cross-compilation targets that are likely populated in the cache.

## Usage

```text
A static site generator for typst-based blog

Usage: tola [OPTIONS] <COMMAND>

Commands:
  init    Init a template site
  build   Deletes the output directory if there is one and rebuilds the site
  serve   Serve the site. Rebuild and reload on change automatically
  deploy  Deletes the output directory if there is one and rebuilds the site
  help    Print this message or the help of the given subcommand(s)

Options:
  -o, --output <OUTPUT>    Output directory path (relative to project root)
  -c, --content <CONTENT>  Content directory path (relative to project root)
  -a, --assets <ASSETS>    Assets directory path (relative to project root)
  -C, --config <CONFIG>    Config file name [default: tola.toml]
  -h, --help               Print help
  -V, --version            Print version

Build/Serve Options:
  --base-url <URL>         Override base URL for deployment (e.g., GitHub Pages)
  --clean                  Clean output directory before building
  -m, --minify             Minify HTML output
  -t, --tailwind           Enable Tailwind CSS processing
  --rss                    Enable RSS feed generation
  --sitemap                Enable sitemap generation
```

You can run `tola` from any subdirectory — it will automatically find `tola.toml` by searching upward.

### Project Structure

```text
.
├── assets/
│   ├── fonts/
│   ├── images/
│   ├── scripts/
│   └── styles/
├── content/
│   ├── posts/
│   ├── index.typ
│   └── about.typ
├── templates/          # Dependency directory (triggers dependent rebuilds)
│   └── base.typ
├── utils/              # Dependency directory (triggers dependent rebuilds)
│   └── helpers.typ
└── tola.toml
```

### Routing

Files under `content/` are mapped to their respective routes:

| Source Path | URL |
|-------------|-----|
| `content/index.typ` | `/index.html` |
| `content/about.typ` | `/about/` |
| `content/posts/hello.typ` | `/posts/hello/` |

### Quick Start

```sh
# Create a new site
tola init my-blog
cd my-blog

# Build for production
tola build

# Start development server
tola serve
```

## Roadmap (v0.7.0)

> **Coming Soon**: Incremental Rendering & VDOM Architecture

The next major release focuses on **instant hot-reloading** with sub-second refresh times:

- **VDOM Core** — Type-safe virtual DOM with TTG (Trees that Grow) pattern
- **Stable Identity** — Span-based node IDs for precise diffing across compilations
- **Binary Patch Protocol** — `rkyv` zero-copy serialization for efficient updates
- **Actor Concurrency** — Non-blocking `FsActor` / `CompilerActor` / `WsActor` via `tokio`

Goal: *"Local refresh feels like a web app"*

## Note

> ⚠️ **Early development & experimental HTML export**

`tola` is usable but evolving — expect breaking changes and rough edges. Feedback and contributions are welcome!

Typst's HTML output is not yet as mature as its PDF output. Some features require workarounds:

- **math rendering** — Equations are exported as inline SVGs, which may need CSS tweaks for proper sizing and alignment ([issue #24](https://github.com/tola-ssg/tola-ssg/issues/24))
- **whitespace handling** — Typst inserts `<span style="white-space: pre-wrap">` between inline elements to preserve spacing ([PR #6750](https://github.com/typst/typst/pull/6750))
- **layout** — Some Typst layout primitives don't translate perfectly to HTML semantics

These are upstream limitations in Typst itself, not `tola`. As Typst's HTML backend matures, these rough edges will smooth out.

## Documentation

**Coming soon!**

In the meantime:
- Run `tola --help` and `tola <command> --help` for CLI usage
- See [tola-ssg/example-sites](https://github.com/tola-ssg/example-sites) for examples
- Open an issue if you have questions

## License

MIT