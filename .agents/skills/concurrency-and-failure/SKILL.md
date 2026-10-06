---
name: concurrency-and-failure
description: Design and review work that runs concurrently, waits, retries, or owns resources. Use when adding or changing threads, tasks, channels, queues, timeouts, retries, cancellation, bounded caches, child processes, temporary directories, or shared state across stages.
---

# Concurrency and Failure

For work that runs beside other work, waits on a peer, or owns resources. `.agents/AGENTS.md` carries
the always-applicable constraints; this file is the discipline behind the concurrent shapes this
repository already uses.

## Bound everything a new edge introduces

- Every queue, channel, batch, retained cache, and message declares a cap next to its owner, and a
  defined full path — drop, coalesce, or block — never unbounded growth. The subprocess relay queue
  holds 2 items, the filesystem watch queue coalesces at 256, image work caps batch bytes, retained
  pixels, decode size, and worker count.
- Waits end at cancellation or at the peer's own deadline (HTTP connect and total, handshake); they
  never end at a guessed wall-clock. No deadline kills a build, a hook, a lock wait, or shutdown.
- Retries are rare and deliberate, each stating why one attempt is insufficient: one rebuild retry
  after watches attach, at most ten bind attempts. Downloads and package fetches attempt once — a
  retry is not a fix for a flaky step.
- A new wait, poll, or retry loop names what ends it, where it observes cancellation, and what
  bounds it.

## Cancellation is an outcome

- Observe the shared token between steps; polling is the whole mechanism, because no signal
  interrupts a running step. `BuildCanceller::token` is that token.
- Classification goes through the cancellation chain (`is_cancelled`), never string matching.
  Cancellation attaches no diagnostic: the CLI prints `Cancelled` and exits 130.
- Work that lost its reason stops at the next recheck. Staging rechecks inputs and cancellation
  before rename; the checked revision rechecks at handoff; a superseded or rejected candidate never
  becomes current.
- Recovery and rollback deliberately ignore cancellation: a restore of the previous tree or output
  finishes even when cancellation arrives.

## Release on every exit path

- Ownership is drop-based: the value that owns a child process, temporary tree, or download
  terminates or removes it when dropped, and handing work on moves that owner instead of sharing
  it. A staged build's private tree disappears if the value drops before commit; a child's drop
  terminates its containment domain; an aborted download's drop cancels the task.
- Error and cancellation paths release through the same drop, not a separate cleanup branch.
- Cleanup failure after success only warns — it never revokes the committed result. A rollback path
  is the opposite: it must finish.

## Make visibility a commit point

- Readers see whole values, never partial mutation: revisions swap atomically, a cache baseline
  advances only through its commit, and a published graph is never partially materialized.
- A step that can rerun leaves nothing consumed: a dropped claim consumes nothing, and a superseded
  attempt must not have advanced any baseline.
- Every concurrent producer declares its ordering: which error wins when two fail at once (assets
  before producers before the compiler, independent of completion order), and what becomes visible
  only after the commit.

## Failures surface as diagnostics

- Errors attach a diagnostic code at the boundary that knows the operation; cancellation
  short-circuits before anything attaches.
- An unclassified internal failure still produces a human sentence; the fallback uses the outermost
  message.

## Current limits

Stated so a new edge never assumes them away, and no unrelated change repairs them in passing:

- Cancellation is observed by polling; there is no deadline or hard kill.
- Downloads, package fetches, and file reads take a single attempt.
- The site build lock waits until acquired or cancelled.
- The Typst package download path observes no cancellation token.
- Producers are bounded by their pool, not by a count cap; only image work caps bytes and workers.
- The image variant cache is never pruned.
- Development rechecks a failed build attempt at most once per settle: the recheck covers reads that
  happened before their watch was attached, and every later change arrives as its own event.
- A hook, or a child process it left behind, that rewrites a declared output after the failed
  attempt's snapshot stays a retry reason; only current content evidence may silence it, and
  `failed_hook_outputs` never proves generation.
- A declared output's ancestor path in an event is quarantined as hook work, and the output's own
  evidence never covers an ancestor, so such an event can spend a failing settle's one recheck.
- A write that lands after a read is never dropped: it stays pending in the event epoch, so a later
  candidate rechecks it even when the settle call declined an immediate recheck.
- In development, a project hook that rewrites a file the build reads without declaring that path as
  an output gives a failing session a real change every round, because nothing attributes the write
  to the hook; declaring the path is what stops it.
- In development, a hook that replaces the directory holding its declared outputs every round, by
  `rm -rf` and `mkdir` or any equivalent directory-level rebuild, keeps the session rebuilding: the
  replacement is a real change, and each one legitimately supersedes the attempt in flight, so this
  loop stays event-driven and the settle recheck's bound does not reach it. Declaring that directory
  as the hook's output tree, not only a file inside it, is what attributes the writes to hook
  evidence.
