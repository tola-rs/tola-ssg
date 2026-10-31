---
name: trim
description: Shrink code, tests, comments, and docs to what carries meaning — delete dead paths, redundant states, and refactor-dragging tests outright, then hold the survivors to the four bars. Use when reducing a change, a file, or a suite; before adding, changing, renaming, consolidating, or deleting a test; and when reviewing a diff for concision, clarity, rigor, and correctness.
compatibility: opencode
metadata:
  scope: target-project
  workflow: scope-identify-remove-extract-run-report
---

# Trim

Delete by default; keep only what a reader of the site, a caller of the API, or the next maintainer would notice go missing. Losing coverage is fine when cost > value — a test that drags refactors, restates the implementation, or pins rendered output has already cost more than it protects the moment you read it. The bar is the same for code: a path, state, wrapper, or abstraction nobody's contract needs is weight the change added, not a safeguard.

Scope: user-provided; none → files touched on this branch; else the whole suite or subsystem.

## The four bars

Every survivor — a function, a name, a comment, a diagnostic, a test — passes all four:

- **concise** — as short as the thing allows: a failure line's worth, a sentence the author cannot shorten.
- **clear** — one meaning, in the vocabulary the reader already holds.
- **rigorous** — no mechanism, helper, count, hedge, or article filling space.
- **correct** — states what the value is or does, and stays true after the next refactor.

Text a human reads — a diagnostic, a comment, a report — also reads **friendly**: `skill://friendly-report`.

## Code

Delete on sight, in the change that exposes it:

- dead paths and redundant states the change revealed;
- obsolete indirection, and pass-through wrappers whose only remaining purpose is historical layering;
- the optional field, feature flag, fallback, retry, second entry point, or abstraction for a call site that has not arrived;
- a repeated check, defensive fallback, or error variant nothing produces — validate once, at the boundary that owns the invariant, and let every consumer downstream trust the type;
- a second representation of one concept; a derived view needs an owner and an invalidation rule, or it goes.

Prefer one authoritative path over old/new, fast/slow, cached/uncached, compatibility forks claiming the same semantics; encode the real valid states instead of a boolean combination plus optional fields. Keep performance work subordinate to semantic equivalence and measurable ownership.

## Remove or consolidate

Remove on sight, without waiting for the suite to break:
- refactor drag — every design change moves it; the behaviour it pins is nobody's contract
- brittle — incidental formatting, wording, hash spelling, or timing rather than a supported contract
- redundant — same behaviour covered elsewhere → one table-driven test
- trivial assertions, no real logic
- mock-heavy, never hit code under test
- one behaviour split across siblings each re-running same setup → one test or one table
- several clauses of one behaviour welded into one test whose name needs "and" twice → clauses that matter, or nothing

Read the body and the boundary it exercises before deleting. Identify the surviving coverage or the low-value detail deliberately left unchecked. A mock, constant, or rendered assertion can protect a real contract; its form alone decides nothing.

Consolidation keeps each block's isolation: two tests each owning temp dir/config/input → merged body gives each its own. Shared temp root ⇒ second block fails "already exists" or silently asserts first block's state. Move assertions, never ownership.

Extract copy-paste setup into shared helpers where it pays off.

Deleting a test can orphan a helper, constant, or import → remove those with it. Worse reverse: NEVER delete a `use` because the file "looks self-contained" — traits arrive only through imports, dependent code looks fine until it runs. Before deleting an import, check every symbol it named for remaining refs.

## Name the one behaviour

Name the observable result in domain language, short enough to read in a failure line. Review the name against the body: what failure does the test catch that a sibling does not? Keep qualifiers that distinguish the contract; remove wording that only narrates the setup.

A rename follows the decision to keep the test. Review names in the changed scope; naming alone is not evidence that a test is useful or redundant.

## Renames

Renames follow `skill://naming`; in a test they reach the input, its reads and writes, and expected values in assertions.

## Comments

Comment rules: `skill://comments`; read before adding, merging, or keeping one.

## Beside other agents

Working beside another agent — one owner per file, staying in your slice, collisions — is `skill://scope-discipline`; read before a shared-tree run.

## Then

Run the changed tests and make them pass. A trimmed suite never executed is unverified: if the workspace cannot build, say which command you could not run and which merges/renames that leaves unproven — do not report the pass as complete.

Verify by search, not memory: grep case-insensitively for the old name/retired word across every path you touched; report the count. Zero, except spellings you deliberately kept — say why each stays.

Summarise what was dropped/consolidated and any coverage intentionally given up.
