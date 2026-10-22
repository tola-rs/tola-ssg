// @tola/module:0.0.0 - project configuration for imported modules

/// The configured record of a managed module.
///
/// `get-module()` (or `get-module(auto)`) selects the module containing the call; it must be
/// called from a configured `@tola-modules` package. `get-module("notes")` selects the alias
/// declared by `[modules.notes]` and also works in site code. Unknown aliases are errors.
/// The record contains `name` (the site alias), `package` (the declared package name), `version`
/// (the declared version), `options` (validated values), `slots` (rooted Typst paths), and
/// `asset-prefix` (the unmounted site-root URL of the module's assets). `assets` contains `enabled`,
/// `files` (selected relative names mapped to native input paths), and `urls` (the same names mapped
/// to final browser URLs, including the deployment mount and cache-busting identity). These maps
/// describe this module's static publisher, not arbitrary Bundle or hook outputs. Project file
/// replacements retain project-rooted input paths. Use the URLs directly, without adding the mount.
/// An alias selects its own module instance even when multiple aliases share one directory. This function is unavailable
/// during options schema validation; schemas can read site configuration from `@tola/site`.
///
/// ```typst site
/// #import "@tola/module:0.0.0": get-module
/// // From a module's own source: #let settings = get-module().options
/// ```
#import "@tola/host:0.0.0": get-module
