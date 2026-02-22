// Tola SSG utility functions
//
// Helper functions for common operations in Typst

// ============================================================================
// CSS Class Utilities
// ============================================================================

/// Join CSS classes with automatic space handling.
/// Accepts strings, arrays, or none. Filters out empty/none values.
/// Normalizes multiple spaces to single space.
///
/// Example:
/// ```typst
/// cls("text-2xl", "font-bold")           // => "text-2xl font-bold"
/// cls("text-2xl font-bold", "mt-4")      // => "text-2xl font-bold mt-4"
/// cls("base", if active { "active" })    // => "base active" or "base"
/// cls("a", none, "b", "")                // => "a b"
/// cls(("a", "b"), "c")                   // => "a b c"
/// cls("a  b", "c")                       // => "a b c" (normalizes spaces)
/// ```
#let cls(..args) = {
  let flatten(items) = {
    let result = ()
    for item in items {
      if type(item) == array { result += flatten(item) } else { result.push(item) }
    }
    result
  }
  let raw = flatten(args.pos())
    .filter(x => x != none and x != "")
    .map(x => str(x))
    .join(" ")
  raw.split(" ").filter(x => x != "").join(" ")
}

/// Remove classes from a class string.
///
/// Example:
/// ```typst
/// cls-rm("a b c", "b")        // => "a c"
/// cls-rm("a b c", "b", "c")   // => "a"
/// cls-rm("a b c", ("a", "c")) // => "b"
/// cls-rm("a  b  c", "b")      // => "a c" (normalizes spaces)
/// ```
#let cls-rm(base, ..remove) = {
  let to-remove = cls(..remove).split(" ")
  cls(base).split(" ").filter(x => x not in to-remove).join(" ")
}

/// Toggle a class: add if missing, remove if present.
///
/// Example:
/// ```typst
/// cls-toggle("a b", "c")   // => "a b c"
/// cls-toggle("a b c", "b") // => "a c"
/// ```
#let cls-toggle(base, class) = {
  let classes = cls(base).split(" ")
  if class in classes {
    classes.filter(x => x != class).join(" ")
  } else {
    cls(base, class)
  }
}

/// Check if a class exists in a class string.
///
/// Example:
/// ```typst
/// cls-has("a b c", "b")   // => true
/// cls-has("a b c", "d")   // => false
/// cls-has("a  b  c", "b") // => true (handles extra spaces)
/// ```
#let cls-has(base, class) = {
  class in cls(base).split(" ")
}

// ============================================================================
// Path Utilities
// ============================================================================

/// Extract trailing number from the last path segment (for natural sorting).
/// Returns `none` if no trailing number found.
///
/// Example:
/// ```typst
/// trailing-num("/blog/post-1/")   // => 1
/// trailing-num("/blog/post-01/")  // => 1
/// trailing-num("/blog/post-1x2/") // => 2
/// trailing-num("/blog/post-10/")  // => 10
/// trailing-num("/blog/post-0/")   // => 0
/// trailing-num("/blog/intro/")    // => none
/// ```
#let trailing-num(s) = {
  let parts = s.split("/").filter(x => x != "")
  if parts.len() == 0 { return none }
  let chars = parts.last().clusters().rev()
  let digits = ()
  for c in chars {
    if c >= "0" and c <= "9" { digits.push(c) } else { break }
  }
  if digits.len() == 0 { none } else { int(digits.rev().join()) }
}

// ============================================================================
// Content Utilities
// ============================================================================

/// Convert content to plain string.
/// Recursively extracts text from content elements.
///
/// Example:
/// ```typst
/// to-string[Hello *world*]  // => "Hello world"
/// to-string(none)           // => ""
/// to-string(42)             // => "42"
/// ```
#let to-string(it) = {
  if it == none { "" }
  else if type(it) == str { it }
  else if type(it) != content { str(it) }
  else if it.has("text") {
    if type(it.text) == str { it.text }
    else { to-string(it.text) }
  }
  else if it.has("children") { it.children.map(to-string).join() }
  else if it.has("body") { to-string(it.body) }
  else if it == [ ] { " " }
  else { "" }
}

// ============================================================================
// HTML Utilities
// ============================================================================

/// Set the browser tab title via inline script.
///
/// Example:
/// ```typst
/// #import "@tola/site:0.0.0": info
/// #set-tab-title(page-title + " | " + info.author + "'s blog")
/// ```
#let set-tab-title(title) = {
  let s = title
    .replace("\\", "\\\\")
    .replace("\"", "\\\"")
    .replace("\n", "")
  html.script("document.title=\"" + s + "\";")
}
