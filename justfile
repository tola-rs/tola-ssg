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

# Run the full workspace gate: formatting, lints, script checks, tests, docs, packaging.
check: quality fmt-nix scripts::check build-library test clippy docs package-check

# Read a context document by section: FILE, then topic and section names.
ctx *args:
    @nu tools/ctx.nu {{ args }}

# Rewrite every file that `just check` verifies for formatting.
fmt:
    cargo fmt --all
    deno fmt
    just --fmt --justfile justfile
    for file in just/*.just; do just --fmt --justfile "$file" || exit; done
    nixfmt flake.nix nix/*.nix

# Run host-local checks; platform matrices and release publication belong to GitHub Actions.
ci: version-check workflows check e2e::run vscode::check flake

# Report the toolchain the recipes call, the version the repository pins, and what is missing.
doctor:
    #!/bin/sh
    set -u
    status=0
    rust_version=$(sed -n 's/^rust-version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
    deno_version=$(sed -n 's/^deno[[:space:]]*//p' .tool-versions | head -n 1)
    typos_version=$(sed -n 's|.*crate-ci/typos@[^ ]* *# *v\([0-9.]*\).*|\1|p' .github/workflows/check.yml | head -n 1)
    typos_note='not in the dev shell'
    if [ -n "$typos_version" ]; then
        typos_note="CI v$typos_version, not in the dev shell"
    fi
    pin() {
        [ -z "$2" ] || printf '%s %s' "$1" "$2"
    }
    report() {
        name=$1
        requirement=$2
        optional=${3:-}
        if command -v "$name" >/dev/null 2>&1; then
            version=$("$name" --version 2>/dev/null | head -n 1)
            version=${version#"$name "}
            [ -n "$version" ] || version='unknown version'
        else
            version=missing
            [ -n "$optional" ] || status=1
        fi
        if [ -n "$requirement" ]; then
            printf '%-11s %-46s %s\n' "$name" "$version" "$requirement"
        else
            printf '%-11s %s\n' "$name" "$version"
        fi
    }
    report cargo "$(pin 'Cargo.toml rust-version' "$rust_version")"
    report rustc "$(pin 'Cargo.toml rust-version' "$rust_version")"
    report just ''
    report deno "$(pin .tool-versions "$deno_version")"
    report nixfmt ''
    report actionlint ''
    report typos "$typos_note" optional
    if [ "$status" -ne 0 ]; then
        printf 'missing required tools; run `nix develop` for the documented environment\n' >&2
    fi
    exit "$status"

# Remove build output and the Node dependency trees Deno installs.
clean:
    rm -rf target node_modules e2e/node_modules extensions/vscode/node_modules
