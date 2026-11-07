set working-directory := '..'

# Install the extension's locked dependencies.
[working-directory('extensions/vscode')]
setup:
    npm ci

# Type-check the extension without emitting.
[working-directory('extensions/vscode')]
typecheck:
    npm run typecheck

# Exercise the integration host; Linux needs Xvfb, and VSCODE_EXECUTABLE_PATH skips the download.
[working-directory('extensions/vscode')]
test:
    cargo build --locked -p tola
    {{ if os() == 'linux' { 'xvfb-run --auto-servernum' } else { '' } }} npm run test

# Build a local VSIX; the prepublish hook type-checks and bundles first.
[working-directory('extensions/vscode')]
package:
    npm run package

# Type-check and build a checked VSIX; run the integration suite with `just vscode::test`.
[default]
check: typecheck package
