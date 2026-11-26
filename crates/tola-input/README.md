# tola-input

Shared input primitives for Tola builds: SHA-256 file/tree digests, `tola.lock` read/write, `.tola` state paths, include/exclude path selection (`FileSelection`), cancellable reads, and input access scopes.

- `InputLock::commit` merges changes under `.tola/state/inputs.lock` and rejects conflicting edits to the same entry.
- `FileSelection` matches relative include/exclude patterns (directories match descendants; excludes win).
- Input scopes: `--locked` keeps `tola.lock` unchanged; `--offline` disables network access; `--pure` also disables host caches and outside-site files.
