## What to look for

The home page lists **Start** and **Write**; the table page presents the same chapters as rows. `chapters.json` is a real generated download containing only each chapter's `id` and `title`. The private input also contains `text`, which the download intentionally omits.

Both pages include `content/index.typ`: the source path stays `index.typ`, while the document output reads `index.html` on one page and `table/index.html` on the other. `current-source()` is captured during ordinary evaluation; `current-document()` is read in `context` where the content is placed.

## One input, two HTML compositions

The root reads the JSON once and hands the same values to two small functions. These functions return ordinary HTML content; they neither choose document destinations nor publish files.

{{file:static/data/chapters.json}}
{{file:site/chapters.typ}}

## The root owns output declarations

Two sibling HTML documents reuse the page wrapper and include the same source body. `asset` publishes encoded bytes under an explicit output path. Reading `static/data/chapters.json` does not publish that input, and placing an asset beside documents is different from including it in a page body.

{{file:site.typ}}

Output filenames are independent of source filenames. The links use `output-to-url`, so the configured `/demo/` mount is applied once. Each output path has one owner.

{{file:content/index.typ}}
{{file:site/page.typ}}
