---
name: code-review
description: "Review the changes since a fixed point (commit, branch, tag, or merge-base) along two axes: Standards (does the code follow this repo's documented coding standards?) and Spec (does the code match what the originating issue/spec asked for?). Runs both reviews in parallel sub-agents and reports them side by side. Use when the user wants to review a branch, a PR, work-in-progress changes, or asks to \"review since X\"."
---

Two-axis review of the diff between `HEAD` and a user-supplied fixed point:

- **Standards**: code conforms to this repo's documented coding standards?
- **Spec**: code faithfully implements the originating issue / spec?

Both axes = **parallel sub-agents** (no context pollution between them) → this skill aggregates findings.

Repo has no issue tracker: standards source = `.agents/AGENTS.md`; spec = the user's request for the change under review.

## Process

### 1. Pin the fixed point

Whatever the user said is the fixed point (commit SHA, branch, tag, `main`, `HEAD~5`, etc.). Unspecified → ask.

Diff command: `git diff <fixed-point>...HEAD` (three-dot → against merge-base). Commits: `git log <fixed-point>..HEAD --oneline`.

Confirm before proceeding: fixed point resolves (`git rev-parse <fixed-point>`), diff non-empty. Bad ref / empty diff MUST fail here, not inside two parallel sub-agents.

### 2. Identify the spec source

Search order:

1. Path the user passed as an argument, or a document they named.
2. The conversation, if the spec was agreed there.
3. Nothing found → ask the user. None exists → **Spec** sub-agent skips, reports "no spec available".

### 3. Identify the standards sources

Repo files documenting how code should be written. Here: `.agents/AGENTS.md`; `.agents/skills/naming/SKILL.md` = rules per name kind; `.agents/skills/comments/SKILL.md` = comment text; `.agents/skills/trim/SKILL.md` = what a test must prove + the name it proves it under, and the bar every survivor — code, comment, or test — meets. Read all three — detail behind three of the standards' shortest sentences.

Standards axis always carries the **smell baseline** on top of repo docs: fixed set of Fowler code smells (_Refactoring_, ch.3), applies even when repo documents nothing. Two rules:

- **Repo overrides.** Documented repo standard always wins; where it endorses something the baseline would flag, suppress the smell.
- **Always a judgement call.** Each smell = labelled heuristic ("possible Feature Envy"), never a hard violation. Like any standard here, skip anything tooling enforces.

Each smell: *what it is* → *how to fix*; match vs diff:

- **Mysterious Name**: function/variable/type whose name doesn't reveal what it does or holds. → rename; no honest name → the design's murky.
- **Duplicated Code**: same logic shape in >1 hunk or file in the change. → extract the shared shape, call from both.
- **Feature Envy**: method reaches into another object's data more than its own. → move it onto the data it envies.
- **Data Clumps**: same few fields/params keep travelling together (a type wanting to be born). → bundle into one type, pass that.
- **Primitive Obsession**: primitive/string standing in for a domain concept deserving its own type. → give the concept its own small type.
- **Repeated Switches**: same `switch`/`if`-cascade on the same type recurs across the change. → polymorphism, or one map both sites share.
- **Shotgun Surgery**: one logical change forces scattered edits across many files in the diff. → gather what changes together into one module.
- **Divergent Change**: one file/module edited for several unrelated reasons. → split so each module changes for one reason.
- **Speculative Generality**: abstraction/params/hooks added for needs the spec doesn't have. → delete; inline back until a real need shows.
- **Message Chains**: long `a.b().c().d()` navigation the caller shouldn't depend on. → hide the walk behind one method on the first object.
- **Middle Man**: class/function that mostly just delegates onward. → cut it, call the real target direct.
- **Refused Bequest**: subclass/implementer ignores or overrides most of what it inherits. → drop the inheritance, use composition.

### 4. Spawn both sub-agents in parallel

**Standards sub-agent prompt**:

- Full diff command + commit list.
- The step-3 standards-source files, **plus the step-3 smell baseline** pasted in full (the sub-agent has no other access to it).
- Brief: "Report, per file/hunk where relevant, (a) every place the diff violates a documented standard: cite the standard (file + the rule); and (b) any baseline smell you spot: name it and quote the hunk. Distinguish hard violations from judgement calls: documented-standard breaches can be hard, but baseline smells are always judgement calls, and a documented repo standard overrides the baseline. Skip anything tooling enforces. Under 400 words."

**Spec sub-agent prompt**:

- Diff command + commit list.
- The path or fetched contents of the spec.
- Brief: "Report: (a) requirements the spec asked for that are missing or partial; (b) behaviour in the diff that wasn't asked for (scope creep); (c) requirements that look implemented but where the implementation looks wrong. Quote the spec line for each finding. Under 400 words."

Spec missing → skip Spec sub-agent, note in final report.

### 5. Aggregate

Two reports under `## Standards` and `## Spec` headings, verbatim or lightly cleaned. Do **not** merge or rerank findings — the two axes are deliberately separate (see _Why two axes_).

End with a one-line summary: total findings per axis, worst issue _within each axis_ (if any). Don't pick a single winner across axes: that's the reranking the separation exists to prevent.

## Why two axes

A change can pass one axis, fail the other:

- Follows every standard but implements the wrong thing → **Standards pass, Spec fail.**
- Does exactly what the issue asked but breaks the project's conventions → **Spec pass, Standards fail.**

Separate reporting stops one axis masking the other.
