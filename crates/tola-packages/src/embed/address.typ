// @tola/address:0.0.0 - Generate slugs and convert between routes, output paths, and browser URLs.
//
// Use `output-of` to get the output path for `#document`, and `output-url` to make a link.

/// Turn a title or other text into a slug for a URL or filename.
///
/// A slug keeps letters, marks, numbers, and the punctuation that is safe in URLs and filenames:
/// `slugify("Deep Notes")` gives `"deep-notes"`. Every other character separates words, and the
/// result is NFC normalized.
///
/// Text with no usable characters raises an error. In `mode: "ascii"`, a language tag with
/// primary subtag `ja` reads Han characters as Japanese. Other tags read them as Chinese.
///
/// Han characters with several readings follow the word they occur in.
///
/// Example: Make a slug from a title:
///
/// ```typst
/// #import "@tola/address:0.0.0": slugify
/// #assert.eq(slugify("Deep Notes"), "deep-notes")
/// #assert.eq(slugify("Deep Notes", case: "upper", separator: "_"), "DEEP_NOTES")
/// ```
///
/// Example: Transcribe Han text and accents to ASCII:
///
/// ```typst
/// #import "@tola/address:0.0.0": slugify
/// #assert.eq(slugify("北京 Café", mode: "ascii"), "bei-jing-cafe")
/// #assert.eq(slugify("重庆", mode: "ascii"), "chong-qing")
/// ```
/// Related: join-route
#import "@tola/host:0.0.0": slugify

/// Join path segments into a route such as `/posts/deep-notes/`.
///
/// Segments keep their original text. Call `slugify` first to change case, replace spaces, or
/// transliterate. A source's `route-segments` suggest a route. The path you pass to `document`
/// determines where the page is published.
///
/// The result starts and ends with `/`. An empty array gives `/`. Each segment must be a valid
/// filename. Empty segments, `.`, `..`, slashes, and backslashes raise an error.
///
/// Example: Use a title as the route for an article:
///
/// ```typ
/// #import "@tola/address:0.0.0": join-route, output-of, slugify
/// #let route = join-route(("posts", "Deep Notes").map(slugify))
/// #assert.eq(output-of(route), "posts/deep-notes/index.html")
/// #document(output-of(route), title: [Deep Notes])[Body]
/// ```
///
/// Example: Read a source’s default route from its file layout:
///
/// ```typst site
/// #import "@tola/address:0.0.0": join-route
/// #import "@tola/source:0.0.0": all-sources
/// #let routes = all-sources().map(source => join-route(source.route-segments))
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
/// Related: slugify, output-of, @tola/source
#import "@tola/host:0.0.0": join-route

/// Convert a route such as `/guide/` into a file path such as `guide/index.html`.
///
/// A route is a logical address within the site, without `base-path` or `origin`; `%` and `#`
/// are literal filename characters. A trailing `/` names a directory, published as its
/// `index.html`, so `/` is `index.html` and `/guide/` is `guide/index.html`; a route without a
/// trailing `/` names a file exactly as spelled, so `/guide` is `guide` and `/manual.pdf` is
/// `manual.pdf`.
///
/// If you have a percent-encoded URL path, call `decode-url-path` once before passing it here.
///
/// Example: Name the output file for a route:
///
/// ```typst
/// #import "@tola/address:0.0.0": output-of
/// #assert.eq(output-of("/"), "index.html")
/// #assert.eq(output-of("/guide/"), "guide/index.html")
/// #assert.eq(output-of("/manual.pdf"), "manual.pdf")
/// ```
///
/// Example: Keep a literal `%` or `#` as a filename character:
///
/// ```typst
/// #import "@tola/address:0.0.0": output-of
/// #assert.eq(output-of("/100%/"), "100%/index.html")
/// #assert.eq(output-of("/a#b.html"), "a#b.html")
/// ```
/// Related: join-route, decode-url-path
#import "@tola/host:0.0.0": output-of

/// Get the route for an output file. For `guide/index.html`, this returns `/guide/`.
///
/// An exact `index.html` filename becomes the directory address that contains it, so `index.html`
/// is `/` and `guide/index.html` is `/guide/`. Every other filename keeps its path under `/`, so
/// `404.html` is `/404.html` and `about` is `/about`.
///
/// The result has no deployment prefix, origin, or percent-encoding.
/// `output-of(route-of(output))` recovers the output; converting `/guide/index.html`
/// to an output and back gives its canonical route `/guide/`.
///
/// Example: Map output files back to routes:
///
/// ```typst
/// #import "@tola/address:0.0.0": route-of
/// #assert.eq(route-of("index.html"), "/")
/// #assert.eq(route-of("guide/index.html"), "/guide/")
/// #assert.eq(route-of("404.html"), "/404.html")
/// ```
/// Related: output-of
#import "@tola/host:0.0.0": route-of

/// Reads site configuration (`[site]` `base-path`): the browser URL an output file is served from.
///
/// Pass a decoded output path under `build.publish-dir`, with no leading `/` or deployment
/// prefix, such as `guide/index.html`: the document's own output or the result of `output-of`.
/// `base-path` is the deployment path below the host root and defaults to this site's
/// `site.base-path`, so the result is host-relative and mounted for the site being built.
/// Pass `base-path: "/"` for a root deployment, or another path to override the site's own. Pass
/// `origin` to get an absolute URL; `origin: site.origin` uses the site's own origin.
///
/// Exact `index.html` filenames use their directory URL: `guide/index.html` becomes `/guide/`.
/// `asset-url` instead keeps a declaration's exact filename, including `index.html`.
///
/// Constraints: `base-path` must start and end with `/`, and any escapes in it are decoded once;
/// `origin` is an absolute `http`/`https` origin and cannot include a deployment path, credentials,
/// query, or fragment; output names are encoded once, so a literal `%20` becomes `%2520`.
///
/// Use `output-url` for a file the site will write, including files generated by hooks.
/// The function returns a URL without checking whether the file exists. For a URL declared
/// in `[assets]`, use `asset-url`. With cache-busting enabled, it appends a content hash
/// as `?h=…`, so the URL changes when the file changes.
///
/// Keep the document output unprefixed, and use the returned URL directly in a link. With
/// `site.base-path = "/docs/"`, this page links to `/docs/guide/` while the output stays
/// `guide/index.html`. Final reference checks verify that the linked output exists.
///
/// Example: Apply the deployment prefix to a link:
///
/// Save this as `site.typ` in a new site and run `tola build`. The home page links to
/// `/guide/`, or to `/docs/guide/` if `site.base-path` is `/docs/`.
///
/// ```typst
/// #import "@tola/address:0.0.0": output-url
/// #let guide-output = "guide/index.html"
/// #document("index.html", title: [Home])[
///   #link(output-url(guide-output))[Read the guide]
/// ]
/// #document(guide-output, title: [Guide])[
///   #title()
///   Start here.
/// ]
/// ```
///
/// Example: Mount an output under a base path and an origin:
///
/// ```typst
/// #import "@tola/address:0.0.0": output-url
/// #assert.eq(output-url("guide/index.html"), "/guide/")
/// #assert.eq(output-url("Hello World/index.html", base-path: "/docs/"), "/docs/Hello%20World/")
/// #assert.eq(
///   output-url("guide/index.html", base-path: "/docs/", origin: "https://example.test"),
///   "https://example.test/docs/guide/",
/// )
/// ```
///
/// Example: Encode a literal `%20` in an output name exactly once:
///
/// ```typst
/// #import "@tola/address:0.0.0": output-url
/// #assert.eq(output-url("100%20.html"), "/100%2520.html")
/// ```
///
/// Example: Link a file generated by a hook:
///
/// ```typst
/// #import "@tola/address:0.0.0": output-url
/// #assert.eq(output-url("assets/pagefind-search/pagefind.js"), "/assets/pagefind-search/pagefind.js")
/// ```
/// Related: output-of, route-url, asset-url, @tola/site
/// - output (string): the decoded file path under `build.publish-dir`, without a leading `/` or deployment prefix.
/// - base-path (auto | string): Path the site is deployed under, such as `/docs/`. If omitted or `auto`, use `site.base-path`.
/// - origin (none | string): Pass an origin such as `https://example.test` to get a full URL. If omitted or `none`, return a path such as `/docs/guide/`.
/// -> string
#let output-url(output, base-path: auto, origin: none) = {
  import "@tola/host:0.0.0": output-url as resolve

  if base-path == auto {
    import "@tola/site:0.0.0": site
    resolve(output, base-path: site.base-path, origin: origin)
  } else {
    resolve(output, base-path: base-path, origin: origin)
  }
}

/// Make a browser URL from a decoded route.
///
/// This has the same result as calling `output-of` and then `output-url`. Both `/guide/` and
/// `/guide/index.html` link to the directory index. `/manual.pdf` keeps its filename.
///
/// Pass decoded text without the deployment prefix: Unicode, literal `%` and `#` are filename
/// characters. Decode an encoded URL path with `decode-url-path` exactly once before passing it.
/// The deployment prefix is added once; `base-path: auto` uses `site.base-path`, and `origin`
/// follows `output-url`.
///
/// Example: Make a link from a route under `/docs/`:
///
/// ```typst
/// #import "@tola/address:0.0.0": route-url, decode-url-path
/// #assert.eq(route-url("/guide/index.html", base-path: "/"), "/guide/")
/// #assert.eq(route-url("/manual.pdf", base-path: "/"), "/manual.pdf")
/// #assert.eq(route-url("/中文/100%/", base-path: "/docs/"), "/docs/%E4%B8%AD%E6%96%87/100%25/")
/// #assert.eq(
///   route-url(decode-url-path("/caf%C3%A9/"), base-path: "/docs/", origin: "https://example.test"),
///   "https://example.test/docs/caf%C3%A9/",
/// )
/// ```
///
/// Related: output-url, output-of, decode-url-path
/// - route (string): the decoded site-root route to convert.
/// - base-path (auto | string): Path the site is deployed under, such as `/docs/`. If omitted or `auto`, use `site.base-path`.
/// - origin (none | string): Pass an origin such as `https://example.test` to get a full URL. If omitted or `none`, return a path such as `/docs/guide/`.
/// -> string
#let route-url(route, base-path: auto, origin: none) = output-url(
  output-of(route),
  base-path: base-path,
  origin: origin,
)

/// Return the browser URL for a file declared in `[assets]`.
///
/// Pass the URL path from `[assets]`, without the deployment prefix.
///
/// - `assets.files`: the entry's own `url`, such as `"/styles/base.css"`.
/// - `assets.trees`: the tree's `url-prefix` followed by the member's path inside the tree.
///   `url-prefix = "/assets"` with `images/logo.svg` in that tree gives
///   `asset-url("/assets/images/logo.svg")`.
///
/// With `follow-file-symlinks = true`, the URL uses the link's name and serves the
/// target file's contents. For example, if `static/downloads/guide.txt` points to
/// `shared/notes.txt`, set the tree's `source` to `"static/downloads"` and
/// `url-prefix` to `"/downloads"`. With the default `site.base-path`, readers
/// open `/downloads/guide.txt` to read the contents of `notes.txt`. In Typst,
/// use `asset-url("/downloads/guide.txt")`. See `tola help "[assets]"` for the
/// full setup.
///
/// The argument follows URL path syntax and is percent-decoded once. Unicode and interior spaces
/// may appear directly or encoded; a literal `%` or `#` in a filename must be written `%25` or
/// `%23`. Queries and fragments are not accepted. Do not pass the decoded result of another call.
/// A build resolves the declaration's exact filename, including `index.html`, and appends
/// `?h=<hash>` when `assets.cache-busting` is on, so changed bytes become a changed address
/// under an unchanged name.
///
/// The returned URL already includes `site.base-path`. A path missing from `[assets]` raises an
/// error. For an image created by `resize-image`, use the `url` it returns.
///
/// How the two differ:
///
/// | | `output-url(output)` | `asset-url(declared URL)` |
/// | --- | --- | --- |
/// | Input | a decoded output file path under `build.publish-dir`, no leading `/` (`assets/logo.png`) | the site-root URL a declaration spells, leading `/` included (`/assets/logo.png`); URL syntax with literal `%` and `#` escaped |
/// | Publication | not checked — a hook's future output resolves too | the `[assets]` declaration must publish the URL, even when something else writes that path |
/// | Result | the mounted canonical URL; `assets/index.html` uses `/assets/` | the mounted exact file URL; `/assets/index.html` stays `/assets/index.html`, plus `?h=<hash>` when `assets.cache-busting` is on |
///
/// Editor checks return the URL without `?h=…`. Run `tola check` to check the final asset URLs
/// without publishing the site.
///
/// Use `output-url("assets/…")` to link to a file generated by a hook.
///
/// Example: Link a declared asset under `/assets`:
///
/// ```typst site
/// #import "@tola/address:0.0.0": asset-url
/// #document("index.html")[
///   #html.img(src: asset-url("/assets/logo.png"), alt: "Example logo")
/// ]
/// ```
/// Related: output-url, @tola/site, @tola/image
#import "@tola/host:0.0.0": asset-url

/// Decode a percent-encoded URL path once so it can be used as a route.
///
/// For example, `/caf%C3%A9/` becomes `/café/` and `/Hello%20World/` becomes `/Hello World/`.
///
/// Pass only a path. An origin, query string, fragment, or encoded slash or backslash raises an
/// error. Each decoded segment must be usable as a filename on supported platforms.
///
/// For metadata, `@tola/schema`'s `url-path` applies these rules while collecting field errors.
/// Its parsed value is already decoded and can be passed directly to `output-of` or `route-url`.
///
/// Example: Decode a percent-encoded path exactly once:
///
/// ```typst
/// #import "@tola/address:0.0.0": decode-url-path
/// #assert.eq(decode-url-path("/caf%C3%A9/"), "/café/")
/// #assert.eq(decode-url-path("/Hello%20World/"), "/Hello World/")
/// #assert.eq(decode-url-path("/plain/"), "/plain/")
/// ```
///
/// Example: Decode a path once, then name the output file:
///
/// ```typst
/// #import "@tola/address:0.0.0": decode-url-path, output-of
/// #assert.eq(decode-url-path("/100%25/"), "/100%/")
/// #assert.eq(output-of(decode-url-path("/100%25/")), "100%/index.html")
/// ```
/// Related: output-of, @tola/schema
#import "@tola/host:0.0.0": decode-url-path
