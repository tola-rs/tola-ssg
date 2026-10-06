---
name: domain-modeling
description: Build and sharpen a project's domain model. Use when discussing codebase terminology, writing or editing a CONTEXT.md, or recording or editing an ADR.
---

# Domain Modeling

Build/sharpen project domain model while designing. Active discipline: challenge terms, invent edge-case scenarios, write glossary + decisions down the moment they crystallise. Merely reading `CONTEXT.md` for vocabulary is NOT this skill — a one-line habit any skill does. This skill changes the model, not consumes it.

## File structure

Most repos: single context.

```
/
├── CONTEXT.md
├── docs/
│   └── adr/
│       ├── 0001-event-sourced-orders.md
│       └── 0002-postgres-for-write-model.md
└── src/
```

`CONTEXT-MAP.md` at root ⇒ multiple contexts; map points to each one's location.

```
/
├── CONTEXT-MAP.md
├── docs/
│   └── adr/                          ← system-wide decisions
├── src/
│   ├── ordering/
│   │   ├── CONTEXT.md
│   │   └── docs/adr/                 ← context-specific decisions
│   └── billing/
│       ├── CONTEXT.md
│       └── docs/adr/
```

Create files lazily, only with something to write: no `CONTEXT.md` ⇒ create at first resolved term; no `docs/adr/` ⇒ create at first needed ADR.

## During the session

### Challenge against the glossary
Term conflicts with existing `CONTEXT.md` language ⇒ call out immediately. "Glossary defines 'cancellation' as X, but you seem to mean Y. Which is it?"

### Sharpen fuzzy language
Vague/overloaded term ⇒ propose precise canonical term. "You're saying 'account': Customer or User? Those differ."

### Discuss concrete scenarios
Domain relationships discussed ⇒ stress-test with specific scenarios. Invent scenarios probing edge cases; force precision on boundaries between concepts.

### Cross-reference with code
User states how something works ⇒ check code agrees. Contradiction ⇒ surface: "Code cancels entire Orders, but you said partial cancellation is possible. Which is right?"

### Update CONTEXT.md inline
Term resolved ⇒ update `CONTEXT.md` there, not batched — capture as they happen. Format: [CONTEXT-FORMAT.md](./CONTEXT-FORMAT.md).

`CONTEXT.md` totally devoid of implementation details: not a spec, scratch pad, or repository for impl decisions — glossary and nothing else.

### Offer ADRs sparingly
ADR only when all three true:
1. Hard to reverse — cost of changing mind later meaningful
2. Surprising without context — future reader wonders "why did they do it this way?"
3. Real trade-off — genuine alternatives, picked one for specific reasons

Any missing ⇒ skip. Format: [ADR-FORMAT.md](./ADR-FORMAT.md).
