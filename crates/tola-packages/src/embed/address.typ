// @tola/address:0.0.0 - slugs, routes, output paths, and browser URLs
//
// Tola does no implicit path joining: paths are the site author's work, and this package only
// offers composable address APIs.
//
// Three concepts follow. A route is purely logical — the site author's to build and compose with —
// and it converts to an output, the path that is actually written.
//
// - A route (`/guide/`): the path a document is reachable at; it starts and ends with `/`, and a
//   trailing `/` names a directory (`/guide/`), while no trailing `/` names a file (`/about`)
// - An output: a file path relative to the output root (`build.publish-dir`), with no leading `/`;
//   `#document("guide/index.html")` writes its export to that file (`guide/index.html`)
// - A browser URL: the address an output is served from; `url-of` and `url-of-asset` add the
//   `base-path` configured under `[site]` for you (`/docs/guide/` when `base-path = "/docs/"`);
//   `url-of-asset` resolves a URL declared under `[assets]`, checking that the declaration exists
//   and appending the `?h=<asset hash>` cache-busting parameter when cache-busting is on. A URL
//   written by hand in a page is the only case where you carry `/docs/` yourself.
//
// A page written as `#document("guide/index.html")` is reachable at `/guide/`; link between pages
// with `url-of`, so the links follow a changed `[site]` `base-path`; point at assets declared
// under `[assets]` with `url-of-asset`. Build an output name from a title with `slugify`
// (`slugify("Deep Notes")` → `"deep-notes"`).
//
// The flow: everything Tola can work out by itself — a `#document(…)` export, a file `asset()`
// writes, a file a `generate-outputs` hook writes through `TOLA_HOOK_OUTPUT_DIR` — lands under
// `build.publish-dir` with no base path in it. The base path is added only when a browser URL is
// built: `url-of` and `url-of-asset` include it automatically, while a URL you write by hand in
// a page (`#link("/guide/")`) must carry it yourself.
//
// ```typ site
// #import "@tola/address:0.0.0": url-of, url-of-asset
// #document("index.html")[
//   #link(url-of("guide/index.html"))[Guide]                      // /docs/guide/ when base-path = "/docs/"
//   #html.img(src: url-of-asset("/assets/logo.png"), alt: "Logo") // /docs/assets/logo.png?h=<hash>
// ]
// ```

// The base path is where the published tree is served, not a folder in it: the tree never contains
// `/docs/`. You name the mount in `[site]` `base-path`; whatever serves the site (GitHub Pages
// serving a project under `/myrepo/`, an nginx `location /docs/`, `tola dev`) strips that prefix
// before looking up the file. `#document("guide/index.html")` writes
// `build.publish-dir/guide/index.html`; `url-of("guide/index.html")` computes `/docs/guide/`.
//
// Do not put the prefix in an output, a declaration, or a hand-written URL, and use the result of
// `url-of` or `url-of-asset` as returned: concatenating the prefix again (`"/docs" + url-of(…)`)
// gives `/docs/docs/…`.
//
// Tola does not rewrite generated strings: `#link("/guide/")` reaches the output as written, and
// the final reference check reports a link that matches no output. Compute links with the
// functions.
//
// A file a hook produces — a search index, a bundle — is an ordinary output: pages reference it
// with `url-of("assets/…")`, and the hook itself carries no base path. When a generated file must
// contain URLs (a sitemap, a JSON with links), write it as a `#document(…)`, where `url-of` is
// available; the scaffold's search instead passes `baseUrl: url-of("index.html")` to its script.

/// Pure function: turn text into a slug, the string you can put in a URL or file name unchanged.
///
/// A slug keeps letters, marks, numbers, and the punctuation that is safe in URLs and filenames:
/// `slugify("Deep Notes")` gives `"deep-notes"`. Every other character separates words, and the
/// result is NFC normalized.
///
/// Constraints: text with nothing left to slug is an error; `mode`, `case`, and `separator` accept
/// only their listed values; under `mode: "ascii"` the readings follow `language`, so only a
/// primary `ja` subtag reads Han as Japanese — every other tag, `en` included, reads it in Chinese.
///
/// Example - slug a title with the defaults, then with options:
///
/// ```typst
/// #import "@tola/address:0.0.0": slugify
/// #assert.eq(slugify("Deep Notes"), "deep-notes")
/// #assert.eq(slugify("Deep Notes", case: "upper", separator: "_"), "DEEP_NOTES")
/// ```
///
/// Example - transcribe Han text and accents to ASCII; a Han word is read as a word, so a character
/// with several readings follows the word it is in:
///
/// ```typst
/// #import "@tola/address:0.0.0": slugify
/// #assert.eq(slugify("北京 Café", mode: "ascii"), "bei-jing-cafe")
/// #assert.eq(slugify("重庆", mode: "ascii"), "chong-qing")
/// ```
/// Related: join-route
#import "@tola/host:0.0.0": slugify

/// Pure function: join a `segments` array into a site-root route.
///
/// Each segment is kept exactly as spelled, so slug it first when you want slugs, other casing, or
/// transcription. A source's `route-segments` are only the default the file layout gives; a
/// document's own output decides where it is reachable.
///
/// Constraints: the result always starts and ends with `/`, and no segments give the site root `/`;
/// a segment that is empty, `.`, `..`, or that holds `/`, `\`, or a character no published filename
/// can carry is an error.
///
/// Example - keep the segments as spelled, then slug each one to publish them:
///
/// ```typ
/// #import "@tola/address:0.0.0": join-route, output-of, slugify
/// #let route = join-route(("posts", "Deep Notes").map(slugify))
/// #assert.eq(output-of(route), "posts/deep-notes/index.html")
/// #document(output-of(route), title: [Deep Notes])[Body]
/// ```
///
/// Example - read the default route the file layout gives every source:
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

/// Pure function: the output file a site-root route publishes to.
///
/// A route is the path your site publishes under, and `%` or `#` in it are ordinary filename
/// characters. A trailing `/` names a directory, published as that directory's `index.html`, so
/// `/` is `index.html` and `/guide/` is `guide/index.html`; a route without a trailing `/` names a
/// file exactly as spelled, so `/guide` is `guide` and `/manual.pdf` is `manual.pdf`.
///
/// Constraints: a route is taken as spelled, so decode a percent-encoded URL path with
/// `decode-url-path` first — exactly once.
///
/// Example - name the output file for a route:
///
/// ```typst
/// #import "@tola/address:0.0.0": output-of
/// #assert.eq(output-of("/"), "index.html")
/// #assert.eq(output-of("/guide/"), "guide/index.html")
/// #assert.eq(output-of("/manual.pdf"), "manual.pdf")
/// ```
///
/// Example - keep a literal `%` or `#` as a filename character:
///
/// ```typst
/// #import "@tola/address:0.0.0": output-of
/// #assert.eq(output-of("/100%/"), "100%/index.html")
/// #assert.eq(output-of("/a#b.html"), "a#b.html")
/// ```
/// Related: join-route, decode-url-path
#import "@tola/host:0.0.0": output-of

/// Pure function: the site-root route an output file is reachable at.
///
/// An exact `index.html` filename becomes the directory address that contains it, so `index.html`
/// is `/` and `guide/index.html` is `/guide/`. Every other filename keeps its path under `/`, so
/// `404.html` is `/404.html` and `about` is `/about`.
///
/// `output-of(route-of(output))` recovers the output; converting `/guide/index.html`
/// to an output and back gives its canonical route `/guide/`.
///
/// Example - map output files back to routes:
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
/// Pass an output, such as `guide/index.html`, either the document's own output or the result of
/// `output-of`. `base-path` is the deployment path below the host root and defaults to this
/// site's `site.base-path`, so the result is host-relative and mounted for the site being built.
/// Pass `base-path: "/"` for a root deployment, or another path to override the site's own. Pass
/// `origin` to get an absolute URL; `origin: site.origin` uses the site's own origin.
///
/// Constraints: `base-path` must start and end with `/`, and any escapes in it are decoded once;
/// `origin` is an absolute `http`/`https` origin and cannot include a deployment path, credentials,
/// query, or fragment; output names are encoded once, so a literal `%20` becomes `%2520`.
///
/// Any output path resolves here, including one a hook produces: nothing is checked, because the
/// address is a name, not a promise that the file exists. A URL a `[assets]` declaration spells is
/// not an output path — `url-of-asset` resolves those, and adds their byte identity.
///
/// `route-of` names the same file without the deployment path, the `origin`, or
/// percent-encoding: it stays `/guide/` under a `/docs/` mount. Compare addresses inside the site
/// with it; hand `url-of` to a browser.
///
/// Keep the document output unprefixed, and use the returned URL directly in a link. With
/// `site.base-path = "/docs/"`, this page links to `/docs/guide/` while the output stays
/// `guide/index.html`. Final reference checks verify that the linked output exists.
///
/// Example - publish a page and link it from another document:
///
/// ```typst
/// #import "@tola/address:0.0.0": url-of
/// #let guide-output = "guide/index.html"
/// #document("index.html", title: [Home])[
///   #link(url-of(guide-output))[Read the guide]
/// ]
/// #document(guide-output, title: [Guide])[
///   #title()
///   Start here.
/// ]
/// ```
///
/// Example - mount an output under a base path and an origin:
///
/// ```typst
/// #import "@tola/address:0.0.0": url-of
/// #assert.eq(url-of("guide/index.html"), "/guide/")
/// #assert.eq(url-of("Hello World/index.html", base-path: "/docs/"), "/docs/Hello%20World/")
/// #assert.eq(
///   url-of("guide/index.html", base-path: "/docs/", origin: "https://example.test"),
///   "https://example.test/docs/guide/",
/// )
/// ```
///
/// Example - encode a literal `%20` in an output name exactly once:
///
/// ```typst
/// #import "@tola/address:0.0.0": url-of
/// #assert.eq(url-of("100%20.html"), "/100%2520.html")
/// ```
///
/// Example - link a hook's own output, which no `assets` declaration names:
///
/// ```typst
/// #import "@tola/address:0.0.0": url-of
/// #assert.eq(url-of("assets/pagefind-search/pagefind.js"), "/assets/pagefind-search/pagefind.js")
/// ```
/// Related: output-of, url-of-asset, @tola/site
/// - output (string): the output path to mount.
/// - base-path (auto | string): the deployment path; `auto` takes the site's own `base-path`.
/// - origin (none | string): the absolute origin the result is written under.
/// -> string
#let url-of(output, base-path: auto, origin: none) = {
  import "@tola/host:0.0.0": url-of as resolve

  if base-path == auto {
    import "@tola/site:0.0.0": site
    resolve(output, base-path: site.base-path, origin: origin)
  } else {
    resolve(output, base-path: base-path, origin: origin)
  }
}

/// Reads site configuration (`[assets]`, `[site]` `base-path`): the browser URL a declared asset URL serves at.
///
/// The argument is the site-root URL the declaration itself spells:
///
/// - `assets.files` - the entry's own `url`, such as `"/styles/base.css"`.
/// - `assets.trees` - the tree's `url-prefix` followed by the member's path inside the tree:
///   `url-prefix = "/assets"` with `images/logo.svg` in that tree gives
///   `url-of-asset("/assets/images/logo.svg")`.
///
/// Write the URL either encoded or decoded: `url-of-asset` parses it once as a path, so the lookup
/// matches the declaration either way. A build resolves it to the address that declaration
/// publishes, and appends `?h=<identity>` when `assets.cache-busting` is on, so changed bytes
/// become a changed address under an unchanged name.
///
/// Constraints: the returned URL starts with `/` and includes `site.base-path`, so use it directly
/// from any document — mounting it again duplicates it; calling it for a URL that no `assets`
/// declaration publishes is an error at the call site, even when something else publishes that
/// path, such as a `resize-image` derivative.
///
/// How the two differ:
///
/// | | `url-of(output)` | `url-of-asset(declared URL)` |
/// | --- | --- | --- |
/// | Input | an output: a file path under `build.publish-dir`, no leading `/` (`assets/logo.png`) | the site-root URL a declaration spells, leading `/` included (`/assets/logo.png`); encoded or decoded |
/// | Checks | nothing — an address is a name, not a promise the file exists, so a hook's future output resolves too | the `[assets]` declaration — an undeclared URL is an error at the call site, even when something else writes that path |
/// | Result | the mounted browser URL (starts with `/`) | the mounted browser URL (starts with `/`) plus `?h=<identity>` when `assets.cache-busting` is on |
///
/// An editor source check resolves the mounted address without the identity, because it does not
/// render asset bytes. `tola check` prepares the complete outputs, including asset identities,
/// without publishing them.
///
/// A hook's own output is such a path: pages link it with `url-of("assets/…")`, and the two
/// calls stay apart — pass a declaration's spelling here, never another function's result.
///
/// Example - resolve a file the site publishes under `/assets`:
///
/// ```typst site
/// #import "@tola/address:0.0.0": url-of-asset
/// #document("index.html")[
///   #html.img(src: url-of-asset("/assets/logo.png"), alt: "Example logo")
/// ]
/// ```
/// Related: url-of, @tola/site, @tola/image
#import "@tola/host:0.0.0": url-of-asset

/// Pure function: decode a percent-encoded site-root URL path exactly once.
///
/// Every escape becomes the byte it names, so `/caf%C3%A9/` is `/café/` and `/Hello%20World/` is
/// `/Hello World/`. Decode here, once.
///
/// Constraints: pass a path and nothing else — an origin, query string, or fragment raises an
/// error, and so does an escaped `/` or `\`, because only an unescaped `/` starts a hierarchy
/// level.
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
/// #import "@tola/address:0.0.0": decode-url-path, output-of
/// #assert.eq(decode-url-path("/100%25/"), "/100%/")
/// #assert.eq(output-of(decode-url-path("/100%25/")), "100%/index.html")
/// ```
/// Related: output-of
#import "@tola/host:0.0.0": decode-url-path
