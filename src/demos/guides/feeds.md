## What to look for

Open the four RSS files linked from the page. They share the same entry identity and summary but deliberately choose different bodies:

| Output | Body |
| --- | --- |
| `summary.xml` | No `content:encoded`; only the summary in `description` |
| `portable.xml` | The directly supplied portable text, including strong text and a URL link |
| `whole.xml` | The whole exported body, including navigation and the outside sidebar |
| `selected.xml` | The article subtree; the outside navigation and sidebar are excluded |

## Metadata is the site's input

`feed-summary` and `feed-content` are fields chosen by this demo. The source declares portable content and gives its rendered article an actual HTML `id`. The API later receives these values as `summary` and `content`.

{{file:content/index.typ}}

A string is plain text, including HTML-looking characters. Direct Typst content is converted from supported text, breaks, emphasis, strong, strike, and URL links. It is not rendered as a document: styled, deferred, and unsupported elements are rejected. Choose exported document HTML when the body needs headings, images, or the page's formatting.

## Four explicit declarations

The `id` on the entry identifies it to subscribers. The `id` inside `(document:, id:)` selects exactly one existing HTML element; it is not the entry identity. Selection keeps the wrappers and styles needed by that subtree and resolves its relative resources against the document URL.

{{file:site/feeds.typ}}

This demo omits `content` for summary-only output. The starter's separate feed recipe defaults absent or `none` `feed-content` to the whole body; assigning a summary alone does not make that recipe summary-only.

## Publish the page and feed outputs together

The root emits the page before declaring the feeds that describe it. The selected HTML uses the compiled document, and all final outputs enter the same complete build. The fixed publication date and configured origin keep the example reproducible; replace the origin before using the export for a real site.

Feed URLs use the configured `https://example.test`, not the temporary preview server's address. Local preview lets you inspect the page and RSS files; absolute image and page URLs inside those feeds still point at that example origin.

{{file:site.typ}}
{{file:tola.toml}}
{{file:site/page.typ}}
