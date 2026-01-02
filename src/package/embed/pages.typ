// @tola/pages:0.0.0 - Page metadata and filtering utilities
//
// pages() uses two-phase compilation:
//
// - "filter" phase (default): Scan phase for draft detection.
//   Returns empty array - dynamic content won't be generated,
//   but scan can complete without errors.
// - "visible" phase: Compile phase with injected data.
//   Returns actual pages data.

#let _phase = sys.inputs.at("__PHASE_KEY__", default: "__FILTER_PHASE__")

#let pages() = {
  let data = sys.inputs.at("__PAGES_KEY__", default: none)
  if data != none {
    data
  } else if _phase == "__FILTER_PHASE__" {
    // Return empty array during scan phase.
    // Dynamic content (links, etc.) won't be generated,
    // but will be correctly generated in compile phase.
    ()
  } else {
    // Visible phase without data - shouldn't happen in normal flow
    panic("@tola/pages: no data available (this is a bug)")
  }
}

#let by-tag(tag) = pages().filter(p => tag in p.tags)

#let by-tags(..tags) = pages().filter(p => tags.pos().all(t => t in p.tags))

#let all-tags() = {
  let result = ()
  for p in pages() { for t in p.tags { if t not in result { result.push(t) } } }
  result.sorted()
}
