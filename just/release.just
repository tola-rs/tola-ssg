# Regenerate the third-party license list at THIRD-PARTY-LICENSES.txt.
licenses:
    cargo about generate -c about.toml -o THIRD-PARTY-LICENSES.txt --workspace --locked about.hbs

# Show the version change a release would make, without touching the checkout.
version-preview version:
    deno run --allow-read --allow-write --allow-run=cargo,git --allow-env scripts/release/version.ts preview {{ quote(version) }}

# Apply a release version to a clean checkout; never commits, tags, or publishes.
version version:
    deno run --allow-read --allow-write --allow-run=cargo,git --allow-env scripts/release/version.ts set {{ quote(version) }}

# Confirm that packages, dependencies, and the lockfile agree on one version.
version-check:
    deno run --allow-read --allow-write --allow-run=cargo,git --allow-env scripts/release/version.ts check

# Exercise the release safety rules in isolated temporary workspaces.
[working-directory('scripts')]
release-tools:
    deno task test release

# Check release sources, CI, crate packages, and this host's archive without uploading.
release-check tag: licenses
    deno run -A scripts/release/check.ts all {{ quote(tag) }}

# Build the release archive for this host into ./release.
release: licenses
    deno run --allow-read --allow-write --allow-run --allow-env scripts/release/build.ts
