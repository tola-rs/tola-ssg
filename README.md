# tola-ssg

A static site generator for Typst-based blogs.


## Table of Contents

- [Features](#features)
- [Philosophy](#philosophy)
- [Examples](#examples)
- [Installation](#installation)
- [Usage](#usage)
- [Documentation](#documentation)
- [Note](#note)

## Features

### Performance

- **font caching** — Fonts loaded once at startup, shared across all compilations
- **file caching** — Only re-read changed files, zero-cost for unchanged files
- **parallel compilation** — Build pages and assets concurrently using rayon
- **incremental rebuilds** — Dependency graph tracks which files need rebuilding (see below)

### Incremental rebuilds

When a template or shared file changes, `tola` tracks dependencies and rebuilds only affected pages:

```text
DependencyGraph
├── forward: content.typ → {template.typ, utils/a.typ, ...}
└── reverse: template.typ → {page1.typ, page2.typ, ...}
```

Edit a utility used by 5 pages? Only those 5 pages rebuild, not the entire site.

### Development Experience

- **auto reload** — Watch mode with automatic rebuild on file changes (300ms debounce)
- **local server** — Built-in HTTP server with directory listing and clean URLs
- **auto config discovery** — Run `tola` from any subdirectory; it finds `tola.toml` automatically
- **graceful error handling** — Human-readable diagnostic messages from Typst

### Content Processing

- **SVG extraction & optimization** — Extract inline SVGs, adjust viewBox, compress to SVGZ
- **dark mode SVG adaptation** — Auto-inject CSS for SVG theme adaptation (enabled by default)
- **HTML/XML minification** — Optional minification for production builds
- **URL slugification** — Configurable slug modes(full, safe, ascii, no) with case options
- **Typst package support** — Uses standard Typst package registry with shared cache

### Site Generation

- **rss-2.0 support** — Auto-generate `feed.xml` from page metadata
- **sitemap support** — Auto-generate `sitemap.xml` for search engines
- **tailwind-css support** — Built-in support, out of the box
- **github pages deployment** — One-command deploy(or use github action)

## Philosophy

> **Keep your focus on the content itself.**

`tola` is built around three core principles:

### Minimal Abstraction

`tola` provides a thin layer over Typst — just enough to handle the boring stuff(routing, live reload, local server, incremental rebuild smartly) without locking you into a rigid framework. Your Typst code stays portable.

### Typst First

If Typst can do something easily, use Typst. `tola` doesn't reinvent the wheel — it leverages Typst's powerful markup and scripting capabilities.

### Developer Joy

- **zero config to start** — `tola init <SITE-NAME>` gets you running in seconds
- **fast feedback loop** — Incremental rebuilds keep iteration snappy
- **escape hatches** — Full access to HTML/CSS/JS when you need it
- **predictable output** — What you write is what you get

## Documentation

**Coming soon!**

> *Academic pressure + writing docs is tedious... but it's on the roadmap!*

In the meantime:
- Check `tola --help` and `tola <command> --help` for CLI usage
- See `resources/starter_example/` for a minimal project structure
- Feel free to open an issue if you have questions

## Note

> ⚠️ **Early development & experimental HTML export**

`tola` is usable but evolving — expect breaking changes and rough edges. Feedback and contributions are welcome!

Meanwhile, Typst's HTML output is not yet as mature as its PDF output. Some features require workarounds:

- **math rendering** — Equations are exported as inline SVGs, which may need CSS tweaks for proper sizing and alignment (see [issue #24](https://github.com/tola-ssg/tola-ssg/issues/24))
- **whitespace handling** — Typst inserts `<span style="white-space: pre-wrap">` between inline elements (e.g., consecutive `html.a`, use block element like `html.div` to resolve it) to preserve spacing ([PR #6750](https://github.com/typst/typst/pull/6750)). This is intentional but can be surprising sometimes.
- **layout** — Some Typst layout primitives don't translate perfectly to HTML semantics

These are upstream limitations (or design decisions — some may not be "limitations" per se) in Typst itself, not `tola`. As Typst's HTML backend matures, these rough edges will smooth out.

`tola` is designed for users who want to use Typst as a replacement for both LaTeX and Markdown for personal blogs, and who are open to embracing HTML/CSS/JS for convenience, simplicity, and aesthetics. It aims to offer maximum flexibility — almost everything can be customized(because typst/html combination), eliminating boilerplate so you can focus purely on your content.

## Examples

[My blog](https://kawayww.com):

![home](/screenshots/home.avif)
![figure1](/screenshots/figure1.avif)
![figure2](/screenshots/figure2.avif)

## Installation

- `cargo install tola`
- Install the binary from the [release page](https://github.com/KawaYww/tola/releases).
- For Nix users, a `flake.nix` already exists in the repo root, and you can use the binary cache at [tola.cachix.org](https://tola.cachix.org):

```nix
# flake.nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    tola = {
      url = "github:kawayww/tola";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # ...
    # ...
  }
  # ...
  # ...
}
```

```nix
# configuration.nix
{ config, pkgs, inputs, ... }:

{
  nix.settings = {
    substituters = [
      "https://tola.cachix.org"
      # ...
      # ...
    ];
    trusted-public-keys = [
      "tola.cachix.org-1:5hMwVpNfWcOlq0MyYuU9QOoNr6bRcRzXBMt/Ua2NbgA="
      # ...
      # ...
    ];
    environment.systemPackages = with pkgs; [
      # ...
      # ...
    ] ++ [
      inputs.tola.packages.${pkgs.system}.default
    ];
  }
}
```

## Usage

- `tola -h`:

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

Your project should follow the directory structure below:

```text
.
├── assets
│   ├── fonts
│   ├── iconfonts
│   ├── images
│   ├── scripts
│   ├── styles
├── content
│   ├── posts/
│   ├── categories/
│   ├── index.typ
│   ├── programming.typ
├── templates
│   └── tola-conf.typ
├── utils
│   └── main.typ
└── utils
    └── main.typ
```

Files under the `content/` directory are mapped to their respective routes:
e.g., `content/posts/examples/aaa.typ` -> `http://127.0.0.1:5277/posts/examples/aaa`
(`content/index.typ` will be specially compiled into `http://127.0.0.1:5277/index.html`)

```text
http://127.0.0.1:5277:
├── assets
│   ├── fonts
│   ├── iconfonts
│   ├── images
│   ├── scripts
│   └─ styles
├── posts/
├── categories/
└── index.html
```