---
name: naming
description: Name what a thing is, not how it is built. Use before writing, renaming, or reviewing a name in Rust code — a type, function, trait, module, file, collection, field, variable, boolean, constant, error, cache, test, CLI flag, or configuration key — and whenever a name is vague, drifts from what it holds, or hides a design nobody can state.
---

# Naming
Scope: names in Rust code. Typst sources — the embedded `@tola/*` packages, generated scaffolding, and site recipes — follow Typst's conventions and the site vocabulary the `tola` skill records.

A name carries 3 facts: what represented, who owns it, when valid. Cannot answer all 3 from name + owner → wrong name.

## Start from the domain

Name the entity or invariant, not the storage technique: `SourceSnapshot` holds frozen source identity; `BytesBlob` says only how bytes are kept.

- Type: the entity or invariant it represents.
- General term: earned by breadth carried, never by default — the precise word when several distinct things really share it (one `Diagnostic` for the record every producer emits; one `Source` for every file the compiler reads). Narrowed to one concrete thing, an earned word still describes it; an unearned one collapses — was filler all along.
- Function: the operation, and the value produced when not obvious.
- Module/file: one cohesive responsibility — not a phase of work, not a bucket for unrelated code.
- Collection: what members are: `DeclaredOutputs`, not `outputList`.
- Boolean: reads as the predicate held: `is_draft`, not `draft_flag`.
- Transformation: names input and output: `normalize_path`, not `process_path`.
- Error: the contract that failed: `MissingPackageRoot`, not `InvalidInput`.
- Cache: the reusable value + boundary where it stops being valid.

## Avoid

- Never `catalog` in Tola-owned identifiers, modules, files, directories — incl case variants and compounds (`PackageCatalog`, `package_catalog`). Name the actual entity: `PublishedPackages`, `PackageVersions`, `IconCollections`. Another vague container word does not fix it. Preserve user-authored names and upstream-API spellings; do not adopt them for Tola's own.
- Placeholder/abstraction nouns: `data`, `info`, `item`, `object`, `context`, `state`, `result`, `policy`, `entry`, `record`, `element`, `input`, `fixture`, `probe`. Name what the value holds or decides: `ReferenceLevel` not `ReferencePolicy`; `SlugSeparator` not `SeparatorPolicy`; `ImportPath` not `Input`; a test helper named for the value it returns (`html_page`, `asset_route`), never the technique that builds it.
- Gerund for a value the domain already names: `reading` for a pronunciation or compiler read; `processing`/`handling` for the thing they produce. Established term wins — `pronunciation`, `read`, read evidence.
- Role suffixes hiding a design nobody can name: `Manager`, `Processor`, `Handler`, `Helper`, `Util`.
- Participant the value does not depend on: `authored`, `provided`, `handled`, `processed`, `managed`. Name states the relationship carried (`configured`, `resolved`, `declared`), not who/what produced it. Participant earns its place only when it changes meaning — `imported_by` for a diagnostic raised inside a package.
- Test either word: state the relationship as a rule another file could follow. `resolved` passes — the filesystem's own spelling, symlinks followed. `authored` fails — no rule distinguishes it from `path`.
- Near-synonym for a word the repo already uses. Established term wins.
- Abstraction whose boundary and invariant cannot be said in its name.
- Type named by listing what it holds: `ResolvedSiteConfigWithSiteDirectory` names a field pair, not the invariant making the pair necessary (`OwnedSiteConfig`). Conjunction only when both parts are domain data that must travel together and no single noun covers them; when one part is plumbing merely keeping the other valid, name the invariant instead. Longer is the symptom, not the fix.
- Stutter with the owner that gives context: `routes::routes`, `Route::route_path`, `Connection::connect_connection`. Inside `routes`, a function states the operation (`site_routes`, `respond`), a type the entity (`Route`); the module already said where they live.

Existing vague names do not license new ones.

## The check

Before accepting a name, answer from name + owner alone:

1. What is represented?
2. Who owns it?
3. When is it valid?

Needs the implementation → change the name.

Four standards, all: **concise** — as short as the entity allows; **clear** — those 3 answers, without the implementation; **rigorous** — no filler noun, no role suffix, no word another concept owns; **correct** — states what the value is, and a rename leaves no old spelling behind.

## Renaming

Rename when the name lies, drifts from what the value holds, or hides a design that cannot be stated. Do it whole: declaration, every call site, imports, strings that spell it, assertions that expect it, comments that mention it. No alias, wrapper, or old spelling stays behind.
