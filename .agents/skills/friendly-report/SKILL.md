---
name: friendly-report
description: Report progress to the human. Use when the human asks how far along things are — "进度如何", "总结汇报", "现在做到哪", "summarize", "where are we" — or any turn that reports status to the human rather than handing work to another agent.
---

# Friendly report

Shape of a progress report to the human: "how far along", "summarize", "where are we". It governs the report a session gives the human — not the technical handoff an agent gives another agent, which keeps its raw evidence and the `.agents/AGENTS.md` Completion standard's checklist.

## Shape

1. **Conclusion first**: one sentence for the overall state.
2. **One block per workstream**, ordered by how the work is actually moving — never per file, directory, or timeline. Each block carries three things:
   - **what it is**: one sentence at mechanism level, how the thing works, not a label;
   - **where it stands**: the capability that exists now + the specific problem just solved; name the problem itself, not the route taken through it;
   - **what remains**: the concrete work left, or where it is stuck; name a blocker and move on.
3. **Close with "decisions for the human"**: only what genuinely needs their call, one line each.

## Forbidden

- Listing files: "`route.rs` changed too, and `serve/` was fixed" is not progress.
- Test tallies as progress — suite, assertion, or pass counts; give them only when asked.
- Replaying command output, logs, or the steps taken.
- Carrying another concurrent agent's compile breakage or half-written file as a concern or todo; that is the working norm, one clause at most.
- Running past one screen.

## Numbers

A number appears only when it stands for a capability ("the site builds end to end", "contrast is a hard gate", "no C dependency"). Evidence means the failing case or the conclusion, never a statistics table.

## Worked contrast

Counter-example, carrying every failure at once — no mechanism sentence, only files and suite counts, another agent's breakage raised as a concern, and no ask:

> Fixed the grace erase in `completion.rs`, three binding problems in `serve`, and I'm on the fourth suite … 365 assertions green, 8 suites red, and another agent is holding `frames.rs` so …

The shape to put in its place:

> **Conclusion**: <overall state, one sentence>.
> **<workstream>** (<mechanism, one sentence>): <capability that exists now>; <specific problem just solved>; <work still open>.
> **Decisions for the human**: ① <call only they can make>; ② <call only they can make>.
