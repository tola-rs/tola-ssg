set working-directory := '.'

import 'just/rust.just'
import 'just/quality.just'
import 'just/release.just'
import 'just/nix.just'

# TypeScript maintenance tooling and the release scripts.
mod scripts 'just/scripts.just'
# Playwright harness covering CLI, development, and browser flows.
mod e2e 'just/e2e.just'
# VS Code client and its language-server integration tests.
mod vscode 'just/vscode.just'
# Cross-checks for the Windows MSVC release target and the GNU cross-compilation target.
mod windows 'just/windows.just'

# List the recipes below, including the ones inside modules.
[default]
help:
    @just --list --list-submodules

# Run the full workspace gate: formatting, lints, test names, script checks, tests, docs, packaging.
check: quality fmt-nix scripts::check build-library test clippy docs package-check

# Run host-local checks; platform matrices and release publication belong to GitHub Actions.
ci: version-check workflows check e2e::run vscode::check flake
