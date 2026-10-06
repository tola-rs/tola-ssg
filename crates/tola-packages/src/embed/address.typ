// @tola/address:0.0.0 - slugs, routes, output paths, and browser URLs

/// Turn text into a URL-safe name: a slug.
///
/// A slug keeps letters, marks, numbers, and the punctuation that is safe in URLs and
/// filenames. Every other character separates words, and the result is NFC normalized. A slug
/// names one path segment, so pass it straight to `route`; percent-encoding happens only when a
/// browser URL is built. Text with nothing to name raises an error.
///
/// Example - slug a title with the defaults, then with options:
///
/// ```typst
/// #import "@tola/address:0.0.0": slugify
/// #assert.eq(slugify("Deep Notes"), "deep-notes")
/// #assert.eq(slugify("Deep Notes", case: "upper", separator: "_"), "DEEP_NOTES")
/// ```
///
/// In `ascii` mode the readings follow `language`: only a primary `ja` subtag reads Han in
/// Japanese; every other tag, `en` included, reads it in Chinese.
///
/// Example - transcribe Han text and accents to ASCII; a Han word is read as a word, so a
/// character with several readings follows the word it is in:
///
/// ```typst
/// #import "@tola/address:0.0.0": slugify
/// #assert.eq(slugify("北京 Café", mode: "ascii"), "bei-jing-cafe")
/// #assert.eq(slugify("重庆", mode: "ascii"), "chong-qing")
/// ```
/// Related: route
#import "@tola/host:0.0.0": slugify

/// Join slugs into a site-root route.
///
/// Each segment is kept exactly as spelled, so slug it first when you want slugs, other casing,
/// or transcription. The result always ends with `/`; pass no segments for the site root `/`. A
/// segment that is empty, `.`, `..`, or holds a separator raises an error.
///
/// A source's `route-segments` are a default: the file layout suggests them, and the document's
/// own output decides where it is reachable. To publish one, slug each segment, join them here,
/// and name the output file:
///
/// ```typ
/// #import "@tola/address:0.0.0": route, route-to-output, slugify
/// #let route = route(("posts", "Deep Notes").map(slugify))
/// #assert.eq(route-to-output(route), "posts/deep-notes/index.html")
/// #document(route-to-output(route), title: [Deep Notes])[Body]
/// ```
///
/// Example - read the default route the file layout gives every source:
///
/// ```typst site
/// #import "@tola/address:0.0.0": route
/// #import "@tola/source:0.0.0": all-sources
/// #let routes = all-sources().map(source => route(source.route-segments))
/// #assert.eq(routes, (
///   "/",
///   "/guide/",
///   "/guide/install/",
///   "/guide/deploy/",
///   "/guide/advanced/plugins/",
///   "/notes/first-post/",
///   "/notes/second-post/",
/// ))
/// ```
/// Related: slugify, route-to-output, @tola/source
#import "@tola/host:0.0.0": route

/// The output file a site-root route publishes to.
///
/// A route is the path your site publishes under, and `%` or `#` in it are ordinary filename
/// characters. A trailing `/` names a directory, published as that directory's `index.html`, so
/// `/` is `index.html` and `/guide/` is `guide/index.html`. A route without a trailing `/` names
/// a file exactly as spelled, so `/guide` is `guide` and `/manual.pdf` is `manual.pdf`. Decode a
/// percent-encoded URL path with `decode-url-path` first, exactly once.
///
/// Example - name the output file for a route:
///
/// ```typst
/// #import "@tola/address:0.0.0": route-to-output
/// #assert.eq(route-to-output("/"), "index.html")
/// #assert.eq(route-to-output("/guide/"), "guide/index.html")
/// #assert.eq(route-to-output("/manual.pdf"), "manual.pdf")
/// ```
///
/// Example - keep a literal `%` or `#` as a filename character:
///
/// ```typst
/// #import "@tola/address:0.0.0": route-to-output
/// #assert.eq(route-to-output("/100%/"), "100%/index.html")
/// #assert.eq(route-to-output("/a#b.html"), "a#b.html")
/// ```
/// Related: route, decode-url-path
#import "@tola/host:0.0.0": route-to-output

/// The site-root route an output file is reachable at.
///
/// An exact `index.html` filename becomes the directory address that contains it, so
/// `index.html` is `/` and `guide/index.html` is `/guide/`. Every other filename keeps its path
/// under `/`, so `404.html` is `/404.html` and `about` is `/about`.
///
/// `route-to-output(output-to-route(output))` recovers the output; converting `/guide/index.html`
/// to an output and back gives its canonical route `/guide/`.
///
/// Example - map output files back to routes:
///
/// ```typst
/// #import "@tola/address:0.0.0": output-to-route
/// #assert.eq(output-to-route("index.html"), "/")
/// #assert.eq(output-to-route("guide/index.html"), "/guide/")
/// #assert.eq(output-to-route("404.html"), "/404.html")
/// ```
/// Related: route-to-output
#import "@tola/host:0.0.0": output-to-route

/// The browser URL an output file is served from.
///
/// Pass an output, such as `guide/index.html`, either the document's own output or the result of
/// `route-to-output`. `base-path` is the deployment path below the host root, and defaults to
/// this site's `site.base-path`, so the result is host-relative and mounted for the site being
/// built. Pass `"/"` for a root deployment, or another path to override the site's own; it must
/// start and end with `/`, and any escapes in it are decoded once. `origin` names an absolute
/// `http`/`https` origin; pass it to get an absolute URL. It cannot include a deployment path,
/// credentials, query, or fragment. Output names are encoded once, so a literal `%20` becomes
/// `%2520`. Pass `origin: site.origin` when the result must be absolute.
///
/// Any output path resolves here, including one a hook produces: nothing is checked, because the
/// address is a name, not a promise that the file exists. A URL a `[assets]` declaration spells is
/// not an output path — `asset-url` resolves those, and adds their byte identity.
///
/// Example - mount an output under a base path and an origin:
///
/// ```typst
/// #import "@tola/address:0.0.0": output-to-url
/// #assert.eq(output-to-url("guide/index.html"), "/guide/")
/// #assert.eq(output-to-url("Hello World/index.html", base-path: "/docs/"), "/docs/Hello%20World/")
/// #assert.eq(
///   output-to-url("guide/index.html", base-path: "/docs/", origin: "https://example.test"),
///   "https://example.test/docs/guide/",
/// )
/// ```
///
/// Example - encode a literal `%20` in an output name exactly once:
///
/// ```typst
/// #import "@tola/address:0.0.0": output-to-url
/// #assert.eq(output-to-url("100%20.html"), "/100%2520.html")
/// ```
///
/// Example - link a hook's own output, which no `assets` declaration names:
///
/// ```typst
/// #import "@tola/address:0.0.0": output-to-url
/// #assert.eq(output-to-url("assets/pagefind-search/pagefind.js"), "/assets/pagefind-search/pagefind.js")
/// ```
/// Related: route-to-output, asset-url, @tola/site
/// - output (string): the output path to mount.
/// - base-path (auto | string): the deployment path; `auto` takes the site's own `base-path`.
/// - origin (none | string): the absolute origin the result is written under.
/// -> string
#let output-to-url(output, base-path: auto, origin: none) = {
  import "@tola/host:0.0.0": output-to-url as resolve

  if base-path == auto {
    import "@tola/site:0.0.0": site
    resolve(output, base-path: site.base-path, origin: origin)
  } else {
    resolve(output, base-path: base-path, origin: origin)
  }
}

/// The browser URL a declared asset URL is served from.
///
/// The argument is the site-root URL the declaration itself spells:
///
/// - `assets.files` - the entry's own `url`, such as `"/styles/base.css"`.
/// - `assets.trees` - the tree's `url-prefix` followed by the member's path inside the tree:
///   `url-prefix = "/assets"` with `images/logo.svg` in that tree gives
///   `asset-url("/assets/images/logo.svg")`.
///
/// Write the URL either encoded or decoded: `asset-url` parses it once as a path, so the lookup
/// matches the declaration either way. A build resolves it to the address that declaration
/// publishes, and appends `?h=<identity>` when `assets.cache-busting` is on, so changed bytes
/// become a changed address under an unchanged name. A check resolves the mounted address
/// without the identity, because only a build renders asset bytes. The returned URL starts with
/// `/` and includes `site.base-path`, so use it directly from any document: mounting it again
/// duplicates it. Calling it for a URL that no `assets` declaration publishes is an error at the
/// call site, even when something else publishes that path, such as a `resize-image` derivative.
/// A hook's own output is such a path: pages link it with `output-to-url("assets/…")`, and the two
/// calls stay apart — pass a declaration's spelling here, never another function's result.
///
/// Example - resolve a file the site publishes under `/assets`:
///
/// ```typst site
/// #import "@tola/address:0.0.0": asset-url
/// #import "@tola/site:0.0.0": site
/// #let logo = asset-url("/assets/logo.png")
/// #assert(logo.starts-with("/"))
/// #assert(logo.contains(site.base-path))
/// ```
/// Related: output-to-url, @tola/site, @tola/image
#import "@tola/host:0.0.0": asset-url

/// Decode a percent-encoded site-root URL path exactly once.
///
/// Pass a path and nothing else: an origin, query string, or fragment raises an error, and so
/// does an escaped `/` or `\`, because only an unescaped `/` starts a hierarchy level. Every
/// other escape becomes the byte it names, so `/caf%C3%A9/` is `/café/` and `/Hello%20World/` is
/// `/Hello World/`. Decode here, once; `route-to-output` takes the decoded text directly.
///
/// Example - decode a percent-encoded path exactly once:
///
/// ```typst
/// #import "@tola/address:0.0.0": decode-url-path
/// #assert.eq(decode-url-path("/caf%C3%A9/"), "/café/")
/// #assert.eq(decode-url-path("/Hello%20World/"), "/Hello World/")
/// #assert.eq(decode-url-path("/plain/"), "/plain/")
/// ```
///
/// Example - decode a path once, then name the output file:
///
/// ```typst
/// #import "@tola/address:0.0.0": decode-url-path, route-to-output
/// #assert.eq(decode-url-path("/100%25/"), "/100%/")
/// #assert.eq(route-to-output(decode-url-path("/100%25/")), "100%/index.html")
/// ```
/// Related: route-to-output
#import "@tola/host:0.0.0": decode-url-path
