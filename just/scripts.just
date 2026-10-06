set working-directory := '..'

# Install the workspace's locked dependencies; a stale lockfile or tree fails here.
setup:
    deno ci

# Type-check the maintenance TypeScript without emitting.
[working-directory('scripts')]
typecheck:
    deno task typecheck

# Check the maintenance TypeScript formatting.
[working-directory('scripts')]
format:
    deno task format

# Check types, run the release safety tests, and start every release command.
[default]
[working-directory('scripts')]
check:
    deno task check

# Refuse test names that describe code instead of the behaviour a test proves.
[working-directory('scripts')]
test-names:
    deno task test-names

# Regenerate the Iconify collection index Tola pins; name collections to refresh only those.
[working-directory('scripts')]
icons-index *collections:
    deno run --allow-read --allow-write --allow-net --allow-run=rustfmt --allow-env icons/index.ts {{ collections }}

# Regenerate a pronunciation table from the source file it is derived from.
[working-directory('scripts')]
pronunciations *args:
    deno run --allow-read --allow-write --allow-net pronunciations/index.ts {{ args }}

# Regenerate the fonts this repository carries from the pinned typst-assets release.
# Pass --check to verify what is committed without writing.
[working-directory('scripts')]
fonts *args:
    deno run --allow-read --allow-write --allow-net fonts/index.ts {{ args }}
