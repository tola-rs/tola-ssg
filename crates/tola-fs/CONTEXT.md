# Native filesystem operations

`tola-fs` owns the platform implementations used by the application, input readers, module
packing, and build publication. It supports Unix and Windows and depends on no Tola domain crate.

## Operations

- `lib.rs` exposes regular-file reads and the platform operations; it has no compatibility exports from other crates.
- `sys/unix.rs` and `sys/windows.rs` implement metadata checks, link counts, opens, links, permissions, path encoding, and no-overwrite file publication.
- `replace.rs` owns a directory replacement workspace, its staged tree, and retention after failed restoration.
- `sys/exchange.rs` uses native directory exchange on Apple, Linux, and Android; `sys/backup.rs` moves the old directory aside and restores it after failed installation elsewhere.

## Invariants

- A handle check uses that handle's metadata. Entry classification is an observation, not proof that a later open sees the same object.
- Windows link classification rejects every reparse point, including those not reported as symbolic links.
- Link counts are returned unchanged; callers decide which counts they accept.
- Unix permission bits are optional on other platforms. Release permissions belong to module packing.
- Directory replacement owns backup names and cleanup. Failed restoration retains the workspace and returns the old directory's location.
- Directory-relative Windows publication requires the caller to keep every ancestor open without delete sharing. No-overwrite publication never falls back to an existence check followed by ordinary rename.
- Selection, cancellation, build journals, and author diagnostics stay with callers.
