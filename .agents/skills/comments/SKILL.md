---
name: comments
description: Write comments that carry an invariant. Use before adding, editing, merging, or deleting any comment, doc comment, or module doc — in source or in tests — and whenever a comment restates the code or the name it sits on, one fact is explained in more than one place, or a rename leaves a name inside a comment stale.
---

# Comments

A comment earns its line when it says something the code cannot. Everything else competes with the code for the reader's attention and goes stale unnoticed.

## Earns a line

- Invariant: what must stay true, what enforces it.
- Reason behind an ordering, an asymmetry, or an arbitrary-looking workaround.
- Removal condition of a workaround or known defect left in place: what still depends on it, what removes it.
- External constraint: protocol, platform rule, library behaviour the code cannot change.

## Delete

- Restatement of the code, the name, or the test above/below.
- Narration of the steps, or a label naming the block it sits on.
- Banners, separators, section markers.
- Story about the implementation that used to be here.
- Note about work nobody asked for.
- Doc comment repeating the signature it documents.

## One fact, one place

Explain an invariant where it is enforced; delete the copies. A caller depending on it need not say it again. Narrowest owner wins: the field or function that keeps the promise.

## How it reads

State the fact in the vocabulary the code already uses. Name the exact thing — the path, the key, the value — not "this" or "it". As long as the fact, no longer. A comment needing its own explanation signals rename (`skill://naming`) or extract, not further explanation.

Four standards, all of them: **concise** — the fact and no longer; **clear** — the reader's vocabulary, one reading; **rigorous** — no hedge, no claim the code does not support; **correct** — verified against the code, fixed or deleted the moment it drifts.

## Renames and doc comments

A name inside a comment is part of a rename: update it with the code; when that name was all the comment carried, delete it.

`///` tells a caller what an item is or does, in the caller's terms, not how it works inside. `//!` states the module's responsibility, not a list of what it contains.

## In tests

Same rules. A step needs no label; a comment earns its line by naming the boundary under test, or why the input has to look the way it does.
