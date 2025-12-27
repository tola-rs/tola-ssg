# tola-ssg

A static site generator for Typst-based blogs.

## Table of Contents

- [Showcase](#showcase)
- [Features](#features)
- [Philosophy](#philosophy)
- [Installation](#installation)
- [Usage](#usage)
- [Note](#note)

## Showcase

> Yeah, my blog is alos built with `tola`.

[My blog](https://kawayww.com):
![home](/screenshots/home.avif)

## Features

### Performance

- **font caching** — Fonts loaded once at startup, shared across all compilations
- **file caching** — Only re-read changed files, zero-cost for unchanged files
- **parallel compilation** — Build pages and assets concurrently using rayon
- **incremental rebuilds** — Intelligent dependency tracking for sub-second hot reload
  - **content**: Direct rebuild of changed file
  - **templates/utils**: Reverse-lookup of dependencies to rebuild only affected pages
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
    tola.url = "github:tola-ssg/tola-ssg/v0.6.2";
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
    # Use cross-compiled binary from cachix (recommended)
    inputs.tola.packages.${pkgs.stdenv.hostPlatform.system}.aarch64-darwin  # Apple Silicon
    # inputs.tola.packages.${pkgs.stdenv.hostPlatform.system}.x86_64-linux  # x86_64 Linux

    # Or build locally (does NOT use cachix):
    # inputs.tola.packages.${pkgs.stdenv.hostPlatform.system}.default
  ];
}
```

> **Note**: The `default` package builds natively and won't hit the cachix cache. Use explicitly named packages (e.g., `aarch64-darwin`, `x86_64-linux`) to use pre-built binaries.

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
├── templates/
│   └── base.typ
├── utils/
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
- See `resources/starter_example/` for a minimal project structure
- Open an issue if you have questions

## License

MIT