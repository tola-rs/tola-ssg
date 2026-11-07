set positional-arguments

# Verify Rust formatting without rewriting files.
fmt-check:
    cargo fmt --all -- --check

# Run every workspace test with all features; arguments after the recipe name reach `cargo test`.
test *args:
    cargo test --locked --workspace --all-features "$@"

# Lint every workspace target, including tests and examples.
[env('RUSTFLAGS', '-D warnings')]
clippy:
    cargo clippy --locked --workspace --all-targets --all-features -- -D warnings

# Compile the debug binary.
build:
    cargo build --locked

# Check libraries without workspace feature unification.
[env('RUSTFLAGS', '-D warnings')]
build-library:
    cargo check --locked -p tola-build --all-targets
    cargo check --locked -p tola-diagnostic --all-targets --no-default-features
    cargo check --locked -p tola-diagnostic --all-targets --all-features
    cargo check --locked -p tola-typst-syntax --all-targets --no-default-features
    cargo check --locked -p tola-typst-syntax --all-targets --no-default-features --features format
    cargo check --locked -p tola-typst --all-targets --no-default-features
    for feature in scan parallel embed-fonts network; do cargo check --locked -p tola-typst --all-targets --no-default-features --features "$feature" || exit; done
    cargo check --locked -p tola-lsp --all-targets --no-default-features
    cargo check --locked -p tola-lsp --all-targets --no-default-features --features network

# Build the public Rust documentation and fail on broken links.
[env('RUSTDOCFLAGS', '-D warnings')]
docs:
    cargo doc --locked -p tola-address -p tola-datalog -p tola-build -p tola-config -p tola-config-macros -p tola-diagnostic -p tola-subprocess -p tola-nu -p tola-highlight -p tola-icons -p tola-image -p tola-webp -p tola-minify -p tola-packages -p tola-pronunciations -p tola-slugify --no-deps
    for package in tola-typst-syntax tola-typst tola-lsp; do cargo doc --locked -p "$package" --no-deps --no-default-features && cargo doc --locked -p "$package" --no-deps --all-features || exit; done
