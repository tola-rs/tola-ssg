# tola-icons

Validated SVG icons for CSS generators, document producers, and native or WebAssembly
applications. No filesystem or network I/O, no cache.

```rust
use tola_icons::{IconCollections, IconCollection, IconPaint};

let mut collection = IconCollection::new();
collection.insert_svg("mark", br#"<svg viewBox="0 0 24 12">
  <path fill="currentColor" d="M0 0h24v12H0z"/>
</svg>"#)?;

let mut collections = IconCollections::new();
collections.mount("brand", collection)?;
let mark = collections.get("brand", "mark").unwrap();
assert_eq!(mark.paint(), IconPaint::CurrentColor);
assert_eq!(mark.aspect_ratio(), 2.0);
# Ok::<(), Box<dyn std::error::Error>>(())
```

`SvgIcon::parse` and `IconCollection::from_iconify` accept borrowed or owned bytes, including
static slices.

## Icon identity

An `IconId` is a validated `collection:name` pair: 1–512 ASCII letters, digits, hyphens, or
underscores, starting alphanumeric and not ending with a hyphen. `mount` chooses the namespace
independently of any IconifyJSON prefix; duplicate names are errors, and lookups come back in
lexical order. Iconify aliases, character references, inherited geometry, and quarter-turn flips
resolve during import.

## SVG contract

An accepted icon is a static, self-contained SVG: scripts, animation, external resources, foreign
content, stylesheet elements, and unsupported properties are errors rather than silently dropped.
The root needs a `viewBox` or a resolvable absolute size.

`IconPaint` says whether the icon works as a color mask: `CurrentColor` (paints follow
`currentColor`), `Fixed` (paints are fixed, including black and multicolor icons), `Mixed` (an
effect makes that unprovable).

Imports are bounded: 4 MiB per standalone SVG; IconifyJSON at 64 MiB with 65,536 icons, 32,768
aliases and mappings, under a 128 MiB budget. `svg()` returns the normalized resource, and
`svg_with_id_prefix` is the inline form. Native and `wasm32-unknown-unknown` share one parser.
