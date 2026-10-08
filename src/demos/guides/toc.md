## What to look for

The home document's TOC contains **Start** and **Detail**. `Deep detail` exceeds the requested depth and `Aside` has `outlined: false`. The other document has its own TOC containing **Outside the guide**. Click each entry: it lands on that heading in the correct document.

## Query the containing document

This reusable function returns contextual content. It runs where that content is placed, so `current-document()` and `headings()` belong to that document. A source file is not a page identity: the same source could be included elsewhere.

{{file:site/toc.typ}}

The depth filter uses the declared heading level. Filtering `outlined` is a separate choice. Linking the native location works for both labeled and unlabeled headings; do not invent or save generated `loc-N` names.

## Give each document a body and a TOC

The root chooses its sources by path, not by array position, and places the same TOC function in two documents.

{{file:site.typ}}

## Labels and outline visibility

`<start>` supplies a stable label for Start. Detail has no label, but its TOC link causes an anchor to be exported. The final output, not the spelling of a generated identifier, establishes where that link lands.

{{file:content/index.typ}}
{{file:content/other.typ}}
{{file:site/page.typ}}
