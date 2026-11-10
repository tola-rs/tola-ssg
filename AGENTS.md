# Repository instructions

Tola is a Rust workspace for a Typst-based static site generator. One root Typst Bundle produces the site; the official Typst compiler and exporter are the semantic authority; every producer converges into one validated output graph before publication. Executable behaviour and tests outrank prose.

## Rules

These rules bind the Lead, every teammate, and every subagent; they are requirements, not advice.

1. **Report every two tool calls, before the third.** After every result, write one or two lines: what changed, what it returned, what waits. A third call without that line is a defect: stop and report before continuing. Announcing what you are about to do is not a report; the report follows the result. The Lead, every teammate, and every subagent report at this cadence, and a final summary never replaces it. A task is not complete until its report reaches the Lead; the Lead reports to the user.
2. Ask before editing. A question, an assessment, or a bug report is not permission to change the tree. Report what you found and the change it implies; wait for approval.
3. Stay in scope. No adjacent cleanup, renames, dependency bumps, formatting churn, or speculative features. No compatibility layers, aliases, or dual paths. Addition needs a reason; deletion does not.
4. Do not build, test, or install to check your work. Read the source and edit it; the Lead runs the build, the tests, and the review once, at the end. A subagent that compiles the workspace or runs a suite starves the machine and stalls every other agent. One exception: a single parse check of a script you just wrote (`nu -c 'use tools/lib/x.nu *'`), never a repeated run.
5. Do not invent tests. Add one only when the author asks or a real bug needs the smallest reproducer. Delete tests that pin rendered wording, private call order, or incidental bytes.
6. Read before concluding. Read the code, callers, tests, and configuration at issue; say when something was not verified.
7. One owner per concept. Keep source paths, logical output paths, routes, and URLs distinct. Validate at the boundary that owns the invariant.
8. Diagnostics address the site author. Name the file, route, key, or command; never Rust internals, host paths, or raw cause chains.
9. Use jj. This is a jujutsu checkout: status, diff, log, describe, commit, new, squash, split, edit, resolve, rebase, undo, and push all go through `jj`. In an agent or PTY always pass `--no-pager` (`jj --no-pager status`), or jj starts its pager and blocks. Any version-control operation that changes state — commit, describe, rebase, squash, split, edit, new, undo, push — runs only when the user allows it; read-only inspection does not.
10. Commit only on request. Write a commit description only when the user asks for one, and follow Linux kernel style: `subsystem: imperative summary` (about 50 characters, no trailing period), a blank line, then a body explaining why, wrapped at 72 columns. Prefer jj; for a pull-request contribution where the user has no jj, git is acceptable. Never push or rewrite history without explicit permission. Preserve unrelated worktree changes.
11. Keep scratch out of the tree. Temporary scripts, probes, fixtures, and experiment output go to the OS temp directory or `.agents/tmp/`, and are deleted when the work ends. Never add them to the repository or leave them in the worktree.
12. Persist only what outlives the conversation. Discussion, research, route design, architecture, and review conclusions go to `.agents/designs/`. Tracked Markdown carries Tola's own memory only: no local project names, no notes from reading another project's source. A crate's `README.md` may name another project only where Tola borrows from it and licence or attribution requires the notice.
13. Treat pushback as evidence. When the user questions, challenges, or corrects you, reflect on the gap first; then ask whether the governing rule or context document should change. Do not silently rewrite a constraint the user did not name.
14. Stop on ambiguity. When the request, scope, or intended outcome is ambiguous, stop and ask before choosing a path; state the options and consequences. Do not pick a default to keep moving, and confirm understanding before a long or irreversible step.
15. Write concise, precise code. No line-count padding; decompose at real boundaries and name locals and helpers so the logic reads at a glance. Names are short, clear, and rigorous: no arbitrary prefixes, no filler conjunctions.
16. Watch the pace. When generation outruns digestion — a long uncommitted stretch, repeated churn, files growing without review — say so and propose a pause: review the recent code for quality, readability, naming, architecture, organization, and concision. The pause is a suggestion; do not start an unrequested refactor.
17. **Judge the target before the path.** When asked whether a boundary should exist or how the code should be shaped, answer the end-state question first: the invariant the boundary would own, the dependency direction it fixes, and whether the solved state is cleaner than the alternative. Existing coupling, prerequisites, and in-flight work decide *when* the change lands, never *whether* the target is right. A `no` must hold even if every solvable objection were solved, and it names the falsifier that would reverse it.

## Context

Four kinds of document carry memory, and each has one job. A fact lives in exactly one of them.

| Document | One job | Never holds |
|---|---|---|
| `AGENTS.md` | rules that always apply | practice, subsystem shape |
| `.agents/DEV-SKILL.md` | how to work: toolchain, language conventions, editing, tests, validation | rules, subsystem shape |
| `.agents/CONTEXT.md` | repository overview: layout, build lifecycle, vocabulary | commands, toolchain, lessons |
| `CONTEXT.md` beside code | one subsystem's current shape, ownership, invariants | rules, practice, history |

Read them — this file, `.agents/DEV-SKILL.md`, every `CONTEXT.md`, and `.agents/designs/*.md` — with `just ctx` by section; never load one whole. The same reader serves all of them:

```sh
just ctx .agents/DEV-SKILL.md
just ctx .agents/DEV-SKILL.md <topic> [<section>]
just ctx --check .agents/DEV-SKILL.md
```

`CONTEXT.md` lives beside the subsystem it describes; the repository overview is `.agents/CONTEXT.md`. Read the closest file before changing a subsystem, then the parent and the overview when a change crosses modules.
