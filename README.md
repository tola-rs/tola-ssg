# tola-ssg

A static site generator for Typst-based websites.  \

v0.7.0 is coming soon (26.02.23)

## Table of Contents

- [Showcase](#showcase)
- [Features](#features)
- [Philosophy](#philosophy)
- [Usage](#usage)
- [Installation](#installation)
- [Roadmap](#roadmap-v070)
- [Note](#note)

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

<details>
<summary>How to make "Recent 5 Posts" with Tola's virtual package system</summary>

Thanks to `typst` and `tailwindcss`, `tola` offers writing flexibility.
Implement `Recent Posts` easily with the `@tola/pages` virtual package:

```typst
#import "@tola/pages:0.0.0": pages
#import "/components/ui.typ" as ui

#let posts = (pages()
  .filter(p => "/posts/" in p.permalink)
  .filter(p => p.at("draft", default: false) == false)
  .filter(p => p.at("date", default: none) != none)
  .sorted(key: p => p.date)
  .rev())

#html.div(class: "space-y-6")[
  #for post in posts.slice(0, calc.min(posts.len(), 5)) {
    ui.post-card(post)
  }
]
```

The `@tola/pages` package provides access to all page metadata (title, date, permalink, tags, etc.) at compile time.

</details>

## Features

### Performance

- **parallel compilation** — Process pages concurrently
- **font preloading** — Fonts loaded once at startup, shared across all compilations
- **snapshot sharing** — Typst compiler snapshot reused across batch compilations, avoiding repeated initialization

### Development Experience

- **zero config to start** — `tola init <SITE-NAME>` gets you running in seconds
- **local server** — Built-in HTTP server with on-demand compilation
- **hot reloading** — File changes are diff/patched to the browser instantly via WebSocket
- **priority queue scheduler** — Prioritizes currently viewed pages for faster feedback
- **incremental rebuilds** — Bidirectional dependency graph + VDOM caching enables minimal rebuilds; only affected pages are recompiled
- **graceful error handling** — Human-readable diagnostic messages from Typst
- **escape hatches** — Full access to HTML/CSS/JS when you need it

### Build & Integration

- **build hooks** — Pre/post build hooks for custom scripts (e.g., esbuild, imagemin)
- **Tailwind CSS** — Built-in CSS processor integration
- **html/xml minification** — Optional minification for production builds
- **SPA navigation** — Optional client-side navigation with DOM morphing and View Transitions API

### Image & SVG (experimental, WIP)

- **SVG math tight bounds** — Removes extra whitespace around equations via Typst's `bounds` edge setting
- **SVG baseline alignment** — Optional vertical-align for inline SVG math
- **SVG viewBox expansion** — Expands viewBox to include stroke boundaries, preventing content clipping
- **SVG dark mode adaptation** — Auto-inject CSS for SVG theme switching (enabled by default)
- **image recolor** — SVG filter-based color adaptation for local images (png/jpg/webp), matching screenshots to site theme
- **image background removal** — Auto-detect and remove image backgrounds

### Routing & SEO

- **clean URLs** — `content/posts/hello.typ` → `/posts/hello/`
- **custom permalinks** — Override URL via page metadata
- **aliases** — Redirect old URLs to new locations
- **url slugification** — Configurable slug modes (full, safe, ascii) with case options
- **url conflict detection** — Errors when multiple pages resolve to the same URL
- **rss/atom support** — Auto-generate `feed.xml` from page metadata
- **sitemap** — Auto-generate `sitemap.xml` for search engines
- **Open Graph & Twitter Cards** — Auto-inject default OG tags from site config, or customize per-page via `og-tags()` in Typst
- **404 typst/html page** — Configurable not-found page(.typ or .md)

### Virtual Packages

Tola injects virtual packages at compile time, enabling cross-page data access without external build steps:

- `@tola/site:0.0.0` — Site configuration (`info`, `info.extra` from `[site.info]` in `tola.toml`)
- `@tola/pages:0.0.0` — All pages metadata (title, date, permalink, tags, draft status...)
- `@tola/current:0.0.0` — Current page info (useful in templates)

```typst
#import "@tola/pages:0.0.0": pages
#import "@tola/site:0.0.0": info

// List all posts
#for post in pages().filter(p => "/posts/" in p.permalink) {
  [#post.title (#post.date)]
}

// Access site title
#info.title
```

See [Virtual Packages in Usage](#virtual-packages-1) for more details.

## Philosophy

> **Keep your focus on the content itself.**

### Typst First

If Typst can easily do it, use Typst. No need to explain Typst's strengths here — even with HTML export losing many layout features, it's still remarkably powerful.

`tola` leverages Typst's markup and scripting capabilities instead of reinventing the wheel.

### Tola Second

Some things are beyond what a standalone `typst` CLI can do — especially batch processing and site-wide coordination:

- Automatic routing from file structure
- Seamless hot reload with VDOM diff/patch
- SVG dark mode adaptation out of the box
- Cross-page state via `sys.inputs` and virtual packages injection
- ...And more!

That's where `tola` steps in — optimizing developer experience and integrating these features seamlessly is no small feat.


## Usage

- [Example Site Structure](#example-site-structure)
- [Shared Dependencies](#shared-dependencies)
- [Configuration](#configuration)
- [Virtual Packages](#virtual-packages-1)
- [Open Graph & Twitter Cards](#open-graph--twitter-cards)
- [Quick Start](#quick-start)

Run `tola --help` or `tola <command> --help` for detailed CLI usage.

You can run `tola` from any subdirectory — it will automatically find `tola.toml` by searching upward.

### Example Site Structure

```text
.
├── tola.toml                 # Site configuration
├── content/                  # Page sources (routes)
│   ├── index.typ             #   -> /
│   ├── about.typ             #   -> /about/
│   ├── posts/
│   │   └── hello.typ         #   -> /posts/hello/
│   └── error.typ             # Custom 404 page
├── templates/                # Shared layouts (default in `build.deps`)
│   ├── tola.typ              #   Default template from `tola init` (fully customizable)
│   ├── post.typ              #   Post layout (can extend tola.typ)
│   └── normal.typ            #   Normal page layout
├── utils/                    # Helper functions (default in `build.deps`)
│   └── tola.typ              #   Utility functions from `tola init` (CSS class, OG tags, etc.)
├── components/               # Custom components (add to `build.deps` manually)
│   ├── layout.typ            #   Reusable layout components
│   └── ui.typ                #   UI components (post-card, tag-list, etc.)
└── assets/
    ├── images/
    ├── fonts/
    │   └── Luciole-math.otf  # Embedded math font (auto-loaded by tola)
    ├── styles/
    │   └── tailwind.css      # Tailwind input (if using `build.hooks.css`)
    └── scripts/
```

### Shared Dependencies

The routing under `content/` is probably intuitive — files map to URLs. But you might wonder about `build.deps` in `tola.toml`. You can actually use it without thinking too hard, but a quick explanation might help:

Typst files in `content/` become pages. But they often `#import` shared code from `templates/`, `utils/`, or something else you prefer — these are just conventional names tola provides by default, feel free to rename them. Tola tracks these dependencies internally. When you declare directories in `build.deps`, tola knows: "if anything here changes, recompile all pages that import from it." This enables instant hot-reload across your entire site.

`templates/` and `utils/` are just default names — you can rename them or add more via `build.deps`. For example: you have `templates/base.typ` that styles math equations with Tailwind classes. When you change `text-base` to `text-2xl` in that file, any page importing it (like `content/example.typ` -> `/example/`) will instantly reflect the larger equations — no manual refresh needed.

### Configuration

Common `tola.toml` settings (run `tola init --dry` to see full defaults):

```toml
# Access in Typst: #import "@tola/site:0.0.0": info
# Then use: info.title, info.author, info.extra.custom
[site.info]
title = "My Blog"
author = "Your Name"
email = "you@example.com"
description = "A blog built with Typst and Tola"
language = "en"
url = "https://example.com"

[site.info.extra]
custom = "This is my custom data"

[site.header]
icon = "assets/images/favicon.ico"
styles = ["assets/styles/custom.css"]
scripts = [
  "assets/scripts/custom.js" # Simple: No defer and async
  { path = "assets/scripts/app.js", defer = true }
  { path = "assets/scripts/app.js", async = true }
]
elements = ['<meta name="darkreader-lock">'] # Extra special html elements

[site.seo]
auto_og = true   # Auto-inject default OG tags (site_name, locale, description, type, twitter:card)

[site.seo.feed]
enable = true
format = "rss"   # "rss" | "atom"

[site.seo.sitemap]
enable = true

[site.nav]
spa = true    # SPA navigation (link interception + DOM morphing)
# transition = { style = "fade", time = 200 }
# preload = { enable = true, delay = 100 }

[build]
content = "content"
output = "public"
minify = true
deps = ["templates", "utils"]  # Shared dependencies — changes trigger range rebuild

[build.assets]
nested = ["assets/images", "assets/styles", "assets/fonts"]

[build.hooks.css]
enable = true
path = "assets/styles/tailwind.css"
command = ["tailwindcss"]
```

### Virtual Packages

Tola provides virtual packages that you can import directly in your Typst files:

| Package | Exports |
|---------|---------|
| `@tola/site:0.0.0` | `info` — Site metadata (title, author, email, description, url, language, copyright, extra) |
| `@tola/pages:0.0.0` | `pages()`, `by-tag(tag)`, `by-tags(..tags)`, `all-tags()` |
| `@tola/current:0.0.0` | `path`, `parent`, `source`, `links-to`, `linked-by`, `headings`, `siblings(pages)`, `children(pages)`, `breadcrumbs(pages)`, `prev(pages, n)`, `next(pages, n)` |

```typst
// content/index.typ — list recent posts
#import "/templates/page.typ": page
#import "/components/ui.typ" as ui
#import "@tola/pages:0.0.0": pages

#show: page.with(title: "Home")

#let posts = (pages()
  .filter(p => "/posts/" in p.permalink and p.date != none)
  .sorted(key: p => p.date)
  .rev())

#for post in posts.slice(0, calc.min(posts.len(), 5)) {
  ui.post-card(post)
}
```

<details>
<summary>Example: Recent Posts</summary>

```typst
#import "@tola/pages:0.0.0": pages

#let recent-posts = (pages()
  .filter(p => "/posts/" in p.permalink)
  .filter(p => p.at("draft", default: false) == false)
  .filter(p => p.at("date", default: none) != none)
  .sorted(key: p => p.date)
  .rev()
  .slice(0, calc.min(5, pages().len())))

#for post in recent-posts {
  [- #link(post.permalink)[#post.title] (#post.date)]
}
```

</details>

<details>
<summary>Example: Pinned Posts</summary>

Use page metadata `pinned: true` to mark featured posts:

```typst
// content/posts/important.typ
#set page(meta: (pinned: true, date: "2025-01-01", title: "Important Post"))
```

```typst
#import "@tola/pages:0.0.0": pages

#let pinned = pages().filter(p => p.at("pinned", default: false) == true)

#if pinned.len() > 0 [
  == Pinned Posts
  #for post in pinned {
    [- #link(post.permalink)[#post.title]]
  }
]
```

</details>

<details>
<summary>Example: Custom Sort Order</summary>

Sort by custom `order` field, then by date:

```typst
// content/docs/intro.typ
#set page(meta: (order: 1, title: "Introduction"))

// content/docs/setup.typ
#set page(meta: (order: 2, title: "Setup Guide"))
```

```typst
#import "@tola/pages:0.0.0": pages

#let docs = (pages()
  .filter(p => "/docs/" in p.permalink)
  .sorted(key: p => (
    p.at("order", default: 999),
    p.at("date", default: "9999-99-99")
  )))

#for doc in docs {
  [- #link(doc.permalink)[#doc.title]]
}
```

</details>

<details>
<summary>Example: Extract Date from Filename</summary>

Use `source` from `@tola/current` to parse date from filename like `2025_02_27_hello.typ`:

```typst
#import "@tola/current:0.0.0": source

// source = "posts/2025_02_27_hello.typ"
#let filename = source.split("/").last()  // "2025_02_27_hello.typ"
#let parts = filename.split("_")
#let date = parts.slice(0, 3).join("-")    // "2025-02-27"
#let slug = parts.slice(3).join("_").replace(".typ", "")  // "hello"
```

You can use this pattern in your template to auto-generate dates:

```typst
// templates/post.typ
#let post-page(body) = {
  import "@tola/current:0.0.0": source

  let filename = source.split("/").last()
  let parts = filename.split("_")
  let auto-date = if parts.len() >= 4 {
    parts.slice(0, 3).join("-")
  } else { none }

  // Use auto-date as fallback if no explicit date in metadata
  // ...
}
```

</details>

<details>
<summary>Example: Prev/Next Navigation</summary>

Navigate between posts in a sorted list:

```typst
#import "@tola/pages:0.0.0": pages
#import "@tola/current:0.0.0": prev, next

#let sorted-posts = (pages()
  .filter(p => "/posts/" in p.permalink and p.date != none)
  .sorted(key: p => p.date))

#let prev-post = prev(sorted-posts)
#let next-post = next(sorted-posts)

#html.nav(class: "flex justify-between")[
  #if prev-post != none {
    link(prev-post.permalink)[← #prev-post.title]
  }
  #if next-post != none {
    link(next-post.permalink)[#next-post.title →]
  }
]
```

</details>

<details>
<summary>Example: Breadcrumbs</summary>

Generate breadcrumb navigation from URL hierarchy:

```typst
#import "@tola/pages:0.0.0": pages
#import "@tola/current:0.0.0": breadcrumbs

#let crumbs = breadcrumbs(pages(), include-root: true)

#html.nav(class: "breadcrumbs")[
  #for (i, crumb) in crumbs.enumerate() {
    if i > 0 [ / ]
    link(crumb.permalink)[#crumb.title]
  }
]
// Output: Home / Blog / My Post
```

</details>

<details>
<summary>Example: Tag Cloud</summary>

List all tags with post counts:

```typst
#import "@tola/pages:0.0.0": pages, all-tags, by-tag

#for tag in all-tags() {
  let count = by-tag(tag).len()
  [#link("/tags/" + tag + "/")[#tag (#count)] ]
}
```

</details>

### Open Graph & Twitter Cards

Tola auto-injects default OG tags from `[site.info]` when `site.seo.auto_og = true`. For page-specific customization, use the `og-tags()` function in your template's `head` parameter:

```typst
#import "/templates/tola.typ": tola-page
#import "/utils/tola.typ": og-tags, parse-date

#let head = og-tags(
  title: "My Post",
  description: "A great article about...",
  url: "https://example.com/posts/my-post/",
  image: "https://example.com/og-image.png",
  type: "article",                      // "website" | "article" | "book" | "profile"
  published: parse-date("2024-01-15"),  // article:published_time
  tags: ("rust", "typst"),              // article:tag
)

// In your template
tola-page(
  title: "My Post",
  head: head,
)[...]
```

When you use `og-tags()`, Tola skips auto-injection and uses your custom tags instead.

### Quick Start

```sh
# Create a new site
tola init my-blog
cd my-blog

# Edit `content/index.typ`

# Build for production
tola build

# Start development server
tola serve
```

## Installation

### Cargo

```sh
cargo install tola
```

### Binary Release

Download from the [release page](https://github.com/tola-rs/tola-ssg/releases).

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

## Note

> **Early development & experimental HTML export**

`tola` is usable but evolving — expect breaking changes and rough edges. Feedback and contributions are welcome!

Typst's HTML output is not yet as mature as its PDF output. Some features require workarounds:

- **math rendering** — Equations are exported as inline SVGs, which may need CSS tweaks for proper sizing and alignment ([issue #24](https://github.com/tola-rs/tola-ssg/issues/24))
- **whitespace handling** — Typst inserts `<span style="white-space: pre-wrap">` between inline elements to preserve spacing ([PR #6750](https://github.com/typst/typst/pull/6750))
- **layout** — Some Typst layout primitives don't translate perfectly to HTML semantics

These are upstream limitations in Typst itself, not `tola`. As Typst's HTML backend matures, these rough edges will smooth out.

## Documentation

**Coming soon!**

In the meantime:
- Run `tola --help` and `tola <command> --help` for CLI usage
- See [tola-rs/example-sites](https://github.com/tola-rs/example-sites) for examples
- Open an issue if you have questions

## License

MIT
