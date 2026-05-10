#!/usr/bin/env bash
set -euo pipefail

RELEASE_DIR="release"
PIDS=()
FAILED=0

mkdir -p "$RELEASE_DIR"

check_darwin_binary() {
    local target=$1
    local binary=$2
    local otool_bin
    local deps

    [[ "$target" == *darwin* ]] || return 0

    if command -v otool >/dev/null 2>&1; then
        otool_bin=otool
    elif command -v llvm-otool >/dev/null 2>&1; then
        otool_bin=llvm-otool
    else
        echo "✗ Cannot check Darwin binary dependencies: otool not found" >&2
        return 1
    fi

    deps=$("$otool_bin" -L "$binary")
    if echo "$deps" | grep -E '/nix/store/.+\.dylib'; then
        echo "✗ Darwin binary depends on Nix store dylibs" >&2
        return 1
    fi
}

build_and_copy() {
    local package=$1
    local target=$2
    local output=$3
    local result

    echo "⏳ Building $target from .#$package..."
    if result=$(nix build -j auto "./#$package" --no-link --print-out-paths); then
        if [[ -f "$result/bin/tola" ]]; then
            check_darwin_binary "$target" "$result/bin/tola"
            cp "$result/bin/tola" "$RELEASE_DIR/$output"
            tar -czvf "$RELEASE_DIR/$output.tar.gz" -C "$RELEASE_DIR" "$output"
            rm "$RELEASE_DIR/$output"
        elif [[ -f "$result/bin/tola.exe" ]]; then
            cp "$result/bin/tola.exe" "$RELEASE_DIR/$output"
            zip -j "$RELEASE_DIR/${output%.exe}.zip" "$RELEASE_DIR/$output"
            rm "$RELEASE_DIR/$output"
        else
            echo "✗ Binary not found for $target" >&2
            return 1
        fi
        echo "✓ Built $output"
    else
        echo "✗ Failed to build $target" >&2
        echo "$result" >&2
        return 1
    fi
}

declare -A PACKAGES=(
    ["x86_64-linux"]="x86_64-linux:tola-x86_64-linux-gnu"
    ["x86_64-linux-static"]="x86_64-linux-static:tola-x86_64-linux-musl"
    ["aarch64-linux"]="aarch64-linux:tola-aarch64-linux-gnu"
    ["aarch64-linux-static"]="aarch64-linux-static:tola-aarch64-linux-musl"
    ["aarch64-darwin-release"]="aarch64-darwin:tola-aarch64-darwin"
    ["x86_64-windows"]="x86_64-windows:tola-x86_64.exe"
)

for package in "${!PACKAGES[@]}"; do
    IFS=: read -r target output <<<"${PACKAGES[$package]}"
    build_and_copy "$package" "$target" "$output" &
    PIDS+=($!)
done

for pid in "${PIDS[@]}"; do
    if ! wait "$pid"; then
        FAILED=$((FAILED + 1))
    fi
done

echo ""
if [[ $FAILED -eq 0 ]]; then
    echo "All builds completed successfully!"
else
    echo "$FAILED build(s) failed"
fi

echo ""
ls -lh "$RELEASE_DIR/"
exit $FAILED
