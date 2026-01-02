//! Tola virtual package types and template rendering.

use std::path::{Path, PathBuf};

use typst_batch::prelude::*;

use crate::embed::{Template, TemplateVars};

use super::Phase;

// =============================================================================
// Constants
// =============================================================================

const TOLA_NAMESPACE: &str = "tola";
const TOLA_VERSION: PackageVersion = PackageVersion::new(0, 0, 0);

// =============================================================================
// Template Constants
// =============================================================================

const SITE_INFO_TYP: Template<SiteInfoTypVars<'static>> = Template::new(include_str!("embed/site.typ"));
const PAGES_TYP: Template<PagesTypVars<'static>> = Template::new(include_str!("embed/pages.typ"));
const CURRENT_TYP: Template<CurrentTypVars<'static>> = Template::new(include_str!("embed/current.typ"));

// =============================================================================
// Template Variables
// =============================================================================

struct SiteInfoTypVars<'a> {
    site_info_key: &'a str,
}

struct PagesTypVars<'a> {
    phase_key: &'a str,
    pages_key: &'a str,
    filter_phase: &'a str,
}

struct CurrentTypVars<'a> {
    current_key: &'a str,
}

impl TemplateVars for SiteInfoTypVars<'_> {
    fn apply(&self, content: &str) -> String {
        content.replace("__SITE_INFO_KEY__", self.site_info_key)
    }
}

impl TemplateVars for PagesTypVars<'_> {
    fn apply(&self, content: &str) -> String {
        content
            .replace("__PHASE_KEY__", self.phase_key)
            .replace("__PAGES_KEY__", self.pages_key)
            .replace("__FILTER_PHASE__", self.filter_phase)
    }
}

impl TemplateVars for CurrentTypVars<'_> {
    fn apply(&self, content: &str) -> String {
        content.replace("__CURRENT_KEY__", self.current_key)
    }
}

// =============================================================================
// TolaPackage Enum
// =============================================================================

/// Tola virtual package type.
///
/// Injected to Typst via `sys.inputs`:
/// - `Site`: Static config from `tola.toml`, always available
/// - `Pages`: All page metadata, available after scan phase
/// - `Current`: Current page context, available at compile time
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TolaPackage {
    Site,
    Pages,
    Current,
}

impl TolaPackage {
    /// Package name (e.g., "site").
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Site => "site",
            Self::Pages => "pages",
            Self::Current => "current",
        }
    }

    /// Check if this package requires iterative compilation.
    ///
    /// Pages that import `@tola/pages` or `@tola/current` may need multiple
    /// compilations to resolve self-referencing metadata.
    #[inline]
    pub const fn requires_iteration(&self) -> bool {
        matches!(self, Self::Pages | Self::Current)
    }

    /// sys.inputs key (e.g., "__tola_site_info").
    pub fn input_key(&self) -> String {
        format!("__tola_{}", self.name())
    }

    /// Rendered lib.typ content.
    ///
    /// Uses Template mechanism to replace placeholders with actual key values,
    /// ensuring single source of truth between Rust and Typst.
    pub fn lib_content(&self) -> String {
        match self {
            Self::Site => SITE_INFO_TYP.render(&SiteInfoTypVars {
                site_info_key: &self.input_key(),
            }),
            Self::Pages => PAGES_TYP.render(&PagesTypVars {
                phase_key: Phase::input_key(),
                pages_key: &Self::Pages.input_key(),
                filter_phase: Phase::Filter.as_str(),
            }),
            Self::Current => CURRENT_TYP.render(&CurrentTypVars {
                current_key: &Self::Current.input_key(),
            }),
        }
    }

    /// Generate typst.toml manifest content.
    pub fn typst_toml(&self) -> String {
        format!(
            r#"[package]
name = "{}"
version = "{TOLA_VERSION}"
entrypoint = "lib.typ"
"#,
            self.name()
        )
    }

    /// Match from PackageId.
    pub fn from_id(pkg: &PackageId) -> Option<Self> {
        if pkg.namespace() != TOLA_NAMESPACE || pkg.version() != TOLA_VERSION {
            return None;
        }

        match pkg.name() {
            "site" => Some(Self::Site),
            "pages" => Some(Self::Pages),
            "current" => Some(Self::Current),
            _ => None,
        }
    }

    /// All tola packages.
    pub const fn all() -> &'static [Self] {
        &[Self::Site, Self::Pages, Self::Current]
    }

    /// Sentinel path for dependency tracking.
    ///
    /// Format: `@tola/package-name` (e.g., `@tola/site`)
    pub fn sentinel(&self) -> PathBuf {
        PathBuf::from(format!("@{}/{}", TOLA_NAMESPACE, self.name()))
    }
}

// =============================================================================
// Public API
// =============================================================================

/// Read a file from the virtual package.
///
/// Returns `None` if the package is not a tola package or the path is unknown.
pub fn read_package(pkg: &PackageId, path: &str) -> Option<Vec<u8>> {
    let tola_pkg = TolaPackage::from_id(pkg)?;
    match path {
        "/lib.typ" => Some(tola_pkg.lib_content().into_bytes()),
        "/typst.toml" => Some(tola_pkg.typst_toml().into_bytes()),
        _ => None,
    }
}

/// Get the sentinel path for a tola virtual package.
///
/// Used as a dependency key in the dependency graph.
pub fn package_sentinel(pkg: &PackageId) -> Option<PathBuf> {
    TolaPackage::from_id(pkg).map(|p| p.sentinel())
}

/// Generate LSP stub packages in `.tola/packages/`.
///
/// Creates stub files for tinymist LSP completion support.
/// Configure tinymist with: `--package-path .tola/packages`
pub fn generate_lsp_stubs(root: &Path) -> std::io::Result<()> {
    let packages_dir = root.join(".tola/packages/tola");

    for pkg in TolaPackage::all() {
        let dir = packages_dir.join(format!("{}/{TOLA_VERSION}", pkg.name()));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("typst.toml"), pkg.typst_toml())?;
        std::fs::write(dir.join("lib.typ"), pkg.lib_content())?;
    }

    Ok(())
}
