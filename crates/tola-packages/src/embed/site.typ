// @tola/site:0.0.0 - the site's configuration values, resolved from `tola.toml`

/// The site's configuration values, resolved from `tola.toml`.
///
/// `site` carries one entry for each `[site]` setting, plus the derived `url`: `origin`,
/// `base-path`, `url`, `title`, `authors`, `description`, `language`, `languages`, `copyright`, and
/// `extra`.
/// Declare your own values under `[site.extra]`; they arrive under `extra`, and a TOML date or
/// datetime among them arrives as a string.
///
/// `base-path` is the deployment path below the host root, `/` by default. `url` joins
/// `origin` and `base-path` into the site's canonical address. A site that declares no
/// `site.origin` publishes relative addresses only, so its `url` is `none`.
///
/// `title`, `description`, and `copyright` hold the text you configured in `tola.toml`; each
/// stays empty when the setting is absent. `authors` holds one `(name:, email:, url:)` record per
/// author. `language` holds `tag`, `lang`, `script`, and `region`: `tag` is the whole canonical
/// tag, such as `zh-Hans-CN`, and the parts are what `#set text` takes. `script` is `auto` and
/// `region` is `none` when the declared language has neither.
///
/// `language` is the site's default language (`en` when unset). `languages` is the configured
/// array of languages, each with the same four fields, in configuration order; it defaults to
/// `()`. A nonempty array must contain the default language. These are template inputs: your
/// site chooses how to route, pair, and switch between languages.
///
/// Example - read the values of a default site:
///
/// ```typst
/// #import "@tola/site:0.0.0": site
/// #assert.eq(site.base-path, "/")
/// #assert.eq(site.url, none)
/// #assert.eq(site.language.tag, "en")
/// #assert.eq(site.languages, ())
/// #assert.eq(site.extra, (:))
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
