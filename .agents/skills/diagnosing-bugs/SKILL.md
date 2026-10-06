---
name: diagnosing-bugs
description: Diagnosis loop for hard bugs and performance regressions. Use when the user says "diagnose"/"debug this", or reports something broken/throwing/failing/slow.
---

# Diagnosing Bugs

Discipline for hard bugs. Skip phases only when explicitly justified.

When exploring codebase: read `CONTEXT.md` (if exists) for clear mental model of relevant modules; check ADRs in area touched.

## Redact

Commands, outputs, captured artifacts get shown. **Redact every secret first** → `<REDACTED>`. Build loops against env vars so credential stays in environment, not in what's shown. Captured artifacts carry auth headers → quote only lines carrying signal.

Redacted output insufficient to diagnose ⇒ say so + ask user.

## Phase 1: Build a feedback loop

**This is the skill.** Everything else mechanical. Tight pass/fail signal for the bug (one red on *this* bug) ⇒ you find the cause; bisection, hypothesis-testing, instrumentation all consume it. No loop ⇒ no amount of staring at code saves you.

Spend disproportionate effort here. **Be aggressive. Be creative. Refuse to give up.**

### Ways to construct one, roughly in this order

1. **Failing test** at whatever seam reaches the bug: unit, integration, e2e.
2. **Curl / HTTP script** against running dev server.
3. **CLI invocation** with recorded input, diffing stdout against known-good snapshot.
4. **Headless browser script** (Playwright / Puppeteer) driving UI, asserting DOM/console/network.
5. **Replay captured trace.** Save real network request / payload / event log to disk; replay through code path in isolation.
6. **Throwaway harness.** Minimal subset of system (one service, mocked deps) exercising bug code path with single function call.
7. **Property / fuzz loop.** Bug is "sometimes wrong output" ⇒ run 1000 random inputs, look for failure mode.
8. **Bisection harness.** Bug appeared between two known states (commit, dataset, version) ⇒ automate "boot at state X, check, repeat" so you can `git bisect run` it.
9. **Differential loop.** Same input through old-version vs new-version (or two configs), diff outputs.
10. **HITL bash script.** Last resort. Human must click ⇒ drive *them* with `scripts/hitl-loop.template.sh` so loop stays structured. Captured output feeds back to you.

Right feedback loop ⇒ bug 90% fixed.

### Tighten the loop

Treat loop as product. Once you have *a* loop, **tighten** it:

- Faster? Cache setup, skip unrelated init, narrow test scope.
- Sharper signal? Assert specific symptom, not "didn't crash".
- More deterministic? Pin time, seed RNG, isolate filesystem, freeze network.

30-second flaky loop barely better than none; 2-second deterministic one tight — debugging superpower.

### Non-deterministic bugs

Goal not clean repro but **higher reproduction rate**. Loop trigger 100×, parallelise, add stress, narrow timing windows, inject sleeps. 50%-flake bug debuggable; 1% not ⇒ raise rate until debuggable.

### When you genuinely cannot build a loop

Stop, say so explicitly. List what you tried. Ask user for: (a) access to environment reproducing it, (b) redacted captured artifact (HAR file, log dump, core dump, screen recording with timestamps), or (c) permission to add temporary production instrumentation. Do **not** hypothesise without a loop.

### Completion criterion: tight loop that goes red

Phase 1 done when loop **tight** + **red-capable**: name **one command** (script path, test invocation, curl) **already run ≥ once** (show invocation + output, redacted), that is:

- [ ] **Red-capable**: drives actual bug code path, asserts **user's exact symptom** ⇒ red on this bug, green once fixed. Not "runs without erroring"; must *catch this specific bug*.
- [ ] **Deterministic**: same verdict every run (flaky bugs: pinned, high reproduction rate, per above).
- [ ] **Fast**: seconds, not minutes.
- [ ] **Agent-runnable**: run unattended; human in loop only via `scripts/hitl-loop.template.sh`.

Reading code to build a theory before this command exists ⇒ **stop: jumping straight to a hypothesis is the exact failure this skill prevents.** No red-capable command, no Phase 2.

## Phase 2: Reproduce + minimise

Run loop. Watch it go red as bug appears.

Confirm:

- [ ] Loop produces failure mode **user** described, not nearby different failure. Wrong bug = wrong fix.
- [ ] Failure reproducible across multiple runs (non-deterministic: reproducible at high enough rate to debug against).
- [ ] Exact symptom captured (error message, wrong output, slow timing) so later phases verify fix addresses it.

### Minimise

Once red, shrink repro to **smallest scenario still going red**. Cut inputs, callers, config, data, steps **one at a time**, re-running loop after each cut; keep only what's load-bearing for failure.

Why: minimal repro shrinks hypothesis space in Phase 3 (fewer moving parts to suspect) + becomes clean regression test in Phase 5.

Done when **every remaining element load-bearing**: removing any one makes loop go green.

Do not proceed until reproduced **and** minimised.

## Phase 3: Hypothesise

Generate **3–5 ranked hypotheses** before testing any. Single-hypothesis generation anchors on first plausible idea.

Each hypothesis **falsifiable**: state its prediction.

> Format: "If <X> is the cause, then <changing Y> will make the bug disappear / <changing Z> will make it worse."

No stated prediction ⇒ hypothesis is a vibe: discard or sharpen.

**Show ranked list to user before testing.** They often have domain knowledge that re-ranks instantly ("we just deployed a change to #3"), or know hypotheses already ruled out. Cheap checkpoint, big time saver. Don't block on it; proceed with your ranking if user AFK.

## Phase 4: Instrument

Each probe maps to specific Phase 3 prediction. **Change one variable at a time.**

Tool preference:

1. **Debugger / REPL inspection** if env supports it. One breakpoint beats ten logs.
2. **Targeted logs** at boundaries distinguishing hypotheses.
3. Never "log everything and grep".

**Tag every debug log** with unique prefix, e.g. `[DEBUG-a4f2]`. Cleanup becomes single grep. Untagged logs survive; tagged logs die.

**Perf branch.** Performance regressions: logs usually wrong. Instead: establish baseline measurement (timing harness, `performance.now()`, profiler, query plan), then bisect. Measure first, fix second.

## Phase 5: Fix + regression test

Write regression test **before fix**, but only if **correct seam** exists.

Correct seam: test exercises **real bug pattern** as it occurs at call site. Seam too shallow (single-caller test when bug needs multiple callers; unit test can't replicate chain that triggered bug) ⇒ regression test there gives false confidence.

**No correct seam ⇒ that itself is the finding.** Note it; codebase architecture preventing bug from being locked down. Flag for next phase.

Correct seam exists:

1. Turn minimised repro into failing test at that seam.
2. Watch it fail.
3. Apply fix.
4. Watch it pass.
5. Re-run Phase 1 feedback loop against original (un-minimised) scenario.

## Phase 6: Cleanup

Required before declaring done:

- [ ] Original repro no longer reproduces (re-run Phase 1 loop)
- [ ] Regression test passes (or absence of seam documented)
- [ ] All `[DEBUG-...]` instrumentation removed (`grep` prefix)
- [ ] Throwaway prototypes deleted (or moved to clearly-marked debug location)
- [ ] Correct hypothesis stated in commit / PR message, so next debugger learns
