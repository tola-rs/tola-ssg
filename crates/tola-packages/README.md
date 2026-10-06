# tola-packages

The builtin `@tola/*` packages site authors import: an identity, the Typst sources it carries, and
the values the engine feeds it. The manifest is generated from the identity, so a package cannot
disagree with the specification it is resolved by.

Site authors import `@tola/site`, `@tola/address`, `@tola/source`, `@tola/collection`,
`@tola/schema`, `@tola/document`, `@tola/web`, `@tola/icon`, `@tola/code`, and `@tola/image`;
`@tola/host` is internal. Imports use the fixed version `0.0.0`; the implementation ships with
Tola.

`builtin_packages()` returns those; `resolvable_packages()` is everything a build
resolves, `@tola/host` included, with relative paths and compiler sources. `tola editor packages` uses
it to install local package views for editors, so an editor resolves every import the compiler does.

`TolaPackage::exports` reads an entrypoint's top-level `#let` bindings and explicit imports, and
each export's hover documentation comes from the entrypoint too. Usage and export documentation
live in the repository README, `tola skill`, and `tola help @tola/<package> [export ...]` (no site
needed).

Every exported function documents its parameters and its return in the shape Tinymist and typlite
read: `- name (type): description` per parameter, then `-> type`. `tola help` and the editor hover
render those annotations, and `bundled_functions_document_every_parameter_type` refuses a function
that leaves a type out.

## Code theme data

`@tola/code` carries one tmTheme file per code theme. `code_themes.rs` is the catalog: the name a
site writes (the `code-themes.<name>` key), the appearance, the file, and the upstream
project, source file, revision, and licence of each theme's bytes. `themes_dict()` turns that
catalog into the `code-themes` values a site passes to `raw.theme`, and `theme_file(name)` gives
the package-relative file path. The `render-code` helper a site calls never chooses the theme:
`raw.theme` selects the primary appearance before Typst highlights the block, and `dark-theme` only
adds the fixed appearance for the site's dark state.

Every carried theme, its exact upstream revision, and a verbatim snapshot of the licence it is
redistributed under are recorded in [`licenses/README.md`](licenses/README.md), which ships beside
release archives as `PROVENANCE-TOLA-PACKAGES.md`. New or changed theme data is audited there
before it enters a release.
