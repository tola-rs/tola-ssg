#show: config(
  summary: [Configure file watching and live reload behavior for `tola dev`.],
  related: ("[server]", "[hooks]"),
)

`tola dev` executes hooks according to each hook's `dev` setting; to preview a production build locally, run `tola preview`. Pass `tola dev --watch=false` to disable watching for a single run.

When developing hooks, Tola automatically watches each hook's `nu` entry script—declare extra input files in `rerun-on` so edits trigger rebuilds. Pass `tola dev --no-reuse` to force hooks with `dev = "reuse"` to run on every build.

#example(title: [Disable automatic rebuilds on file changes])[
  ```toml
  [dev]
  watch = false
  ```
]

#key("watch")[
  Rebuild and reload connected browsers automatically when watched files change.
]
