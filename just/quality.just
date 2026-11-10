# Check source formatting.
quality: fmt-check fmt-just

# Check this justfile and every module; `just --fmt` only visits the justfile it is given.
fmt-just:
    just --fmt --check
    for file in just/*.just; do just --fmt --check --justfile "$file" || exit; done

# Check dependency licenses against deny.toml's allow list.
licenses-check:
    cargo deny --locked check licenses

# Validate the GitHub workflows with the development shell's actionlint.
workflows:
    actionlint
