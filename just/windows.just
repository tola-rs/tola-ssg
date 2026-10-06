set working-directory := '..'

# A bare `just windows` lists these recipes instead of the argument-taking `check`.
[default]
help:
    @just --list windows

# Check application dependencies and isolated no-default library APIs on Windows.
# MSVC is the published Windows target; GNU keeps the cross-compilation target checking.
check target:
    cargo check --locked --target {{ quote(target) }} -p tola-build --all-targets
    cargo check --locked --target {{ quote(target) }} -p tola-subprocess --all-targets
    cargo check --locked --target {{ quote(target) }} -p tola --all-targets
    cargo check --locked --target {{ quote(target) }} -p tola-icons --all-targets
    cargo check --locked --target {{ quote(target) }} -p tola-typst-syntax --all-targets --no-default-features
    cargo check --locked --target {{ quote(target) }} -p tola-typst --all-targets --no-default-features
    cargo check --locked --target {{ quote(target) }} -p tola-lsp --all-targets --no-default-features

# Run the native tests, keeping each library's feature resolution isolated.
[env('RUSTFLAGS', '-D warnings')]
test:
    cargo test --locked -p tola-build
    cargo test --locked -p tola-subprocess
    cargo test --locked -p tola
    cargo test --locked -p tola-icons

# Check and test the published Windows target.
msvc: (check 'x86_64-pc-windows-msvc') test

# Cross-check the GNU target; releases ship MSVC only.
gnu: (check 'x86_64-pc-windows-gnu')
