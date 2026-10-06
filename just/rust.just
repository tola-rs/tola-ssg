# Verify Rust formatting without rewriting files.
fmt-check:
    cargo fmt --all -- --check

# Rewrite Rust sources in place.
fmt-write:
    cargo fmt --all

# Run every workspace test with all features enabled.
test:
    cargo test --locked --workspace --all-features

# Lint every workspace target, including tests and examples.
[env('RUSTFLAGS', '-D warnings')]
clippy: build-library
    cargo clippy --locked --workspace --all-targets --all-features -- -D warnings

# Compile the debug binary.
build:
    cargo build --locked

# Check libraries without workspace feature unification.
[env('RUSTFLAGS', '-D warnings')]
build-library:
    cargo check --locked -p tola-build --all-targets
    cargo check --locked -p tola-typst-syntax --all-targets --no-default-features
    cargo check --locked -p tola-typst-syntax --all-targets --no-default-features --features format
    cargo check --locked -p tola-typst --all-targets --no-default-features
    for feature in scan parallel embed-fonts network legacy-serialization; do cargo check --locked -p tola-typst --all-targets --no-default-features --features "$feature" || exit; done
    cargo check --locked -p tola-lsp --all-targets --no-default-features
    cargo check --locked -p tola-lsp --all-targets --no-default-features --features network

# Build the public Rust documentation and fail on broken links.
[env('RUSTDOCFLAGS', '-D warnings')]
docs:
    cargo doc --locked -p tola-address -p tola-build -p tola-subprocess -p tola-icons -p tola-image -p tola-minify -p tola-packages -p tola-pronunciations -p tola-slugify --no-deps
    for package in tola-typst-syntax tola-typst tola-lsp; do cargo doc --locked -p "$package" --no-deps --no-default-features && cargo doc --locked -p "$package" --no-deps --all-features || exit; done

# Cargo keys its temporary registry by build path; each run must verify freshly packaged sources.
package-check:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target
    package_directory="$(mktemp -d "$PWD/target/package-check.XXXXXX")"
    trap 'rm -rf "$package_directory"' EXIT
    CARGO_TARGET_DIR="$package_directory" CARGO_BUILD_BUILD_DIR="$package_directory" \
        cargo package --workspace --locked --all-features --registry crates-io --offline --allow-dirty
