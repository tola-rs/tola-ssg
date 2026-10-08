# tola-typst

Typst compilation and export, independent of the rest of Tola. The crate wraps the official
`typst` compiler (re-exported as `tola_typst::typst`) with the resources a compilation needs:
source snapshots, fonts, packages, file caches, virtual files, and structured diagnostics.

Source analysis — names, imports, formatting — is `tola-typst-syntax`'s job; the two libraries
are independent.

## Usage

Everything goes through one world. `TypstWorld` is what Typst gets to read: sources, packages,
fonts, `sys.inputs`. Build one, then compile it:

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
`compile_bundle_world` compiles an official Typst Bundle; `compile_source_with_evidence`
compiles one file, which is what an editor needs when the root program never reached it.
Results carry their diagnostics and file reads.

File inputs have an explicit lifetime. `with_local_cache` keeps the first observation of each
file, including failures, until `TypstWorld::reset`. `with_shared_cache` reuses parsed values
while each compilation observes fresh inputs; direct world reads also revalidate on access.
Within a compilation, source text and raw bytes always come from the same observation.

`with_snapshot` freezes the explicitly listed `SourceSnapshot` members and observes other files
for each compilation. `with_file_snapshot` shares a `FileSnapshot`, including its resolver,
across worlds that must retain the same file and package observations. Create another file
snapshot to observe changes; resetting a world does not change an externally supplied snapshot.
A file snapshot pins files on first access, rather than capturing a whole filesystem at one
instant. Callers publishing results must verify that the retained input evidence is still current.

Diagnostics retain Typst's messages, spans, and call traces. `Diagnostics::extend_distinct`
combines compilation phases by native diagnostic identity and merges package navigation.
`diagnostic::resolve_source_location` captures bounded excerpts from the compiled source;
each excerpt records its scalar and UTF-16 starting columns, including clipped line windows.

Metadata stays a native Typst value (`metadata_all`, `metadata_first`, `metadata_unique`,
`metadata_declarations`); `with_inputs_dict` supplies native `sys.inputs`.

Modules: `world`, `compile`, `bundle`, `html`, `diagnostic`, and `codegen` (the retired JSON
layer behind `legacy-serialization`).

## Features

| Feature | Default | Purpose |
| --- | --- | --- |
| `scan` | yes | evaluation-only scanning APIs |
| `parallel` | yes | load and parse sources in parallel |
| `embed-fonts` | no | embedded Typst fonts |
| `network` | no | download missing Typst Universe packages |
| `legacy-serialization` | no | deprecated Typst value and diagnostic JSON APIs |

`embed-fonts` carries the fonts Typst's default documents read. `licenses/README.md` records
where they come from, and the crate's `NOTICE` carries their licences.

With `default-features = false`, HTML and Bundle compilation keep working in every output
format; network access is opt-in per store.

## Cancellation

A `BundleCancellation` is checked around Typst's synchronous compile and export, and inside
snapshot loading, font I/O, inventory construction, and hashing. The crate follows Typst 0.15.1.
