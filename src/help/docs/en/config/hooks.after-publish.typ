#show: config(
  summary: [Run post-publish commands—such as deployment or HTML verification—against the published directory.],
  related: ("[hooks]", "@tola-scripts/context", "@tola-scripts/diagnostic"),
)

Read the published site from the read-only `hook.inputDir`. Because publication has already completed, a failure in this stage returns a build error while leaving the newly published files in place. To add files to the published site, use #ref("[[hooks.generate-outputs]]") instead.

`tola dev` skips `after-publish` by default. When `dev = "run"` is set, the hook runs in the background after each successful build; if multiple rebuilds finish while it is busy, only the latest state runs next.

#example(title: [Upload the published site with rsync])[
  ```toml
  [[hooks.after-publish]]
  name = "upload"
  nu = "tools/upload.nu"
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
  Nu script path relative to the site root. Mutually exclusive with `argv`.
]

#key("args")[
  Optional argument list passed to the `nu` script.
]

#key("enabled-by")[
  Dot-separated boolean option path under `[options]` controlling whether this hook runs.
]

#key("dev")[
  Behavior during `tola dev`: `"skip"` (default) skips the hook; `"run"` executes it in the background after each successful dev build.
]

#key("rerun-on")[
  Extra watched inputs (`{ include = [...], exclude = [...] }`) that trigger a rebuild and re-run enabled post-publish hooks in `tola dev`.
]
