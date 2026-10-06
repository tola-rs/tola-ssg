# Check source formatting, TypeScript lints, and test names.
quality: fmt-check fmt-typescript fmt-just lint-typescript scripts::test-names

# Check TypeScript formatting in the workspace packages without rewriting files.
fmt-typescript:
    deno fmt --check

# Lint the workspace TypeScript packages.
lint-typescript:
    deno lint

# Check dependency licenses against the reviewed allow list in deny.toml, and dependency
# advisories against the RustSec database.
deny:
    cargo deny --locked check advisories licenses

# Validate the GitHub workflows with the development shell's actionlint.
workflows:
    actionlint

# Check this justfile and every module against the official formatter; `just --fmt` only visits the
# justfile it is given, so modules need their own pass.
fmt-just:
    just --fmt --check
    for file in just/*.just; do just --fmt --check --justfile "$file" || exit; done
