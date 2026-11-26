#show: script(
  summary: [Read invocation paths, stage metadata, and options inside a build hook, module command, or initializer.],
)

#command(
  "main",
  summary: [Return the active execution context provided by Tola.],
  params: (
    "--json": [Emit JSON text instead of a Nu record. Useful when passing context to external programs.],
  ),
)[
  Available only when invoked by Tola inside a build hook, module command, or site initializer; running it directly in a shell returns an error. Temporary directories and `reportPath` are scoped to the current run.

  #table(
    columns: 2,
    table.header([Field], [Availability and meaning]),
    [`executable`], [Path to the running `tola` binary.],
    [`inputScope`], [Active input policy: `online`, `offline`, or `pure`.],
    [`site.root`, `site.configPath`, `site.contentRoot`], [Site root, `tola.toml` path, and content directory (build hooks and module commands). A `before-build` hook may write generated `.typ` sources under `site.contentRoot`.],
    [`module.name`, `module.root`, `module.manifest`], [Module alias, root directory, and `tola-module.toml` path (module hooks, commands, and initializers).],
    [`module.options`], [Resolved module options table for the site (module hooks and commands).],
    [`hook.name`, `hook.stage`, `hook.tempDir`], [Configured hook name, stage, and per-run temporary directory (build hooks and `before-pack`).],
    [`hook.mode`, `hook.cacheDir`], [Build mode (`prod` or `dev`) and persistent cache directory across runs (build hooks).],
    [`hook.inputDir`], [Compiled outputs in `generate-outputs`, published directory in `after-publish`, or module source root in `before-pack`.],
    [`hook.outputDir`], [Staging directory where `generate-outputs` writes files declared in `outputs`.],
    [`init.outputDir`, `init.targetDir`, `init.interactive`], [Initializer staging directory, final target site directory for relative paths, and whether terminal prompts are permitted.],
    [`reportPath`], [File path where external hooks or module commands may write a JSON diagnostic report before exiting.],
  )

  #example(title: [Read fields in a Nu hook])[
    Use `get -o` for stage-specific fields such as `hook.outputDir` that may be absent in other stages.
    ```nu
    use tola get-context

    let ctx = get-context
    let out = $ctx | get -o hook.outputDir
    if $out != null {
      "Built with Tola\n" | save ($out | path join "build.txt")
    }
    ```
  ]

  #example(title: [Query context from an external shell hook])[
    ```sh
    "$TOLA_EXECUTABLE" nu @tola-scripts/context --json
    ```
  ]
]
