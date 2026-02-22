// @tola/current:0.0.0 - Current page context and navigation

#let _tola_current = sys.inputs.at("__CURRENT_KEY__", default: (:))
#let path = _tola_current.at("path", default: none)
#let parent = _tola_current.at("parent", default: none)

/// Pages this page links to (outgoing links).
/// Returns an array of page objects with permalink, title, date, etc.
#let links-to = _tola_current.at("links_to", default: ())

/// Pages that link to this page (backlinks).
/// Returns an array of page objects with permalink, title, date, etc.
#let linked-by = _tola_current.at("linked_by", default: ())

/// Document headings extracted during scan.
/// Returns an array of heading objects with `level` (1-6) and `text`.
#let headings = _tola_current.at("headings", default: ())

#let siblings(pages) = {
  if parent == none { return () }
  pages.filter(p => p.permalink != path and p.permalink.starts-with(parent) and {
    p.permalink.slice(parent.len()).split("/").filter(s => s != "").len() == 1
  })
}

#let children(pages) = {
  if path == none { return () }
  pages.filter(p => p.permalink != path and p.permalink.starts-with(path) and {
    p.permalink.slice(path.len()).split("/").filter(s => s != "").len() == 1
  })
}

#let breadcrumbs(pages) = {
  if path == none { return () }
  let parts = path.split("/").filter(s => s != "")
  let crumbs = ()
  let cur = "/"
  for part in parts {
    cur = cur + part + "/"
    let page = pages.find(p => p.permalink == cur)
    crumbs.push((permalink: cur, title: if page != none { page.title } else { part }))
  }
  crumbs
}

/// Find page at offset in a sorted list.
/// Returns none if current page not found or offset out of bounds.
#let at-offset(sorted-pages, offset) = {
  let idx = sorted-pages.position(p => p.permalink == path)
  if idx == none { return none }
  let target = idx + offset
  if target < 0 or target >= sorted-pages.len() { return none }
  sorted-pages.at(target)
}

/// Find previous page in a sorted list.
#let prev(sorted-pages, n: 1) = at-offset(sorted-pages, -n)

/// Find next page in a sorted list.
#let next(sorted-pages, n: 1) = at-offset(sorted-pages, n)

/// Take previous n pages in a sorted list.
#let take-prev(sorted-pages, n: 1) = {
  let idx = sorted-pages.position(p => p.permalink == path)
  if idx == none { return () }
  let start = calc.max(0, idx - n)
  sorted-pages.slice(start, idx)
}

/// Take next n pages in a sorted list.
#let take-next(sorted-pages, n: 1) = {
  let idx = sorted-pages.position(p => p.permalink == path)
  if idx == none { return () }
  let end = calc.min(sorted-pages.len(), idx + 1 + n)
  sorted-pages.slice(idx + 1, end)
}
