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
- brittle — exact formatting, rendered wording, markdown or link syntax, hashes, timing
- redundant — same behaviour covered elsewhere → one table-driven test
- trivial assertions, no real logic
- mock-heavy, never hit code under test
- one behaviour split across siblings each re-running same setup → one test or one table
- several clauses of one behaviour welded into one test whose name needs "and" twice → clauses that matter, or nothing

An unclear owner is a delete: a test re-added after a real regression earns its place; one kept out of inertia never will.

Consolidation keeps each block's isolation: two tests each owning temp dir/config/input → merged body gives each its own. Shared temp root ⇒ second block fails "already exists" or silently asserts first block's state. Move assertions, never ownership.

Extract copy-paste setup into shared helpers where it pays off.

Deleting a test can orphan a helper, constant, or import → remove those with it. Worse reverse: NEVER delete a `use` because the file "looks self-contained" — traits arrive only through imports, dependent code looks fine until it runs. Before deleting an import, check every symbol it named for remaining refs.

## Name the one behaviour

Test name = identifier for the one behaviour the test proves, not a sentence describing the code it runs.

Write the name before the body — it is the contract the body MUST prove; a name formed after the input describes whatever the input did. Holds for tests added mid-other-work: a feature's own new tests are not exempt.

- No scaffolding prefixes: NEVER `a_`, `an_`, `the_`, `test_`, `it_`, `should_`, `when_`, letters, or order-marking numbers. Test order is not a contract; a name opening like a sentence describes the case, not the behaviour.
- No filler article, leading or mid-name: `a`/`an` are prose an identifier drops — `failed_command_reports_bounded_output` not `a_failed_command_reports_bounded_output_from_both_streams`; `absent_variant_misses` not `absent_variant_is_a_miss`. `the` only where the verb acts on one specific thing (`auto_encoding_follows_the_source`).
- One clause. `and`/`or` join two values of one domain — `query_and_fragment_are_refused`, `base_href_decides_internal_or_external` — NEVER two behaviours. A second connective, a connective introducing an article (`..._or_a_background`), or `but` is refused. A name welding two clauses (`segment_settles_or_retracts`) splits into two tests or drops the irrelevant clause.
- State the observable result, not the mechanism: `missing_package_names_each_directory` not `checks_package_resolution_tiers`.
- Short enough to read in a failure line: aim 3–5 words. Ceiling 56 chars — the longest the suite carries today; `just scripts::test-names` refuses longer.
- Do not encode the body: no helper names, byte counts, file paths, or verdict endings (`works`, `succeeds`, `is_ok`, `is_handled`, `passes`, `functions`); no hedge in the outcome's place (`correctly`, `properly`, `successfully`, `as_expected`, `fine`, `valid`). `handles`/`behaves` stand in for an unstated behaviour; one word (`parses`) names a topic.
- Rename every test you touch or review that violates this, even if the test stays.

Naming also shrinks the suite. Open the body before renaming: what does this test fail on that no sibling fails on? Sibling already fails on it → merge (one setup, surviving assertions, one name for what the merged test proves). Nothing fails on it → delete. Then cut the name to the shortest form still stating what the body proves, dropping qualifiers it never checks; a name shortened by losing a checked behaviour was cut wrong. Search the suite for the words the name carries before writing a new one.

Shortening the suite's longest name lowers the ceiling: 56 is what the suite carries today, not a budget to spend.

## Gate: audit every test name you write, before you report

Rules are not advice against finishing sooner: a name breaking them is unfinished work; shipping one is the failure this gate stops. Audit **every file whose tests you added, changed, or reviewed** before reporting anything:

```sh
just scripts::test-names
```

Recipe is part of `just check` ⇒ a forbidden name fails the workspace gate, not a reviewer. Reads every test function the workspace declares (whichever attribute declares it) + case titles of e2e, VS Code, maintenance suites; a declaration it cannot read fails the gate rather than going unjudged.

Every hit is a rename you owe here, not a note for later. A case the audit keeps: file the name under the shape it broke in `scripts/test-names.test.ts`; add the shape to `scripts/test-names.ts` when no rule caught it. Then grep the identifier ban no test name carries:

```sh
grep -rniE 'fixture|probe' --include='*.rs' --include='*.ts' <the files you touched>
```

Every hit = a type, function, or variable named after the technique that builds it → a rename you owe here. Then read the names the audit cannot judge — length, one clause, observable result, domain's words — and rename those too. Report audit output + renames together; "the tests pass" says nothing about whether names are finished.

## Sweep the suite for names

A name defect is cheap to see, invisible to whoever opens only the files a change touched ⇒ sweep the suite before reporting the trim. Scope decides which tests you *change*; it never narrows this read. `just scripts::test-names` carries these patterns; each names a shape no correct test name has:

- **article or scaffold prefix** — `^\s*(async )?fn (a|an|the|test|it|should|when)_`
- **case position** — `^\s*(async )?fn [a-z]_(first|second|third|fourth|fifth|last|next|other|another)_`. A bare letter is not one: `b_tree_splits_at_the_middle` and `k_means_clusters_points` name their algorithm
- **connective chain** — two connectives in one name, `(?:_(and|or|but)_[a-z0-9_]*){2}`
- **connective prose** — connective introducing an article or `then`, or `but`: `_(and|or)_(a|an|the|its|their|they|it|this|that|then)_`, `_but_`
- **dangling article** — `_(a|an|the)$`: article where a value belongs
- **indefinite article** — `_(a|an)_`: `a`/`an` mark prose an identifier does not carry (`absent_variant_misses` not `absent_variant_is_a_miss`); `the` stays where it names the object the verb acts on (`follows_the_source`)
- **bare verdict** — `_(works|succeeds|is_ok|is_handled|passes|functions)$`
- **hedge** — `_(correctly|properly|successfully|gracefully|cleanly|smoothly|as_expected|as_intended|fine|good|ok|valid)$`
- **generic verb** — `(^|_)(handles|behaves|does_the_right_thing)$`
- **single word** — `^[a-z0-9]+$`: a topic, not a behaviour
- **order marker** — `_\d+$`, `_part_\d+$`
- **placeholder word** — `(^|_)(fixture|probe)(_|$)` in a test name
- **ceiling** — `fn [a-z0-9_]{57,}\(`: report it; rename one only when already in that file

Report the counts. A rename outside the files this change owns is separate scope: list old → new pairs, then wait for approval.

`first_`, `second_`, `_2` break the rule only when marking test order. Correct when naming a real domain pair — two resampling passes (`first_pass_window`, `second_axis`), the earliest of several candidates (`tree_conflict_names_first_foreign_owner`) ⇒ read the body before renaming one.

`fixture`/`probe` are not identifiers: NEVER a type, function, variable, constant, module, or field. Name a helper for the value it returns (`html_page`, `asset_route`), not the technique that builds it; `#[cfg(test)]` already says where the code lives, so `test_` there is a placeholder noun too.

A sentence title — `test('vendor freezes a package for a pure build')` in the maintenance suite — is prose, not an identifier: article, connector, ceiling rules do not reach it; the shapes above still do — **scaffold prefix**, **bare verdict**, **hedge**, **placeholder word**, **single word**. One-behaviour and observable-result rules reach a title by reading, not pattern.

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
