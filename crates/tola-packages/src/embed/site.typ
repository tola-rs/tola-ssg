// @tola/site:0.0.0 - the site's configuration values, resolved from `tola.toml`

/// The site's configuration values, resolved from `tola.toml`.
///
/// `site` carries one entry for each `[site]` setting, plus the derived `url`: `origin`,
/// `base-path`, `url`, `title`, `authors`, `description`, `language`, and `languages`.
/// `site.options` is the dictionary from the top-level `[options]` table. It defaults to `(:)`.
///
/// `base-path` is the deployment path below the host root, `/` by default. `url` joins
/// `origin` and `base-path` into the site's canonical address. A site that declares no
/// `site.origin` publishes relative addresses only, so its `url` is `none`.
///
/// `title` and `description` hold the text you configured in `tola.toml`; each
/// stays empty when the setting is absent. `authors` holds one `(name:, email:, url:)` record per
/// author. `language` is the site's default language (`en` when unset), a record with `tag`, `lang`,
/// `script`, and `region`: `tag` is the whole canonical tag, such as `zh-Hans-CN`, and the parts are
/// what `#set text` takes. `script` is `auto` and `region` is `none` when the declared language has
/// neither.
///
/// `languages` is every configured language, in configuration order; it defaults to `()`, and a
/// nonempty array must contain `language`. Both are template inputs: the site itself decides how to
/// route, pair, and switch between languages.
///
/// Example - read the values of a default site:
///
/// ```typst
/// #import "@tola/site:0.0.0": site
/// #assert.eq(site.base-path, "/")
/// #assert.eq(site.url, none)
/// #assert.eq(site.language.tag, "en")
/// #assert.eq(site.languages, ())
/// ```
///
/// Example - read the values of a configured site:
///
/// ```typst site
/// #import "@tola/site:0.0.0": site
/// #assert.eq(site.title, "Example site")
/// #assert.eq(site.base-path, "/")
/// ```
/// Related: @tola/web
#import "@tola/host:0.0.0": site

/// A native path rooted in the site, including when called from a theme package.
///
/// Use this for project data or generated inputs whose location the theme knows.
/// It creates a path without reading the file; normal input boundaries still apply.
///
/// ```typst site
/// #import "@tola/site:0.0.0": project-path
/// #let data = project-path("generated/catalog.json")
/// ```
#import "@tola/host:0.0.0": project-path
