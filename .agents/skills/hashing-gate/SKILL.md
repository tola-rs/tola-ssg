---
name: hashing-gate
description: Gate any new hash, digest, checksum, signature, nonce, MAC, or content-addressed identifier. Use when about to introduce hashing or a cryptographic primitive, or when a plan reaches for a digest for identity, change detection, cache keys, locking, or integrity.
---

# Hashing Gate

Adding any hash, digest, checksum, signature, nonce, MAC, cryptographic primitive, or content-addressed identifier = architectural expansion.

Before adding one, ask all three:

1. Explicitly required by user's request or acceptance criteria?
2. Existing project format, protocol, API, or compatibility requirement already requires this exact mechanism?
3. Concrete observed failure a simpler mechanism cannot solve without hashing/cryptography?

Any answer not clearly yes ⇒ do not add.

Do not invent SHA-256 or another digest merely for: approval tokens, change detection, identity, deduplication, freshness checks, locking / leases, cache keys, integrity, defensive validation, future-proofing.

Prefer existing identifiers, timestamps, version fields, object identity, or direct comparison when they already satisfy the task.

Hashing useful but not part of the original requirement ⇒ report as possible follow-up, do not implement.

## Three-strikes rule

Before typing `SHA`, `hash`, `digest`, `checksum`, `nonce`, `signature`, `BLAKE`, `MD5`, `CRC`, `xxHash`, or similar:

STOP. Re-read original request three times. Original requirement lacks a hash-like mechanism ⇒ do not introduce one.
