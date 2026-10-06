#import "@tola/address:0.0.0": output-to-url

/// The stylesheet for Pagefind's search UI.
/// -> array
#let search-head() = (
  html.link(
    rel: "stylesheet",
    href: output-to-url("assets/pagefind-search/pagefind-ui.css"),
  ),
)

/// A search box whose resources and result links honor the site's deployment path.
/// -> content
#let search-box() = {
  let options = json.encode((
    element: ".pagefind-search",
    bundlePath: output-to-url("assets/pagefind-search/index.html"),
    baseUrl: output-to-url("index.html"),
  ), pretty: false)
  [
    #html.elem("div", attrs: (class: "pagefind-search"))
    #html.elem("script", attrs: (src: output-to-url("assets/pagefind-search/pagefind-ui.js")))
    #html.elem("script")[#("new PagefindUI(" + options + ");")]
  ]
}
