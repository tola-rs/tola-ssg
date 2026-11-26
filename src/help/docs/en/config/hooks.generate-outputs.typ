#show: config(
  summary: [Produce additional publishable files—such as search indexes or manifests—from compiled outputs.],
  related: ("[hooks]", "@tola-scripts/context", "@tola/address#output-url"),
)

Read compiled HTML and static assets from the read-only `hook.inputDir` and write every file declared in `outputs` into `hook.outputDir` (which starts empty on each run). Link to generated files from Typst using #ref("@tola/address#output-url"); Tola validates those links after hooks complete.

With `dev = "reuse"`, `tola dev` reuses the previous successful outputs when site content, hook configuration, and `rerun-on` inputs are unchanged. Use `dev = "run"` if the script depends on external or time-varying state.

#example(title: [Build a Pagefind search index])[
  ```toml
  [[hooks.generate-outputs]]
  name = "search"
  nu = "tools/index-search.nu"
  dev = "reuse"
  outputs = [{ tree = "pagefind" }]
  ```
]

#key("enable")[
  Enable this hook. Set to `false` to skip it without evaluating `enabled-by`.
]

#key("name")[
  Unique non-empty hook name within this stage (no whitespace).
]

#key("order")[
  Non-negative execution priority within the same site or module instance. Lower values run first; equal values may run concurrently.
]

#key("argv")[
  External command and arguments executed from the site root. Mutually exclusive with `nu`.
]

#key("nu")[
  Nu script path relative to the site root. Automatically watched during `tola dev`. Mutually exclusive with `argv`.
]

#key("args")[
  Optional argument list passed to the `nu` script.
]

#key("enabled-by")[
  Dot-separated boolean option path under `[options]` controlling whether this hook runs.
]

#key("dev")[
  Behavior during `tola dev`: `"run"` runs every build, `"reuse"` reuses prior outputs when inputs are unchanged, and `"skip"` omits the hook in dev mode.
]

#key("rerun-on")[
  Extra watched inputs (`{ include = [...], exclude = [...] }`) that trigger rebuilds and invalidate `dev = "reuse"`.
]

#key("outputs")[
  Required list of publish-relative output declarations, each `{ file = "path" }` or `{ tree = "dir" }`. Actual files written to `hook.outputDir` must match the declaration, stay outside `_tola`, and not collide with other outputs.
]
