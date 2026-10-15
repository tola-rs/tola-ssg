# Check source formatting and TypeScript lints.
quality: fmt-check fmt-typescript fmt-just lint-typescript

# Check TypeScript formatting in the workspace packages without rewriting files.
fmt-typescript:
    deno fmt --check

# Lint the workspace TypeScript packages.
lint-typescript:
    deno lint

# Check dependency licenses against deny.toml's allow list, and advisories against RustSec.
deny:
    cargo deny --locked check advisories licenses

# Validate the GitHub workflows with the development shell's actionlint.
workflows:
    actionlint

# Check this justfile and every module; `just --fmt` only visits the justfile it is given.
fmt-just:
    just --fmt --check
    for file in just/*.just; do just --fmt --check --justfile "$file" || exit; done
