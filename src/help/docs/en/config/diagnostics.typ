#show: config(
  summary: [Limit how many errors and warnings are printed to the terminal per run.],
  related: ("@tola-scripts/diagnostic",),
)

These limits affect terminal output only; any error still fails the build even if hidden from the summary. To inspect every diagnostic without truncation, pipe `tola check --json` into #ref("@tola-scripts/diagnostic#render").

#example(title: [Show up to 10 errors and suppress terminal warnings])[
  ```toml
  [diagnostics]
  max_errors = 10
  max_warnings = 0
  ```
]

#key("max_errors")[
  Maximum number of errors printed in the terminal. Set to `0` to hide errors from terminal output.
]

#key("max_warnings")[
  Maximum number of warnings printed in the terminal. Set to `0` to suppress warnings from terminal output.
]
