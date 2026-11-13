// @tola/icon:0.0.0 - configured icons as inline SVG, SVG bytes, or a published URL

/// Draw a configured icon as an inline SVG, one em tall.
///
/// It produces HTML elements, so call it from an HTML document. `icon-bytes` and `icon-url`
/// return values any target can use.
///
/// For the full Iconify setup, see [`[icons]`](tola-help://config/icons). Once configured,
/// call `icon("lucide:rocket")` in an HTML document.
///
/// An icon's color comes from its own SVG content. Artwork drawn with `currentColor` follows the
/// `color` of the page around it, so a theme or a CSS class recolors it; artwork with fixed fills
/// and strokes keeps its own colors. Prefer icons drawn that way: every `lucide` icon is painted
/// with `currentColor`, so it takes on the page's text color as it is.
/// The id is checked when `icon()` is called, including when its returned content is unused.
///
/// Example: Insert decorative and labeled icons:
///
/// Configure `brand:mark` as shown in `tola help "[icons]"`. Put this in the entry file of a
/// new site and run `tola build`. The home page contains an inline SVG.
///
/// ```typst site
/// #import "@tola/icon:0.0.0": icon
/// #assert.eq(type(icon("brand:mark")), content)
/// #document("index.html")[
///   #icon("brand:mark")
///   #icon("brand:mark", label: "Example site logo", attrs: (class: "brand",))
/// ]
/// ```
///
/// Example: Use an inline SVG and an image URL:
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
/// Example: Publish an icon's bytes at a chosen path:
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
/// - label (none | string): Omit this option or pass `none` for a decorative icon. For an informative icon,
///   pass a label such as `"Site logo"` for screen readers. Blank labels are errors.
/// - attrs (dictionary): Extra attributes on the root SVG, with string values, such as `class`, `width`,
///   `height`, and `style`. `label` owns `aria-label`, `aria-labelledby`, `aria-hidden`,
///   `role`, and `focusable`, and naming any of them in `attrs` is an error. `class` and
///   `style` append to existing values. If omitted, no extra attributes are added.
/// -> content
#let icon(id, label: none, attrs: (:)) = {
  import "@tola/host:0.0.0": icon as render, icon-bytes
  let svg = icon-bytes(id)
  context {
    // The native reads the element's location, which only a call in context supplies.
    if target() != "html" {
      panic("icon() needs an HTML document")
    }
    render(svg, label: label, attrs: attrs)
  }
}

/// The normalized SVG bytes of a configured icon, ready for `asset()` or Typst's `image(bytes)`.
///
/// To publish these bytes at a path you choose, pass them to `asset()`. Use `icon-url()`
/// to publish the SVG and get a browser URL in one call.
///
///
/// Configure `brand:mark` as shown in `tola help "[icons]"`. Add this to `site.typ` and
/// run `tola build`. With the default publish directory, it writes `public/mark.svg`.
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
///
/// Configure `brand:mark` as shown in `tola help "[icons]"`. Add this to `site.typ` and
/// run `tola build`. The home page loads the published SVG through an image element.
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
