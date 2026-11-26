# tola-typst

Standalone Typst compiler and exporter wrapper (`typst` 0.15.1, re-exported as `tola_typst::typst`).

## Usage

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

- Entrypoints: `compile_world` (HTML), `compile_bundle_world` (Typst Bundle), `scan_world` (evaluation only), and `compile_source_with_evidence` (single-file editor checks).
- Supports shared/local file caches, `FileView` pinning, `sys.inputs`, metadata queries, and cooperative `BundleCancellation`.
- See [`CONTEXT.md`](CONTEXT.md) for layout and invariants.

## Features

| Feature | Default | Purpose |
| --- | --- | --- |
| `scan` | yes | Evaluation-only scanning APIs |
| `parallel` | yes | Parallel source loading and parsing |
| `embed-fonts` | no | Embedded default Typst fonts (see [`licenses/README.md`](licenses/README.md) and `NOTICE`) |
| `network` | no | Download missing Typst Universe packages |
