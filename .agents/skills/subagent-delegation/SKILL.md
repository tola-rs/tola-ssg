---
name: subagent-delegation
description: Run work through subagents instead of the main thread. Use when planning multi-step work, searching or surveying a codebase, implementing against a fixed brief, running review passes, researching outside the repo, or checking subagent model routing.
---

# Subagent Delegation

Delegation: default for work that can proceed independently. Buys parallel wall-clock and lean main context.

## Stays in main thread

Planning, decomposition, decisions, trade-offs, user conversation, result integration, final verification. Main model = reasoning budget; mechanical reading, searching, transcribing spend it for nothing.

## Goes out

- Reconnaissance/surveys (`scout`), implementation against complete brief (`task`; `sonic` mechanical), review passes (`reviewer`), web research.
- Independent slices batched together, one owner per file/area.
- Stay inline: one-line edit, reasoning existing only in this conversation, anything needing user — brief would cost more than work.

## Route models for the task

Select a model appropriate to the task through client routing. Spawn resolves model in order: client per-agent override → agent definition's own model → session's own model. An agent no override covers inherits the session model; configured fallback chains may change the resolved model.

- The only excluded subagent model is the model the user calls `6 astra`; no provider, family, or tier is required or otherwise excluded.
- Know which routing rule decides a spawn's model before making it; after start or a fallback, inspect the actual model (`resolvedModel` or the client's agent view). Stop and reroute for this model restriction only when the actual model is `6 astra`.

## Limit = cooldown, not verdict

Spawn dying on rate limit, overload, or provider outage is transient — not evidence delegation is unavailable, not reason to absorb work into main thread.

- Assume recovery first, including after main model itself hit a limit: session running again usually means account limits cleared, subagents included. Re-attempt delegation before inline work; let a new failure — not the old — decide otherwise.
- Recover the slice, don't replace: message failed agent to retry (`write agent://<id>` — still holds brief and context), or spawn slice again. Fresh run → fresh retry budget; configured fallback chains may already have moved it to another provider.
- NEVER hard-cancel a limit-stopped spawn: `proc://<id>/kill` aborts it and releases the session — terminal state; slice rebuildable only by fresh agent reading `history://<id>`, losing whatever original held. Leave idle/parked, wake with `write agent://<id>` once account works — still holds brief and context, resumes in place. Account switch is exactly this case ⇒ waking is recovery, not redo.
- Inline execution replaces one slice for one moment, never the mode: "subagents unavailable" does not carry into rest of session. Client applies same rule to fallback models — fallback returns to primary when cooldown expires.
- Non-transient failure (bad brief, failing acceptance) → re-brief, not blindly retry.

## Brief carries everything

Subagent starts blank: no conversation, no plan, none of parent's context — only workspace, context files, spawn itself. Spawn states goal, interface it must respect, files it owns, what not to touch, acceptance criterion, shape of answer. Subagent that has to guess guesses wrong at full price.

## Read through pointer

Delivery holds preview; `agent://<id>` holds full output. Work from pointer, pull only lines that matter. Follow-up: message live agent (`write agent://<id>`) — already holds context — instead of briefing fresh one.
