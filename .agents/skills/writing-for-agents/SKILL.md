---
name: writing-for-agents
description: Writing documents for agents. Use when creating or editing skills, or modifying AGENTS.md or CLAUDE.md.
---

Any doc an agent consumes: skill, `AGENTS.md`/`CLAUDE.md`, doc reached by pointer. Packaging differs; writing does not — same levers make each predictable, since agent takes same _process_ every run, not same output.

Writing a skill → read [`SKILL-MECHANICS.md`](SKILL-MECHANICS.md) for frontmatter, invocation choice, router skills.

## Context pointers

**Context pointer**: reference in agent's context naming out-of-context material + condition for reaching it. Skill description = one; line in `AGENTS.md` naming a doc = same object. Pointer's _wording_, not target, decides when + how reliably agent reaches material. Must-have target behind weakly worded pointer = variance bug: sharpen wording first; inline only if sharpening fails.

Pointer does two jobs: state what material is; list **branches** that trigger reaching it. Branch = distinct case the doc covers, so different runs take different paths.

Every word of always-loaded pointer costs every turn ⇒ harder pruning than body:
- **Front-load leading word**: pointer is where it triggers.
- **One trigger per branch.** Synonyms renaming one branch = one branch twice; collapse, keep only genuinely distinct branches.
- **Cut identity body already carries.**

## The two loads

Every doc + pointer spends one of two budgets:
- **Context load**: always-loaded material on agent's window (`AGENTS.md` line, skill description, anything in context every turn) — tokens + attention spent whether or not it fires.
- **Cognitive load**: cost on human: which docs exist, when to reach each. Human = the index. NOT a cost to minimise: price of human agency. Spend where human judgement matters; remove where it does not.

Material reached only through pointer escapes context load at price of pointer's own line; material with no pointer rides entirely on cognitive load.

## Information hierarchy

Doc = two content types: **steps** (ordered actions agent performs) + **reference** (definitions, rules, facts consulted on demand). Mix freely: all steps (recipe), all reference (review's rules, this skill), or both. Core decision: where each piece sits on **information hierarchy** — ladder ranked by how immediately agent needs it:

1. **In-file step**: primary tier — what agent does, in order.
2. **In-file reference**: consulted on demand. Often legitimately flat peer-set (every rule of a review on one rung) — fine, not a smell.
3. **Disclosed reference**: separate file, reached by context pointer, loaded only when pointer fires. Spans sibling file same folder → fully external reference living anywhere, any doc can point at.

Push too little down → top bloats; too much → hides needed material. That tension = whole decision.

**Progressive disclosure**: move down ladder (out of main file, behind pointer) so top stays legible. Not primarily token optimisation — how hierarchy is protected. Branching = cleanest disclosure test: inline what every branch needs; push behind pointer what only some branches reach. Doc with steps: in-file reference that should be disclosed buries them, turns attending into coin-flip — variance lever, not just legibility.

**Co-location**: within-file companion. Ladder decides _how far down_ a piece sits; co-location decides _what sits beside it_. Keep concept's definition, rules, caveats under one heading, not scattered, so reading one part brings neighbours. Test: doc should read like documentation written for agent. Grouped does; scattered does not. ( ≠ duplication: duplication repeats one meaning in two places; scattering fragments one meaning across many.)

**Sprawl**: failure mode — doc too long even when every line live and unique. Attention thins across excess; every extra line = one more to keep relevant. Cure = ladder: disclose reference behind pointers; split by branch or sequence so each path carries only what it needs.

## Steps and completion criteria

Every step ends on **completion criterion**: condition that tells agent work is done. Two levers:
- **Clarity**: can agent tell done from not-done? Vague bound ("understanding reached") invites **premature completion**: ending step before genuinely done, attention slipping to _being done_. Visible steps ahead (**post-completion steps**) supply pull; criterion's clarity = resistance. Defend in order: **sharpen bound first** (local, cheap); only if irreducibly fuzzy AND you observe the rush, hide later steps by splitting sequence. Hiding works only across real context boundary (hand-off or subagent dispatch; inline call leaves later steps in context, clears nothing).
- **Demand**: how much it requires. "Every modified model accounted for" forces thorough work; "produce a change list" does not. Demand drives **legwork** (digging agent does within work, latent in wording rather than written as its own step). Not step-bound: "every rule applied" binds body of flat reference as "every step done" binds a sequence — how an all-reference doc still carries an exhaustiveness bar.

Strongest criteria: both checkable AND exhaustive.

## When to split

Splitting one doc into two spends one of two loads ⇒ split only when cut earns it:
- **By sequence**: split run of steps where post-completion steps tempt agent to rush the one in front. Keeping them out of view drives more legwork on current task. Beware reverse: merging sequences exposes each step's later steps to what follows, inviting premature completion.
- **By invocation**, skill-specific: see [`SKILL-MECHANICS.md`](SKILL-MECHANICS.md).

## Leading words

**Leading word**: compact concept already in model's pretraining that agent thinks with while running doc (_lesson_, _fog of war_, _tracer bullets_). Repeated as token, never sentence, accumulates distributed definition, anchors whole region of behaviour in fewest tokens, by recruiting priors model already holds. Coining your own works if defined clearly, but made-up word recruits no priors: you pay in definition tokens what pretrained word gives free. Reach for existing word first.

Anchors twice. Body → _execution_: agent reaches for same behaviour every time word appears; inside flat reference focuses attention on a class of thing to look for. Pointer → _invocation_: same word in prompts, docs, codebase ⇒ agent links shared language to material, reaches it more reliably.

Hunt opportunities to refactor with leading words: a triad spelled out at three sites, a pointer spending a sentence to gesture at one idea. Each passage begs collapse into single token:
- "fast, deterministic, low-overhead" → _tight_ (a _tight_ loop).
- "a loop you believe in" → _red_, turning fuzzy gate into binary observable state (loop goes _red_ on the bug, or it doesn't).

Win twice: fewer tokens, sharper hook to hang thinking on. Assume every doc carries restatements leading words retire. Go find them.

**Negation**: failure mode beside this lever. Steering by prohibition drags forbidden behaviour into context, makes it _more_ available, not less. _Don't think of an elephant_ → elephant is all there is; negation = weak modifier the strongly-activated concept overruns, so ban half-reads as instruction to do the thing. Prompt the **positive**: state target behaviour ("write one-line comments") so banned one never spoken. Prohibition earns its place only as hard guardrail you cannot phrase positively; even then pair with positive target so attention lands on what to do.

## Pruning

- **Single source of truth**: one authoritative place per meaning ⇒ changing behaviour = one-place edit. **Duplication** (same meaning >1 place) costs maintenance + tokens, inflates meaning's prominence on ladder past its real rank. (Accidental inverse of leading word, which repeats a token on purpose, never the meaning.)
- **Environment** is a source of truth too (`package.json` scripts, config files, directory layout, `--help` output). Doc restating it = **cache**: copy of a lookup, earns load only when lookup expensive. Cache what agent cannot find by looking: unwritten convention, reason behind a choice, gotcha no config confesses. Leave one-file, one-command lookups to environment, where they cannot go stale.
- Check every line for **relevance**: does it still bear on what doc does? Line loses relevance by never bearing on task (mere exposition, or branch that should be disclosed) or by going stale as behaviour/world changes. Shorter docs easier to keep relevant. Without pruning discipline, default fate = **sediment**: stale layers settle because adding feels safe, removing risky, until you core down through them to find what is live.
- Hunt **no-ops** sentence by sentence: instruction model already obeys by default pays load to say nothing. Test (does it change behaviour vs default?) is model-relative, not reader-relative: two people disagreeing about a no-op disagree about the default; settle by running the doc, not debate. Sentence fails → delete whole sentence, don't trim words. Test also grades leading words: word too weak to beat default (_be thorough_ when agent already thorough-ish) = no-op; fix = stronger word (_relentless_), not different technique.
