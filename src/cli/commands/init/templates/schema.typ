// Add custom page fields here; unknown fields are errors.

#import "@tola/schema:0.0.0": describe, literal, nullable, optional, schema, union, one-or-many, trim
#import "@tola/site:0.0.0": site

/// One page author: a `name`, with an optional `email` and `url`.
///
/// Unknown author fields stay available to templates; the feed recipe reads `name`, `email`, and
/// `url`.
#let author = schema((
  name: str,
  email: optional(nullable(str)),
  url: optional(nullable(str)),
), unknown: "keep")

/// The page metadata declaration: the fields a source may set, each defaulting to the site's
/// value where one applies.
#let page-schema = schema((
  title: describe(
    optional(nullable(union(trim(str), content)), default: site.title),
    "the page title, shown in the browser tab and in lists",
  ),
  description: describe(
    optional(nullable(union(trim(str), content)), default: site.description),
    "the page summary, shown in search results and link previews",
  ),
  authors: describe(
    optional(nullable(one-or-many(union(str, author))), default: site.authors),
    "who wrote the page: a name, or a dictionary with name, email, and url",
  ),
  tags: describe(
    optional(one-or-many(str), default: ()),
    "the topics this page belongs to",
  ),
  draft: describe(
    optional(bool, default: false),
    "keeps the page out of the published site",
  ),
  permalink: describe(
    optional(nullable(str), default: none),
    "overrides the file-based route: `/about/` is a directory, `/about.html` is a file",
  ),
  feed: describe(
    optional(bool, default: true),
    "includes the page in feeds when a publication date is set",
  ),
  sitemap: describe(
    optional(bool, default: true),
    "includes the page in `sitemap.xml`",
  ),
  id: describe(
    optional(union(str, literal(auto)), default: auto),
    "a stable feed identifier; `auto` derives one from the page's address",
  ),
  published: describe(
    optional(nullable(union(datetime, str)), default: none),
    "the publication date: a complete `datetime` or an RFC 3339 string with timezone",
  ),
  updated: describe(
    optional(nullable(union(datetime, str)), default: none),
    "the last change: a complete `datetime` or an RFC 3339 string with timezone",
  ),
  feed-summary: describe(
    optional(nullable(union(trim(str), content)), default: none),
    "the summary a feed shows in place of the whole page: plain text, or markup limited to text, breaks, emphasis, strong, strike, and URL links",
  ),
  feed-content: describe(
    optional(nullable(union(str, content, dictionary)), default: none),
    "the feed body: plain text, markup limited to text, breaks, emphasis, strong, strike, and URL links, or a `(document:, id:)` selection",
  ),
))
