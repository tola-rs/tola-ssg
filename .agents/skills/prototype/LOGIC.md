# Logic Prototype

One self-contained HTML file — a **shareable demo** — lets anyone drive a state model by clicking buttons. Use when the question is **business logic, state transitions, or data shape**: looks reasonable on paper, feels wrong only once pushed through real cases.

One file, nothing to install ⇒ hand to a non-developer (designer, PM, domain expert), let them feel the model. Speaks their language, not the code's.

## Right shape when

- "Unsure if this state machine covers the edge case where X then Y."
- "Does this data model let me represent the case where…"
- "Want to feel out what the API should look like before writing it."
- Anyone wants to **press buttons and watch state change**.

Question = "what should this look like" ⇒ wrong branch, use [UI.md](UI.md).

## Process

### 1. State the question

Before code, write down the state model + the question. One paragraph, top of demo, visible intro (not just a comment). A logic prototype answering the wrong question is pure waste ⇒ make the question explicit so it can be checked later — user watching now or returning AFK.

### 2. Isolate the logic in a portable module

Actual logic (the bit answering the question) in a single `<script>` block, written as a small pure module liftable into the real codebase later. Page around it throwaway; module isn't.

Shape depends on the question:

- **Pure reducer**: `(state, action) => state`. Actions discrete events, state a single value.
- **State machine**: explicit states + transitions. "Which actions are legal right now" part of the question.
- **Small set of pure functions** over a plain data type. No implicit current state, just transformations.
- **Class/module with clear method surface** when the logic genuinely owns ongoing internal state.

Pick shape best fits the question, *not* easiest to wire to a page. Keep pure: no DOM, no `document`, no button handlers reaching inside. Page calls into it; nothing flows the other way. Makes the prototype useful past its own lifetime: once answered, the validated reducer / machine / function set lifts into the real module on its own.

### 3. Build the shareable HTML file

One file, plain HTML/CSS/JS: no framework, no bundler, no server, everything inline ⇒ opens by double-click, survives emailing. Anyone runs it by opening it.

Write for a non-developer. Every label in **domain language**, not code: buttons + state read like the business, not the reducer. Explain in plain words.

Clean hierarchy, top to bottom:

1. **Title + one-line explanation** of what this demo lets you explore (the question from step 1).
2. **Current state**: full relevant state, readable panel (labelled fields, not a raw JSON dump), re-rendered after every click so the change is visible. Where it helps a non-developer follow, call out what just changed.
3. **Free-play buttons**: one per action, always available, any order. Click dispatches its action, re-renders the state.
4. **Guided walkthroughs**: **scenarios**, one per tab. Each tab: short plain-language description (situation it sets up + what to watch for), underneath it, ordered **buttons to press**. Each step a real button: click performs that action, moves to next step. Starting a walkthrough resets to a known initial state ⇒ runs the same every time.

Choose scenarios demonstrating the awkward cases, hard to reason about on paper: happy path, tricky edge case, attempt at something that should be illegal.

Beautiful but restrained: clean typography, generous spacing, one accent colour. No animations, no gimmicks — nothing competing with the state and buttons.

### 4. Hand it over

Send the file, or open it for them. They click through walkthroughs + free-play whenever they get to it. Interesting moments: "wait, that shouldn't be possible" / "huh, I assumed X would be different" — bugs in the _idea_, the whole point. Want new actions or a new scenario ⇒ add them. Prototypes evolve.

### 5. Capture the answer and the prototype

Once the prototype has answered its question, capture the answer, then the prototype per [SKILL](SKILL.md). Logic-specific mapping: validated reducer / machine / function set lifts into the real module (decision, absorbed); HTML shell rides along to the throwaway branch that keeps the prototype as a primary source — one self-contained file, so trivially re-runnable there.

## Anti-patterns

- **No tests.** A prototype that needs tests is no longer a prototype.
- **Don't wire it to the real database.** In-memory state unless the question is specifically persistence.
- **Don't generalise.** No "what if we wanted to support X later." The prototype answers one question.
- **Don't blur the logic and the page together.** Pure module referencing the DOM, `document`, or button handlers = no longer liftable. Page = thin shell over a pure module.
- **Don't reach for a framework, bundler, or server.** One file the recipient double-clicks; a React app or dev server defeats "shareable".
- **Don't ship the HTML shell into production.** Page optimised for hand-clicking. Logic module behind it is the bit worth keeping.
