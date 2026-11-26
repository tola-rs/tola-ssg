// @tola/site:0.0.0 - the site's configuration values, resolved from `tola.toml`

/// Read the resolved `[site]` settings and `[options]` from Typst.
///
/// Read the `[site]` settings by name, including the derived `url`: `origin`,
/// `base-path`, `url`, `title`, `authors`, `description`, `language`, and `languages`.
/// `site.options` is the dictionary from the top-level `[options]` table. It defaults to `(:)`.
///
/// `base-path` is the path where you deploy the site, `/` by default. `url` joins
/// `origin` and `base-path` into the site's canonical address. Without `site.origin`, `site.url` is `none`.
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
/// `site.build-mode` is the command's build mode, the same value hooks read as `hook.mode`: `"dev"`
/// under `tola dev`, and `"prod"` under `tola build`, `tola check`, and `tola preview`. It is `none`
/// in the editor, `tola query`, and `tola inspect`, which compile outside a build. A site can do less
/// while you write, for example publish only its newest posts when `site.build-mode == "dev"`.
///
/// Example: Read default settings:
///
/// With the default `tola.toml` settings, these are the values available in `site.typ`.
///
/// ```typst
/// #import "@tola/site:0.0.0": site
/// #assert.eq(site.base-path, "/")
/// #assert.eq(site.url, none)
/// #assert.eq(site.language.tag, "en")
/// #assert.eq(site.languages, ())
/// ```
///
/// Example: Read a configured title:
///
/// Set `title = "Example site"` in `[site]` in `tola.toml` to read it in Typst.
///
/// ```typst site
/// #import "@tola/site:0.0.0": site
/// #assert.eq(site.title, "Example site")
/// #assert.eq(site.base-path, "/")
/// ```
/// Related: @tola/head
#import "@tola/host:0.0.0": site

/// A native path rooted in the site, including when called from a theme package.
///
/// Use this to locate a site file from a theme, such as JSON written by a hook.
/// Pass the returned path to `read` or `json` to open the file.
///
///
/// Create `data/catalog.json` in the site root.
///
/// ```json data/catalog.json
/// {"title": "Field notes"}
/// ```
///
/// Replace `site.typ` with the following code. Run `tola build` from the site root.
/// With the default publish directory, `public/index.html` displays “Field notes”.
///
/// ```typst site
/// #import "@tola/site:0.0.0": project-path
/// #let catalog = json(project-path("data/catalog.json"))
/// #document("index.html")[#catalog.title]
/// ```
#import "@tola/host:0.0.0": project-path
