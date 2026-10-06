# ADR Format

ADRs: `docs/adr/`, sequential numbering — `0001-slug.md`, `0002-slug.md`, …

`docs/adr/` created lazily: on first ADR needed.

## Template

```md
# {Short title of the decision}

{1-3 sentences: what's the context, what did we decide, and why.}
```

Single paragraph suffices. Value = recording *that* a decision was made and *why*, not filling out sections.

## Optional sections

Only when adding genuine value; most ADRs need none.

- **Status** frontmatter (`proposed | accepted | deprecated | superseded by ADR-NNNN`): when decisions revisited
- **Considered Options**: only when rejected alternatives worth remembering
- **Consequences**: only when non-obvious downstream effects need calling out

## Numbering

Scan `docs/adr/` for highest existing number → increment by one.

## When to offer an ADR

All three MUST be true:

1. **Hard to reverse** — cost of changing your mind later meaningful
2. **Surprising without context** — future reader sees code, wonders "why on earth did they do it this way?"
3. **Real trade-off** — genuine alternatives, picked one for specific reasons

Easy to reverse → skip; you'll just reverse it. Not surprising → nobody wonders why. No real alternative → nothing to record beyond "we did the obvious thing."

### What qualifies

- **Architectural shape.** "We're using a monorepo." "The write model is event-sourced, the read model is projected into Postgres."
- **Integration patterns between contexts.** "Ordering and Billing communicate via domain events, not synchronous HTTP."
- **Technology choices carrying lock-in.** Database, message bus, auth provider, deployment target. Not every library: just ones that'd take a quarter to swap out.
- **Boundary and scope decisions.** "Customer data is owned by the Customer context; other contexts reference it by ID only." Explicit no-s as valuable as yes-s.
- **Deliberate deviations from the obvious path.** "We're using manual SQL instead of an ORM because X." Where a reasonable reader assumes the opposite. Stops next engineer from "fixing" something deliberate.
- **Constraints not visible in the code.** "We can't use AWS because of compliance requirements." "Response times must be under 200ms because of the partner API contract."
- **Rejected alternatives when rejection is non-obvious.** Considered GraphQL, picked REST for subtle reasons → record it; otherwise someone suggests GraphQL again in six months.
