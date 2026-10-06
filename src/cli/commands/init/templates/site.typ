
#set text(
  lang: site.language.lang,
  script: site.language.script,
  region: site.language.region,
)

/// Every page this site publishes: one entry per source, paired with the output file its route
/// names.
#let pages = select-pages(all-sources())

#for page in pages { page-template(page) }

#not-found-template()
