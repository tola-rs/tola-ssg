#import "@tola/schema:0.0.0": map, nullable, trim

/// Treat an empty string as absent.
///
/// An empty attribute would claim a value that is not there, so the web helpers write no entry
/// for it.
#let empty-to-none(value) = if value == "" { none } else { value }

/// A schema for one optional text option: the string is trimmed, and an empty one counts as absent.
#let optional-text = map(
  nullable(trim(str)),
  empty-to-none,
  output: nullable(str),
)

/// The HTML attributes a math wrapper carries: `role` is `math`, `class` names the wrapper, and
/// `aria-label` holds the `alt` you gave, when one is set.
///
/// Every `attrs` value must be a string, and `attrs` may not name `role` or `aria-label`. A
/// `class` you pass keeps its value and gains the wrapper's own token.
#let math-attributes(equation, alt, attrs, wrapper-class, operation) = {
  assert(
    alt == auto or alt == none or type(alt) == str,
    message: operation + " `alt` must be `auto`, `none`, or a string",
  )
  assert(type(attrs) == dictionary, message: operation + " `attrs` must be a dictionary")
  for (name, value) in attrs {
    let attribute-name = lower(name)
    assert(
      attribute-name not in ("role", "aria-label"),
      message: operation + " `" + name + "` belongs to the math wrapper"
        + if attribute-name == "aria-label" { "; set the alternative with `alt`" } else { " and cannot be overridden" },
    )
    assert(type(value) == str, message: operation + " `attrs` values must be strings")
  }
  let attributes = attrs + (role: "math")
  let class-key = attrs.keys().find(name => lower(name) == "class")
  if class-key == none {
    attributes.insert("class", wrapper-class)
  } else {
    attributes.insert(class-key, wrapper-class + " " + attributes.at(class-key))
  }
  let alternative = if alt == auto { equation.at("alt", default: math.equation.alt) } else { alt }
  if alternative != none { attributes.insert("aria-label", alternative) }
  attributes
}
