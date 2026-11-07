set working-directory := '.'

import 'just/rust.just'
import 'just/quality.just'
import 'just/release.just'
import 'just/nix.just'

# Nushell maintenance tooling and the release scripts.
mod tools 'just/tools.just'
# VS Code client and its language-server integration tests.
mod vscode 'just/vscode.just'
# Cross-checks for the Windows MSVC release target and the GNU cross-compilation target.
mod windows 'just/windows.just'

# List the recipes below, including the ones inside modules.
[default]
help:
    @just --list --list-submodules

# Run the full workspace gate: formatting, lints, script checks, tests, and docs.
check: quality fmt-nix tools::check build-library test clippy docs

# Private-module fetch/auth checks; needs TOLA_CI_* environment.
private-module *args:
    cargo run --locked -p tola -- nu tools/private-module.nu {{ args }}

# Read a context document by section: FILE, then topic and section names.
ctx *args:
    @nu tools/ctx.nu {{ args }}

# Rewrite every file that `just check` verifies for formatting.
fmt:
    cargo fmt --all
    just --fmt --justfile justfile
    for file in just/*.just; do just --fmt --justfile "$file" || exit; done
    nixfmt flake.nix nix/*.nix

# Run host-local checks; platform matrices and release publication belong to GitHub Actions.
ci: version-check workflows check vscode::check flake

# Remove build output and the Node dependency tree the extension installs.
clean:
    rm -rf target node_modules extensions/vscode/node_modules
