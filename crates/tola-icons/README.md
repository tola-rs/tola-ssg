# tola-icons

Validates static SVG icons and imports IconifyJSON collections with no filesystem or network I/O. Works on native and `wasm32-unknown-unknown`.

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

- Rejects scripts, animations, external references, stylesheets, and `<foreignObject>`. Roots require a `viewBox` or absolute size.
- `IconPaint` classifies icons as `CurrentColor`, `Fixed`, or `Mixed`.
- Import limits: 4 MiB per SVG; 64 MiB and 65,536 icons per IconifyJSON file (128 MiB memory budget).
