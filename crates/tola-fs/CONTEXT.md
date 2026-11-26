# tola-fs

Platform filesystem primitives (Unix and Windows) used by input readers, module packing, and build publication. Depends on no other Tola crate.

## Operations

- `lib.rs` — regular-file reads and platform helpers.
- `sys/unix.rs`, `sys/windows.rs` — metadata checks, link counts, opens, links, permissions, path encoding, and no-overwrite writes.
- `replace.rs` — staged directory replacement and rollback state.
- `sys/exchange.rs`, `sys/backup.rs` — atomic directory exchange (Apple/Linux/Android) and backup-and-restore fallback elsewhere.

## Invariants

- Metadata checks inspect the opened handle; directory entries are not proof that a later open hits the same object.
- On Windows, every reparse point is rejected as a link, even if not marked as a symlink.
- Link counts are returned raw; callers decide which counts to accept.
- Unix mode bits are optional on non-Unix platforms; release permissions belong to module packing.
- Directory replacement owns backup naming and cleanup; if rollback fails, it keeps the workspace and returns the backup path.
- Windows directory-relative publication keeps every ancestor handle open without delete sharing; no-overwrite writes never fall back to check-then-rename.
- Path selection, cancellation, journals, and user diagnostics belong to callers.
