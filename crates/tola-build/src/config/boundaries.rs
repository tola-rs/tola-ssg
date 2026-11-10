//! Filesystem boundaries of the configured site layout.

use std::path::{Path, PathBuf};

use super::section::AssetsConfig;
use super::section::build::HooksConfig;
use super::{ConfigDiagnostics, FieldPath, SiteConfigSchema};

/// Keeps configured assets disjoint from content discovery and owned output/state.
/// The content restriction applies to these mappings, not to Typst reads of colocated files:
/// Bundle assets and image producers can still use those files as inputs.
pub(super) fn validate_asset_source_boundaries(
    assets: &AssetsConfig,
    site_root: &Path,
    content_root: &Path,
    output_root: &Path,
    diagnostics: &mut ConfigDiagnostics,
) {
    let mut declarations = assets
        .tree_sources()
        .enumerate()
        .map(|(index, source)| (AssetsConfig::FIELDS.trees, index, source))
        .chain(
            assets
                .file_sources()
                .enumerate()
                .map(|(index, source)| (AssetsConfig::FIELDS.files, index, source)),
        )
        .peekable();
    if declarations.peek().is_none() {
        return;
    }

    let internal_root = site_root.join(tola_input::INTERNAL_DIR);
    let protected_boundaries = [
        ("content root", content_root),
        ("output root", output_root),
        ("Tola's `.tola` directory", internal_root.as_path()),
    ]
    .map(|(owner, protected)| {
        (
            owner,
            protected,
            tola_input::FilesystemSourceIdentity::from_path(protected),
        )
    });
    for (field, index, source) in declarations {
        let source_identity = tola_input::FilesystemSourceIdentity::from_path(source);
        for (owner, protected, protected_identity) in &protected_boundaries {
            if source_identity.intersects(protected_identity) {
                diagnostics.error(
                    field,
                    format!(
                        "`{}[{index}]` source `{}` overlaps the {owner} `{}`",
                        field.as_str(),
                        tola_diagnostic::display_path(source, site_root),
                        tola_diagnostic::display_path(protected, site_root),
                    ),
                );
            }
        }
    }
}

pub(crate) fn validate_hook_output_boundaries(
    hooks: &HooksConfig,
    root: &Path,
    configuration: &Path,
    build_output: &Path,
    vendor: Option<&Path>,
    vendor_workspace: Option<&Path>,
    diagnostics: &mut ConfigDiagnostics,
) {
    let boundaries = crate::filesystem::DeclaredOutputBoundaries::for_site(
        root,
        configuration,
        build_output,
        vendor,
        vendor_workspace,
    );
    let outputs = crate::config::section::build::BeforeBuildHookConfig::FIELDS.generates;
    // Every granted declaration, so a shared path names the entry that claimed it first.
    let mut granted = Vec::<(
        &crate::config::section::build::BeforeBuildHookConfig,
        PathBuf,
        tola_input::FilesystemSourceIdentity,
    )>::new();
    for (entry, hook) in hooks.before_build.iter().enumerate() {
        // Disabled entries reserve nothing, and each entry keeps the index it was written at.
        if !hook.enable {
            continue;
        }
        // The scope has the entry's own index, so a failure names the entry the author must
        // edit rather than the first one of the array.
        diagnostics.with_array_element(
            HooksConfig::FIELDS.before_build.as_str(),
            entry,
            |diagnostics| {
                for output in &hook.generates {
                    let resolved = boundaries.resolve(output);
                    let violation = boundaries
                        .violation(&resolved, granted.iter().map(|(_, _, identity)| identity));
                    let shown = tola_diagnostic::display_path(&root.join(output), root);
                    match violation {
                        None => {}
                        Some(crate::filesystem::DeclaredOutputViolation::OutsideSite) => {
                            diagnostics.error_with_help(
                                outputs,
                                format!("hook output `{shown}` is outside the site root"),
                                "Declare a path inside the site",
                            );
                        }
                        Some(crate::filesystem::DeclaredOutputViolation::InsidePublished) => {
                            diagnostics.error(
                                outputs,
                                format!(
                                    "hook output `{shown}` intersects build.publish-dir `{}`",
                                    tola_diagnostic::display_path(build_output, root)
                                ),
                            );
                        }
                        Some(crate::filesystem::DeclaredOutputViolation::InsideReserved {
                            kind,
                            path,
                        }) => {
                            diagnostics.error(
                                outputs,
                                format!(
                                    "hook output `{shown}` intersects {kind} `{}`",
                                    tola_diagnostic::display_path(&path, root)
                                ),
                            );
                        }
                        Some(crate::filesystem::DeclaredOutputViolation::OwnedElsewhere(
                            claimed,
                        )) => {
                            let (owner, existing, _) = &granted[claimed];
                            diagnostics.error(
                                outputs,
                                format!(
                                    "hook output `{shown}` from `{}` conflicts with `{}` from `{}`",
                                    hook.name,
                                    tola_diagnostic::display_path(existing, root),
                                    owner.name
                                ),
                            );
                        }
                    }
                    granted.push((hook, root.join(output), resolved));
                }
            },
        );
    }
}

/// Module directories and archive files that publication must not replace.
pub(super) fn module_input_paths(schema: &SiteConfigSchema, root: &Path) -> Vec<PathBuf> {
    schema
        .modules
        .iter()
        .flat_map(|(name, module)| {
            let mut paths = match &module.source {
                tola_module::ModuleSource::Local { path } => vec![root.join(path)],
                tola_module::ModuleSource::Git { .. }
                | tola_module::ModuleSource::Tarball { .. } => {
                    let vendor = schema.vendor.path.as_deref().unwrap_or(Path::new("vendor"));
                    vec![
                        tola_input::layout::modules(root),
                        root.join(vendor).join("modules").join(name),
                    ]
                }
            };
            if let tola_module::ModuleSource::Tarball { tarball } = &module.source
                && let tola_input::TarballLocation::File { path } = &tarball.location
            {
                paths.push(root.join(path));
            }
            paths
        })
        .collect()
}

/// Identify a module root a before-build hook cannot write to.
pub(crate) fn overlapping_module_root<'a>(
    root: &Path,
    modules: &std::collections::BTreeMap<String, tola_module::ModuleDeclaration>,
    module_roots: &'a [PathBuf],
    target: &tola_input::FilesystemSourceIdentity,
    author_hook: bool,
) -> Option<&'a Path> {
    use tola_input::FilesystemSourceIdentity;
    use tola_module::declaration::ModuleSource;

    let site = FilesystemSourceIdentity::from_path(root);
    module_roots.iter().find_map(|module| {
        let identity = FilesystemSourceIdentity::from_path(module);
        if !target.intersects(&identity) {
            return None;
        }
        let writable = author_hook
            && identity.is_within(&site)
            && target.is_within(&identity)
            && target.physical_path() != identity.physical_path()
            && target.logical_path() != identity.logical_path()
            && !target.intersects_path(&module.join("tola-module.toml"))
            && modules
                .values()
                .any(|declaration| match &declaration.source {
                    ModuleSource::Local { path } => {
                        identity.logical_path()
                            == tola_input::lexical_path_identity(&root.join(path))
                    }
                    ModuleSource::Git { .. } | ModuleSource::Tarball { .. } => false,
                });
        (!writable).then_some(module.as_path())
    })
}

/// The output root and the protected inputs it must not contain.
pub(super) fn check_publish_dir(
    schema: &SiteConfigSchema,
    config_path: &Path,
    root: &Path,
    package_locations: &tola_typst::PackageLocations,
    module_roots: &[PathBuf],
    vendor_workspace: Option<&Path>,
) -> Result<crate::filesystem::OutputBoundary, crate::filesystem::OutputBoundaryError> {
    crate::filesystem::OutputBoundary::resolve(
        root,
        &schema.build.publish_dir,
        super::resolved::protected_input_paths(
            config_path,
            std::iter::once(("content-dir", schema.build.content_dir.as_path())).chain(
                schema
                    .build
                    .entry
                    .project_path()
                    .map(|path| ("build entry", path)),
            ),
            &schema.assets,
            &schema.typst.fonts,
            &schema.icons,
            &schema.vendor,
            package_locations,
        )
        .into_iter()
        .chain(
            module_roots
                .iter()
                .map(|path| ("module root", path.as_path())),
        )
        .chain(vendor_workspace.map(|path| ("vendor working directory", path))),
    )
}

pub(super) fn validate_site_paths(
    config: &SiteConfigSchema,
    root: &Path,
    config_path: &Path,
    vendor_workspace: Option<&Path>,
    module_roots: &[PathBuf],
    diagnostics: &mut ConfigDiagnostics,
) {
    config
        .vendor
        .validate_internal_boundaries(root, diagnostics);
    config.icons.validate_internal_boundaries(root, diagnostics);
    let boundary = crate::resources::source_boundary_with_exclusions(
        root,
        tola_input::InputScope::Online,
        super::resolved::generated_exclusions(&config.build.publish_dir, vendor_workspace, None),
    );
    for (field, path) in [
        ("build.entry", config.build.entry.project_path()),
        (
            "build.content-dir",
            Some(config.build.content_dir.as_path()),
        ),
    ] {
        if let Some(path) = path
            && let Err(error) = boundary.check(path)
        {
            diagnostics.error(FieldPath::new(field), error.to_string());
        }
    }
    for (index, path) in config.typst.fonts.paths.iter().enumerate() {
        diagnostics.with_array_element("typst.fonts.paths", index, |diagnostics| {
            if let Err(error) = boundary.check(path) {
                diagnostics.error(FieldPath::new("typst.fonts.paths"), error.to_string());
            }
        });
    }
    validate_hook_output_boundaries(
        &config.hooks,
        root,
        config_path,
        &config.build.publish_dir,
        config.vendor.path.as_deref(),
        vendor_workspace,
        diagnostics,
    );
    for (index, hook) in config.hooks.before_build.iter().enumerate() {
        if !hook.enable {
            continue;
        }
        for output in &hook.generates {
            let target = tola_input::FilesystemSourceIdentity::from_path(&root.join(output));
            if let Some(module) =
                overlapping_module_root(root, &config.modules, module_roots, &target, true)
            {
                diagnostics.with_array_element("hooks.before-build", index, |diagnostics| {
                    diagnostics.error(
                        FieldPath::new("hooks.before-build.generates"),
                        format!(
                            "hook output `{}` overlaps module root `{}`",
                            tola_diagnostic::display_path(&root.join(output), root),
                            tola_diagnostic::display_path(module, root)
                        ),
                    );
                });
            }
        }
    }
    validate_asset_source_boundaries(
        &config.assets,
        root,
        &config.build.content_dir,
        &config.build.publish_dir,
        diagnostics,
    );
}
