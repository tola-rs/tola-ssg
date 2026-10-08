## What to look for

The navigation reads **Sources become pages → Advanced → Getting started**. `Advanced` is published at `/demo/chosen/`; there is no draft page. The declared-metadata JSON still contains the untrimmed title and the draft: parsing changes the returned records, not the source declarations.

## Declare, validate, then choose

`title`, `draft`, `order`, and `permalink` are this site's conventions. Every source is parsed before drafts are filtered. A draft's Typst or schema error still fails the build. Unknown fields are errors in this schema; add a field here before using it in content.

{{file:site/schema.typ}}

`optional` handles absent keys; `nullable` handles explicit `none`. The title is trimmed before `non-empty` rejects it. Editorial order is chosen explicitly, with the source identity as a tie-breaker. The permalink is decoded once and then converted to an output path.

{{file:site/selection.typ}}

## Reuse selected records

The root receives complete source records, produces `(source:, output:)` records, and passes those to navigation and the page wrapper. `source.file` is a Typst input path; it is not a browser URL. Keeping destinations in these records prevents menus and documents from choosing different routes.

{{file:site.typ}}
{{file:site/navigation.typ}}

## The source and its rendered title

The declaration keeps surrounding spaces; the schema's returned value gives the document and navigation their trimmed title. `#title()` reads the document title chosen by the root.

{{file:content/start.typ}}
{{file:content/advanced.typ}}
{{file:content/draft.typ}}

The shared wrapper supplies the browser head and stylesheet. Configuration owns the deployment mount; output paths stay unprefixed.

{{file:site/page.typ}}
{{file:tola.toml}}
