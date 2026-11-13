# `@tola-scripts/diagnostic`

Display a JSON report in the terminal or report errors and warnings from a Nu hook.

## `main`

Run `tola nu @tola-scripts/diagnostic` to list the subcommands.

In a Nu hook, call `diagnostic submit` once with all the issues, or pass `[]` when there are none. You can leave out `location` in a regular hook. In a standalone script, use `diagnostic render` to display the list.

Example: Warn about a TODO from a Nu hook:

In a site created with `tola init`, put a line containing `TODO` in `content/index.typ`. Create `tools/` and save this as `tools/check-draft.nu`:

```nu
use tola diagnostic

let issues = if (open --raw content/index.typ | str contains "TODO") {
  [
    (diagnostic make "Remove the TODO from the home page." --code draft-todo)
  ]
} else {
  []
}
$issues | diagnostic submit
```

Add the hook to `tola.toml`:

```toml
[[hooks.before-build]]
name = "check-draft"
nu = "tools/check-draft.nu"
```

Run `tola check`. If the home page source contains `TODO`, the hook prints a warning. Add `--severity error` to `diagnostic make` to fail the check instead.

## `render`

Read a JSON report from standard input. Put `--stdin` before the script name. Pass either an array of diagnostics or an object with a `diagnostics` array.

**Diagnostic fields**

- `severity`: Required. One of `error`, `warning`, `info`, or `hint`.
- `message`: Required. Describe the problem.
- `code`: Optional rule identifier, such as `missing-alt`.
- `location`: Where the issue occurs. If present, include `path`, either absolute or relative to the directory where you run the command. Optional `line` and `column` count from 1.
- `hints`: Optional strings that suggest a fix.
- `help`: Optional fixes. Give each fix a `message` and, if useful, a `location`.
- `notes`: Optional strings that give context.
- `related`: Optional details. Give each entry a `message` and, if useful, a `location`.

Example: Display a report from a file:

Start in a site with `content/index.typ`. Save this as `issues.json` next to `tola.toml`:

```json
[
  {
    "severity": "warning",
    "message": "The cover image needs alt text.",
    "code": "missing-alt",
    "location": {"path": "content/index.typ", "line": 1},
    "hints": ["Describe the image in the page's alt text."]
  }
]
```

Run this from the site root.

```sh
tola nu --stdin @tola-scripts/diagnostic render < issues.json
```

The terminal shows the `missing-alt` warning at line 1 of `content/index.typ`, followed by the suggested fix.

Example: Display the report from `tola check`:

```sh
tola check --json > check.json
tola nu --stdin @tola-scripts/diagnostic render < check.json
```
