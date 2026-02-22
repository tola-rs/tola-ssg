// Tola SSG utility functions (v__VERSION__)
//
// AUTO-GENERATED - Avoid modifying this file directly.
// Instead, extend it or create your own copy to reduce migration
// difficulty when upgrading to future versions with breaking changes.
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
  let raw = flatten(args.pos()).filter(x => x != none and x != "").map(x => str(x)).join(" ")
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
  if it == none { "" } else if type(it) == str { it } else if type(it) != content { str(it) } else if it.has("text") {
    if type(it.text) == str { it.text } else { to-string(it.text) }
  } else if it.has("children") { it.children.map(to-string).join() } else if it.has("body") {
    to-string(it.body)
  } else if it == [ ] { " " } else { "" }
}

// ============================================================================
// Date Utilities
// ============================================================================

/// Check if a year is a leap year.
#let _is-leap-year(year) = {
  calc.rem(year, 4) == 0 and (calc.rem(year, 100) != 0 or calc.rem(year, 400) == 0)
}

/// Get the number of days in a month.
#let _days-in-month(year, month) = {
  if month in (1, 3, 5, 7, 8, 10, 12) { 31 } else if month in (4, 6, 9, 11) { 30 } else if month == 2 {
    if _is-leap-year(year) { 29 } else { 28 }
  } else { 0 }
}

/// Parse a date string into a datetime object.
/// Only supports format: "YYYY-MM-DD" (e.g., "2024-01-15", "2024-1-5")
///
/// Example:
/// ```typst
/// parse-date("2024-01-15")  // => datetime(year: 2024, month: 1, day: 15)
/// parse-date("2024-1-5")    // => datetime(year: 2024, month: 1, day: 5)
/// parse-date(none)          // => none
/// ```
#let parse-date(s) = {
  if s == none { return none }
  if type(s) == datetime { return s }
  let s = str(s).split("T").at(0) // Strip time part if present
  let parts = s.split("-")
  assert(parts.len() == 3, message: "Invalid date format: '" + s + "', expected YYYY-MM-DD")
  let year = int(parts.at(0))
  let month = int(parts.at(1))
  let day = int(parts.at(2))
  assert(month >= 1 and month <= 12, message: "Invalid month: " + str(month) + " in date '" + s + "'")
  let max-days = _days-in-month(year, month)
  assert(
    day >= 1 and day <= max-days,
    message: "Invalid day: " + str(day) + " for month " + str(month) + " in date '" + s + "'",
  )
  datetime(year: year, month: month, day: day)
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
  let s = title.replace("\\", "\\\\").replace("\"", "\\\"").replace("\n", "")
  html.script("document.title=\"" + s + "\";")
}

// ============================================================================
// SEO Utilities (Open Graph / Twitter Cards)
// ============================================================================

/// Generate a single OG meta tag.
/// - `prop`: property name (e.g., "og:title", "twitter:card")
/// - `content`: content value
#let _og-meta(prop, content) = {
  if content == none or content == "" { return none }
  let c = if type(content) == datetime {
    content.display("[year]-[month]-[day]")
  } else {
    to-string(content)
  }
  // OG/article/book/profile use "property" attribute, not supported by html.meta
  if (
    prop.starts-with("og:") or prop.starts-with("article:") or prop.starts-with("book:") or prop.starts-with("profile:")
  ) {
    html.elem("meta", attrs: (property: prop, content: c))
  } else {
    html.meta(name: prop, content: c)
  }
}

/// Generate Open Graph and Twitter Card meta tags.
///
/// All parameters are optional. Missing values will be omitted from output.
///
/// Supported og:type values:
/// - "website" (default for pages)
/// - "article" (default for posts)
/// - "book"
/// - "profile"
///
/// Parameters:
/// - `title`: Page title (og:title, twitter:title)
/// - `description`: Page description (og:description, twitter:description)
/// - `url`: Canonical URL (og:url)
/// - `image`: Preview image URL (og:image, twitter:image)
/// - `type`: Content type (og:type)
/// - `site-name`: Site name (og:site_name)
/// - `locale`: Locale string e.g. "en_US" (og:locale)
/// - Article-specific (type="article"):
///   - `author`: Author name or URL (article:author)
///   - `section`: Section/category (article:section)
///   - `published`: Published date ISO string (article:published_time)
///   - `modified`: Modified date ISO string (article:modified_time)
///   - `tags`: Array of tags (article:tag)
/// - Book-specific (type="book"):
///   - `author`: Author name or URL (book:author)
///   - `isbn`: ISBN number (book:isbn)
///   - `release-date`: Release date (book:release_date)
///   - `tags`: Array of tags (book:tag)
/// - Profile-specific (type="profile"):
///   - `first-name`: First name (profile:first_name)
///   - `last-name`: Last name (profile:last_name)
///   - `username`: Username (profile:username)
///   - `gender`: Gender (profile:gender)
/// - Twitter Card:
///   - `twitter-card`: Card type, default "summary_large_image"
///   - `twitter-site`: Twitter @username for the site
///   - `twitter-creator`: Twitter @username for the author
#let og-tags(
  title: none,
  description: none,
  url: none,
  image: none,
  type: "article",
  site-name: none,
  locale: none,
  // Article
  author: none,
  section: none,
  published: none,
  modified: none,
  tags: (),
  // Book
  isbn: none,
  release-date: none,
  // Profile
  first-name: none,
  last-name: none,
  username: none,
  gender: none,
  // Twitter
  twitter-card: "summary_large_image",
  twitter-site: none,
  twitter-creator: none,
) = context {
  if target() != "html" { return }

  // Open Graph basic
  _og-meta("og:title", title)
  _og-meta("og:description", description)
  _og-meta("og:url", url)
  _og-meta("og:image", image)
  _og-meta("og:type", type)
  _og-meta("og:site_name", site-name)
  _og-meta("og:locale", locale)

  // Type-specific metadata
  if type == "article" {
    _og-meta("article:author", author)
    _og-meta("article:section", section)
    _og-meta("article:published_time", published)
    _og-meta("article:modified_time", modified)
    for tag in tags { _og-meta("article:tag", tag) }
  } else if type == "book" {
    _og-meta("book:author", author)
    _og-meta("book:isbn", isbn)
    _og-meta("book:release_date", release-date)
    for tag in tags { _og-meta("book:tag", tag) }
  } else if type == "profile" {
    _og-meta("profile:first_name", first-name)
    _og-meta("profile:last_name", last-name)
    _og-meta("profile:username", username)
    _og-meta("profile:gender", gender)
  }
  // "website" has no additional properties

  // Twitter Cards
  _og-meta("twitter:card", twitter-card)
  _og-meta("twitter:title", title)
  _og-meta("twitter:description", description)
  _og-meta("twitter:image", image)
  _og-meta("twitter:site", twitter-site)
  _og-meta("twitter:creator", twitter-creator)
}
