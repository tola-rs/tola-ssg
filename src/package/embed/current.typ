// @tola/current:0.0.0 - Current page context and navigation

#let _tola_current = sys.inputs.at("__CURRENT_KEY__", default: (:))

/// Current page's permalink (URL path).
/// Example: "/blog/hello/"
#let current-permalink = _tola_current.at("current-permalink", default: none)

/// Parent page's permalink.
/// Example: "/blog/"
#let parent-permalink = _tola_current.at("parent-permalink", default: none)

/// Source file path relative to content directory.
/// Example: "blog/2025_02_27_hello.typ"
#let path = _tola_current.at("path", default: none)

/// Source filename only (last segment of `path`).
/// Example: "2025_02_27_hello.typ"
#let filename = _tola_current.at("filename", default: none)

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
  if parent-permalink == none { return () }
  pages.filter(p => (
    p.permalink != current-permalink
      and p.permalink.starts-with(parent-permalink)
      and {
        p.permalink.slice(parent-permalink.len()).split("/").filter(s => s != "").len() == 1
      }
  ))
}

#let children(pages) = {
  if current-permalink == none { return () }
  pages.filter(p => (
    p.permalink != current-permalink
      and p.permalink.starts-with(current-permalink)
      and {
        p.permalink.slice(current-permalink.len()).split("/").filter(s => s != "").len() == 1
      }
  ))
}

#let breadcrumbs(pages, include-root: false) = {
  if current-permalink == none { return () }
  let parts = current-permalink.split("/").filter(s => s != "")
  let crumbs = ()
  let root-page = pages.find(p => p.permalink == "/")
  if include-root {
    let root-title = if root-page != none {
      root-page.at("title", default: none)
    } else {
      none
    }
    crumbs.push((
      permalink: "/",
      title: if root-title != none { root-title } else { "/" },
      exists: root-page != none,
      page: root-page,
    ))
  }
  let cur = "/"
  for part in parts {
    cur = cur + part + "/"
    let page = pages.find(p => p.permalink == cur)
    let page-title = if page != none {
      page.at("title", default: none)
    } else {
      none
    }
    crumbs.push((
      permalink: cur,
      title: if page-title != none { page-title } else { part },
      exists: page != none,
      page: page,
    ))
  }
  crumbs
}

/// Find page at offset in a sorted list.
/// Returns none if current page not found or offset out of bounds.
#let at-offset(sorted-pages, offset) = {
  let idx = sorted-pages.position(p => p.permalink == current-permalink)
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
  let idx = sorted-pages.position(p => p.permalink == current-permalink)
  if idx == none { return () }
  let start = calc.max(0, idx - n)
  sorted-pages.slice(start, idx)
}

/// Take next n pages in a sorted list.
#let take-next(sorted-pages, n: 1) = {
  let idx = sorted-pages.position(p => p.permalink == current-permalink)
  if idx == none { return () }
  let end = calc.min(sorted-pages.len(), idx + 1 + n)
  sorted-pages.slice(idx + 1, end)
}
