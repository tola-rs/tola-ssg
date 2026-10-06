# Repository instructions

Applies to entire workspace. More specific `AGENTS.md` may add local constraints, MUST NOT weaken rules below.

## Project contract

Tola: Rust workspace, Typst-based static site generator. Preserve product model: one root Typst Bundle produces site; official Typst Bundle compiler + exporter: semantic authority; all producers converge into one validated output graph before publication.

Authority, descending:
1. Executable behavior + tests.
2. `README.md`, generated `tola init` scaffolding, CLI help, public crate docs.
3. Current module docs + configuration types.

Disagreement: determine implementation defect, stale description, or unfinished migration. Do not combine conflicting designs. Resolve to one coherent current model; update only authoritative surfaces affected by requested task.

User request defines allowed change. Tests, comments, old notes, nearby TODOs, incidental code smells: evidence about requested work, not independent authorization for more work.

## Non-negotiable working principles

### Report before editing; user decides

Question, assessment request, or problem report is not authorization to change tree. Investigate first — relevant code, callers, tests, current repo state — then report findings + change they imply. Edit only after user approves plan; asking to understand is not asking to fix.

Once authorized, implement exactly what was approved:

- No drift into adjacent refactors, renames, cleanup, "while here" improvements, however obviously correct.
- No shortcuts leaving stated outcome unmet; no silent substitution of easier nearby problem.
- No unannounced operations. Commits, pushes, file deletions, dependency changes, lockfile rewrites, writes outside approved scope: never incidental side effects.
- No quiet rerouting around obstacle. Approved approach wrong/impossible ⇒ stop and report; do not select different one.

A user decision is never made silently. Genuine tradeoff — competing designs, user-visible behavior choice, irreversible/destructive action, change to agreed scope, any question with >1 defensible answer ⇒ state options + consequences, then wait. Picking a default to keep moving is a scope violation, not initiative.

### Deletion means historical absence

Delete feature, type, option, path, compatibility layer, or behavior ⇒ codebase looks as though it never existed:

- Remove implementation, branches, aliases, adapters, configuration, docs, test inputs, tests.
- Remove code whose only purpose was migration, compatibility, deprecation, fallback, or detection of deleted surface.
- No tombstones, "must remain deleted" tests, compatibility tests, rejection branches, reserved names, explanatory comments, speculative guards against return.
- Do not preserve obsolete abstraction merely to reduce diff.
- Version control: historical record. Current tree describes only current design.

Retain migration/backward compatibility only when user explicitly requests it or current documented product contract requires it.

Trace deletion through every representation of removed concept. Per surface inspect + remove:

- declarations, exports, imports, trait implementations, constructors, call sites;
- enum variants, match arms, serialized field names, defaults, feature flags, configuration parsing;
- CLI arguments, help text, diagnostics, terminal rendering, exit behavior;
- cache keys, dependency tracking, invalidation branches, revision messages, persisted metadata;
- embedded Typst, JavaScript, generated scaffolding, examples, test inputs, repository scripts;
- tests, snapshots, benchmark cases, documentation, dependencies existing only for removed concept.

Search by identifier + user-facing spelling. Post-deletion searches MUST find no current implementation reference. Historical mention in non-normative review document does not justify editing that document unless user included it in scope.

Do not convert deleted behavior into explicit error merely to preserve awareness of it. If current grammar/type system naturally rejects old input, that ordinary behavior suffices.

### Stay inside task; understand beyond edited lines

Change only what necessary to complete requested task. Do not bundle unrelated cleanup, redesign, formatting churn, dependency upgrades, speculative features.

Before editing, inspect relevant callers, callees, ownership boundaries, tests, configuration, docs, platform/feature-gated variants. Narrow scope is not permission for shallow reasoning. Follow effects far enough to make scoped change complete + correct.

Required fix crosses boundary ⇒ change all affected pieces in that causal chain. Adjacent issue independent ⇒ leave alone; report only when it materially affects requested work.

Scope test per prospective edit:
1. Identify exact user-visible or internal outcome requested.
2. Explain how edit required to produce that outcome or keep its invariants true.
3. Follow causal chain through all representations that must change together.
4. Stop when chain ends; proximity, similarity, convenience not reason to continue.

Do not infer authorization for larger API redesign, dependency upgrade, repository-wide rename, format conversion, or cleanup campaign from local task. Conversely, do not omit required caller, test, or docs update merely because it lives in another module.

Worktree already contains user changes ⇒ distinguish them from requested edit before acting. Read overlapping changes as current context, preserve intent, never claim in handoff unless this task actually changed them.

### Be aggressive and correct within scope

Within task's real boundary, prefer clean final design over patches preserving accidental structure:

- Remove dead paths + redundant states exposed by change.
- Collapse obsolete indirection; make ownership explicit.
- Strengthen invariants at narrowest authoritative boundary.
- Fix the cause, not downstream exceptions.
- Finish migrations completely; no parallel old/new paths.

Aggressive does not mean broad. Every changed line MUST have direct reason tied to task; every structural change MUST preserve/improve observable correctness.

Within boundary:

- Prefer one authoritative path over old/new, fast/slow, cached/uncached, compatibility forks claiming same semantics.
- Replace boolean combinations + optional fields with representation encoding real valid states when task exposes invalid-state problem.
- Move validation to invariant owner, not repeated defensive checks at consumers.
- Remove pass-through wrappers whose only remaining purpose is historical layering.
- Keep performance optimizations subordinate to semantic equivalence + measurable ownership.
- Complete renames + ownership moves across code, tests, docs, exports in same change.

Do not leave `old_*`, `new_*`, `legacy_*`, `temporary_*`, dual-write paths, silent fallback paths as final design. Temporary scaffolding MUST be gone before handoff.

Excess is the original sin: a change leaving code no smaller/clearer than it found it has not paid for itself. Restructure proves behavior survived, does not assert it — see `skill://refactoring`.

### Use exact domain language

Names MUST state what value owns, represents, validates, transforms, or publishes.

- Do not introduce `plan`, `artifact`, `fact`, `fixture`, `probe`, `reading` in variable, function, type, trait, module, file, directory names, including compound forms. `read`, `reads`, `readable`, `unreadable` name compiler input — stay.
- Use repository vocabulary consistently: source, content unit, document, route, resource, output, candidate, snapshot, revision, dependency evidence, diagnostic, publication — each distinct.
- Do not create abstraction until its boundary + invariant can be named precisely.

Existing vague names are not authorization for more. Rename existing one only when inside requested change + rename makes change materially clearer.

Rules per name kind in Rust code (type, function, module, collection, boolean, transformation, error, cache) + carrying rename through every place old name written: `skill://naming`. Typst code — the embedded `@tola/*` sources and generated `tola init` scaffolding — is named for site authors under Typst's conventions; Rust naming rules do not govern it.

### Write diagnostics for site author

Every rendered diagnostic — message, note, help, trace frame; on terminal, development overlay, editor problems, command summaries — read by site author, not Tola developer. MUST tell what Tola could not do in their site + what to do next.

- Never render foreign/internal detail: text produced by another library/OS, `anyhow` cause chains, absolute/host-specific paths, temporary/staging/cache/virtual paths, Rust module/type/function/field names, pipeline/cache/revision/dependency bookkeeping. Raw chains only in explicitly requested debug output: `--verbose` tracing + `--log-file` record.
- Speak in site vocabulary: site for the thing being built, page or document for a rendered thing; sources, content, routes, assets, configuration keys, commands name the rest. Name exact file, route, class, option — not "invalid input".
- Literal author can see/type goes in backticks — path, configuration key, value, command, syntax fragment (`content/post.typ`, `build.output`, `tola dev`, `..`). Never wrap site text in single quotes; shell snippets `tola init` prints keep own quoting.
- Message: one plain-language phrase/sentence, lowercase unless first word proper noun, no trailing period. Help: imperative sentence naming concrete next action. Note: user-relevant consequence, never internal cause.
- Diagnostic paths site-root-relative, forward slashes, exact line/column when producer knows. Codes stable, lowercase, dotted; each producing surface declares own once in its crate's `codes` module: construction site names constant, never text.
- `tola_build::filesystem::display_path` is the one renderer for path inside site; application layers invocation-relative fallback in `src/terminal/path.rs` on top. Producers never spell relative path by hand.
- Typst can only locate diagnostic inside a package ⇒ still tells site author where to act: carries `imported_by`; note names site files importing that package, at most three + count.
- Unexpected internal failure still produces human sentence, never error dump.

## Architecture boundaries

Layout, ownership, subsystem mechanisms: `.agents/architecture.md` — project memory for fresh agent. Read before changing subsystem; refresh once at end of finished batch, never per edit — same restraint Validation rules put on checks.

- Domain + build modules MUST NOT depend on `cli`/`terminal` presentation.
- Build construction sequence: before-build hooks, discovery, root Bundle convergence, output collection, reference validation, candidate validation, then publication.
- Successful build exposes one complete sealed graph. Never publish partially converged producer output; never mutate graph after sealing.
- Dev server serves immutable published revisions. Hot reload may apply only adjacent proven-safe diff; ambiguity, unsupported changes, revision gap ⇒ full navigation.
- Cache reuse: optimization, never second semantic path. Reused output MUST preserve same diagnostics, dependency evidence, observable bytes as full authoritative build.
- Paths + URLs: different domains. Normalize filesystem paths at ownership boundary; keep URL paths slash-based; preserve Windows behavior when changing path logic.
- Preserve deterministic ordering for diagnostics, outputs, routes, serialized data.
- Long-running build + scan work MUST remain cancellable. Do not hide cancellation inside normal error; do not publish work from superseded revision.
- `tola-icons`, `tola-typst`: public libraries. Keep feature boundaries valid, avoid accidental host-only dependencies, document public API changes, retain `forbid(unsafe_code)`.
- `tola-build`: native public library. Preserve independence from application services, verify public API + platform boundaries, retain `forbid(unsafe_code)`.

## Project skills

Workflow skills live in `.agents/skills/`, discovered from this repository ⇒ load only for sessions whose working directory is inside this checkout. Maintained here: edit skill in place when its rules/trigger change; keep each skill's directory self-contained so it can be read on its own.

Three are standing rules, not optional reading: read `skill://naming` (Rust code) + `skill://comments` before first source edit of a change; `skill://trim` before first test edit.

|Skill|Invocation|Use it for|
|---|---|---|
|`grill-with-docs`|`/skill:grill-with-docs <change>`, user-invoked|Interviewing a change whose plan is fuzzy; resolves the project's words into `CONTEXT.md`, records hard decisions under `docs/adr/`|
|`grilling`|`/skill:grilling`, or model-invoked|Same interview, nothing written to disk|
|`domain-modeling`|`/skill:domain-modeling`, or model-invoked|Sharpening vocabulary and the `CONTEXT.md`/ADR discipline on its own|
|`wait-what`|`/skill:wait-what`, user-invoked|Re-pitching an explanation the reader did not follow|
|`eli5`|`/skill:eli5 <topic>`|Picture-first explainer before reading unfamiliar code|
|`scope-discipline`|model-invoked|Holding a change inside its requested scope: no invented requirements, unrequested robustness, concurrency, security, extensibility, or cleanup work; no reach into another agent's slice|
|`hashing-gate`|model-invoked|Gating any new hash, digest, checksum, signature, nonce, MAC, or content-addressed identifier|
|`concurrency-and-failure`|model-invoked|Designing/reviewing work that runs concurrently, waits, retries, owns resources|
|`subagent-delegation`|model-invoked|Running work through subagents, selecting task-appropriate models through client routing, checking resolved models|
|`trim`|model-invoked|Shrinking code, tests, comments, and docs to what carries meaning: deleting dead paths, redundant states, and refactor-dragging tests, then holding survivors to the four bars|
|`comments`|model-invoked|Writing, merging, or deleting a comment or doc comment; one fact in one place; keeping names inside comments current through a rename|
|`naming`|model-invoked|Choosing/replacing a name in Rust code (type, function, module, file, collection, boolean, transformation, error, cache), carrying a rename through every place the old name was written|
|`code-review`|model-invoked|Two-axis review of a diff since a fixed point: Standards (`.agents/AGENTS.md` + Fowler smell baseline) and Spec (does it implement what was asked)|
|`refactoring`|model-invoked|Restructuring code keeping observable behavior identical, proven by difference testing|
|`diagnosing-bugs`|model-invoked|Diagnosis loop for a hard bug: build a loop that goes red on this bug before forming a hypothesis|
|`resolving-merge-conflicts`|model-invoked|Resolving an in-progress merge/rebase hunk by hunk from each side's intent, then finishing the operation|
|`research`|model-invoked|Investigating a question against primary sources; writing cited findings into the repo|
|`prototype`|model-invoked|Throwaway code answering a design question, as a shareable HTML demo or several UI variants on one route|
|`friendly-report`|model-invoked|Reporting progress to the human — "how far along", "summarize", "where are we": conclusion-first shape, one block per workstream, decisions for the human last|
|`handoff`|`/skill:handoff`, user-invoked|Compacting the conversation into a handoff document|
|`writing-for-agents`|model-invoked|Writing any document an agent consumes: a skill, this file, or a doc reached by a pointer|

`grill-with-docs`, `wait-what`, `handoff` set `disable-model-invocation` ⇒ never appear in an agent's skill list, MUST be named.

Harness/repo-specific skill notes:

- `grill-with-docs` is a one-line delegation to `grilling` + `domain-modeling`; body names those two explicitly because this harness has no `Skill` tool to call them. Only first link installed: `grill-with-docs` does not hand off to `to-spec`, `to-tickets`, `implement`; work continues in same conversation.
- `handoff` names skills for reader to open via `skill://<name>`, same reason.
- `code-review` names `.agents/AGENTS.md` as standards source, takes spec from user, because this repo has no issue tracker.
- `trim` holds test names, renames, merges, deletions, and verification to rules this repo enforces.
- `tola` is user-facing: `.agents/skills/tola/SKILL.md` (exported verbatim by `tola skill`) guides the site author, not the Tola developer. Speak in site vocabulary — sources, pages, routes, assets, commands, configuration keys, package calls, the browser result — and keep Tola's internal machinery (pipeline stages, caches, revisions, Rust modules, repository files, terminal rendering) out of the guide.

`CONTEXT.md` at repo root + `docs/adr/` created by these skills first time a term/decision qualifies, committed like any other documentation.

## Subagents

- Delegate independent work to subagents by default — reconnaissance, implementation against fixed brief, review passes, web research — keep reasoning, decisions, integration, final verification in main thread.
- Read `skill://subagent-delegation` before spawning or recovering a subagent; its model-routing rule is authoritative.
- Work from pointer, not payload: result arrives as preview with `agent://` output behind it; follow-up belongs to live agent, not fresh spawn.

## Workspace hygiene

Temporary files, probes, fixtures, instrumentation never land in project; its history never collects them:

- Scratch files live outside repo (OS temp dir) or in a path project already ignores; probe, generated fixture, debug artifact is never a tracked extra.
- Verification genuinely requires editing project ⇒ revert that edit the moment verification finishes: tree ends as it began + intended change, no leftover probe code, scratch scripts, debug output.

## Implementation rules

- Read full edited function/module + its focused tests before changing. Search all uses of renamed/removed symbols.
- Prefer invalid states unrepresentable over late validation branches.
- Implement first real case directly; introduce abstraction when second real use case arrives.
The over-design and dead-weight removals below, and the four bars a survivor meets: `skill://trim`.
- Build what the request needs and stop there. An optional field, a feature flag, a fallback, a retry, a second entry point, or abstraction for a call site that has not arrived is over-design: delete it on sight, in the change that exposes it.
- Validate once, at the boundary that owns the invariant; every consumer downstream trusts the type. A repeated check, a defensive fallback where the type already holds, or an error variant nothing produces is dead weight: remove it rather than thread it through.
- Keep one authoritative representation per concept. Derived views need clear owner + invalidation rule.
- Do not swallow errors or silently fall back to different semantics. Add context at boundary that knows what operation failed.
- Keep library code free of direct terminal output. Return structured values + diagnostics.
- Secrets/credentials never enter source, tests, logs, diagnostics.
- Avoid cloning solely to bypass ownership design. Use owned snapshots or shared immutable values where they match lifecycle.
- Before adding dependency, check workspace's existing crates + standard library; read their docs/types before assuming capability missing.
- Do not hand-edit `Cargo.lock`. Change dependencies through Cargo; include lockfile changes only when dependency graph intentionally changed.
- Search with client's own search tool when provided; shell search runs `rg`, not `grep`.
- Preserve unrelated user changes in dirty worktree: version control read-only — inspect with `git show`, `git diff`, `git log` — no operation rewriting worktree/index/history runs without user's explicit authorization for that exact operation.
- Update current docs, examples, embedded templates, CLI help when user-visible behavior changes. Do not update historical notes as though current specifications.
- `tola help` translations live in `src/i18n/zh-Hans/`; changing a page phrase in `src/cli/commands/help.rs` or a bundled package's overview, export documentation, or parameter documentation requires the matching translation in the same change. `cargo test -p tola` enforces coverage, stale keys, and fenced-example parity. Translations keep Tola's English terms (`metadata`, `route`, `output`, `URL`, `label`, `location`, `fragment`, `anchor`, `helper`, `tag`, `lineage`) and translate only the prose around them.
- Virtual package export carries two derived descriptions — `README.md` and `.agents/skills/tola/SKILL.md` — changing export, signature, or meaning updates every one in same change; `.agents/architecture.md` records how they + packages kept in agreement.
- Match existing visibility. Do not make item `pub`/`pub(crate)` merely to make test/call site convenient.
- Keep parsing, normalization, validation, rendering distinct when different failure semantics.
- Preserve structured errors until presentation boundary. String matching is not an internal protocol.
- Use deterministic containers/explicit sorting wherever iteration affects generated bytes, diagnostics, tests, public output.
- Concurrency: document + enforce who may mutate shared data, at which lifecycle phase.
- Filesystem work: identify whether operation reads source snapshot, stages candidate, or commits published output. Do not blur these roots.
- Embedded source changes: inspect both embedding Rust code + runtime consumer before editing.
- Comments explain invariant, ownership reason, or unavoidable external constraint. Keep concise, clear, natural, correct, precise: no filler, no restating code, no narrating obvious step, no story about removed implementation, no speculative notes.

## Tests

- Tests live inline in source file whose code they test, inside `#[cfg(test)] mod tests` in that module. Never create separate `test.rs`/`tests.rs` module file, `tests/` directory, or Cargo integration-test target in root crate or any workspace crate. Migrating existing test file: merge into owning module, do not create another test module tree.
- Test changes MUST NOT introduce `#[cfg(test)]` on production imports, fields, variants, functions, trait implementations, branches. Keep test-only setup, inputs, fakes, assertions inside owning `mod tests`; genuine seam required ⇒ express as ordinary production invariant, not test-conditioned path.
- Reduce repeated test setup with narrowly named helper functions/builders inside same `mod tests`. Helpers MUST preserve explicit assertions, MUST NOT hide behavior/boundary test proves.
- Add tests for new behavior, corrected behavior, boundary cases, regressions that can still occur in current design.
- Place tests at lowest authoritative layer that can prove contract. Prefer observable behavior over private implementation detail.
- Deletion normally deletes its tests. Do not replace with tests proving removed syntax, names, files, branches, compatibility behavior remain absent.
- Keep test inputs minimal, domain-named. Avoid broad snapshots when focused semantic assertion clearer.
- `proptest` is reserved for input spaces that cannot be enumerated by hand, as in `tola-slugify`'s canonical-spelling and ASCII-repertoire properties. An enumerable space — however large — is a table or example test. A property test proves one invariant, lives in the crate that owns the tested value, and keeps failure seeds under that crate's ignored `target/proptest/`.
- Bundle semantics changes MUST exercise official Bundle path; do not validate private shortcut as substitute.
- SPA/reload JavaScript changes require browser suite when its dependencies available.
- `e2e/` owns flows crossing real user boundaries: CLI process, filesystem edits, development rebuilds, publication, HTTP/WebSocket delivery, browser behavior. A browser is one possible endpoint, not definition of end-to-end coverage.
- Real-process e2e cases MUST create isolated site directory, own + terminate every child process they start, wait for observable process/server/revision events. No fixed sleeps as synchronization.
- Tests use the documented project test toolchain, without separately installed application CLIs. Generator-hook cases run repository-owned deterministic scripts through the harness runtime; missing third-party generators are not a reason to skip coverage.
- Group e2e cases by user surface: `cli/` command outcomes, `dev/` long-running edit-to-publication lifecycle, `site/` interactions with published pages. Create group only when it owns a test; do not classify tests by current runner/browser driver.
- Test name is the contract body proves: one behaviour, domain's words, short enough for failure line. `just scripts::test-names` refuses name breaking `skill://trim` rules, reading both Rust test functions + case titles of e2e, VS Code, maintenance suites; test declaration/case title whose name audit cannot read fails the gate instead of going unjudged. Name breaking rules is unfinished work; rename reaches every name you touch.

Test selection follows changed contract:

- Pure parsing/normalization: focused unit tests.
- Cross-module ownership, serialization, publication, Bundle behavior: test at that boundary, inline in module owning contract.
- Bug fix: smallest assertion failing for actual cause, succeeding for corrected invariant.
- Do not duplicate same assertion at several layers unless each owns distinct contract.
- Delete a test whose cost exceeds the behaviour it protects. Deleting is the repair for a test that freezes rendered wording, markdown, link syntax, a private call order, or a byte-exact fixture: it drags every later design change and proves nothing a reader of the site or a caller of the API relies on.
- A test red from a half-landed redesign is deleted in the same change, never nursed. Either the behaviour matters and the surviving assertion states it, or the test goes; re-pinning an incidental-format assertion is the same waste as writing one.
- Keep time, path ordering, filesystem events, concurrent completion deterministic in tests; do not repair flakes with sleeps/generous retries.
- Existing behavior intentionally changes ⇒ update/remove old assertion in same edit.

Public crate changes need extra attention:

- `tola-build`: verify independent crate, validated configuration, complete site construction, input observation, cancellation, publication, public docs affected by change.
- `tola-typst`: verify official compile/export behavior, diagnostics, virtual files, read evidence, feature gates, cancellation boundaries affected by change.
- `tola`: verify CLI behavior at command boundary + complete site build when producer/publication semantics change.

## Validation

Run checks at meaningful checkpoints, not after every edit. Builds, tests, lints expensive; repeated verification of small/unchanged edits wastes machine time, prohibited.

- Finish coherent unit of work before running anything.
- Run check only when change complete, diagnosing real failure, or result genuinely uncertain — never reflex after small edit.
- Do not repeat check whose inputs unchanged since it passed.
- Prefer smallest check that can falsify change; slow command runs in background while you keep working.
- Run workspace-wide baseline once per completed scope, not per crate/edit.
- Comment, documentation, formatting-only edits require no compile/test run.

Use `--locked` for repository verification.

Baseline commands (run at completion):

```sh
cargo fmt --all -- --check
cargo test --locked --workspace --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
```

`just check` runs full workspace gate in one command: Rust formatting, TypeScript formatting and linting, justfile formatting, test names, maintenance script checks, library builds, tests, clippy, documentation, package checks.

Focused command per changed surface keeps failures attributable; surface-to-check map in `.agents/architecture.md`.

For changes to e2e harness, embedded SPA, reload JavaScript, or CLI/development/browser flows it covers, run:

```sh
just e2e
```

Required check cannot run locally ⇒ state exactly which check was skipped + why. Never claim check passed without running it.

## Completion standard

Before handoff:

- Re-read user request; map every diff hunk to one explicit requirement or necessary causal consequence.
- Review final diff for scope, stale names, dead branches, duplicate paths, unintended public API changes.
- Search every removed/renamed identifier + its user-facing spelling.
- Confirm no temporary compatibility path, fallback, alias, dual representation, editing scaffold remains.
- Confirm current docs + tests describe only resulting design.
- Confirm validation level matches risk; record exact commands actually run.
- Report concrete outcome + verification performed, plus any remaining risk specific to task.
- Do not add speculative follow-up work, compatibility scaffolding, generic TODOs.
