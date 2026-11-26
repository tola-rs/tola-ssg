# tola-build

Builds, validates, and publishes Tola sites.

Compiles the root Typst Bundle through `tola-typst`, merges documents, assets, icons, feeds, sitemaps, and hook outputs into one output graph, and publishes validated builds atomically.

## Usage

```rust,no_run
use std::path::Path;

use tola_build::BuildSession;
use tola_build::config::loading::{BuildOverrides, load_site_config};

let loaded = load_site_config(
    Some(Path::new("site/tola.toml")),
    tola_typst::PackageLocations::discover(None, None)?,
    &BuildOverrides::default(),
)?;
let config = loaded.config();
let mut session = BuildSession::new();
let written = session.build_and_write(config, Default::default())?;
println!("Built {}", written.output_root().display());
if let Err(error) = written.run_after_publish(&Default::default()) {
    eprintln!("after-publish hook failed: {error:#}");
}
# Ok::<(), anyhow::Error>(())
```

- Reuse `BuildSession` across rebuilds to keep caches and recover interrupted publishes.
- Use `build_site` to validate without writing to disk, or `BuildSession::prepare` / `install_revision` for custom scheduling.
- See [`CONTEXT.md`](CONTEXT.md) for module layout and invariants.
