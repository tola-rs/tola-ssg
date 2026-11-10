# tola-input

Input identity shared by Tola builds: SHA-256 digests for files and directory trees, the strict
`tola.lock` format, logical and physical source identities, the `.tola` path layout, cancellable
chunked reads, and the scope and network policy one build attempt runs under. A lock records
facts, never declarations; every digest can be recomputed from the bytes.

`FileSelection` matches include and exclude patterns against paths relative to an input
directory. A directory match applies to its descendants, and excludes take precedence.
Each caller supplies the base directory and decides how to read and use matching paths.

Module archive locations and compressed-file digests are distinct from expanded directory
digests. `--locked` forbids new or changed lock entries; `--offline` forbids network access; `--pure`
also excludes host inputs and caches. Cache writes do not change dependency selection.
