# Tola dev skill

Development conventions and operational detail. Read it by section; never load the whole file. `AGENTS.md` holds the hard rules and wins on conflict.

```sh
just ctx .agents/DEV-SKILL.md
just ctx .agents/DEV-SKILL.md <topic> <section>
just ctx --check .agents/DEV-SKILL.md
```

## Thinking

### Read

- Read the real code, tests, configuration, and callers before concluding; never infer from names, layout, or memory.
- Read enough to restate the design and its trade-offs. A skimmed heading, a snippet, or a guess is not reading.
- Name the file, function, or document section a conclusion rests on. Cannot name one: not checked.
- State uncertainty; verify with one command when possible.
- Treat prior code and prose as material, not authority. Current behaviour and tests decide.

### Ambiguity

- When the request, scope, or intended outcome is ambiguous, stop and ask; state the options and consequences instead of choosing a default.
- Confirm your understanding back before a long, destructive, or hard-to-reverse step.
- A wrong path taken to keep moving costs more than one question.

### Answer

- Answer in the user's language and register; no ceremony.
- Conclusion first, then the reason; keep only details that change a decision.
- Report commands actually run and their results; say when something was not run.
- Raise a wrong premise in the first sentence.
- When the question is what a boundary should be, answer the target before the path; see `Design`.

### Design

- A question about what a boundary should be is answered from the target, not from the current tree. State the invariant the boundary would own, the dependency direction it fixes, and the end-state graph before costing the move.
- Existing coupling, missing prerequisites, and in-flight work decide when the change lands, never whether the target is right. Name them as schedule when they are schedule; never let them stand in for design merit.
- Test every perceived blocker as a claim: say what it would take to dissolve it and whether the dissolved state is cleaner than the alternative. A blocker that dissolves into a worse interface is the argument for `no`.
- A `no` must hold even if its objections were solved, and it names the falsifier that would reverse it.
- Separate the two answers — "the target should be X" and "the path is Y then Z" — and do not merge them to avoid defending the target.

### Report

- After every step, report: what changed, what ran, what it returned, what comes next.
- Do not run a long silent batch of edits. One coherent step, one report.
- Name decisions the user must make instead of choosing silently.

### Pacing

- Watch the ratio of writing to reading, deleting, and reviewing; when it drifts, say so instead of producing more.
- Propose a pause: re-read the recent change, then improve quality, readability, naming, architecture, organization, concision, and clarity.
- The pause is the author's call; do not begin an unrequested refactor.

### Digestion

- Generation outruns digestion: writing outruns reading, deleting, refactoring, and maintaining. That, not architecture or language, is the failure mode to watch.
- Volume is not quality: test count, layer count, and document count do not make a codebase maintainable, and bloated, duplicated, hard-to-read code is failure regardless of them.
- Deletion is digestion: addition needs a reason, deletion does not; prior code, documents, and conclusions are material, not authority — judge them again.
- Keep context small enough to read in one pass; keep tests only where they protect observable behaviour.

### Pushback

- When the user questions, challenges, or corrects you, reflect on the gap between the rule and the outcome before defending or fixing.
- Then ask whether the governing rule or context document should change; do not silently rewrite a constraint the user did not name.
- Record the decision in the document that owns it: `AGENTS.md` for hard rules, `DEV-SKILL.md` for practice, `CONTEXT.md` for shape.

### Subagents

- Delegate independent reconnaissance, fixed-brief implementation, or review. Keep planning, decisions, integration, and verification.
- A subagent edits; it does not build. Hand it files and boundaries, and keep compilation, test runs, and the final review for the Lead. Several agents compiling one workspace starve the machine and stall each other.
- A brief is self-contained: goal, rejected options, relevant files, boundaries.
- Follow the live agent for follow-ups; do not respawn for the same result.

## Context

### Ownership

Each document has one job; a fact lives in exactly one of them.

| Document | Holds | Never holds |
|---|---|---|
| `AGENTS.md` | hard rules | practice, subsystem shape |
| this file | toolchain, language conventions, editing, tests, validation, digestion | rules, subsystem shape, current commands |
| `.agents/CONTEXT.md` | layout, build lifecycle, vocabulary, child-context index | commands, toolchain, lessons |
| `CONTEXT.md` beside code | one subsystem's current shape, ownership, invariants, boundaries | rules, practice, history |

- A fact a reader needs before touching code belongs in `CONTEXT.md`; a fact about how to work belongs here; a fact that must never be broken belongs in `AGENTS.md`.
- Command lines belong here under Toolchain. A `CONTEXT.md` names a command only when the choice of command is itself the invariant.
- History, incidents, and "we learned that" are practice: they belong here, never in a `CONTEXT.md`.
- When a fact moves, delete it from where it was; never copy it into two documents.

### Reading

- `just ctx FILE` prints the preamble and heading tree; `just ctx FILE TOPIC [SECTION]` prints one section.
- One file per invocation; section paths are heading levels separated by spaces; `PARENT / CHILD` also resolves.
- Prefer `just ctx` over `cat`, an editor, or a whole-file read for `AGENTS.md`, `DEV-SKILL.md`, every `CONTEXT.md`, and `.agents/designs/*.md`; open the file only to edit it or when it has no headings.
- Read the outline first. Never `cat` a context document whole.
- Do not pipe `ctx` output through `head` or `tail`: the outline is already bounded, and section selection is exact; truncating hides content and breaks the pipe.
- `CONTEXT.md` nearest the code, then its parent, ending at `.agents/CONTEXT.md`; `DEV-SKILL.md` for how to work; `AGENTS.md` for hard rules.

### Format

- ATX headings only: one `#` title, then `##` topics and `###` sections. No level jumps, no H4+, no setext.
- Exactly one `#`, first; a non-empty preamble under it.
- A heading path is unique; keep headings stable when editing a section.
- `just ctx --check FILE` enforces: one H1, first heading H1, non-empty preamble, no level jump, non-empty title, unique full path.

### Writing

- Keep each context file small enough to review in one pass; one section, one subject.
- Headings are the reader's entry point: name each section plainly, keep one subject per section, and make the outline navigable without reading the body.
- No volatile metrics: line counts, file counts, test counts, commit counts, and sizes go stale; state the principle or the decision instead.
- Describe current ownership and invariants, not history or plans.
- When a subsystem moves, update the nearest `CONTEXT.md`; when a rule changes, update `AGENTS.md` or this file.

## Toolchain

### just

- `just` is the entry point. `just --list` shows recipes.
- `just check` runs the workspace gate; `just test`, `just fmt`, `just docs`, `just clippy` are focused pieces.
- Recipes hold no hidden logic; put scripts in `tools/` and forward.

### Nix

- `nix develop` supplies the pinned toolchain; a missing tool fails the recipe that needs it.
- `direnv` loads the same shell from `.envrc` when installed; `nix develop` remains the documented entry point.
- Change tool versions in `flake.nix` or `.tool-versions`, never by assuming the host has them.

### Rust

- Workspace crates live under `crates/`; the application is `src/` (`tola`).
- Focused loops: `cargo test --locked -p tola <name>`, `cargo test --locked -p tola-build`.
- Format with `cargo fmt --all`; lint with `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`.
- Test-only imports and helpers live inside `mod tests`. Never mark an item or import `#[cfg(test)]` beside production code: a helper two test modules share is written in each `mod tests` where it is used.

### Nushell

Nushell 0.116.1 — the version `.tool-versions` and the dev shell pin; `nu --version` reports the one you are running. Nu is a typed pipeline shell: commands pass tables, records, and lists through pipes instead of text, so `where`, `each`, `select`, and `columns` replace `awk`, `sed`, and `jq`.

Maintenance scripts live in `tools/` and `just` forwards to them; shared helpers are `tools/lib/*.nu`. Read Key rules and Common mistakes before writing a script, then the table for the category you need. The tables at the end list every built-in in the categories Tola uses, with Nu's own descriptions, taken from `scope commands` on 0.116.1; refresh one with `nu -c 'scope commands | where category == "strings" | select name description'`.

#### Key rules

- String interpolation uses parentheses: `$"hello ($name)"`. Braces are literal text.
- No shell syntax: `;` sequences commands, `$env.VAR` reads a variable, `(cmd)` substitutes, and `o+e>|` merges stderr into the pipe.
- Values are immutable. `mut x = 1` then `$x = 2` is the only way to reassign.
- Pipelines carry structured data. `$in` is the current value inside a block, and column names are in scope inside `where` and `each`.
- Double quotes interpolate and escape; single quotes are literal. Backticks name a command, they are not strings.
- A command that returns `nothing` (`save`, `print`) cannot feed the next stage; pipe the value instead of chaining after the write.

#### Common mistakes

| Mistake | Fix |
|---|---|
| `$"hello {$name}"` | `$"hello ($name)"` |
| `cmd 2>&1` | `cmd o+e>\| ...` at statement level |
| `$HOME/path` | `$"($env.HOME)/path"` |
| `export FOO=bar` | `$env.FOO = "bar"` |
| `where ($in.a > 1) and ($in.b > 2)` | the second `$in` rebinds to the bool: `where a > 1 and b > 2` |
| `where not ($in.a == 1)` | `$in` is out of scope there: `where a != 1` or `where { \|r\| not ($r.a == 1) }` |
| `where not a == 1` | parses as `not a`: `where a != 1` |
| `where col \| cmd` | parsed as two stages: `where ($in.col \| cmd)` |
| `return -1` | parses as a flag: `return (-1)` |
| `let left-pre = 1` | identifiers cannot contain hyphens: `left_pre` |
| `str contains --regex x` | no such flag: `parse --regex` with a named group |
| `open x.json \| from json` | `open` already decodes by extension; the pipe fails |
| `http post --data ...` | the body is an argument: `http post <url> <data>` |
| `just ctx ... \| head` | the reader fails with `broken_pipe`: select a section instead |

#### Scripts

- `main` is the entry point; positionals first, then `--flags` with defaults: `main [command: string, operand?: string, --output: string = '']`. `--help` is generated from that signature.
- `$env.FILE_PWD` is the script's own directory and `$nu.os-info` gives `name` (`macos`, `linux`, `windows`), `arch`, and `family`. Resolve every path from those, never from the caller's working directory.
- `error make { msg: '...' }` prints the message and exits 1; `exit 2` is for usage errors.
- A script returns its last expression. End with `null` when it should print nothing.

#### Modules

- Shared helpers are `export def` and `export const` in `tools/lib/*.nu`, imported with `use ./lib/<file>.nu *`, which resolves relative to the importing file.
- Give a custom command typed parameters and a return type: `export def archive-name [name: string, version: string]: nothing -> string`.
- `export use` re-exports another module's surface. One module per subject, no cycles, and no catch-all utility module.

#### Data and pipelines

- Tables are lists of records: `where`, `each`, `par-each`, `select`, `get`, `reject`, `group-by`, `sort-by`, `transpose`, `columns`.
- Record key order is preserved, so `columns` is the order to iterate when it matters.
- `open x.json`, `x.toml`, `x.yaml`, and `x.csv` already decode by extension; `open --raw` gives text or bytes.
- `from json`, `from toml`, `to json`, and `to toml` convert between text and values. `http get` and `http post` already decode JSON.
- `parse --regex` needs named capture groups and returns a table; an empty table means no match.
- `describe` names a value's type; `into int`, `into float`, `into string`, and `into datetime` convert.
- `open --raw <file> | hash sha256` hashes bytes; `save --force` writes a file.

#### External commands

- `^` runs a program: `^tar`, `^git`, `^cargo`. Without it, Nu resolves builtins and aliases first.
- `^cmd ... | complete` captures `stdout`, `stderr`, and `exit_code`. Without it a failing command does not stop the script, so check `$env.LAST_EXIT_CODE`.
- `o+e>|` merges stderr into the pipe at statement level; inside a subexpression it redirects the rest of the script, so keep it a statement.
- Arguments are values, not a shell string: `^cmd $arg`, never `^cmd "$a $b"`.

#### Errors

- `try { ... } catch { |e| ... }` handles a failure; `$e.msg` is its message.
- `error make { msg: '...', label: { text: '...', span: (metadata $value).span } }` points at the value at fault.
- A failing script must exit non-zero: `error make` does, a bare `print` does not.

#### Core

| Command | Purpose |
|---|---|
| `alias` | Alias a command (with optional flags) to a new name. |
| `attr` | Various attributes for custom commands. |
| `attr category` | Attribute for adding a category to custom commands. |
| `attr complete` | Attribute for using another command as a completion source for all arguments. |
| `attr complete external` | Attribute for enabling use of the external completer for internal commands. |
| `attr deprecated` | Attribute for marking a command or flag as deprecated. |
| `attr example` | Attribute for adding examples to custom commands. |
| `attr interactive` | Attribute marking a completer as interactive. |
| `attr search-terms` | Attribute for adding search terms to custom commands. |
| `break` | Break a loop. |
| `collect` | Collect a stream into a value. |
| `commandline` | View the current command line input buffer. |
| `commandline complete` | Complete a string using the default completions. |
| `commandline edit` | Modify the current command line input buffer. |
| `commandline get-cursor` | Get the current cursor position. |
| `commandline set-cursor` | Set the current cursor position. |
| `commandline set-prompt` | Replace the current prompt and repaint it in place, without disturbing the line being edited. |
| `const` | Create a parse-time constant. |
| `continue` | Continue a loop from the next iteration. |
| `def` | Define a custom command. |
| `describe` | Describe the type and structure of the value(s) piped in. |
| `do` | Run a closure, providing it with the pipeline input. |
| `echo` | Returns its arguments, ignoring the piped-in value. |
| `error` | Various commands for working with errors. |
| `error make` | Create an error. |
| `export` | Export definitions or environment variables from a module. |
| `export alias` | Alias a command (with optional flags) to a new name and export it from a module. |
| `export const` | Use parse-time constant from a module and export them from this module. |
| `export def` | Define a custom command and export it from a module. |
| `export extern` | Define an extern and export it from a module. |
| `export module` | Export a custom module from a module. |
| `export use` | Use definitions from a module and export them from this module. |
| `extern` | Define a signature for an external command. |
| `for` | Loop over a range. |
| `help` | Display help information about different parts of Nushell. |
| `help aliases` | Show help on nushell aliases. |
| `help commands` | Show help on nushell commands. |
| `help escapes` | Show help on nushell string escapes. |
| `help externs` | Show help on nushell externs. |
| `help modules` | Show help on nushell modules. |
| `help operators` | Show help on nushell operators. |
| `help pipe-and-redirect` | Show help on nushell pipes and redirects. |
| `hide` | Hide definitions in the current scope. |
| `hide-env` | Hide environment variables in the current scope. |
| `if` | Conditionally run a block. |
| `ignore` | Ignore selected output streams from the previous command in the pipeline. |
| `is-admin` | Check if nushell is running with administrator or root privileges. |
| `let` | Create a variable and give it a value. |
| `loop` | Run a block in a loop. |
| `match` | Conditionally run a block on a matched value. |
| `module` | Define a custom module. |
| `mut` | Create a mutable variable and give it a value. |
| `overlay` | Commands for manipulating overlays. |
| `overlay hide` | Hide an active overlay. |
| `overlay list` | List all overlays with their active status. |
| `overlay new` | Create an empty overlay. |
| `overlay use` | Use definitions from a module as an overlay. |
| `return` | Return early from a custom command. |
| `run` | Runs a script file in an isolated scope as part of a pipeline. |
| `scope` | Commands for getting info about what is in scope. |
| `scope aliases` | Output info on the aliases in the current scope. |
| `scope commands` | Output info on the commands in the current scope. |
| `scope engine-stats` | Output stats on the engine in the current state, including interactive last-result size info. |
| `scope externs` | Output info on the known externals in the current scope. |
| `scope modules` | Output info on the modules in the current scope. |
| `scope variables` | Output info on the variables in the current scope. |
| `source` | Runs a script file in the current context. |
| `source-env` | Source the environment from a source file into the current environment. |
| `try` | Try to run a block, if it fails optionally run a catch closure. |
| `use` | Use definitions from a module, making them available in your shell. |
| `version` | Display Nu version, and its build configuration. |
| `while` | Conditionally run a block in a loop. |

#### Filters

| Command | Purpose |
|---|---|
| `all` | Test if every element of the input fulfills a predicate expression. |
| `any` | Tests if any element of the input fulfills a predicate expression. |
| `append` | Append any number of rows to a table. |
| `chunk-by` | Divides a sequence into sub-sequences based on a closure. |
| `chunks` | Divide a list, table or binary input into chunks of `chunk_size`. For binary input, `chunk_size` can also be specified as a filesize. |
| `columns` | Given a record or table, produce a list of its columns' names. |
| `combinations` | Generates all combinations of size k from the input list. |
| `compact` | Creates a table with non-empty rows. |
| `default` | Sets a default value if a row's column is missing or null. |
| `difference` | Returns a list of unique elements in the input that are not present in the other list. |
| `drop` | Remove items/rows from the end of the input list/table, or remove bytes from the end of binary data. Counterpart of `skip`. Opposite of `last`. For binary input, `rows` can also be specified as a filesize. |
| `drop column` | Remove N columns at the right-hand end of the input table. To remove columns by name, use `reject`. |
| `drop nth` | Drop the selected rows. |
| `each` | Run a closure on each row of the input list, creating a new list with the results. |
| `each while` | Run a closure on each row of the input list until a null is found, then create a new list with the results. |
| `enumerate` | Enumerate the elements in a stream. |
| `every` | Show (or skip) every n-th row, starting from the first one. |
| `find` | Search for terms in the input data. |
| `first` | Return only the first several rows of the input. Counterpart of `last`. Opposite of `skip`. For binary input, rows can also be specified as a filesize. |
| `flatten` | Flatten a table by extracting nested values. |
| `group-by` | Splits a list or table into groups, and returns a record containing those groups. |
| `headers` | Use the first row of the table as column names. |
| `insert` | Insert a new column, using an expression or closure to create each row's values. |
| `interleave` | Read multiple streams in parallel and combine them into one stream. |
| `intersect` | Returns a list of unique elements present in both the input and the provided list. |
| `is-empty` | Check for empty values. |
| `is-not-empty` | Check for non-empty values. |
| `items` | Given a record, iterate on each pair of column name and associated value. |
| `join` | Join two tables. |
| `last` | Return only the last several rows of the input. Counterpart of `first`. Opposite of `drop`. For binary input, rows can also be specified as a filesize. |
| `length` | Count the number of items in an input list, rows in a table, or bytes in binary data. |
| `lines` | Converts input to lines. |
| `matrix` | Various commands for working with matrices. |
| `matrix add` | Add a matrix or scalar to a matrix. |
| `matrix get-col` | Extract a column from a 2D matrix. |
| `matrix get-row` | Extract a row from a matrix. |
| `matrix identity` | Create an identity matrix of the given size. |
| `matrix map` | Apply a closure to each element of a matrix and return a new matrix. |
| `matrix max` | Find the maximum value in a matrix, or max along an axis. |
| `matrix mean` | Compute the mean of all elements in a matrix. |
| `matrix multiply` | Multiply two matrices using dot product. |
| `matrix reduce` | Reduce all elements of a matrix to a single value. |
| `matrix reshape` | Change the dimensions of a matrix. |
| `matrix scale` | Multiply all elements of a matrix by a scalar. |
| `matrix set-col` | Replace a column in a 2D matrix. |
| `matrix set-row` | Replace a row in a matrix. |
| `matrix subtract` | Subtract a matrix or scalar from a matrix. |
| `matrix sum` | Sum all elements of a matrix, or sum along an axis. |
| `matrix transpose` | Transpose a matrix (swap rows and columns). For n-dimensional arrays, reverses all axes. |
| `matrix zeros` | Create a matrix filled with zeros. |
| `merge` | Merge the input with a record or table, overwriting values in matching columns. |
| `merge deep` | Merge the input with a record or table, recursively merging values in matching columns. |
| `move` | Moves columns relative to other columns or make them the first/last columns. Flags are mutually exclusive. |
| `par-each` | Run a closure on each row of the input list in parallel, creating a new list with the results. |
| `permutations` | Generates all permutations of the input list. |
| `prepend` | Prepend any number of rows to a table. |
| `reduce` | Aggregate a list (starting from the left) to a single value using an accumulator closure. |
| `rename` | Creates a new table with columns renamed. |
| `reverse` | Reverses the input list or table. |
| `roll` | Rolling commands for tables. |
| `roll down` | Roll table rows down. |
| `roll left` | Roll record or table columns left. |
| `roll right` | Roll table columns right. |
| `roll up` | Roll table rows up. |
| `rotate` | Rotates a table or record clockwise (default) or counter-clockwise (use --ccw flag). |
| `semver` | Various commands for working with semantic versions. |
| `semver bump` | Bump a semantic version to the next level. |
| `shuffle` | Shuffle rows randomly. |
| `skip` | Skip the first several rows of the input. Counterpart of `drop`. Opposite of `first`. For binary input, n can also be specified as a filesize. |
| `skip until` | Skip elements of the input until a predicate is true. |
| `skip while` | Skip elements of the input while a predicate is true. |
| `slice` | Return only the selected rows. |
| `sort` | Sort the input in increasing order. |
| `sort-by` | Sort by the given cell path or closure. |
| `split list` | Split a list into multiple lists using a separator. |
| `take` | Take only the first n elements of a list, or the first n bytes of a binary value. For binary input, n can also be specified as a filesize. |
| `take until` | Take elements of the input until a predicate is true. |
| `take while` | Take elements of the input while a predicate is true. |
| `tee` | Copy a stream to another command in parallel. |
| `transpose` | Transposes the table contents so rows become columns and columns become rows. |
| `union` | Returns a list of unique elements from both the input and the provided list. |
| `uniq` | Return the distinct values in the input. |
| `uniq-by` | Return the distinct values in the input by the given column(s). |
| `update` | Update an existing column to have a new value. |
| `update cells` | Update the table cells. |
| `upsert` | Update an existing column to have a new value, or insert a new column. |
| `values` | Given a record or table, produce a list of its columns' values. |
| `where` | Filter values of an input list based on a condition. |
| `window` | Creates a sliding window of `window_size` that slide by n rows/elements across input. |
| `wrap` | Wrap the value into a column. |
| `zip` | Combine a stream with the input. |

#### Strings

| Command | Purpose |
|---|---|
| `char` | Output special characters (e.g., 'newline'). |
| `decode` | Decode bytes into a string. |
| `detect` | Various commands for detecting things. |
| `detect columns` | Attempt to automatically split text into multiple columns. |
| `detect type` | Infer Nushell datatype from a string. |
| `encode` | Encode a string into bytes. |
| `format` | Various commands for formatting data. |
| `format date` | Format a given date using a format string. |
| `format duration` | Outputs duration with a specified unit of time. |
| `format filesize` | Converts a column of filesizes to some specified format. |
| `format pattern` | Format columns into a string using a simple pattern. |
| `nu-check` | Validate and parse Nushell input content. |
| `nu-highlight` | Syntax highlight the input string. |
| `parse` | Parse columns from string data using a simple pattern or a supplied regular expression. |
| `print` | Print the given values to stdout. |
| `split` | Split contents across desired subcommand (like row, column) via the separator. |
| `split chars` | Split a string into a list of characters. |
| `split column` | Split a string into multiple columns using a separator. |
| `split row` | Split a string into multiple rows using a separator. |
| `split words` | Split a string's words into separate rows. |
| `str` | Various commands for working with string data. |
| `str camel-case` | Convert a string to camelCase. |
| `str capitalize` | Capitalize the first letter of text. |
| `str contains` | Checks if string input contains a substring. |
| `str distance` | Compare two strings and return the edit distance/Levenshtein distance. |
| `str ends-with` | Check if an input ends with a string. |
| `str escape-regex` | Escapes special characters in the input string with '\'. |
| `str expand` | Generates all possible combinations defined in brace expansion syntax. |
| `str index-of` | Returns start index of first occurrence of string in input, or -1 if no match. |
| `str join` | Concatenate multiple strings into a single string, with an optional separator between each. |
| `str kebab-case` | Convert a string to kebab-case. |
| `str length` | Output the length of any strings in the pipeline. |
| `str lowercase` | Convert text to lowercase. |
| `str pascal-case` | Convert a string to PascalCase. |
| `str replace` | Find and replace text in the input string. |
| `str reverse` | Reverse every string in the pipeline. |
| `str screaming-snake-case` | Convert a string to SCREAMING_SNAKE_CASE. |
| `str snake-case` | Convert a string to snake_case. |
| `str starts-with` | Check if an input starts with a string. |
| `str stats` | Gather word count statistics on the text. |
| `str substring` | Get part of a string. Note that the first character of a string is index 0. |
| `str title-case` | Convert a string to Title Case. |
| `str trim` | Trim whitespace or specific character. |
| `str uppercase` | Convert text to uppercase. |
| `url decode` | Converts a percent-encoded web safe string to a string. |
| `url encode` | Converts a string to a percent encoded web safe string. |

#### Conversions

| Command | Purpose |
|---|---|
| `decode base32` | Decode a Base32-encoded value. |
| `decode base32hex` | Encode a base32hex value. |
| `decode base64` | Decode a Base64-encoded value. |
| `decode hex` | Decode a hex-encoded value. |
| `encode base32` | Encode a string or binary value using Base32. |
| `encode base32hex` | Encode a binary value or a string using base32hex. |
| `encode base64` | Encode a string or binary value using Base64. |
| `encode hex` | Hex encode a binary value or a string. |
| `fill` | Fill and align text in columns. |
| `format bits` | Convert value to a string of binary data represented by 0 and 1. |
| `format number` | Format a number. |
| `from` | Parse a string or binary data into structured data. |
| `from csv` | Parse text as .csv and create table. |
| `from json` | Convert JSON text into structured data. |
| `from kdl` | Convert KDL text into structured data. |
| `from md` | Convert markdown text into human-friendly structured rows. Use --verbose for the full AST. |
| `from msgpack` | Convert MessagePack data into Nu values. |
| `from msgpackz` | Convert brotli-compressed MessagePack data into Nu values. |
| `from nuon` | Convert from nuon to structured data. |
| `from ods` | Parse OpenDocument Spreadsheet(.ods) data and create table. |
| `from ssv` | Parse text as space-separated values and create a table. The default minimum number of spaces counted as a separator is 2. |
| `from toml` | Parse text as .toml and create record. |
| `from tsv` | Parse text as .tsv and create table. |
| `from url` | Parse url-encoded string as a record. |
| `from xlsx` | Parse binary Excel(.xlsx) data and create table. |
| `from xml` | Parse text as .xml and create record. |
| `from yaml` | Parse text as .yaml/.yml and create table. |
| `from yml` | Parse text as .yaml/.yml and create table. |
| `into` | Commands to convert data from one type to another. |
| `into binary` | Convert value to a binary primitive. |
| `into bool` | Convert value to a boolean. |
| `into cell-path` | Convert value to a cell-path. |
| `into datetime` | Convert text or timestamp into a datetime. |
| `into duration` | Convert value to a duration. |
| `into filesize` | Convert value to a filesize. |
| `into float` | Convert data into floating point number. |
| `into glob` | Convert value to a glob pattern. |
| `into int` | Convert value to an integer. |
| `into matrix` | Convert a nushell table or list of lists into a matrix. |
| `into record` | Convert value to a record. |
| `into semver` | Convert a value (string, record, or semver) to a semantic version. |
| `into semver-range` | Convert a string to a semver range. |
| `into sqlite` | Convert table into a SQLite database. |
| `into string` | Convert value to a string. |
| `into value` | Convert custom values into base values. |
| `matrix into-nu` | Convert a matrix to a nushell table (list of lists by default). |
| `split cell-path` | Split a cell-path into its components. |
| `to` | Translate structured data to various formats. |
| `to csv` | Convert table into .csv text . |
| `to html` | Convert table into simple HTML. |
| `to json` | Converts table data into JSON text. |
| `to kdl` | Converts structured data into KDL text. |
| `to md` | Convert table into simple Markdown. |
| `to msgpack` | Convert Nu values into MessagePack. |
| `to msgpackz` | Convert Nu values into brotli-compressed MessagePack. |
| `to nuon` | Converts table data into Nuon (Nushell Object Notation) text. |
| `to text` | Convert data into plain text format. |
| `to toml` | Convert record into .toml text. |
| `to tsv` | Convert table into .tsv text. |
| `to txt` | Convert data into plain text format. |
| `to xml` | Convert special record structure into .xml text. |
| `to yaml` | Convert table into .yaml/.yml text. |
| `to yml` | Convert table into .yaml/.yml text. |

#### Filesystem

| Command | Purpose |
|---|---|
| `cd` | Change the current working directory. |
| `cp` | Copy files using uutils/coreutils cp. |
| `du` | Find disk usage sizes of specified items. |
| `glob` | Creates a list of files and/or folders based on the glob pattern provided. |
| `idx` | Manage in-memory file index state. |
| `idx dirs` | List indexed directories, or fuzzy-match directories by query. |
| `idx drop` | Drop the current idx runtime from memory. |
| `idx files` | List indexed files, or fuzzy-match files by query. |
| `idx find` | Search idx with fuzzy matching across files and directories by default. |
| `idx init` | Initialize the in-memory idx index for a path. |
| `idx search` | Search indexed file contents. |
| `idx status` | Show status information for the global in-memory idx runtime. |
| `idx watch` | Stream filesystem change events from the live idx index. |
| `load-env` | Loads an environment update from a record. |
| `ls` | List the filenames, sizes, and modification times of items in a directory. |
| `mkdir` | Create directories, with intermediary directories if required using uutils/coreutils mkdir. |
| `mktemp` | Create temporary files or directories using uutils/coreutils mktemp. |
| `mv` | Move files or directories using uutils/coreutils mv. |
| `open` | Load a file into a cell, converting to table if possible (avoid by appending '--raw'). |
| `path` | Explore and manipulate paths. |
| `path basename` | Get the final component of a path. |
| `path dirname` | Get the parent directory of a path. |
| `path exists` | Check whether a path exists. |
| `path expand` | Try to expand a path to its absolute form. |
| `path join` | Join a structured path or a list of path parts. |
| `path parse` | Convert a path into structured data. |
| `path relative-to` | Express a path as relative to another path. |
| `path self` | Get the absolute path of the script or module containing this command at parse time. |
| `path split` | Split a path into a list based on the system's path separator. |
| `path type` | Get the type of the object a path refers to (e.g., file, dir, symlink). |
| `rm` | Remove files and directories. |
| `save` | Save a file. |
| `start` | Open a folder, file, or website in the default application or viewer. |
| `touch` | Creates one or more files. |
| `watch` | Watch for file changes and execute Nu code when they happen. |

#### Math

| Command | Purpose |
|---|---|
| `bits` | Various commands for working with bits. |
| `bits and` | Performs bitwise and for ints or binary values. |
| `bits not` | Performs logical negation on each bit. |
| `bits or` | Performs bitwise or for ints or binary values. |
| `bits rol` | Bitwise rotate left for ints or binary values. |
| `bits ror` | Bitwise rotate right for ints or binary values. |
| `bits shl` | Bitwise shift left for ints or binary values. |
| `bits shr` | Bitwise shift right for ints or binary values. |
| `bits xor` | Performs bitwise xor for ints or binary values. |
| `bytes` | Various commands for working with byte data. |
| `bytes add` | Add specified bytes to the binary input. |
| `bytes at` | Get bytes from the input defined by a range. |
| `bytes build` | Create a binary value from the provided arguments. |
| `bytes collect` | Concatenate multiple binary into a single binary, with an optional separator between each. |
| `bytes ends-with` | Check if binary data ends with a pattern. |
| `bytes index-of` | Returns start index of first occurrence of pattern in bytes, or -1 if no match. |
| `bytes length` | Output the length of any bytes in the pipeline. |
| `bytes remove` | Remove specified bytes from the input. |
| `bytes replace` | Find and replace bytes in binary data. |
| `bytes reverse` | Reverse the bytes in the pipeline. |
| `bytes split` | Split input into multiple items using a separator. |
| `bytes starts-with` | Check if binary data starts with a pattern. |
| `date` | Date-related commands. |
| `date from-human` | Convert a human readable datetime string to a datetime. |
| `date humanize` | Print a 'humanized' format for the date, relative to now. |
| `date list-timezone` | List supported time zones and their current offsets. |
| `date now` | Get the current date. |
| `date to-timezone` | Convert a date to a given time zone. |
| `hash` | Apply hash function. |
| `hash md5` | Hash a value using the md5 hash algorithm. |
| `hash sha256` | Hash a value using the sha256 hash algorithm. |
| `hash sha512` | Hash a value using the sha512 hash algorithm. |
| `math` | Use mathematical functions as aggregate functions on a list of numbers or tables. |
| `math abs` | Returns the absolute value of a number. |
| `math arccos` | Returns the arccosine of the number. |
| `math arccosh` | Returns the inverse of the hyperbolic cosine function. |
| `math arcsin` | Returns the arcsine of the number. |
| `math arcsinh` | Returns the inverse of the hyperbolic sine function. |
| `math arctan` | Returns the arctangent of the number. |
| `math arctanh` | Returns the inverse of the hyperbolic tangent function. |
| `math avg` | Returns the average of a list of numbers. |
| `math cbrt` | Returns the real-valued cube root of the input number. |
| `math ceil` | Returns the ceil of a number (smallest integer greater than or equal to that number). |
| `math cos` | Returns the cosine of the number. |
| `math cosh` | Returns the hyperbolic cosine of the number. |
| `math exp` | Returns e raised to the power of x. |
| `math floor` | Returns the floor of a number (largest integer less than or equal to that number). |
| `math ln` | Returns the natural logarithm. Base: (math e). |
| `math log` | Returns the logarithm for an arbitrary base. |
| `math max` | Returns the maximum of a list of values, or of columns in a table. |
| `math median` | Computes the median of a list of numbers. |
| `math min` | Finds the minimum within a list of values or tables. |
| `math mode` | Returns the most frequent element(s) from a list of numbers or tables. |
| `math product` | Returns the product of a list of numbers or the products of each column of a table. |
| `math round` | Returns the input number rounded to the specified precision. |
| `math sin` | Returns the sine of the number. |
| `math sinh` | Returns the hyperbolic sine of the number. |
| `math sqrt` | Returns the square root of the input number. |
| `math stddev` | Returns the standard deviation of a list of numbers, or of each column in a table. |
| `math sum` | Returns the sum of a list of numbers or of each column in a table. |
| `math tan` | Returns the tangent of the number. |
| `math tanh` | Returns the hyperbolic tangent of the number. |
| `math variance` | Returns the variance of a list of numbers or of each column in a table. |

#### Network

| Command | Purpose |
|---|---|
| `http` | Various commands for working with http methods. |
| `http delete` | Delete the specified resource. |
| `http get` | Fetch the contents from a URL. |
| `http head` | Get the headers from a URL. |
| `http options` | Requests permitted communication options for a given URL. |
| `http patch` | Send a PATCH request to a URL with a request body. |
| `http pool` | Configure and reset builtin http connection pool. |
| `http post` | Send a POST request to a URL with a request body. |
| `http put` | Send a PUT request to a URL with a request body. |
| `port` | Get a free TCP port from system. |
| `url` | Various commands for working with URLs. |
| `url build-query` | Converts record or table into query string applying percent-encoding. |
| `url join` | Convert a record to a URL string. |
| `url parse` | Parse a URL string into structured data. |
| `url split-query` | Converts query string into table applying percent-decoding. |

#### Environment

| Command | Purpose |
|---|---|
| `ast` | Print the abstract syntax tree (ast) for a pipeline. |
| `cal` | Display a calendar. |
| `complete` | Capture the outputs and exit code from an external piped in command in a nushell table. |
| `config` | Edit nushell configuration files. |
| `config env` | Edit nu environment configurations. |
| `config flatten` | Show the current configuration in a flattened form. |
| `config nu` | Edit nu configurations. |
| `config reset` | Reset nushell environment configurations to default, and saves old config files in the config location as oldconfig.nu and oldenv.nu. |
| `config use-colors` | Get the configuration for color output. |
| `debug` | Debug print the value(s) piped in. |
| `debug env` | Show environment variables as external commands would get it. |
| `debug experimental-options` | Show all experimental options. |
| `debug info` | View process memory info. |
| `debug profile` | Profile pipeline elements in a closure. |
| `exec` | Execute a command, replacing or exiting the current process, depending on platform. |
| `explain` | Explain closure contents. |
| `export-env` | Run a block and preserve its environment in a current scope. |
| `generate` | Generate a list of values by successively invoking a closure. |
| `inspect` | Inspect pipeline results while running a pipeline. |
| `metadata` | Get the metadata for items in the stream. |
| `metadata access` | Access the metadata for the input stream within a closure. |
| `metadata set` | Set the metadata for items in the stream. |
| `panic` | Causes nushell to panic. |
| `ps` | View information about system processes. |
| `random` | Generate a random value. |
| `random binary` | Generate random bytes. |
| `random bool` | Generate a random boolean value. |
| `random chars` | Generate random chars uniformly distributed over ASCII letters and numbers: a-z, A-Z and 0-9. |
| `random float` | Generate a random float within a range [min..max]. |
| `random int` | Generate a random integer [min..max]. |
| `random pass` | Generate a cryptologically secure password. |
| `random uuid` | Generate a random uuid string of the specified version. |
| `run-external` | Runs external command. |
| `seq` | Output sequences of numbers. |
| `seq char` | Print a sequence of ASCII characters. |
| `seq date` | Print sequences of dates. |
| `sys` | View information about the system. |
| `sys cpu` | View information about the system CPUs. |
| `sys disks` | View information about the system disks. |
| `sys host` | View information about the system host. |
| `sys mem` | View information about the system memory. |
| `sys net` | View information about the system network interfaces. |
| `sys temp` | View the temperatures of system components. |
| `sys users` | View information about the users on the system. |
| `timeit` | Time how long it takes a closure to run. |
| `tutor` | Run the tutorial. To begin, run: tutor. |
| `uname` | Print certain system information using uutils/coreutils uname. |
| `view` | Various commands for viewing debug information. |
| `view blocks` | View the blocks registered in nushell's EngineState memory. |
| `view files` | View the files registered in nushell's EngineState memory. |
| `view ir` | View the compiled IR code for a block of code. |
| `view source` | View a block, module, or a definition. |
| `view span` | View the contents of a span. |
| `which` | Finds a program file, alias or custom command. If `application` is not provided, all deduplicated commands will be returned. |
| `with-env` | Runs a block with an environment variable set. |

### Node

- The VS Code extension is the only Node package: `npm ci`, then `npm run <script>` in `extensions/vscode`; `package-lock.json` is committed.
- Do not add a second package manager or a root `package.json`.

## Working tree

### jj

This checkout is jj-colocated: `.jj/repo/store` points at `.git`, so jj and git see the same repository. Use `jj` for state, history, and commits; do not use `git` commands to change the tree.

- Run jj or git only when the user allows it. This covers every state-changing version-control operation — commit, describe, rebase, squash, split, edit, new, undo, push — not only commit. Read-only inspection is fine.
- Always pass `--no-pager` in an agent or PTY. On a TTY jj starts `ui.pager` (delta to less) and blocks on `:`; `PAGER` and `GIT_PAGER` do not override it.
- `jj --no-pager status`, `diff`, and `log` inspect. `jj describe -m "subsystem: summary"` edits the current change; `jj commit -m ...` is describe plus `new`.
- `jj new`, `squash`, `split`, `edit`, `resolve`, and `undo` are the normal operations. Ask before rewriting history or touching a remote.
- The tree may carry unrelated user changes: read them as context, never revert or claim them.
- Prefer jj. A pull-request contribution from a user without jj may use git; do not be rigid about it.
- Format the title as `scope: imperative summary`, about 50 characters with no trailing period. In [Scoped Commits](https://scopedcommits.com/), `scope` names the affected subsystem, module, or area. Choose the area that owns the change, or a shared parent area for a change spanning modules.
- After a blank line, write a [Linux kernel-style body](https://docs.kernel.org/process/submitting-patches.html#describe-your-changes), wrapped at 72 columns. Describe the problem, previous behaviour, and impact; then explain the final change and why it addresses the problem. Include significant trade-offs or compatibility effects when relevant. Keep simple changes brief; fixed section headings are unnecessary.
- Add relevant issue or design-discussion links when useful. Keep the essential reasoning in the body so the description makes sense without opening those links or reading the conversation.
- Split independent changes that can be reviewed and reverted separately. A structural change needed to fix a bug can belong in the same commit as the fix; explain that dependency rather than splitting solely to fit `feat`, `fix`, or `refactor` labels.

## Workspace

### Temporary files

- Scratch scripts, probes, fixtures, and experiment output go to the OS temp directory or `.agents/tmp/`; `.agents/tmp/` is ignored by git.
- Delete them when the work ends. A verification that edits project files is reverted the moment it finishes.
- Never leave debug output, generated fixtures, or scratch scripts in the worktree.

### Designs

- Discussion, research, route design, architecture, and review conclusions that must outlive the conversation go to `.agents/designs/`, one Markdown file each; the directory is ignored by git.
- Read design notes by section with `just ctx`, like any other memory document.
- Anything citing a local project or another project's source code stays here and never enters tracked Markdown.
- Tracked Markdown carries Tola's own memory only. A crate's `README.md` may name another project only for licence or attribution of borrowed work.
- Current design belongs in `CONTEXT.md`; hard rules in `AGENTS.md`; practice in this file. Do not copy the same fact into all three.

## Editing

### Quality

- Quality is code a reader can understand, change, and delete, not test, layer, or document count.
- Bloat, duplication, special cases, and unreadable code are failure, whatever the tests and documents say.
- A change that leaves the code no smaller or clearer has not paid for itself.
- Write the fewest clear lines that do the job; never pad a function to look thorough.
- Decompose at real boundaries and use local variables and helpers where they make the logic obvious, not to hide it.
- Readability and maintainability outrank cleverness; code should read at a glance.

### Naming

- Name what a value owns or represents, not how it is built.
- Rust: `snake_case` functions/modules, `CamelCase` types, `SCREAMING_SNAKE_CASE` constants; booleans read `is_` or `has_`.
- No `data`, `info`, `obj`, `tmp`, `manager`, `handler`, `utils`, `common`; no type prefixes or numeric suffixes.
- Names are short, clear, and rigorous; a reader should know what the value is without opening its definition.
- No arbitrary prefixes such as `a_foo_bar`, and no filler conjunctions (`and`, `or`, `with`, `for`) that name no real distinction.
- One authoritative representation per concept; derived views name their owner.
- Use the concrete noun the code holds or returns. Reject umbrella words when a narrower one exists: `policy` → `scope`/`access`/`limits`, `facts` → the field names, `plan` → `entries`/`hooks`/`steps`.
- Verbs name the concrete action: `read`, `load`, `select`, `store`, `pass`, `move`, `record`; not `resolve`, `carry`, `thread`, or `surface`.
- Cargo `description` and doc comments are declarative: what the crate holds and what it does. Drop filler such as "shared by" or "one build attempt runs under". A crate description is a short noun phrase naming the crate's subject.

### Comments

- A comment earns its line by stating an invariant, an ownership reason, or an unavoidable external constraint.
- No restating code, narrating steps, or stories about removed implementations.
- Keep names inside comments current through a rename; a stale comment is a defect.
- Name the concrete field, value, file, or action; no metaphor or management vocabulary (`policy`, `facts`, `plan`).

### Diagnostics

- Every rendered diagnostic addresses the site author: what Tola could not do in their site and what to do next.
- Site vocabulary, exact file/route/key/option in backticks; no Rust names, absolute paths, staging paths, or raw cause chains.
- Message: one lowercase sentence without trailing period. Help: imperative next action. Note: user-visible consequence.
- Codes are stable, lowercase, dotted, declared once in `tola-diagnostic`'s `codes` module.

### Errors

- Preserve structured errors until the presentation boundary; string matching is not a protocol.
- Validate once at the owner of the invariant; consumers trust the type.
- Never swallow an error or fall back to different semantics.

### Libraries

- Domain and build crates stay free of `cli`/`terminal` presentation and direct terminal output.
- Public crates keep documented APIs, feature boundaries, and `forbid(unsafe_code)`.
- Deterministic ordering for diagnostics, outputs, routes, and serialized data.

## Tests

### When

- Default: no new tests. Add one when the author asks, or when a real bug needs the smallest reproducer.
- Do not add smoke tests, defensive matrices, or coverage padding.

### Where

- Inline `#[cfg(test)] mod tests` in the module that owns the behaviour. No separate test files or integration targets.
- Prove observable behaviour, not private call order. Keep inputs minimal and domain-named.

### Deleting

- Delete a test whose cost exceeds the behaviour it protects: rendered wording, private call order, byte-exact fixtures, refactor drag.
- Deleting the behaviour deletes its tests in the same change.

## Validation

### Levels

- Docs, comments, formatting: no compile or test run.
- Focused unit change: the owning crate's focused test.
- Cross-module, public API, publication, Bundle semantics: the owning crate's full suite.
- Workspace-wide baseline once per finished scope, not per edit.

### Commands

```sh
just fmt
just check
cargo fmt --all -- --check
cargo test --locked --workspace --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
just tools::check   # parse every Nushell script without running it
```

### Skipping

- State exactly which check was skipped and why; never claim a check passed without running it.
- Builds and test runs belong to the Lead and happen once, after the change is complete. A subagent reports what it edited and what must be verified; it does not compile the workspace, install dependencies, or run a suite.

## Documentation

### Ownership

- `AGENTS.md`: hard rules only. `DEV-SKILL.md`: how to work. `CONTEXT.md`: current shape and ownership. `TOLA-SKILL.md`: the site-author guide served by `tola skill`.
- Update the authoritative surface when user-visible behaviour changes.

### Drift

- Re-read a changed context file once at the end of a batch, not per edit.
- Delete stale context; do not annotate it.
