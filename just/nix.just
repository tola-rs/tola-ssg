# Evaluate the flake for every advertised system without building it.
flake:
    nix --extra-experimental-features 'nix-command flakes' flake check --no-update-lock-file --all-systems --no-build --show-trace

# Verify that every Nix file is formatted with nixfmt without rewriting it.
fmt-nix:
    nixfmt --check flake.nix nix/*.nix
