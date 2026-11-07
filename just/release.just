# Regenerate the third-party license list at THIRD-PARTY-LICENSES.txt.
licenses:
    cargo about generate -c about.toml -o THIRD-PARTY-LICENSES.txt --workspace --locked about.hbs

# Show the version change a release would make, without touching the checkout.
version-preview version:
    nu tools/release/version.nu preview {{ quote(version) }}

# Apply a release version to a clean checkout; never commits, tags, or publishes.
version version:
    nu tools/release/version.nu set {{ quote(version) }}

# Confirm that packages, dependencies, and the lockfile agree on one version.
version-check:
    nu tools/release/version.nu check

# Check release sources, CI, crate packages, and this host's archive without uploading.
release-check tag: licenses
    nu tools/release/check.nu all {{ quote(tag) }}

# Build this host's release archive into ./release with its license material and checksums.
release: licenses
    nu tools/release/package.nu build --output release
    nu tools/release/package.nu licenses --output release
    nu tools/release/package.nu checksums release
