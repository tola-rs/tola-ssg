## What to look for

Open **Topic**. Alpha writes two links to it, and Beta writes one native body link: the incoming list shows **/a/** and **/b/** once each. Menus linking to Topic do not count. Home's only body link to Topic is absolute, so Home does not appear, even though that URL names `site.origin`.

## Select occurrences, then choose page rows

`references(to: auto, from-within: <body>)` reads native references written inside the labeled article bodies. It returns occurrences, so Alpha contributes two. The following `dedup` is this site's choice to display one row per linking document; the query itself keeps distinct occurrences from the same document.

{{file:site/backlinks.typ}}

The incoming list belongs outside `<body>`. Its own links therefore cannot enter the region it reads, which avoids a list extending its own query. A heading label names only the heading; the article label names the containing body region.

## Keep navigation outside the body region

The wrapper's menu links reach all pages, but the root labels only the included article content. The same body label in several documents selects all those body regions. Topic's whole-document label is a native target for links from source files.

{{file:site.typ}}
{{file:content/a.typ}}
{{file:content/b.typ}}
{{file:content/index.typ}}
{{file:content/topic.typ}}

Native targets retain document locations; URL targets identify outputs. `to: auto` accepts either route to this document. Final page, resource, and fragment validity is still checked by the complete build.
