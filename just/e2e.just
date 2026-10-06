set working-directory := '..'
set positional-arguments

binary := if os() == 'windows' { 'tola.exe' } else { 'tola' }
profile := env('TOLA_E2E_PROFILE', 'dev')
binary-directory := if profile == 'dev' { 'debug' } else { profile }

# Install the workspace's locked dependencies and Chromium; CI passes --with-deps.
[working-directory('e2e')]
setup *args:
    deno ci
    deno task browsers "$@"

# Type-check the harness without running it.
[working-directory('e2e')]
typecheck:
    deno task typecheck

# Build the workspace binary and run the suite against it; arguments after the recipe name reach
# Playwright, such as a spec path or --grep. Set TOLA_E2E_PROFILE=release for a release build.
[default]
[working-directory('e2e')]
run *args: typecheck
    cargo build --locked -p tola --profile {{ quote(profile) }}
    TOLA_E2E_BIN="$PWD"/{{ quote('../target/' + binary-directory + '/' + binary) }} deno task test "$@"
