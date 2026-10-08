#import "@tola/schema:0.0.0": schema, optional, nullable, non-empty, trim
#let page-schema = schema((
  title: non-empty(trim(str)),
  order: optional(int, default: 0),
  draft: optional(bool, default: false),
  permalink: optional(nullable(str), default: none),
))
