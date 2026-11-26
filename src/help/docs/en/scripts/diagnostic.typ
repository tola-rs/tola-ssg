#show: script(
  summary: [Render JSON diagnostic reports in the terminal or submit issues from a Nu hook.],
)

#command(
  "main",
  summary: [List subcommands. Inside Nu hooks, use `use tola diagnostic` to submit issues directly.],
)[
  Inside a Nu build hook or module command, call `diagnostic submit` once with the full list of issues (pass `[]` when there are none). Any issue with severity `error` fails the hook. In standalone scripts, use `diagnostic render` to display issues locally.

  #example(title: [Report a warning from a Nu hook])[
    ```nu
    use tola diagnostic

    let issues = if (open --raw content/index.typ | str contains "TODO") {
      [(diagnostic make "Remove TODO from the home page." --code draft-todo)]
    } else {
      []
    }
    $issues | diagnostic submit
    ```
  ]
]

#command(
  "render",
  summary: [Read a JSON diagnostic report from standard input and print it with terminal formatting.],
)[
  Place `--stdin` before the script name. Input may be either a JSON array of diagnostics or an object containing a `diagnostics` array (such as `tola check --json`).

  #table(
    columns: 2,
    table.header([Field], [Meaning]),
    [`severity`], [Required. One of `error`, `warning`, `info`, or `hint`.],
    [`message`], [Required. Concise description of the issue.],
    [`code`], [Optional rule identifier, such as `missing-alt`.],
    [`location`], [Optional source location object with `path` (absolute or relative to the working directory) and optional 1-based `line` and `column`.],
    [`hints`], [Optional list of short fix suggestions.],
    [`help`], [Optional list of structured fix objects with `message` and optional `location`.],
    [`notes`], [Optional list of contextual notes.],
    [`related`], [Optional list of related details, each with `message` and optional `location`.],
  )

  #example(title: [Render diagnostics from `tola check`])[
    ```sh
    tola check --json | tola nu --stdin @tola-scripts/diagnostic render
    ```
  ]

  #example(title: [Render a custom JSON report])[
    ```json
    [
      {
        "severity": "warning",
        "message": "The cover image needs alt text.",
        "code": "missing-alt",
        "location": { "path": "content/index.typ", "line": 1 },
        "hints": ["Add a descriptive alt attribute."]
      }
    ]
    ```
  ]
]
