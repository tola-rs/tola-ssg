set working-directory := '..'

# Install the workspace's locked dependencies, including the shared UI driver's.
setup:
    deno ci

# Type-check the extension without emitting.
[working-directory('extensions/vscode')]
typecheck:
    deno task typecheck

# Exercise the integration host against the debug workspace binary.
# Linux needs Xvfb; the launcher downloads VS Code unless VSCODE_EXECUTABLE_PATH is set.
[working-directory('extensions/vscode')]
test:
    cargo build --locked -p tola
    {{ if os() == 'linux' { 'xvfb-run --auto-servernum' } else { '' } }} deno task test

# Build a local VSIX; the prepublish hook type-checks and bundles first.
[working-directory('extensions/vscode')]
package:
    deno task package

# Run the integration suite and build a checked VSIX.
[default]
check: test package
