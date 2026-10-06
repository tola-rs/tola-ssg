---
name: resolving-merge-conflicts
description: "Use when you need to resolve an in-progress git merge/rebase conflict."
---

1. Inspect merge/rebase state: git history, conflicting files.
2. Per conflict, find primary sources — why each change made, original intent: commit messages, PRs, original issues/tickets.
3. Resolve each hunk. Preserve both intents where possible; where incompatible, pick the one matching merge's stated goal and note trade-off. NEVER invent new behaviour. Always resolve; never `--abort`.
4. Discover project automated checks, run them — typically typecheck → tests → format. Fix anything merge broke.
5. Finish: stage everything, commit. If rebasing, continue until all commits rebased.
