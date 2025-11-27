#!/usr/bin/env bash
set -euo pipefail

RELEASE_DIR="release"
PIDS=()
FAILED=0

mkdir -p "$RELEASE_DIR"

build_and_copy() {
    local target=$1
    local output=$2
    local result

    echo "⏳ Building $target..."
    if result=$(nix build -j auto "./#$target" --no-link --print-out-paths 2>&1); then
        if [[ -f "$result/bin/tola" ]]; then
            cp "$result/bin/tola" "$RELEASE_DIR/$output"
        elif [[ -f "$result/bin/tola.exe" ]]; then
            cp "$result/bin/tola.exe" "$RELEASE_DIR/$output"
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

declare -A TARGETS=(
    ["x86_64-linux"]="tola-x86_64-linux-gnu"
    ["x86_64-linux-static"]="tola-x86_64-linux-musl"
    ["aarch64-linux"]="tola-aarch64-linux-gnu"
    ["aarch64-linux-static"]="tola-aarch64-linux-musl"
    ["aarch64-darwin"]="tola-aarch64-darwin"
    ["x86_64-windows"]="tola-x86_64.exe"
)

for target in "${!TARGETS[@]}"; do
    build_and_copy "$target" "${TARGETS[$target]}" &
    PIDS+=($!)
done

for pid in "${PIDS[@]}"; do
    if ! wait "$pid"; then
        FAILED=$((FAILED + 1))
    fi
done

echo ""
if [[ $FAILED -eq 0 ]]; then
    echo "🎉 All builds completed successfully!"
else
    echo "⚠️  $FAILED build(s) failed"
fi

echo ""
ls -lh "$RELEASE_DIR/"
exit $FAILED