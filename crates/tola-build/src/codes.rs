//! Every diagnostic identifier this library publishes, grouped by the surface that produces it.
//!
//! [`DiagnosticCode::new`] checks the `<surface>.<contract>` shape while this module compiles, so
//! a code is declared once here and used by name at its construction sites. Consumers outside this
//! crate read the same constants, so a recognized diagnostic stays the same diagnostic across the
//! library, the application, and the editor integration.

use crate::diagnostic::DiagnosticCode;

/// Typst Bundle construction and output payloads.
pub mod build {
    use super::DiagnosticCode;

    /// The hook contract itself is invalid.
    pub const HOOKS: DiagnosticCode = DiagnosticCode::new("build.hooks");
    /// A stylesheet or script Tola minifies could not be rewritten.
    pub const MINIFY: DiagnosticCode = DiagnosticCode::new("build.minify");
    /// The site candidate itself could not be built.
    pub const SITE: DiagnosticCode = DiagnosticCode::new("build.site");
}

/// Read-only source checks.
pub mod check {
    use super::DiagnosticCode;

    /// A checked source could not be analyzed.
    pub const SOURCE: DiagnosticCode = DiagnosticCode::new("check.source");
    /// A source imports a name its own spellings never read.
    pub const UNUSED_IMPORT: DiagnosticCode = DiagnosticCode::new("check.unused_import");
}

/// Configuration parsing, normalization, and validation.
pub mod config {
    use super::DiagnosticCode;

    /// A field this release no longer treats as stable.
    pub const EXPERIMENTAL: DiagnosticCode = DiagnosticCode::new("config.experimental");
    /// A configuration value is invalid.
    pub const INVALID: DiagnosticCode = DiagnosticCode::new("config.invalid");
    /// A configuration file could not be read.
    pub const IO: DiagnosticCode = DiagnosticCode::new("config.io");
    /// A configuration source could not be loaded.
    pub const LOAD: DiagnosticCode = DiagnosticCode::new("config.load");
    /// A changed configuration could not be read back.
    pub const RELOAD: DiagnosticCode = DiagnosticCode::new("config.reload");
    /// `tola.toml` is not valid TOML.
    pub const TOML: DiagnosticCode = DiagnosticCode::new("config.toml");
    /// A configuration problem that does not stop the build.
    pub const WARNING: DiagnosticCode = DiagnosticCode::new("config.warning");
}

/// Vendored third-party inputs.
pub mod vendor {
    use super::DiagnosticCode;

    /// A replacement of the vendored inputs stopped before it committed.
    pub const INCOMPLETE: DiagnosticCode = DiagnosticCode::new("vendor.incomplete");
}

/// Hook execution.
pub mod hook {
    use super::DiagnosticCode;

    /// A committed revision could not be consumed by an `after-publish` command.
    pub const AFTER_PUBLISH: DiagnosticCode = DiagnosticCode::new("hook.after_publish");
    /// A hook command failed.
    pub const COMMAND: DiagnosticCode = DiagnosticCode::new("hook.command");
}

/// Icon collection sources.
pub mod icons {
    use super::DiagnosticCode;

    /// Verified remote icon bytes are not available locally.
    pub const CACHE: DiagnosticCode = DiagnosticCode::new("icons.cache");
    /// A configured icon collection is invalid or unavailable.
    pub const COLLECTION: DiagnosticCode = DiagnosticCode::new("icons.collection");
}

/// Logical output ownership and its conflicts.
pub mod output {
    use super::DiagnosticCode;

    /// Two producers claim the same logical output path.
    pub const DUPLICATE: DiagnosticCode = DiagnosticCode::new("output.duplicate");
}

/// Publication of the staged output tree.
pub mod publish {
    use super::DiagnosticCode;

    /// The staged tree could not be published.
    pub const SITE: DiagnosticCode = DiagnosticCode::new("publish.site");
}

/// References between published documents and resources.
pub mod reference {
    use super::DiagnosticCode;

    /// A document base names another origin, so the page's relative references are not checked.
    pub const BASE_HREF_EXTERNAL_ORIGIN: DiagnosticCode =
        DiagnosticCode::new("reference.base_href_external_origin");
    /// Tola image natives read a different physical file than the matching asset declaration.
    pub const ASSET_URL_SHADOWED: DiagnosticCode =
        DiagnosticCode::new("reference.asset_url_shadowed");
    /// A link target names a fragment the target document does not have.
    pub const FRAGMENT_MISSING: DiagnosticCode = DiagnosticCode::new("reference.fragment_missing");
    /// A link target names a document this build does not publish.
    pub const NAVIGATION_MISSING: DiagnosticCode =
        DiagnosticCode::new("reference.navigation_missing");
    /// A link to a media resource does not match what the target publishes.
    pub const RESOURCE_MEDIA_MISMATCH: DiagnosticCode =
        DiagnosticCode::new("reference.resource_media_mismatch");
    /// A resource a document references is not published.
    pub const RESOURCE_MISSING: DiagnosticCode = DiagnosticCode::new("reference.resource_missing");
}

/// The published site as a whole.
pub mod site {
    use super::DiagnosticCode;

    /// The site published no pages.
    pub const NO_PAGES: DiagnosticCode = DiagnosticCode::new("site.no_pages");
    /// The site published no page at `404.html`.
    pub const NOT_FOUND_MISSING: DiagnosticCode = DiagnosticCode::new("site.not_found_missing");
    /// The complete output set could not be collected.
    pub const OUTPUTS: DiagnosticCode = DiagnosticCode::new("site.outputs");
}

/// Source metadata declarations.
pub mod source {
    use super::DiagnosticCode;

    /// A source still declares metadata through `#metadata((...)) <tola-meta>`.
    pub const DECLARATION_DEPRECATED: DiagnosticCode =
        DiagnosticCode::new("source.declaration_deprecated");
    /// A source's declaration events could not be read.
    pub const DECLARATION_PROTOCOL: DiagnosticCode =
        DiagnosticCode::new("source.declaration_protocol");
}

/// Typst compilation, export, and the files it reads.
pub mod typst {
    use super::DiagnosticCode;

    /// Bundle export failed.
    pub const BUNDLE_EXPORT: DiagnosticCode = DiagnosticCode::new("typst.bundle_export");
    /// Typst could not compile the site.
    pub const COMPILE: DiagnosticCode = DiagnosticCode::new("typst.compile");
    /// A configured compiler font could not be read.
    pub const FONT: DiagnosticCode = DiagnosticCode::new("typst.font");
    /// HTML export failed.
    pub const HTML_EXPORT: DiagnosticCode = DiagnosticCode::new("typst.html_export");
    /// A compiler input could not be supplied.
    pub const INPUT: DiagnosticCode = DiagnosticCode::new("typst.input");
    /// A file this build reads could not be read.
    pub const IO: DiagnosticCode = DiagnosticCode::new("typst.io");
    /// A source snapshot failed.
    pub const SNAPSHOT: DiagnosticCode = DiagnosticCode::new("typst.snapshot");
    /// Source analysis failed.
    pub const SOURCE_ANALYSIS: DiagnosticCode = DiagnosticCode::new("typst.source_analysis");
    /// The site program could not be compiled.
    pub const SITE_PROGRAM: DiagnosticCode = DiagnosticCode::new("typst.site_program");
    /// The Typst world could not be constructed.
    pub const WORLD: DiagnosticCode = DiagnosticCode::new("typst.world");
}

/// Staged output and its observation.
pub mod write {
    use super::DiagnosticCode;

    /// A staged input changed while the build was reading it.
    pub const STALE: DiagnosticCode = DiagnosticCode::new("write.stale");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Every code this library declares, so the uniqueness rule covers all of them.
    fn declared() -> Vec<DiagnosticCode> {
        let mut codes = Vec::new();
        codes.extend([
            build::HOOKS,
            build::MINIFY,
            build::SITE,
            check::SOURCE,
            check::UNUSED_IMPORT,
            config::EXPERIMENTAL,
            config::INVALID,
            config::IO,
            config::LOAD,
            config::RELOAD,
            config::TOML,
            config::WARNING,
            hook::AFTER_PUBLISH,
            hook::COMMAND,
            icons::CACHE,
            icons::COLLECTION,
            output::DUPLICATE,
            publish::SITE,
            reference::FRAGMENT_MISSING,
            reference::NAVIGATION_MISSING,
            reference::RESOURCE_MEDIA_MISMATCH,
            reference::ASSET_URL_SHADOWED,
            reference::RESOURCE_MISSING,
            site::NOT_FOUND_MISSING,
            site::NO_PAGES,
            site::OUTPUTS,
            source::DECLARATION_DEPRECATED,
            source::DECLARATION_PROTOCOL,
            typst::BUNDLE_EXPORT,
            typst::COMPILE,
            typst::FONT,
            typst::HTML_EXPORT,
            typst::INPUT,
            typst::IO,
            typst::SNAPSHOT,
            typst::SITE_PROGRAM,
            typst::SOURCE_ANALYSIS,
            typst::WORLD,
            vendor::INCOMPLETE,
            write::STALE,
        ]);
        codes
    }

    #[test]
    fn declared_codes_have_distinct_text() {
        let codes = declared();
        let unique = codes.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(codes.len(), unique.len(), "two constants share one code");
    }

    #[test]
    fn every_code_names_declared_surface() {
        let mut surfaces = BTreeSet::new();
        for code in declared() {
            let surface = code.as_str().split_once('.').unwrap().0;
            surfaces.insert(surface.to_owned());
        }
        assert_eq!(
            surfaces,
            [
                "build",
                "check",
                "config",
                "hook",
                "icons",
                "output",
                "publish",
                "reference",
                "site",
                "source",
                "typst",
                "vendor",
                "write",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
        );
    }
}
