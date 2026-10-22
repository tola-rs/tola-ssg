#import "@tola/site:0.0.0": site
#import "@tola/web:0.0.0": head-metadata

/// The document's character set, viewport, title and description.
#let head-entries(title, description: none, page: none) = {
  head-metadata(site, title: title, description: description)
}
