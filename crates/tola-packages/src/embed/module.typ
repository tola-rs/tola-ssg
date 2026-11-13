// @tola/module:0.0.0 - project configuration for imported modules

/// Read the alias, options, and assets of a configured module instance.
///
/// Inside an `@tola-modules` package, `get-module()` reads the current instance.
/// In a site file, pass an alias. For example, `get-module("notes")` reads
/// the instance declared in `[modules.notes]`. Each alias has its own instance, even when several share one directory.
///
/// The record contains `name` (the site alias), `options` (the options after applying site overrides), and `assets`.
/// Its `assets.files` maps selected names to rooted Typst paths. Replacement files use project
/// paths. Its `assets.urls` maps the same names to final browser URLs, including the deployment
/// path prefix and, if cache-busting is enabled, the file hash. Both maps describe static assets.
///
/// Example: Use a module's options and stylesheet:
///
/// Start with the `notes` example in `tola help "[modules]"`. In `theme/tola-module.toml`,
/// add `[assets]` with `trees = [{ source = "assets", url-prefix = "/" }]`. Put
/// `body { color: rebeccapurple; }` in `theme/assets/style.css`. In the site's `tola.toml`,
/// set `title = "Lab notes"` under `[modules.notes.options]`.
///
/// Replace the example's `content/index.typ` with:
///
///     #import "@tola/module:0.0.0": get-module
///     #let notes = get-module("notes")
///     #html.link(rel: "stylesheet", href: notes.assets.urls.at("style.css"))
///     = #notes.options.title
///
/// Keep the `site.typ` from `tola init`. Run `tola build` from the site root.
/// The home page displays “Lab notes” and loads the stylesheet. With the default settings,
/// the files are `public/index.html` and `public/_tola-modules/notes/style.css`.
#import "@tola/host:0.0.0": get-module
