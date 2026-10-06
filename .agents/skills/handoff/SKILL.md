---
name: handoff
description: Compact the current conversation into a handoff document for another agent to pick up.
argument-hint: "What will the next session be used for?"
disable-model-invocation: true
---

Write handoff doc summarizing current conversation → fresh agent continues the work. Save to user OS temp dir, NOT current workspace.

Include `suggested skills` section: name skills next agent should read via `skill://<name>`, each with the trigger that should make it reach for it.

Do NOT duplicate content already in other artifacts (specs, plans, ADRs, issues, commits, diffs) — reference by path or URL instead.

Redact sensitive info: API keys, passwords, personally identifiable information.

Args passed → treat as description of next session's focus; tailor doc accordingly.
