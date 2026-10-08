// @tola/icon:0.0.0 - configured icons as inline SVG, SVG bytes, or a published URL

/// Draw a configured icon as an inline SVG, one em tall.
///
/// It produces HTML elements, so call it from an HTML document. `icon-bytes` and `icon-url`
/// return values any target can use.
///
/// Ids come from the collections the site declares in `tola.toml` under `icons.collections`:
/// `tola help config icons` documents each source type, and `tola inspect icons` lists the configured
/// namespaces and, in one you name, its icon names.
///
/// An icon is colored by its own SVG. Artwork drawn with `currentColor` follows the `color` of the
/// page around it, so a theme or a CSS class recolors it; artwork with fixed fills and strokes keeps
/// its own colors. A collection can already be drawn that way: every `lucide` icon is painted with
/// `currentColor`, so it takes on the page's text color as it is.
///
/// Example - render a configured icon in an HTML document:
///
/// ```typst site
/// #import "@tola/icon:0.0.0": icon
/// #assert.eq(type(icon("brand:mark")), content)
/// #document("index.html")[
///   #icon("brand:mark")
///   #icon("brand:logo", label: "Example site logo", attrs: (class: "brand",))
/// ]
/// ```
///
/// Example - insert an icon and load a published icon URL into one page:
///
/// ```typst site
/// #import "@tola/icon:0.0.0": icon, icon-url
/// #import "@tola/site:0.0.0": site
/// #assert(icon-url("brand:mark").starts-with(site.base-path))
/// #document("index.html")[
///   #icon("brand:mark")
///   #html.img(src: icon-url("brand:mark"), alt: "Example mark")
/// ]
/// ```
///
/// Example - publish an icon's collected bytes with `asset()`:
///
/// ```typst site
/// #import "@tola/icon:0.0.0": icon-bytes
/// #let mark = icon-bytes("brand:mark")
/// #assert.eq(type(mark), bytes)
/// #asset("mark.svg", mark)
/// ```
///
/// Related: icon-bytes, icon-url, @tola/site
///
/// - id (string): `"collection:name"`, from the collections you declared under
///   `icons.collections`. Names are case-sensitive.
/// - label (none | string): `none` marks the icon as decoration; a nonempty label marks an
///   informative image. A label that holds only whitespace is an error.
/// - attrs (dictionary): root SVG attributes with string values, such as `class`, `width`,
///   `height`, and `style`. `label` owns `aria-label`, `aria-labelledby`, `aria-hidden`,
///   `role`, and `focusable`, and naming any of them in `attrs` is an error. `class` and
///   `style` extend the values the SVG already carries.
/// -> content
#let icon(id, label: none, attrs: (:)) = context {
  import "@tola/host:0.0.0": icon as render
  // The native reads the element's location, which only a call in context supplies.
  if target() != "html" {
    panic("icon() needs an HTML document")
  }
  render(id, label: label, attrs: attrs)
}

/// The normalized SVG bytes of a configured icon, ready for `asset()` or Typst's `image(bytes)`.
///
/// Only `icon-url()` publishes an icon as an SVG file.
///
/// Example - publish an icon's bytes as a Bundle file:
///
/// ```typst site
/// #import "@tola/icon:0.0.0": icon-bytes
/// #let mark = icon-bytes("brand:mark")
/// #assert.eq(type(mark), bytes)
/// #assert(mark.len() > 0)
/// #asset("mark.svg", mark)
/// ```
///
/// Related: icon, icon-url
#import "@tola/host:0.0.0": icon-bytes

/// Publish a configured icon and return the URL it is served from.
///
/// A build publishes exactly the icons its documents ask for. The URL starts with `/`, includes
/// `site.base-path`, and is ready for an HTML image or link attribute. Icons with identical
/// normalized SVG bytes share one published file.
///
/// The page's `color` does not reach a published icon: a file of its own cannot inherit it. Use
/// `icon()` for an icon that should follow the theme.
///
/// Example - publish an icon and use its URL in an image:
///
/// ```typst site
/// #import "@tola/icon:0.0.0": icon-url
/// #import "@tola/site:0.0.0": site
/// #let url = icon-url("brand:mark")
/// #assert(url.starts-with(site.base-path))
/// #assert(url.ends-with(".svg"))
/// #document("index.html")[#html.img(src: url, alt: "Example mark")]
/// ```
///
/// Related: icon, icon-bytes, @tola/site
#import "@tola/host:0.0.0": icon-url
