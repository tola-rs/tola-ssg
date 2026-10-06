---
name: scope-discipline
description: Keep a change inside its requested scope. Use when a plan is accreting robustness, concurrency, security, extensibility, cleanup, or infrastructure work nobody asked for, or when an implementation step is drifting past the stated deliverable and acceptance criteria, or when another agent is working the same tree and a step reaches for its files.
---

# Scope Discipline

Smallest correct change satisfying user's explicit deliverable + acceptance criteria.

Inspect broadly, modify narrowly.

No invented requirements. Possible risk ≠ authorization to fix it.

Optional robustness, concurrency, security, extensibility, cleanup: never prerequisites.

Every implementation step MUST map to an explicit acceptance criterion; delete plan steps that don't.

New subsystems, dependencies, protocols, persistent state, locks, leases, IPC, configuration surfaces, generalized abstractions: explicit approval required.

Prefer existing framework mechanisms + project patterns.

Accepted plan viable → execute. No recursive prerequisites unless new concrete evidence shows plan cannot satisfy an explicit requirement.

Real but non-blocking issue → Follow-ups, do not implement.

## Beside other agents

Work split across agents: your slice is the boundary.

- Work your slice only. Never edit a file another agent owns, pick up their work, or reach into their area; expansion requires user authorization; default — stay in your lane — needs no question.
- One owner per file, shared manifests + lockfiles included.
- Re-read a file immediately before editing it; smallest edit that lands; never rewrite a whole file for a small change.
- Collision ⇒ boundary was fuzzy. Compare both intents, decide from code which content is correct — not which agent — keep that, continue; no halt for permission; no wholesale revert of the other side.

All acceptance criteria pass → STOP.
