---
name: refactoring
description: Restructure code while keeping observable behavior identical, and prove it with difference testing. Use when refactoring, simplifying, splitting or merging modules, migrating to a new implementation, or replacing one mechanism with an equivalent.
---

# Refactoring

Refactor: changes structure, not observable behavior. Two obligations: prove behavior survived; leave code simpler or clearer.

## Prove preservation with difference testing

Difference testing (= differential testing in literature): run same inputs through old + new implementation, compare observer-visible output: return values, emitted output, written files, errors, built site. NEVER compare internals.

- Pick observable boundary — public function, CLI invocation, published tree, `inspect` projection — compare at highest one the refactor moves.
- Keep both versions runnable side by side for the duration (second module, test-only entry point, recorded golden of old output); drive with inputs reaching changed paths: existing tests, fixtures, boundary cases, generated inputs.
- Normalize what contract does not cover before comparing: timestamps, host paths, ids, unordered collections, log framing. Normalize narrowly — sorting everything can hide real regression.
- Investigate EVERY difference + classify: accidental regression, unstated behavior change, bug in old implementation, nondeterminism. Only a classified, intended difference stays.
- Passing proves equivalence only for inputs exercised + observations compared. Name surfaces not pinned.

No second implementation: record old behavior first — characterization test, golden capture of built output — treat capture as reference refactor compared against.

## Simplify, or the refactor has not paid for itself

Excess = original sin: point of restructuring is code ends smaller + clearer.

- Delete what change exposes: dead paths, redundant states, obsolete indirection, pass-through wrappers, leftovers of old shape. End state describes only current design.
- Restructure keeping same duplication under new names changed nothing that matters — fix shape, not spelling.
- New abstraction waits for second real use case + nameable boundary; until then, inline it.
- Behavior changes riding along: named + proven, NEVER smuggled inside refactor — mixed goal makes difference test meaningless.

## Evidence

Report difference run — inputs, normalized comparison, classification of every difference — plus checks `AGENTS.md`'s Validation section asks for. Refactor without difference run = unverified; full comparison impossible → say which surfaces stay unpinned.
