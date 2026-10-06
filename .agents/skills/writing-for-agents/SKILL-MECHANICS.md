# Skill mechanics

Skill-specific branch of [`writing-for-agents`](SKILL.md): what changes when doc is a skill — frontmatter, invocation choice, router skills. All else: universal reference in `SKILL.md`.

## Invocation

Two choices, trading two loads.

**Model-invoked**: keeps `description` → agent fires it autonomously; other skills can reach it. User can still type its name — model-invocation always includes user reach; description adds agent discovery, never removes human's. Description = skill's top-level context pointer, forced always-loaded: permanent context load for discoverability. A model-invoked skill with all-reference content = one home for shared reference — another skill can invoke it, so reference several skills need lives in one place. Mechanics: omit `disable-model-invocation`; write model-facing description carrying trigger branches (pointer-writing rules in `SKILL.md` apply in full).

**User-invoked**: strips description from agent's reach — only human typing its name invokes it; no other skill can. Zero context load; spends cognitive load — you are the index that must remember it exists. Mechanics: set `disable-model-invocation: true`; `description` becomes human-facing — one-line summary, trigger lists stripped.

Pick model-invocation only when agent must reach skill on its own, or another skill must. Fires only by hand → user-invoked, pay no context load.

Shared reference two user-invoked skills both need can live in neither: no descriptions → neither can fire the other. Push to plain file outside skill system — external reference any skill can point at.

## Splitting by invocation

Invocation cut of splitting (sequence cut in `SKILL.md`): split off model-invoked skill when you have a distinct leading word that should trigger it on its own (a trigger word you actually use in prompts), or another skill must reach it. New always-loaded description costs context load → independent reach must be worth it.

## Router skills

User-invoked skills multiply past what you can remember → piled cognitive load cured by **router skill**: one user-invoked skill naming the others and when to reach for each — human remembers one skill instead of many. It can only hint, never fire them: user-invoked skills have no description → nothing but human can reach them.
