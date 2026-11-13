# `@tola-scripts/context`

Look up paths and settings from a build hook, module command, or site setup script.

## `main`

Run this from a hook, module command, or setup script started by Tola. Running it directly in a terminal reports an error.

#### Context fields

Temporary directories and the report path are available for the current invocation.

| Field | When to use it |
| --- | --- |
| `executable` | Path to the running Tola executable. |
| `inputScope` | Build hooks, module commands, and initializers get `online`, `offline`, or `pure`. |
| `site.root`, `site.configPath`, `site.contentRoot` | Build hooks and module commands get the site directory, its configuration file, and its content directory. A `before-build` hook can write content for Typst under `site.contentRoot`. |
| `module.name`, `module.root`, `module.manifest` | Module build hooks, module commands, and initializers get the module alias, directory, and manifest path. |
| `module.options` | The module settings used by this site. Available in module build hooks and commands. |
| `hook.name`, `hook.stage`, `hook.tempDir` | Build hooks and `before-pack` get their configured name, stage, and temporary directory. |
| `hook.mode`, `hook.cacheDir` | Build hooks get `prod` or `dev` and a cache directory that persists across runs. |
| `hook.inputDir` | Read the compiled pages and assets in `generate-outputs` or the published files in `after-publish`. In `before-pack`, this points to the module source directory. |
| `hook.outputDir` | Write the files declared in `outputs` here during `generate-outputs`. |
| `init.outputDir`, `init.targetDir`, `init.interactive` | Initializers write files to `outputDir`. Compute relative configuration paths from `targetDir`, the final site directory. Prompt only when `interactive` is true. |
| `reportPath` | An external build hook, `before-pack` hook, or module command can write its JSON diagnostic report here before exiting. |

Example: Read the hook name in Nu:

Import `get-context` in a Nu script and use `get` to read the hook name:

```nu
use tola get-context
print (get-context | get hook.name)
```

Use `get-context --json` if you need JSON text, for example to pass to an external program.

Example: Inspect context from an external command:

In a shell hook, use `$TOLA_EXECUTABLE` to call Tola:

```sh
"$TOLA_EXECUTABLE" nu @tola-scripts/context --json
```

Leave off `--json` to read the result in a terminal.

Example: Publish a file from a hook:

The script writes `build.txt` to `hook.outputDir`. Tola publishes it at the path listed in `outputs`.

Save this as `write-build-info.nu` next to `tola.toml`:

```nu
use tola get-context
let output_dir = get-context | get hook.outputDir
"Built with Tola\n" | save ($output_dir | path join "build.txt")
```

Add this hook to `tola.toml`:

```toml
[[hooks.generate-outputs]]
name = "build-info"
nu = "write-build-info.nu"
outputs = [{ file = "build.txt" }]
```

Run `tola build` from the site root. With the default publish directory, `public/build.txt` contains `Built with Tola`.

Example: Read a field that can be absent:

Only `generate-outputs` provides `hook.outputDir`. If a script also runs in other stages, use `get -o` to get `null` when the field is missing:

```nu
use tola get-context
let out = get-context | get -o hook.outputDir
if $out != null {
  print $out
}
```

### Options

- `--json`: Print JSON. Without this option, the command displays a record in the terminal or returns a record to a Nu script.
