#show: config(
  summary: [Run commands before content discovery to generate Typst sources, data, or static inputs.],
  related: ("[hooks]", "@tola-scripts/context", "[assets]"),
)

Write generated inputs directly to the source paths Tola reads (for example `content/` or `static/`), and declare every generated file or directory in `generates`. Tola checks those paths after the hook finishes and watches them during `tola dev`.

When `dev = "reuse"` is set alongside `generates`, `tola dev` skips re-running the hook within the same session as long as the command, configuration, `rerun-on` inputs, and generated outputs are unchanged. List every extra file read by the hook in `rerun-on`; use `dev = "run"` if the output depends on time, network, environment variables, or external tools. Pass `tola dev --no-reuse` to force re-execution.

#example(title: [Generate a Typst content page before discovery])[
  ```toml
  [[hooks.before-build]]
  name = "release-notes"
  nu = "tools/write-release-notes.nu"
  dev = "reuse"
  rerun-on = { include = ["data/releases.json"] }
  generates = ["content/releases.typ"]
  ```
]

#key("enable")[
  Enable this hook. Set to `false` to skip it without evaluating `enabled-by`.
]

#key("name")[
  Unique non-empty hook name within this stage (no whitespace), shown in status lines and diagnostics.
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
  Dot-separated boolean option path under `[options]` (for example `"images.enable"`). Quote segments that contain dots.
]

#key("dev")[
  Behavior during `tola dev`: `"run"` runs on every build, `"reuse"` reuses unchanged `generates` outputs when tracked inputs match, and `"skip"` skips the hook in dev mode.
]

#key("rerun-on")[
  Extra watched inputs: `{ include = [...], exclude = [...] }` (`exclude` is optional). Triggers rebuilds and invalidates `dev = "reuse"` when matched files change.
]

#key("generates")[
  Site-relative files or directories produced by this hook. Must stay outside `build.publish-dir` and `.tola`.
]
