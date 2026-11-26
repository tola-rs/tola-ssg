#show: config(
  summary: [Run Nu scripts or external commands before discovery, after compilation, or after publication.],
  related: ("@tola-scripts/context", "@tola-scripts/diagnostic", "[build]"),
)

= Stages

#table(
  columns: 2,
  table.header([Stage], [Purpose]),
  [#ref("[[hooks.before-build]]")], [Generate Typst pages, templates, data files, or static inputs before content discovery.],
  [#ref("[[hooks.generate-outputs]]")], [Produce additional publishable files (such as search indexes) from compiled outputs.],
  [#ref("[[hooks.after-publish]]")], [Inspect or deploy the published site directory after files are written.],
)

= Execution order and concurrency

Each hook runs either a bundled-runtime Nu script (`nu` with optional `args`) or an external executable (`argv`), with the site root as its working directory.

- Within one stage and one site or module instance, hooks run in ascending `order` (default `0`). Hooks sharing the same `order` may run in parallel; configure concurrency with `build.par-hooks` and `build.hook-jobs` in #ref("[build]").
- Across instances, modules run in alphabetical alias order. `before-build` runs site hooks before module hooks; `generate-outputs` and `after-publish` run module hooks first. If a batch fails, later batches are skipped.
- All `generate-outputs` hooks read the same compiled snapshot; combine dependent steps into one script if one needs another's output.

= Command matrix

#table(
  columns: 5,
  table.header([Command], [Mode], [`before-build`], [`generate-outputs`], [`after-publish`]),
  [`tola build`], [`prod`], [Run], [Run], [Run],
  [`tola check`], [`prod`], [Run], [Run], [Skip],
  [`tola preview`], [`prod`], [Run], [Run], [Skip],
  [`tola dev`], [`dev`], [Per `dev`], [Per `dev`], [Per `dev`],
  [`tola inspect` / `vendor`], [—], [Skip], [Skip], [Skip],
)

Pass `--no-hooks` to `tola check` to skip build hooks and reuse existing generated inputs. Run `tola inspect config --mode prod --json --pretty` (or `--mode dev`) to inspect resolved hook batches and skip reasons.

= Context, diagnostics, and watching

- Read stage paths and options via `use tola get-context` in Nu or #ref("@tola-scripts/context") from external commands. `TMPDIR`/`TMP`/`TEMP` point to a fresh `hook.tempDir` per run; persistent state belongs in `hook.cacheDir`.
- Submit warnings or errors with `use tola diagnostic` (`diagnostic submit`) or write JSON to `reportPath` (see #ref("@tola-scripts/diagnostic")).
- Tola automatically watches the `nu` entry script. List extra input files in `rerun-on = { include = [...], exclude = [...] }`. In Nu hooks, `use tola get-hook; get-hook --rerun-on-digest` returns a content digest of tracked `rerun-on` inputs for custom cache keys.
