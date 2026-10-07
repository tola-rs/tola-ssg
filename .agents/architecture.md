# Tola architecture

Agent memory: workspace map, subsystem responsibilities, mechanisms behind the `.agents/AGENTS.md` rules. Read to orient in the codebase. Refreshed once per finished batch that moved a crate, module, output identity, or check; small edits leave it alone, per-edit updates are churn. `AGENTS.md` keeps rules; this file keeps shape.

## Workspace map

Layout and ownership, not file inventory. Responsibility statements state who decides and who owns the invariant, so they survive renames and new members. Verify current members with tools, not this list.

- `src/`: the `tola` application: command policy, development runtime, editor integration.
- `crates/tola-address/`: slugs, routes, logical output paths, site URLs, deployment mounts, browser reference rules, incl. shared portable component rules for logical output and URL identities. A route is assembled from a slug; that slug is transformed by `tola-slugify`.
- `crates/tola-slugify/`: turning text into a slug: naming rules, character policy, rule sets for scripts that spell their own pronunciation. Encodes no URLs, chooses no output paths, names no language a site reads.
- `crates/tola-pronunciations/`: pronunciations Tola names for text: tables carried behind one feature per source, lookup preferring the longest named word, provenance and licence of every redistributed table.
- `crates/tola-build/`: independently consumable site construction engine used by `tola`.
- `crates/tola-typst/`: reusable Typst World/resources, official compilation/export, read evidence, and generic native diagnostic transport, independent of site rules. With `embed-fonts`, the fonts Typst's default documents read are carried compressed from the pinned typst-assets release, decoded on first use; `licenses/README.md` records their provenance and `NOTICE` their licences.
- `crates/tola-typst-syntax/`: immutable source indexes and cross-source name graphs derived from Typst's AST; syntax selection, scoped declarations/import interfaces, positions, declaration documentation read from the `///` blocks attached to declarations, editor edits, and optional formatting. Owns no host I/O or expression evaluation: unresolved import targets require the host's compiler evidence. Depends on and re-exports `typst-syntax` and `typst-library` (native metadata and colours); `format` adds `typstyle-core`. Compiler/IDE dependencies and protocol projection remain outside this crate.
- `crates/tola-icons/`: validated SVG values and immutable icon collections shared by producers.
- `crates/tola-image/`: deterministic image decoding, colour management, resampling, encoding. Owns which colour declaration a source's pixels are read by, and the pipeline revision every derivative shares. Encoded-sRGB convolution distinguishes fully covered RGBA8 rows from exact u16 coverage products, with signed i16/i32 intermediates and safe widening arithmetic; one byte-bounded per-thread scratch owner retains either representation. Oriented identity crops transfer their owned RGBA8 plane.
- `crates/tola-packages/`: builtin `@tola/*` packages: identity, sources carried, manifest each is resolved by, natives imported, values the engine supplies, site-native diagnostic meaning and evidence, and the documentation and signature each export carries from its entrypoint without evaluation.
- `crates/tola-config/` and `crates/tola-config-macros/`: reusable configuration protocol, runtime support, derive macros.
- `crates/tola-subprocess/`: trusted external command execution: one declared program with process-tree containment, bounded output capture, live relay to a caller-owned observer, cancellation observed against the child's own pipes. Owns the per-platform calls, so crates that forbid `unsafe` never name them; schedules no stages, owns no declared outputs, renders nothing a command printed.
  The bounded relay carries each reader's capture or failure with its completion; a failed reader ends the run, while closed streams retain cancellation polling until the child exits. Cancellation preserves the direct child's reaped status. Captured head and tail do not overlap.
- `crates/tola-lsp/`: independent Tola language-service coordination, JSON-RPC framing, connection-owned unsaved source versions, UTF-16 edits, host-aware queries, projection of source answers and site facts (routes, packages, configuration, diagnostics) into protocol shapes, and one client's negotiated wire shapes (snippets, documentation markup, hierarchical symbols, file renames) kept out of those answers. Source answers belong to `tola-typst-syntax`; site semantics to `tola-build`.
  Module families split by responsibility: `query/` prepares a request's source (`context`, `repair`), resolves what a spelling means (`name_graph`, `imports`, `meta`, `records/`), shapes the answer (`hover`, `completion`, `definition`, `signature`, `actions`, `labels`, `citations/`, `colors`, `postfix`, `editing`), and reads the checked world and configuration (`semantic`, `site_schema`, `schema`, `package`, `config`, `paths`, `hints`); `connection/` owns one client's protocol state (lifecycle, routing, replies, changes, scheduling, progress, the completion and source-job queues, code actions, commands); `server/` owns the host scopes and the two worker lanes; `compiler/` runs one job per request (jobs, analyze, check, compilations, readback, worker); `analysis/` builds the source-local name graph (disk, graph, selection); `diagnostic/` stores, publishes, projects, and refines what a client reads (store, publish, projection, refine, unread); `lint/` owns editor hints (branch, contentless, discard, fonts, imports, math); `code_actions/` owns fix construction (edits, near_miss, imports, narrowing, wrapping, headings, ancestors). Top-level owners: `identity.rs` (file and URI identity), `protocol.rs` (wire types), `capabilities.rs` (negotiated shapes), `markdown.rs` (markdown and plaintext rendering), `sentence.rs` (name-list phrases), `packages.rs` (the package access a query reads).
  A workspace that holds no site configuration is checked as the documents it holds, each compiled on its own.
- `crates/tola-minify/`: deterministic CSS and JavaScript minification of declared asset sources: languages a site minifies, source names that select them, the transformation incl. which comments and bindings survive it. Identity taken from published bytes, and diagnostic a failure becomes, belong to `tola-build`; HTML serialization is the Typst Bundle's own.
- `extensions/vscode/`: independent VS Code Tola client.
- `e2e/`: Deno and Playwright harness for user-observable flows across the CLI, development lifecycle, publication, and browser.

Keep responsibilities aligned with this map:

### Application

- CLI modules parse intent, choose command policy; they own no build semantics. The CLI configuration adapter translates argument overrides; command execution owns candidate construction through the build library. The help command dispatches configuration-table selectors and bundled-package selectors to separate renderers, loading no site. Empty configuration groups point to their child tables; package selectors validate every requested export before emitting output.
- `src/config/`: application configuration, shared `server` listener settings, development watching, CLI diagnostic display limits, invocation overrides retained across reloads.
- `src/dev/`: development serving, rebuild loop, revision installation, filesystem change boundaries, conservative browser reload. Development modules coordinate revisions and cancellation; they create no alternate build semantics.
- `src/terminal/`, `src/cli/output.rs`, `src/cli/log/`: terminal presentation and interaction. Input receives an explicit cancellation predicate; terminal code owns no command lifecycle, log files, or site configuration. `session` is the process's one owner of raw mode and the alternate screen: one view at a time claims the terminal (a prompt or a second view asking meanwhile is refused with `interactive view is open`), holds raw mode for its lifetime; a fullscreen view also holds the stderr lock and alternate screen, writes every frame through the locked writer rather than the log path, redraws and then waits 50 ms for the next crossterm event (skipping key releases, retrying interrupted waits, treating a closed input as cancellation), and observes the caller's cancellation between polls; a `dumb` terminal or a window with no size stays plain text, and `terminal::restore` restores both layers without taking the lock for the panic hook. `src/terminal/ui/` holds the vocabulary every screen builds on — the `Action` set, the one key table, the frame loop with its resize and zero-size guard, and the pager, table, filter, and hints components — with colors taken only from `Palette` roles. The seam a fallback can reuse is the driver and the injection, not the component bodies: `ui::Screen` (the frame loop, `Viewport::Fixed`), the `session::Terminal` trait (modes, events, size) that a test scripts, `Surface`/`Action`, the keymap table, `Pager`, and `Table`; every component's `draw` takes `&mut Frame` and builds ratatui `Line`/`Span`/`Paragraph`/`Block`, so a fallback rewrites their bodies. `tola help --interactive` drives the layer through `session::show`. `help::model` parses the complete Markdown page once, retaining export-section roles from the parsed package declarations. Export anchors are reserved before prose anchors; the shared declaration/comment parser used by LSP is unchanged. `help::layout` derives terminal lines, link fragments, anchor rows, and document positions in one pass; only it knows how a document position maps to wrapped rows. The generic `ui::Pager` owns scrolling and line search without help-specific types. Any content line can remain at the top through redraw and resize; `G` still selects the last full screen. Help derives `f`/`b` navigation from export heading roles, leaving screen paging separate. `help::navigation` retains at most 128 visits, sharing immutable documents and saving source positions and search focus. `help::jump` assigns one label per visible link occurrence and makes only drawn labels selectable. `help::view` composes those owners with mutually exclusive reading, search, and jump modes; resize reconciles viewport geometry before rebuilding labels, and all content movement clears hover. Mouse hit testing and hover use the same content-relative link fragments. Typed and pasted search edits share one transition; reflow and history restore the document position and focused match. Failed page or anchor resolution leaves navigation unchanged. The session owns mouse capture, title, and synchronized-update cleanup from the attempted mode acquisition, including partial failures. Plain `tola help` documentation leaves through the pager the environment names (`TOLA_PAGER`, then `PAGER`, then `less`), and directly to the terminal when stdout is not a terminal, `TERM` is unset or `dumb`, `--no-pager` is given, or no pager is available; a pager that cannot start falls back to direct output with a note. The interactive view never uses the external pager: a terminal that cannot draw prints the plain page instead. Convert structured outcomes to user-facing output only at the command boundary.
- Interactive inspection identifies selection by the original row index, or no row when filtering leaves none. Reformatting cells preserves the table and its detail scroll. Changing the selected match updates an open detail; the detail owns its visible height for paging. Filter and export prompts are exclusive; every screen's footer spells only keys whose action still changes what is on screen, so a movement that would not move, a page that already fits, or an end already reached shows no hint.
- `src/i18n/`: the language `tola help` writes bundled-package documentation in, and the packaged translations of page phrases and package units. English stays authoritative in `help.rs` and the packages' own Typst sources; a translated unit overrides exactly one of them, and a unit without a translation reads English. No other command or output follows the language.
- `src/editor/`: site-local editor setup and explicit native-editor package inputs. Applying setup and its dry run share checked preparation of merged files, package links, and obsolete-path removals; templates require no site. Editor-specific formats belong to the editors they configure. Listing and editor selection precede package preparation; completed setup reports automatic settings separately from manual snippets.
- `src/writes.rs`: checked creation and replacement of site files shared by commands and editors.
- `src/cli/commands/init/`: editable site scaffolding. Slot order resolves competing features without rendering files. Explicit CLI selections must satisfy dependencies; interactive selections add them before display and preserve them through every change. The same selection drives file marks, configuration, hooks, and templates; Deno tasks and imports come only from selected tools. Page templates use explicit placeholders, and both stylesheet options share one CSS base. Generated `selection.typ` validates metadata before filtering drafts and choosing outputs; feed and sitemap helpers consume those same outputs.

### Site construction

- `crates/tola-build/src/compiler/`: Typst inputs, official compilation, exported Bundle meaning incl. raw-text payloads generated HTML carries, read evidence, reusable compiler resources incl. host-selected network, download, file-cache, and font resources. Compiler modules publish no filesystem output.
- `crates/tola-build/src/build/`: construction and validation of a complete site candidate; command policy stays outside this layer. Owns hook, producer, compile, seal sequence, reusable producer resources and accepted caches, input observation and validity checks, complete build values, checked revision handoff, cross-process site build lock, virtual-package inputs shared by builds, source inspection, editor checks. Build modules combine producers into one candidate; they decide no terminal presentation or command-specific commit policy.
- `crates/tola-build/src/check.rs`: read-only editor source checks that prepare compiler resources without running hooks, writing output, or publishing a revision. Owns the licence that lets a revision reuse the previous revision's source analysis — identity first, then bytes — the read evidence that licence rests on, and the documents a check realized.
- `crates/tola-build/src/output/`: output ownership, conflict detection, manifests, graph construction, recoverable writes. Output modules own logical output identity, collision rules, revision comparison, publication; producers write nothing around the output graph.
- `crates/tola-build/src/cancellation.rs`: the cancellation token and cancelled error every stage, producer, and host worker shares.
- `crates/tola-build/src/site/`: realized documents and resources, route indexes, references, `SiteIndex` values, immutable revisions.
- `crates/tola-build/src/content/` and `metadata/`: content discovery, source identity, shared native Typst source dictionaries. Feed-specific author and content validation belongs to SEO.
- `crates/tola-build/src/filesystem/`: source identities, immutable source snapshots, fingerprints, shared path normalization, immutable unsaved source overrides shared by Typst and icon inputs, temporary directory ownership, atomic file writes.
- `crates/tola-build/src/config/` and `diagnostic/`: shared parsed configuration sources, validated build sections, structured diagnostics. Configuration modules deserialize, normalize, validate configuration; consumers receive values whose path and URL meaning is already established. Development and diagnostic display settings belong to the application; the library returns complete diagnostic records. The diagnostic protocol stays independent of concrete producers, output adapters, terminal rendering, Typst-specific diagnostic storage.
- `crates/tola-build/src/hooks/`: hook child processes and the environment each stage supplies — its stage name and build mode (`dev` or `prod`), an existing per-identity cache directory below `.tola`, the input and output directories the stage defines, and one private temporary directory — plus bounded output capture and cancellation. Content slug rules and SEO dates belong to their consumers.
- `crates/tola-build/src/html.rs`: shared HTML text escaping without compiler or site policy.

### Names and pronunciations

- `crates/tola-slugify/src/slug.rs`: turning text into a slug. Owns naming rules (characters, case, separator), the character policy deciding what a slug may hold, and how text no table names is left alone. Encodes no URLs, chooses no output paths.
- `crates/tola-slugify/src/romanization/`: scripts that spell their own pronunciation, and rules that read them (kana as Hepburn, Hangul as the Revised Romanization). A rule set owns its standards' examples as tests; never invents a pronunciation a rule cannot derive.
- `crates/tola-pronunciations/`: pronunciations named by the tables Tola carries, one feature per source, each table carried compressed and decoded on first use, lookup preferring the longest named word, provenance and licence recorded for every redistributed table. Which language a site reads belongs to the caller, not here.

### Icon collections

- `crates/tola-icons/src/collections.rs`: validated namespace ownership. The collection module owns collection values and Iconify import; SVG document handling owns the transient import tree and its validation. Frozen SVG values retain normalized bytes and local-reference indexes, not the import tree.

### Configuration protocol

- `crates/tola-config/src/`: field identities, TOML presence, diagnostic values, lifecycle status validation. A host configuration module supplies no runtime adapter.
- Configuration templates carry values only: a key's meaning is the declaration's own documentation, which `tola help` prints beside the defaults it renders. The scaffold pairs each section with one `tola help` pointer.

### Editors and language service

- `crates/tola-lsp/`: compiler preparation stays in `tola-build`; invocation configuration and native pipe I/O belong to the application's LSP command. Owns the public source URI grammar `tola-package:/namespace/name/version/path`, no authority, query, or fragment; its `connection/` filters watched file events by their own generation state, holding no syntax or compilation semantics. An import whose target identity the name graph cannot decide answers from the compiler lane, where the official engine's source inspection and full value trace are the only evidence: values normalize to one package file; an empty, ambiguous, truncated, or uncompiled trace refuses instead. A statement inside a function body refuses the same way: its call sites may live in another file with another argument, so the file's own evaluation observes one call, not the set of targets the statement binds to. A definition, a references set, a rename, and the highlight or rename preparation the editor asks for the same spelling all take that lane, which keeps the request's own id, serial, source view, and cancellation and enters its work without re-admitting the request or answering twice. The name graph injects proved targets through its one `ImportQuery::{Path, Expression}` shape and stays data: no second parser, no evaluation of its own. The source-only lane answers from the name graph alone: a definition, references set, rename, highlight, or token classification of a spelling the file itself establishes needs no compilation and survives a site that does not compile, while both lanes share one admission, cancellation, and supersede sequence. Unread-import reporting is that lane's diagnostics, licensed by the site-wide selection index: a file-scope binding another source can select from that file is left unnamed, and the payload carries the removal a fix applies. The call hierarchy projects the same graph the other way: an item is a source, its callers are the sources that reach it — the pages whose body carries it, which the check's realized documents prove, and the files whose `include` or `import` writes its path — and a caller's detail is the page it serves.
  The host reports the workspace as the site its configuration names, or as the documents it holds when it holds no configuration: those documents are checked one by one, and no diagnostic claims what a site's program would establish.
  A watched-file notification costs a revision only when the last completed check's own read paths, or the file's own kind (a source, the configuration, a bibliography and its style sheet, an ignore rule), can reach an answer; a membership change always matters, because a directory listing decides inventories no read of one file states. Each worker lane reuses the site's parsed files while their bytes are unchanged, identified by the same content digest a compiler read records, and re-reads the file list every pass, so a source the site gained is never missed. Name-graph reuse additionally compares every reached source's text and resolved import targets after following imports, so hidden and package dependencies participate in validity. Every bibliography the compiled site realizes says which files it reads and which style and language it renders with; citation answers read those files, reusing each parse by the same content digest, and a key's spellings are read from the sources the site's documents carry — the site's own files, searched on disk, only when the program did not compile.
  The connection derives opt-in source-check status from its existing queued/running revision and the official Bundle outcome returned by `SourceRevision::compiled`; superseded or cancelled checks cannot announce a current outcome. This is source feedback, not complete-build validity or development publication. Actions reuse the open source/name snapshot and existing site-wide selection evidence, validate echoed ranges against current syntax, and group each unused-import removal into one applicable edit. The reply boundary attaches the owned document version through negotiated `TextDocumentEdit`; clients lacking document changes retain unversioned edits. Checked routes already carry the deployment mount, so preview adds only the serving origin. Development preview independently lists installed HTML graph pages, optionally source-filtered, and exposes no selectable routes when its saved-input or revision fence fails.
  A pending check holds only its revision and sources until the compiler lane may run it; dispatch freezes the site's selection evidence with that revision, and the lane reads each open document's liveness — unused `let`/`for` bindings and stores no read observes — from the index that revision parsed, under the check's cancellation, so the connection only projects the findings.
- `extensions/vscode/`: source synchronization and read-only virtual package document presentation for the independent VS Code client, one client per enabled workspace folder. The client names the answering folder in every package document it shows and restores the server's exact URI grammar on every request, so two folders keep separate documents, a reload resolves each to its own service, and a folder whose service is not running says so instead of answering from another site. A package document or feature request arriving while services start waits for the transition deciding its folder. An unsaved Typst buffer answers from the one site its author chose (the only enabled site needs no choice) and sends every request and change to that client alone.
  A folder that holds no site configuration still gets the language client and answers about the documents it holds, while the pages view, build tasks, preview, route lenses, and `tola.site` stay with a configuration that names a site.

### Harness

- `e2e/` owns flows that cross real user boundaries. Group cases by user surface. Embedded runtime files are production source code: changes require the same ownership, compatibility, and test discipline as Rust changes.
- The root `justfile` imports Rust, quality, release and Nix recipes from `just/`; imports preserve root command names while subsystem modules retain namespaces. `just quality` owns formatting, TypeScript linting and test names; `just check` adds maintenance checks and Rust verification. CI shares Rust setup through `.github/actions/setup-rust` and leaves platform matrices in workflows.
- CI independently compiles every workspace target with all features on the minimum Rust read from `[workspace.package].rust-version` in the root manifest; ordinary checks retain their existing compiler selection.
- Cargo package verification owns a unique target/build directory per invocation; the path also gives Cargo's temporary registry fresh extracted sources. Release CI supplies shared licenses separately from target archives, then verifies their combined directory before publication.
- Release CI fixes every job to the invocation commit. Manual updates replace the generated assets, verify them, then move the tag; they retain the Release identity and status, with a choice to preserve or regenerate its title and notes. Updates skip crates.io availability because GitHub assets can change without republishing crates; Cargo package verification still runs. GitHub replacement is not atomic: a failed update can leave partial asset changes.
- `nix/per-system.nix` wires package, check and development-shell definitions to one build environment. `devShells.default` (`nix/dev-shell.nix`) carries the Rust toolchain, Just, Deno and actionlint, and validates its Deno version against `.tool-versions` during evaluation. Continuous integration evaluates the flake for every advertised system without building its derivations; portable release archives come from Cargo targets.

## Architecture mechanisms

### Compiler inputs and native warnings

`compiler/bundle.rs::EvaluatedSiteProgram` owns each converged source's paired file reads and package
checks. Build and editor checks consume one deterministic reader view, with the root Bundle reader
last. Their content roots, retained-reader reuse, unsaved-source identity, and freshness licences stay
with their respective callers.

`WorldBuilder::build(&BundleCancellation)` uses the caller's token for font preparation, including
fontless worlds. Required cache/font selections are checked first. Cancelled font preparation leaves
the store uninitialized; an active later attempt may prepare that same store. Official system-font
discovery and synchronous Typst compilation remain indivisible cancellation boundaries.

`NativeDiagnostic` restores a retained warning's original `Span` identity before resolving locations.
The official tracked sink owns warning replay, deduplication, and final-realization selection.
`ProducerDiagnosticOrigin` shares an opaque producer name and ordered text fields; its Serde
projection writes that name and field order without interpreting their meaning, preserving retained
diagnostic JSON and content IDs. `tola-packages` owns image-warning evidence, numbered-span tracked
source access and failures, and message/help text. `tola-build` interprets that producer evidence to
assign its diagnostic code. Legacy value/content conversion and resolved-serialization APIs remain
behind `legacy-serialization`.

Inline SVG baseline alignment belongs to `tola-build::compiler::html::frames`, after native
layout and before export. `BundleCompilation::style_html_frames` prepares CSS changes in one tree
walk, sharing unchanged subtrees and installing document copies only after all callbacks succeed.

### Build and publication lifecycle

Keep the lifecycle boundaries explicit:

1. Before-build hooks may produce declared inputs.
2. Discovery freezes the content and configured-asset inputs for this attempt.
3. Source analysis and the root Typst Bundle converge against a consistent source view.
4. Typst, configured assets, SEO, and embedded producers enter one candidate output graph.
5. Route, ownership, conflict, and reference checks validate the complete candidate.
6. A command policy publishes the validated graph or installs it as the next development revision.
7. After-publish hooks receive a read-only temporary view of committed output. Development executes them serially without delaying browser updates, retaining only the latest matching pending revision behind the running command; session shutdown cancels the current command and discards pending work.

Every hook stage builds one child environment: `TOLA_HOOK_STAGE`, `TOLA_BUILD_MODE` (`dev` or `prod`, from the attempt's real mode, never from the host environment), an already-created `TOLA_HOOK_CACHE_DIR`, and `TOLA_HOOK_TEMP_DIR` naming the invocation's private temporary directory — the same path `TMPDIR`, `TMP`, and `TEMP` carry. `TOLA_HOOK_INPUT_DIR` names the read-only upstream snapshot or committed revision the stage reads; only output generation also sets `TOLA_HOOK_OUTPUT_DIR`, the command's private generated root, whose written paths are relative to the final site output. Every variable a stage does not define is removed from the child, never inherited from the host, and unrelated `TOLA_*` variables stay untouched. The cache directory is identified by site root, stage, and configured name, so the same site reuses it across builds, dev restarts, and configuration reordering while other sites, stages, and names stay isolated; it is created before the command starts, survives success, failure, and cancellation, and is never read, cleaned, or deleted by Tola — validity of its contents belongs to the command.
Hooks are trusted scripts outside Tola's input restrictions, incl. `--offline` and `--pure`. Their declared inputs participate in observation; output commands must enter the validated graph. Tola backs up or restores no arbitrary script side effects. An after-publish failure belongs to the installed revision object its consumer ran for; installing a later revision clears it even when the output bytes are identical. Tola's own build failure or cancellation before publication leaves its last output and revision intact. Never advance revision state, dependency evidence, caches representing published success, or browser notifications for an unpublished candidate. Verified font and icon resources may be retained following their own successful preparation even when later source compilation fails; this establishes no published revision.

### Incremental development

Incremental work may skip recomputation only when retained evidence proves the authoritative result unchanged. A cache entry must have an explicit value, owner, and validity condition.

- Filesystem events: hints to re-evaluate evidence, not semantic truth by themselves.
- A redelivered event is settled before it enters `EventEpoch`, while the newest claim stands committed: a regular file at most 4 MiB whose bytes still equal what that claim captured owes no rebuild, so one save's split macOS event bursts cannot rebuild twice; an in-flight or failed attempt keeps every event, and one event reads at most 16 MiB. Timestamps stay outside that comparison, declared hook outputs keep their own evidence path, and anything unproven (other kinds, larger files, unreadable paths) keeps its event. The capture map is capped at 4096 paths and dropped whole on overflow.
- `EventEpoch` owns uncommitted accepted changes: at most 256 detailed paths, with epoch-qualified `FullSite` and producer uncertainty carrying genuine spill. Delivery bounds both messages and their path details; debounce, quarantine, claims, and superseding requests share the path bound. Dropped, failed, and cancelled claims consume nothing. Hook-descendant spill keeps affected bits in one fixed configured-output identity snapshot, not a growing descendant list or historical root union. Only current content evidence for affected roots may represent it; an ancestor is not covered, and a spill after the observed aggregate epoch remains owed. A declaration cutover folds unresolved former coverage at its former epoch without rewinding a newer site-rebuild or uncertainty marker.
- Each staged watch preparation derives configured requirements once, adds dynamic requirements, and shares one path/symlink/metadata observation map across observing union, candidate subscriptions, and configured-only coverage. The map never crosses rounds. Watch attachment and candidate freshness verification still precede installation.
- `CurrentSite` atomically installs an `InstalledRevision` through the authoritative `BuildSession`/`CheckedRevision` handoff: the immutable library revision and application-derived HTML slots move together. Slots exist only for requested graph outputs plus the welcome page; the session separately owns its unpublished page. Endpoint port, token, generation, and mount qualify injection reuse. Cold HEAD prepares only script metadata and exact representation length; first GET selects a safe leading-markup offset and three shared byte fragments outside the slot-map lock. The offset owner inspects leading comments/doctype/html/head tokens and quoted attributes, stopping before other content rather than interpreting raw-text payloads. Plain bodies use one byte owner, and responses retain only their needed bytes, never a historical site or representation cache.
- One original parser-blocking external runtime script carries `data-tola-bootstrap`: a Serde object of the existing port, session, revision, output, page availability, encoded mount, and generation fields, HTML-attribute escaped by the shared build helper. It claims its actual `Document` before metadata validation, captures and seals that metadata before site scripts, and initializes once the parser is ready. Reinjected fragment scripts cannot create another runtime or replace the parent document's output/revision/session ownership; there is no framework-specific routing integration.
- A plain change under the content root is admitted as an input change only for a `.typ` path or a path another subscription covers (a real read, assets, icons, declared outputs, `rerun-on`); structural events stay conservative, so an unread non-Typst write cannot loop a build and a real edit cannot be lost. An editor-artifact name drops only when no subscription keeps the event: a path at or above a subscription's root stays reachable, a subscription reading its whole scope (an asset tree, a font directory, a named file) keeps it, and one selecting members by name (content discovery reads only names carrying the source extension) does not.
- Structural changes must invalidate inventories depending on directory membership.
- Compiler read evidence must follow the source snapshot that produced the successful output. Recovery watches retain the latest failed attempt's typed input paths alongside physical reads and package checks, preserving producer, scope, and requiredness through the same requirement mapping as published observations. Replacing the failed attempt or installing a revision releases the preceding failure's paths.
- Source-analysis and complete-build caches retain the complete diagnostics of accepted producer output, including typed Typst producer identity. Reuse never reconstructs classification from display text; terminal limits stay outside library results and logs.
- Cancellation belongs to the superseded build attempt, must not render as a normal failure. Compiler world preparation receives the caller's `BundleCancellation`, including during font preparation; cancelled construction maps to the same cancellation outcome.
- The browser receives updates only after a complete revision is installed. Scriptless pages retain proven-safe positional document updates; a bounded transient set of captured nodes prevents restoration from resetting surviving controls, focus, or media. Native scrolling and anchoring own in-place edits. Linked CSS and directly referenced images can update on scripted pages without restoring application state. Scripted class-only updates compare one revision-fenced original HTML baseline against the next source, prove target identity against the live page, and leave unchanged widget subtrees untouched. Only a successful commit advances the baseline. Other scripted document edits, conflicting targets, unknown dependencies, and revision gaps require navigation.
- Browser resource observations retain at most 16384 distinct local output paths and bounded subsets describing fetch/XHR, other, and unsafe consumption; queries and representations share one dependency. The captured mount is decoded before observation begins. A live observer continues after the browser's historical timing buffer fills. Final validation reuses the same dependency reader, excludes runtime staging, and rejects changed consumers before any visual commit. Readable external CSS image URLs use the live CSSOM's installed representations, merged with the incoming diff; only staged sheets are rewritten. Each sheet prepares at most 128 images with four workers, preserving old resources through loading and decoding. Font faces must match existing loaded faces; new, changed, or unloaded faces remain a navigation boundary.
- `tola:before-update` lets application-owned data caches synchronously accept changed non-HTML output paths. Frozen event projections never expose mutable protocol messages. A path has one callback owner; at most 32 callback groups share a five-second browser-peer deadline and cancellation signal. Fetch/XHR data needs explicit acceptance even when the same bytes also have a built-in CSS/image consumer; known execution and unsupported consumers cannot be claimed. Visual preparation precedes callbacks, and successful callbacks precede final validation and one synchronous visual commit. Failure aborts peer work and requires navigation; arbitrary application side effects are not rolled back. Tola owns no Pagefind-specific runtime adapter.
- Editor source checks stop at official Bundle realization. Configured-asset transformation and SEO file production belong to complete builds, not each unsaved editor revision.

### Command output

`terminal::development` owns the development view, drawn on the alternate screen as one frame
holding the log, hook, and address header, the round counter, the body, hook tails, and the shared
key hints. The output sink's mutex orders frames, ordinary output, completed rounds, and hook
events; the input worker holds no output lock while polling. Its backend writer owns only the
stream destination, avoiding a cycle through the sink. Dropping the view stops and joins the
worker. `session::ViewModes` owns mode restoration, including failed startup.
`cli::output::CompletedRound` gives terminal presentation and JSONL recording the same publication
outcome.

Each completed round owns its styled transcript and the local arrival time; `Arc` shares a round
between the latest result and history. An arriving result takes over the view — a superseded
attempt's reading position goes with it — except a result whose diagnostics are exactly the
selected round's, which refreshes that one entry in place, moved to the most recent position with
the time of the newest observation and the earlier reading offset kept. A build with no
diagnostics never enters history. History holds 64 rounds with distinct diagnostics, matched by
the whole diagnostic set rather than by code, so the same code at another source location is its
own round; eviction preserves the selected round. The counter above the body names the selected round
`round x/n` with the time the result arrived, or `Ready` for a round with no diagnostics, and the
hints use the shared help styling at the window's bottom. The body and hook tails wrap with
`Paragraph::wrap`, so a resize reflows the same content: no text survives from an earlier size, a
zero-size window draws nothing and recovers on the next frame, and each visible hook keeps its name
line. Left/right switch rounds and reset scrolling; Home/End address the current body. Ctrl+C
stops development and retains the selected transcript once after mode restoration. Each running
hook keeps its latest 200 lines, with partial lines and stdout/stderr tracked separately. Hook
identity includes its development span, so overlapping attempts do not share output. Redirected,
quiet, dumb, or zero-size terminals use plain output. Command-owned JSONL logs record complete
rounds and hook output independently of the view.

### Native diagnostics

`tola-typst::diagnostic` transports opaque producer names and shared, ordered evidence beside the
original upstream diagnostic. Meaning belongs to the producer: the image package's
`AssetUrlShadowed` decoder names its warning, and `tola-build` maps it to the site diagnostic code.
Producers construct an upstream warning and emit through the official tracked sink; ordering,
memoized replay, and final realization remain Typst's authority. The process-local carrier retains
the original `Span` and evidence, restored on native ingress before filtering and source resolution,
with no additional source read at emission or ingress. A raw producer-helper return is still an
upstream egress value, not an already-normalized native diagnostic. Terminal text and deliberate
upstream projections do not carry classification authority.

Native messages and call traces survive resolution and site conversion; panic framing is removed
only by the shared display-message view. Cross-phase warning collection uses native span, message,
severity, and producer identity, merging package navigation while retaining the first context.
Source excerpts retain bounded head/tail windows from the compiled snapshot, with scalar and
UTF-16 origins for each window. Reference and minify diagnostics capture those same excerpts.
The terminal maps retained windows to dense rows, labels primary/help spans, and renders compact
styled call traces. Its line/column spelling follows Typst CLI (one-based lines, zero-based
columns); stored positions stay one-based and exact ranges stay zero-based UTF-16. Errors precede
warnings within terminal batches; `[diagnostics]` caps neither stored records nor editor reports.
Only an incomplete excerpt actually displayed by the terminal produces a shortening notice.
Grouped reference diagnostics count distinct pages in first-seen order.

### Source attributes

The source dictionary is collected only from a source's own `tola-meta(...)` declaration, an event that source's evaluation wrote, matched to its actual Source AST. A `metadata(...)` call carrying a `<tola-meta>` label registers nothing and is reported once for the site as `source.declaration_deprecated`. Do not execute an AST prefix or inspect metadata payloads as declarations. Field names and value types are user-defined; page and feed field mappings belong to editable site code. Ordinary compiled metadata/query semantics remain Typst's authority. A repeated source state and an exhausted iteration budget are distinct outcomes.

An accepted native declaration's dictionary and exact call range travel as one optional
`SourceMetadataDeclaration` through source inputs and analysis caches. Absence means no native
declaration; a present declaration carries both. A declaration emitted before evaluation fails
still contributes to the next metadata round.

### Native document relations

`tola-packages` queries the native Bundle relations a source writes; final exported-reference validity belongs to the complete output graph. A query's selected region defines its owners: only a selected outermost `ref` or `link` owns nested relations, so an owner outside the region cannot erase its selected inner references. The ordered native query is walked parent-first, checking each selected location against the latest retained owner through native singleton-location ancestry; this keeps Typst authoritative without submitting nested reference-range lists to its `within` range search.

Native reference targets retain the introspector’s anchor and location without a URL round trip.
URL targets report a decoded fragment and, for document URLs, the containing document’s location;
that location selects the whole document, including when the URL also names a fragment.

Final HTML references retain the browser-resolved query and serialized fragment. HTML fragment
matching checks the serialized id before its decoded spelling and accepts the document-top target;
browser fragment directives and non-HTML fragments have separate semantics. HTML URL-use rules,
including JavaScript MIME essences, belong to `tola-typst::html`. One reference is internal only
when it resolves to the document's own synthesized origin: the configured `site.origin` names
another origin's address, and a document `<base href>` naming one makes every relative reference of
the page external, which the build reports from that base's own span instead of skipping silently.
The build applies target and media constraints before projecting missing-anchor severity. Cached
reference resolution retains its address dependencies, while every build projects source positions
from its current world.

### Output and address identity

Keep source paths, normalized physical paths, logical output paths, public URLs, and document routes as separate types or clearly separate boundaries. Do not compare them as interchangeable strings.

- One logical output path has one owner. Conflicts are errors, not last-writer-wins behavior.
- A route's trailing slash is identity: `/x/` and `/x/index.html` name the output `x/index.html`, while `/x` names the file `x`; no lookup adds or drops a segment to guess a directory.
- Percent decoding happens once, at the URL boundary: an encoded URL path becomes a decoded route and a decoded route becomes a logical output path, so a literal `%` or `#` is an ordinary filename character and no consumer decodes a second time.
- The final output graph, not the site program, decides the not-found contract: a complete build publishing no `404.html` HTML document adds one `site.not_found_missing` warning naming the configured entry.
- `PageAvailability` derives HTML-page presence from the complete output graph: any HTML document counts. It owns the `site.no_pages` decision and the development host's welcome response at an unresolved mounted root; source discovery alone cannot decide either.
- URL construction follows site base-path semantics; filesystem joining must not determine URLs. The reserved `_tola` namespace is refused in a mount as in an output path: Tola's own endpoints and published assets live there.
- Route and fragment normalization must be deterministic and consistent between production, diagnostics, references, feeds, sitemap generation.
- `.tola` owns disposable local work, caches, logs, and editor mirrors, not formal source inputs. In every input scope, source checks reject that directory, `.tola-build.lock`, the published output tree, and the generated publication and vendor workspaces, incl. physical aliases; a layout check still refuses an output root overlapping its protected inputs, which read exclusion cannot decide. A declared hook output is refused against the configuration source, the published output, and the same owned roots while the configuration loads, and again before each `before-build` command runs. A hook cache directory lives below `.tola` and outlives the invocation that created it.
- `.tola-build.lock`: the stable site coordination authority outside that disposable directory. Build, vendor, and publication operations share it; lower publication borrows an already-held guard, reopening no lock. Idle development serving does not hold it.
- Production writing stages a complete tree in the validated `.<output-name>-publish` workspace beside the output, which also owns recovery of the transient previous tree. Keep workspace ownership, source overlap, and physical symlink boundaries checked. An empty unowned workspace is adopted in place, a workspace holding any entry stays foreign and is never deleted, and the `staging-*` trees a killed build left are reclaimed once the lock is held and the workspace record names this output. The deployed output remains a normal directory with its `_tola/owner` marker; neither it nor the workspace is a source input. Publishing commands recover an interrupted output before compilation; read-only checks and inspections do not. Failed installation restores the previous tree, while cleanup failure after commit does not revoke publication.
- Directory replacement is cross-platform and recoverable, not a version store or atomic durability guarantee. The rename sequence can leave the destination briefly absent for external filesystem readers; development serving switches complete immutable in-memory revisions atomically.
- `--offline` refuses Tola network access but permits host inputs and caches. `--pure` also excludes host package roots, system fonts, and source reads physically outside the site, incl. links inside package trees. Cached remote originals do not prove pure completeness; recomputable derived-image caches may be reused.

## Derived descriptions

A virtual package's exports have two derived descriptions outside the package source: `README.md` and `.agents/skills/tola/SKILL.md`. Changing an export, its signature, or its meaning requires updating both in the same change. `tola skill` compiles `SKILL.md` into the binary, so a stale list reaches every site author who runs it. Names come from the packages: `TolaPackage::exports` reads each entrypoint's top-level `#let` bindings (destructuring and closures included) and the members its explicit `import` statements publish, under the name the statement spells or renames with `as`, once per name; a wildcard import and a statement inside a function body publish nothing. `package_exports_match_their_entrypoints` holds every real entrypoint to the set it publishes, so a form the reader stops understanding fails a test instead of dropping a name silently.

`tola-packages::docs` reads package overviews from the entrypoint's opening `//` block and export
documentation from attached `///` blocks. A `Related:` line outside a fenced example names the
sibling exports and bundled packages an export points a reader to; `tola help` renders them as a
related-targets section that leaves out every target the same page already shows. Alias resolution
follows preceding lexical bindings;
native metadata supplies parameter types, defaults, and constant choices. Source expressions stay
unevaluated. The CLI help command composes Markdown sections from those declarations; command options
stay with Clap's own `tola <command> --help`. `terminal::documentation` parses Markdown with pulldown-cmark,
lays out paragraphs, lists, and tables using grapheme display widths, then applies ANSI styles.
Typst's syntax highlighter and TOML source spans color code without reflowing it. Narrow tables
become labelled fields. Stdout color and width are independent of diagnostic stderr; pipes use
fixed wrapping and no automatic color. Neither extraction nor rendering loads a site or evaluates
its program.

The inline `documented_examples_compile` test in `tola-build::package::tola` consumes those same
overviews and export docs. Every fenced `typ`/`typc`/`typst` block is a standalone Bundle program
under default site bindings and empty source records; a trailing `site` info word selects the
example site instead — one declared content source, a published asset tree, and an icon collection,
as `world_from_example_site` documents. The test supplies no imports or wrapper. Examples carry
their own assertions and any document context. An unknown info word or a fence syntax error fails
rather than skip a block. The existing workspace test job owns this check; no separate CI example
inventory exists.

## Validation surface

Prefer focused commands so failures stay attributable, for example:

```sh
cargo test --locked -p tola <focused-test-name>
cargo test --locked -p tola-typst <focused-test-name>
```

Choose final checks from the changed surface:

- Rust formatting: the workspace formatting command above.
- Site construction library: `cargo check --locked -p tola-build --all-targets` and `cargo test --locked -p tola-build`.
- Public site construction documentation: `cargo doc --locked -p tola-build --no-deps` with `RUSTDOCFLAGS=-D warnings`.
- Root generator behavior: `cargo test --locked -p tola`.
- Typst adapter behavior: `cargo test --locked -p tola-typst --all-features`.
- Feature or platform boundary changes: run the exact affected native or Windows checks from `.github/workflows/check.yml`.
- Test names across the workspace: `just scripts::test-names`. Maintenance tooling, including the audit implementation: `just scripts::check` runs type checks, behavior tests and release-command startup.
- Release packaging changes: evaluate the flake and build the affected release target rather than assuming a native Cargo build proves packaging.
