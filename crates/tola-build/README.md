# tola-build

Builds a Tola site: analyzes the sources, compiles the root Typst Bundle through `tola-typst`,
and converges documents, assets, icons, feeds, sitemaps, and hook output into one output graph.
Every attempt produces one candidate: its output graph and diagnostics. A validated candidate
is published whole, or installed whole as the next revision; there is no partial publication.

Hooks run in three stages: `before-build` prepares declared inputs, `generate-outputs` adds
outputs to the candidate graph, and `after-publish` reads the committed output.

## Build and write

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

Keep the session and call `build_and_write` after edits. It recovers an interrupted publish, runs
the before-build hooks, builds, rechecks inputs, replaces the output directory, and reuses caches.
A failure or cancellation before installation leaves the previous output untouched.

After-publish hooks run as a separate step: `WriteOutcome` keeps the committed graph,
configuration, and mode, and `run_after_publish` executes its consumers. For a build without
publishing, `build_site` returns a `SiteBuild` with its graph and diagnostics.

Diagnostics keep their messages and call traces; `Diagnostic::display_message()` provides the
site's panic sentence for presentation without changing the stored message. Source excerpts carry
their starting columns, so clients can locate clipped text within the original UTF-16 range.

## Custom scheduling

`BuildSession::prepare` with a `BuildRequest` produces a `BuildAttempt` to run on your own
worker; `BuildResources` sets the invocation's input scope. With your own revision store, check
candidates with `UncheckedRevision::check` and install them through
`BuildSession::install_revision`.
