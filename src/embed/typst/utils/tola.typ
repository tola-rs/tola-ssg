// Tola SSG utility functions
//
// Helper functions for common operations in Typst

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
