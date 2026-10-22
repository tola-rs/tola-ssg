//! Pure `tola.toml` schema and source-text parsing.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tola_config::Config;

use super::diagnostic::config_error;
use super::section::build::hooks::OutputCommandConfig;
use super::section::build::{
    AfterPublishHookConfig, BeforeBuildHookConfig, HooksConfig, MinifyConfig, ReferencesConfig,
};
use super::section::{
    AssetsConfig, FontsConfig, FormatterConfig, IconsConfig, SiteSectionConfig, TypstSectionConfig,
    VendorConfig,
};
use super::{
    BuildSectionConfig, ConfigDiagnostics, ConfigPresence, ConfigSource, FieldPath, ParsedConfig,
};
use crate::resources::InputScope;

/// Pure `tola.toml` schema before invocation overrides or path resolution.
///
/// Fields are editable inputs. Use [`Self::resolve`] to validate a programmatically
/// constructed schema before giving it to a build. To retain exact TOML source
/// presence and diagnostics, use [`crate::config::loading::resolve_site_config`] instead.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "")]
pub struct SiteConfigSchema {
    #[config(sub)]
    pub site: SiteSectionConfig,

    #[config(sub)]
    pub build: BuildSectionConfig,

    #[config(sub)]
    pub assets: AssetsConfig,

    #[config(sub)]
    pub typst: TypstSectionConfig,

    #[config(sub)]
    pub icons: IconsConfig,

    #[config(sub)]
    pub vendor: VendorConfig,

    #[config(sub)]
    pub formatter: FormatterConfig,

    #[config(sub)]
    pub theme: crate::modules::ThemeConfig,

    /// Local modules selected by namespace.
    pub modules: BTreeMap<String, crate::modules::ModuleDeclaration>,
}

impl SiteConfigSchema {
    /// Every top-level section this schema decodes.
    ///
    /// Each section declares its own key, so adding a section to this schema is the one edit
    /// that extends the list a host needs to decode the core sections.
    pub const SECTIONS: [&'static str; 9] = [
        SiteSectionConfig::TEMPLATE_SECTION,
        BuildSectionConfig::TEMPLATE_SECTION,
        AssetsConfig::TEMPLATE_SECTION,
        TypstSectionConfig::TEMPLATE_SECTION,
        IconsConfig::TEMPLATE_SECTION,
        VendorConfig::TEMPLATE_SECTION,
        FormatterConfig::TEMPLATE_SECTION,
        "modules",
        crate::modules::ThemeConfig::TEMPLATE_SECTION,
    ];

    /// Decode the core sections while preserving host-owned sections in the source document.
    pub(super) fn parse_at(
        path: &Path,
        content: &str,
        scope: InputScope,
    ) -> Result<ParsedConfig<Self>> {
        ConfigSource::parse(path, content, scope)?.decode_site()
    }

    pub(super) fn validate_theme(&self, presence: &ConfigPresence) -> Result<()> {
        let mut diagnostics = ConfigDiagnostics::new();
        for (namespace, module) in &self.modules {
            for (slot, path) in &module.overrides {
                if path.as_os_str().is_empty()
                    || !path.components().all(|part| {
                        matches!(
                            part,
                            std::path::Component::Normal(_) | std::path::Component::CurDir
                        )
                    })
                {
                    diagnostics.error(
                        FieldPath::new("modules"),
                        format!(
                            "`modules.{namespace}.overrides.{slot}` must be relative to the site root"
                        ),
                    );
                }
            }
        }
        if let Some(module) = &self.theme.module {
            if !self.modules.contains_key(module) {
                diagnostics.error(
                    FieldPath::new("theme.module"),
                    format!("the theme module `{module}` is not declared in `modules`"),
                );
            }
            if presence.contains(FieldPath::new("build.entry").as_str()) {
                diagnostics.error(FieldPath::new("theme.module"), "`theme.module` and an explicit `build.entry` cannot both select the site entry");
            }
        }
        diagnostics.into_result().map_err(config_error)
    }

    /// Validate source paths before site-root resolution.
    pub(super) fn validate_source_paths(&self) -> Result<()> {
        let mut diagnostics = ConfigDiagnostics::new();

        self.build.validate_paths(&mut diagnostics);
        self.assets.validate_paths(&mut diagnostics);
        self.typst.fonts.validate_paths(&mut diagnostics);
        self.icons.validate_paths(&mut diagnostics);
        self.vendor.validate_paths(&mut diagnostics);

        diagnostics.into_result().map_err(config_error)
    }
}

/// Every configuration key the core schema declares, in declaration order.
///
/// Each declaration supplies its own keys, starting with the root table's, so a new field or
/// section extends the list with one edit.
///
/// Sections outside the core schema belong to the host that decodes them, so their keys are not
/// listed here.
pub fn field_paths() -> impl Iterator<Item = FieldPath> {
    core_declarations()
        .into_iter()
        .flat_map(|(fields, _)| fields.iter().map(|(path, _)| *path))
}

/// The documentation a declared configuration key has.
///
/// A key answers with the comment its declaration wrote, or the one-line note its scaffold
/// provides; a key that opens a section answers with that section's own documentation, so the
/// table header and the key writing it read the same text. A key the schema does not declare
/// answers nothing.
pub fn field_documentation(path: &str) -> Option<&'static str> {
    core_declarations()
        .into_iter()
        .flat_map(|(fields, _)| fields.iter())
        .find(|(field, _)| field.as_str() == path)
        .and_then(|(_, documentation)| *documentation)
}

/// The values a declared configuration key accepts, when its type states them.
///
/// A key whose type says what it takes answers with those values — a boolean takes `true` and
/// `false` — and what every other key accepts is its documentation's to describe.
pub fn field_values(path: &str) -> Option<&'static [&'static str]> {
    core_declarations()
        .into_iter()
        .flat_map(|(_, values)| values.iter())
        .find(|(field, _)| field.as_str() == path)
        .map(|(_, values)| *values)
}

/// What `tola help` adds under a configuration table.
///
/// A section whose reference needs more room than a `tola.toml` comment should hold writes it
/// here, where the page has space for examples and the full rules.
pub fn section_help(section: &str) -> Option<&'static str> {
    [
        (SiteSectionConfig::TEMPLATE_SECTION, SiteSectionConfig::HELP),
        (
            BuildSectionConfig::TEMPLATE_SECTION,
            BuildSectionConfig::HELP,
        ),
        (MinifyConfig::TEMPLATE_SECTION, MinifyConfig::HELP),
        (ReferencesConfig::TEMPLATE_SECTION, ReferencesConfig::HELP),
        (AssetsConfig::TEMPLATE_SECTION, AssetsConfig::HELP),
        (VendorConfig::TEMPLATE_SECTION, VendorConfig::HELP),
        (FormatterConfig::TEMPLATE_SECTION, FormatterConfig::HELP),
        ("modules", crate::modules::HELP),
        (
            crate::modules::ThemeConfig::TEMPLATE_SECTION,
            crate::modules::ThemeConfig::HELP,
        ),
        (FontsConfig::TEMPLATE_SECTION, FontsConfig::HELP),
        (IconsConfig::TEMPLATE_SECTION, IconsConfig::HELP),
        (HooksConfig::TEMPLATE_SECTION, HooksConfig::HELP),
        (
            BeforeBuildHookConfig::TEMPLATE_SECTION,
            BeforeBuildHookConfig::HELP,
        ),
        (
            OutputCommandConfig::TEMPLATE_SECTION,
            OutputCommandConfig::HELP,
        ),
        (
            AfterPublishHookConfig::TEMPLATE_SECTION,
            AfterPublishHookConfig::HELP,
        ),
    ]
    .into_iter()
    .find_map(|(name, help)| (name == section).then_some(help))
}

type DeclaredConfigSection = (
    &'static [(FieldPath, Option<&'static str>)],
    &'static [(FieldPath, &'static [&'static str])],
);

/// Each declaration's keys and value sets, in the order the sections nest.
fn core_declarations() -> [DeclaredConfigSection; 16] {
    [
        (
            SiteConfigSchema::DECLARED_FIELDS,
            SiteConfigSchema::DECLARED_VALUES,
        ),
        (
            crate::modules::ThemeConfig::DECLARED_FIELDS,
            crate::modules::ThemeConfig::DECLARED_VALUES,
        ),
        (
            SiteSectionConfig::DECLARED_FIELDS,
            SiteSectionConfig::DECLARED_VALUES,
        ),
        (
            BuildSectionConfig::DECLARED_FIELDS,
            BuildSectionConfig::DECLARED_VALUES,
        ),
        (AssetsConfig::DECLARED_FIELDS, AssetsConfig::DECLARED_VALUES),
        (
            TypstSectionConfig::DECLARED_FIELDS,
            TypstSectionConfig::DECLARED_VALUES,
        ),
        (FontsConfig::DECLARED_FIELDS, FontsConfig::DECLARED_VALUES),
        (IconsConfig::DECLARED_FIELDS, IconsConfig::DECLARED_VALUES),
        (VendorConfig::DECLARED_FIELDS, VendorConfig::DECLARED_VALUES),
        (
            FormatterConfig::DECLARED_FIELDS,
            FormatterConfig::DECLARED_VALUES,
        ),
        (HooksConfig::DECLARED_FIELDS, HooksConfig::DECLARED_VALUES),
        (
            BeforeBuildHookConfig::DECLARED_FIELDS,
            BeforeBuildHookConfig::DECLARED_VALUES,
        ),
        (
            OutputCommandConfig::DECLARED_FIELDS,
            OutputCommandConfig::DECLARED_VALUES,
        ),
        (
            AfterPublishHookConfig::DECLARED_FIELDS,
            AfterPublishHookConfig::DECLARED_VALUES,
        ),
        (MinifyConfig::DECLARED_FIELDS, MinifyConfig::DECLARED_VALUES),
        (
            ReferencesConfig::DECLARED_FIELDS,
            ReferencesConfig::DECLARED_VALUES,
        ),
    ]
}

impl ParsedConfig<SiteConfigSchema> {
    /// Validate declared settings without requiring the site directory to exist.
    ///
    /// This checks relative source paths and field semantics. Filesystem ownership,
    /// physical input/output separation, and invocation overrides are validated by
    /// [`crate::config::loading::resolve_parsed_site_config`] before building.
    pub fn validate_settings(&self) -> Result<()> {
        self.validate_source_paths()?;
        self.schema
            .validate_theme(self.source.core_presence())
            .and_then(|()| {
                settings_diagnostics(
                    &self.schema.site,
                    &self.schema.build,
                    &self.schema.assets,
                    &self.schema.icons,
                    &self.schema.typst,
                    &self.schema.vendor,
                    self.source.core_presence(),
                )
                .into_result()
                .map_err(config_error)
            })
            .map_err(|error| self.source.attach_load(error))
    }

    /// Validate relative source paths without reading site inputs or resolving outputs.
    pub fn validate_source_paths(&self) -> Result<()> {
        self.schema
            .validate_source_paths()
            .map_err(|error| self.source.attach_load(error))
    }
}

pub(super) fn settings_diagnostics(
    site: &SiteSectionConfig,
    build: &BuildSectionConfig,
    assets: &AssetsConfig,
    icons: &IconsConfig,
    typst: &TypstSectionConfig,
    vendor: &VendorConfig,
    presence: &ConfigPresence,
) -> ConfigDiagnostics {
    let mut diagnostics = ConfigDiagnostics::new();
    diagnostics.set_presence(presence.clone());
    site.validate_field_status(&mut diagnostics);
    build.validate_field_status(&mut diagnostics);
    assets.validate_field_status(&mut diagnostics);
    typst.fonts.validate_field_status(&mut diagnostics);
    icons.validate_field_status(&mut diagnostics);
    vendor.validate_field_status(&mut diagnostics);
    site.validate_address(&mut diagnostics);
    site.validate_language(&mut diagnostics);
    build.hooks.validate(&mut diagnostics);
    assets.validate(&mut diagnostics);
    icons.validate(&mut diagnostics);
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modules_retained_without_reading_manifest() {
        let site = tempfile::tempdir().unwrap();
        let config = crate::config::loading::resolve_site_config(
            &site.path().join("tola.toml"),
            "[modules.forest]\npath = \"theme\"\n",
            tola_typst::PackageLocations::default(),
            &crate::config::loading::BuildOverrides::default(),
        )
        .unwrap()
        .into_config();
        assert_eq!(
            config.modules()["forest"].source,
            crate::modules::ModuleSource::Local {
                path: std::path::PathBuf::from("theme")
            }
        );
        assert!(!site.path().join("theme").exists());
    }

    #[test]
    fn module_roots_protected_publication_hooks() {
        let site = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(site.path().join("themes/forest")).unwrap();
        let config = |extra: &str| format!("[modules.forest]\npath = \"themes/forest\"\n{extra}");
        let resolve = |source: &str| {
            crate::config::loading::resolve_site_config(
                &site.path().join("tola.toml"),
                source,
                tola_typst::PackageLocations::default(),
                &crate::config::loading::BuildOverrides::default(),
            )
        };
        for output in ["themes", "themes/forest", "themes/forest/output"] {
            let error =
                resolve(&config(&format!("[build]\npublish-dir = \"{output}\"\n"))).unwrap_err();
            assert!(format!("{error:#}").contains("module root"), "{error:#}");
        }
        let error = resolve(&config("[[build.hooks.before-build]]\nenable = true\nname = \"overwrite\"\ncommand = [\"tool\"]\ngenerates = [\"themes/forest/data\"]\n")).unwrap_err();
        assert!(format!("{error:#}").contains("module root"), "{error:#}");
        assert!(resolve(&config("[[build.hooks.before-build]]\nenable = false\nname = \"off\"\ncommand = [\"tool\"]\ngenerates = [\"themes/forest/data\"]\n")).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_root_protected_physical_identity() {
        use std::os::unix::fs::symlink;
        let site = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(site.path().join("actual/forest")).unwrap();
        symlink(
            site.path().join("actual/forest"),
            site.path().join("theme-link"),
        )
        .unwrap();
        let source = "[modules.forest]\npath = \"theme-link\"\n[build]\npublish-dir = \"actual\"\n";
        let error = crate::config::loading::resolve_site_config(
            &site.path().join("tola.toml"),
            source,
            tola_typst::PackageLocations::default(),
            &crate::config::loading::BuildOverrides::default(),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("module root"), "{error:#}");
    }

    #[test]
    fn theme_module_excludes_explicit_entry() {
        let site = tempfile::tempdir().unwrap();
        let resolve = |source: &str| {
            crate::config::loading::resolve_site_config(
                &site.path().join("tola.toml"),
                source,
                tola_typst::PackageLocations::default(),
                &crate::config::loading::BuildOverrides::default(),
            )
        };
        assert!(
            resolve("[modules.forest]\npath = \"theme\"\n[theme]\nmodule = \"forest\"\n").is_ok()
        );
        assert!(resolve("[theme]\nmodule = \"missing\"\n").is_err());
        assert!(resolve("[modules.forest]\npath = \"theme\"\n[theme]\nmodule = \"forest\"\n[build]\nentry = \"site.typ\"\n").is_err());
    }

    #[test]
    fn settings_validate_without_site() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("site");
        let parsed = ConfigSource::parse(
            &root.join("tola.toml"),
            "[site]\norigin = \"https://example.com\"\nlanguage = \"zh-CN\"\n",
            InputScope::Online,
        )
        .unwrap()
        .decode_site()
        .unwrap();

        parsed.validate_settings().unwrap();

        assert!(!root.exists());
    }

    #[test]
    fn refused_settings_still_name_their_line() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("site");
        let path = root.join("tola.toml");
        for (source, field) in [
            ("[site]\nlanguage = \"english\"\n", "site.language"),
            ("[site]\norigin = \"example.com\"\n", "site.origin"),
            ("[build]\nentry = \"../entry.typ\"\n", "build.entry"),
        ] {
            let parsed = ConfigSource::parse(&path, source, InputScope::Online)
                .unwrap()
                .decode_site()
                .unwrap();
            let error = parsed.validate_settings().unwrap_err();
            let diagnostics = crate::config::tests::attached_diagnostics(&error);

            assert!(
                diagnostics.iter().any(|diagnostic| {
                    diagnostic.severity == tola_diagnostic::Severity::Error
                        && diagnostic.notes.iter().any(|note| note.as_str() == field)
                        && diagnostic
                            .location
                            .as_ref()
                            .is_some_and(|location| location.path.ends_with("tola.toml"))
                }),
                "{source}: {diagnostics:?}"
            );
            assert!(!root.exists());
        }
    }

    /// Every key an author may write is offered once, whatever section declares it.
    #[test]
    fn field_paths_cover_every_section() {
        let paths = field_paths()
            .map(|field| field.as_str())
            .collect::<Vec<_>>();

        for expected in [
            "site.title",
            "build.entry",
            "build.hooks.before-build",
            "build.minify.html",
            "typst.fonts.paths",
            "icons.collections",
            "formatter.print-width",
        ] {
            assert!(paths.contains(&expected), "`{expected}` is missing");
        }

        let mut sorted = paths.clone();
        sorted.sort_unstable();
        let mut unique = sorted.clone();
        unique.dedup();
        assert_eq!(unique.len(), sorted.len(), "field paths repeat");
    }

    /// Every declared path documents itself, so hover answers for the whole file.
    #[test]
    fn every_declared_path_documents_itself() {
        for path in field_paths().map(|field| field.as_str()) {
            let documentation = field_documentation(path);
            assert!(
                documentation.is_some_and(|documentation| !documentation.is_empty()),
                "`{path}` has no documentation"
            );
        }
    }
}
