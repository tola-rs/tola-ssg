---
name: grilling
description: Grill the user relentlessly about a plan, decision, or idea. Use when the user wants to stress-test their thinking, or uses any 'grill' trigger phrases.
---

Interview the user relentlessly → shared understanding. Map as **design tree**: every decision branches into decisions hanging off it.

Work tree in **rounds**. **Frontier** = every decision whose prerequisites settled: questions askable _now_ without guessing answers not yet heard. Ask whole frontier one round: number each question + give recommended answer. Wait for user answers before next round.

Round format:

```
❓ **Q1** - **<question title>**: <question body, might be multiple paragraphs, including multiple choices>

➡️ <your recommended answer>

---

❓ **Q2** - **<question title>**: <question body, might be multiple paragraphs, including multiple choices>

➡️ <your recommended answer>
```

Each round's answers reshape tree: settled decisions push frontier outward, unblock dependent questions. Recompute frontier; ask next round. Question whose answer depends on another question still open this round → _later_ round, not this one.

Finding _facts_ your job, NEVER user's. Frontier question needing environment fact (filesystem, tools): dispatch sub-agent; never ask user what you could look up yourself. DON'T block: running exploration = unsettled prerequisite → only questions downstream of it wait for sub-agent report; ask rest of frontier now. _Decisions_ user's: put each to them and wait.

Session done when frontier empty: every branch of design tree visited, nothing silently assumed. Do not act until user confirms shared understanding.
