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

#let find-prev(pages, key: p => p.date, filter: p => true) = {
  let me = pages.find(p => p.permalink == path)
  if me == none or key(me) == none { return none }
  let candidates = pages.filter(p => filter(p) and p.permalink != path and key(p) != none and key(p) > key(me))
  if candidates.len() > 0 { candidates.sorted(key: key).first() } else { none }
}

#let find-next(pages, key: p => p.date, filter: p => true) = {
  let me = pages.find(p => p.permalink == path)
  if me == none or key(me) == none { return none }
  let candidates = pages.filter(p => filter(p) and p.permalink != path and key(p) != none and key(p) < key(me))
  if candidates.len() > 0 { candidates.sorted(key: key).rev().first() } else { none }
}
