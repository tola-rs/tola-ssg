---
name: prototype
description: Build a throwaway prototype to answer a design question. Use when the user wants to sanity-check whether a state model or logic feels right, or explore what a UI should look like.
---

# Prototype

Prototype = throwaway code answering a question. Question decides shape.

## Pick branch

Identify question from user prompt, surrounding code, or ask if user reachable.
- "Does this logic / state model feel right?" → [LOGIC.md](LOGIC.md). Single shareable HTML file: free-play buttons + tabbed guided walkthroughs; drives state machine through cases hard to reason on paper; non-developer can drive.
- "What should this look like?" → [UI.md](UI.md). Several radically different UI variants on one route, switchable via URL search param + floating bottom bar.

Branches produce very different artifacts → wrong pick wastes whole prototype.
Genuinely ambiguous + user unreachable → default to branch matching surrounding code (backend module → logic; page/component → UI); state assumption at top of prototype.

## Both branches

1. Throwaway day one, clearly marked. Locate next to module/page it prototypes for. Name so casual reader sees prototype, not production. Throwaway UI routes: obey project routing convention; no new top-level structure.
2. Trivial to run. UI: one command in project task runner — `pnpm <name>`, `python <path>`, `deno <path>`. Logic: single HTML file user double-clicks. No thinking to start.
3. No persistence by default; state in memory. Persistence = what prototype checks, not a dependency. Question involves DB → scratch DB or local file named "PROTOTYPE, wipe me".
4. Skip polish: no tests, no error handling beyond runnable, no abstractions. Point: learn fast.
5. Surface state: after every action (logic) or variant switch (UI), print/render full relevant state so user sees what changed.
6. Capture when done: fold validated decision into real code; commit prototype to throwaway branch, out of main, as primary source; leave context pointer to that branch on the implementation issue. Capture answer too (verdict + question it settled) in issue or commit. Main keeps only validated decision.
