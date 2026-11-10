// @tola/module:0.0.0 - project configuration for imported modules

/// The configured record of a managed module.
///
/// `get-module()` selects the instance whose package contains the call.
/// `get-module("notes")` selects the alias declared by `[modules.notes]` and also works in
/// project files. Each alias has its own instance, even when several share one directory.
///
/// The record contains `name` (the site alias), `options` (the effective options), and `assets`.
/// Its `assets.files` maps selected names to rooted Typst paths. Replacement files use project
/// paths. Its `assets.urls` maps the same names to final browser URLs, including the deployment
/// mount and any cache-busting identity. Both maps describe statically published module assets.
///
/// ```typst site
/// #import "@tola/module:0.0.0": get-module
/// // From a module's own source: #let settings = get-module().options
/// ```
#import "@tola/host:0.0.0": get-module
