# tola-typst

Typst compilation and export, independent of the rest of Tola. The crate wraps the official
`typst` compiler (re-exported as `tola_typst::typst`) and supplies the resources a compilation
needs: explicit sources, fonts, packages, file caches, virtual files, and structured diagnostics.

Source analysis (names, imports, formatting) belongs to `tola-typst-syntax`; the two crates are
independent.

## Usage

Everything goes through one world. `TypstWorld` is what Typst reads: sources, packages, fonts,
`sys.inputs`. Build one, then compile it:

```rust,no_run
use std::path::Path;
use std::sync::Arc;
use tola_typst::prelude::*;

fn render(entry: &Path, root: &Path, cancellation: &BundleCancellation) -> Result<Vec<u8>, CompileError> {
    let world = TypstWorld::builder(entry, root)
        .with_local_cache()
        .with_fonts(Arc::new(FontStore::new()))
        .build(cancellation)?;
    compile_world(&world)?.html()
}
```

`compile_world` produces one HTML document; `scan_world` evaluates a module without layout;
`compile_bundle_world` compiles an official Typst Bundle; `compile_source_with_evidence` compiles
one file, which is what an editor needs when the root program never reached it. Results carry
their diagnostics and file reads.

File inputs have an explicit lifetime. `with_local_cache` keeps the first observation of each
file, failures included, until `TypstWorld::reset`. `with_shared_cache` reuses parsed values while
each compilation observes fresh inputs; direct world reads also revalidate on access. Within one
compilation, source text and raw bytes always come from the same observation.

`with_sources` freezes the listed sources and observes other files per compilation.
`with_file_view` shares a `FileView`, including its resolver, across worlds that must keep the same
file and package observations. Create another file view to observe changes; resetting a world
does not affect an externally supplied view. A file view pins files on first access rather than
capturing the filesystem at one instant, so callers publishing results must verify that the
retained input evidence is still current.

Diagnostics retain Typst's messages, spans, and call traces. `Diagnostics::extend_distinct`
combines phases by native diagnostic identity and merges package navigation.
`diagnostic::resolve_source_location` captures bounded excerpts; each excerpt records its scalar
and UTF-16 starting columns, including clipped line windows.

Metadata stays a native Typst value (`metadata_all`, `metadata_first`, `metadata_unique`,
`metadata_declarations`); `with_inputs_dict` supplies native `sys.inputs`.

Modules: `world`, `compile`, `bundle`, `html`, `diagnostic`, `svg`, and `spelling`.

## Features

| Feature | Default | Purpose |
| --- | --- | --- |
| `scan` | yes | evaluation-only scanning APIs |
| `parallel` | yes | load and parse sources in parallel |
| `embed-fonts` | no | embedded Typst fonts |
| `network` | no | download missing Typst Universe packages |

`embed-fonts` carries the fonts Typst's default documents read. `licenses/README.md` records where
they come from, and the crate's `NOTICE` carries their licences.

With `default-features = false`, HTML and Bundle compilation keep working in every output format;
network access is opt-in per store.

## Cancellation

A `BundleCancellation` is checked around Typst's synchronous compile and export, and inside source
loading, font I/O, inventory construction, and hashing. The crate follows Typst 0.15.1.
