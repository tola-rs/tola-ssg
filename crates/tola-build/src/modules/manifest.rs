//! Build-side manifest wrappers: read module manifests for commands and configuration.

use std::collections::BTreeMap;
use std::path::Path;

use tola_input::{InputMode, InputScope};
use tola_module::declaration::{ModuleDeclaration, ModuleSource};
use tola_module::manifest::{
    InitializerSource, ManifestError, ModuleDeclarations, NuModule, read_one, read_selected,
    read_with_options,
};

/// Read the Nu entry of every declared module.
pub fn read_nu_modules(
    site_root: &Path,
    vendor: Option<&Path>,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    mode: InputMode,
    cancellation: &crate::cancellation::BuildCancellation,
) -> std::result::Result<BTreeMap<String, NuModule>, ManifestError> {
    let roots = crate::inputs::module_roots(site_root, mode, vendor, declarations, cancellation)?;
    read_selected(site_root, declarations, &roots, mode, &[])?
        .into_iter()
        .map(|(name, loaded)| {
            let assets_enabled = loaded.manifest.assets.is_some()
                && declarations[&name]
                    .assets
                    .as_ref()
                    .is_none_or(|assets| assets.enable);
            let entry = loaded.manifest.nu.map(|nu| nu.entry);
            let font_directories = loaded
                .manifest
                .typst
                .map_or_else(Vec::new, |typst| typst.fonts);
            if let Some(entry) = &entry {
                tola_module::module_set::validate_module_path(
                    site_root,
                    &name,
                    &loaded.root,
                    entry,
                    false,
                )?;
            }
            let id = tola_module::ModuleId::parse(&name)
                .map_err(|_| ManifestError::Namespace { name: name.clone() })?;
            let root = loaded.root;
            Ok((
                name,
                NuModule {
                    name: id,
                    manifest: loaded.path,
                    root,
                    entry,
                    font_directories,
                    assets_enabled,
                    options: loaded.options,
                },
            ))
        })
        .collect()
}

/// Select the same fonts as the compiler using the manifests already read for Nu.
pub fn nu_font_options(
    config: &crate::config::SiteConfig,
    resources: &crate::BuildResources,
    modules: &BTreeMap<String, NuModule>,
) -> anyhow::Result<tola_typst::FontOptions> {
    let mut config = config.clone();
    super::site::select_module_fonts(
        &mut config,
        resources.input_scope(),
        modules.iter().map(|(alias, module)| {
            (
                alias.as_str(),
                module.root.as_path(),
                module.font_directories.as_slice(),
            )
        }),
    )?;
    Ok(config.compiler_font_options(resources))
}

/// Read an initializer declaration before a destination site exists.
pub fn find_initializer(
    source: &Path,
    alias: Option<&str>,
    mode: InputMode,
) -> std::result::Result<InitializerSource, ManifestError> {
    let root = source
        .canonicalize()
        .map_err(|source| ManifestError::InitializerRoot { source })?;
    let alias = alias.ok_or(ManifestError::InitializerName)?;
    let loaded = read_one(&root, alias, root.clone(), mode, &[])?;
    let initializer = loaded
        .manifest
        .init
        .ok_or(ManifestError::InitializerMissing)?;
    Ok(InitializerSource {
        name: alias.to_owned(),
        root: loaded.root,
        manifest: loaded.path,
        action: initializer.action,
    })
}

/// Discover commands, queries, and Nu entries from module directories already on disk.
pub fn read_available_declarations(
    site_root: &Path,
    vendor: Option<&Path>,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    scope: InputScope,
) -> std::io::Result<BTreeMap<String, ModuleDeclarations>> {
    let site = site_root.canonicalize()?;
    let mode = InputMode::new(scope, true);
    let vendor_modules = vendor.map(|dir| dir.join("modules"));
    let mut result = BTreeMap::new();
    for (name, declaration) in declarations {
        let Ok(root) = crate::inputs::declaration_root(&site, vendor, name, &declaration.source)
        else {
            continue;
        };
        if scope == InputScope::Pure
            && !matches!(
                &declaration.source,
                ModuleSource::Local { .. } | ModuleSource::Builtin { .. }
            )
            && !vendor_modules
                .as_ref()
                .is_some_and(|dir| root.starts_with(dir))
        {
            continue;
        }
        let Ok(loaded) = read_with_options(&site, name, root, mode, &[], &declaration.options)
        else {
            continue;
        };
        let manifest = loaded.manifest;
        let mut commands: BTreeMap<_, _> = manifest
            .commands
            .into_iter()
            .map(|(name, command)| (name, command.help))
            .collect();
        if manifest.init.is_some() {
            commands.insert("init".into(), "Create a site using this module.".into());
        }
        result.insert(
            name.clone(),
            ModuleDeclarations {
                commands,
                queries: manifest.queries.into_keys().collect(),
                nu_entry: manifest.nu.map(|nu| nu.entry),
            },
        );
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn nu_selection_prepares_no_site_inputs() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("tools.nu"),
            "export def main [] { 42 }\n",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("tola-module.toml"),
            r#"
[nu]
entry = "tools.nu"
[typst]
entry = "not-generated.typ"
[assets]
trees = [{ source = "not-generated-assets", url-prefix = "/" }]
"#,
        )
        .unwrap();
        let declared = BTreeMap::from([(
            "local".into(),
            ModuleDeclaration {
                source: ModuleSource::Local { path: ".".into() },
                ..Default::default()
            },
        )]);
        let loaded = read_nu_modules(
            directory.path(),
            None,
            &declared,
            InputMode::new(InputScope::Pure, false),
            &Default::default(),
        )
        .unwrap();
        assert_eq!(
            loaded["local"].entry.as_deref(),
            Some(Path::new("tools.nu"))
        );
    }

    #[test]
    fn nu_must_stay_within_selected_module() {
        let directory = tempfile::tempdir().unwrap();
        let module = directory.path().join("module");
        std::fs::create_dir(&module).unwrap();
        std::fs::write(
            directory.path().join("outside.nu"),
            "export def main [] {}\n",
        )
        .unwrap();
        let declared = BTreeMap::from([(
            "local".into(),
            ModuleDeclaration {
                source: ModuleSource::Local {
                    path: "module".into(),
                },
                ..Default::default()
            },
        )]);
        for entry in ["", "../outside.nu"] {
            std::fs::write(
                module.join("tola-module.toml"),
                format!("[nu]\nentry={entry:?}\n"),
            )
            .unwrap();
            assert!(
                read_nu_modules(
                    directory.path(),
                    None,
                    &declared,
                    InputMode::new(InputScope::Pure, false),
                    &Default::default()
                )
                .is_err()
            );
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("../outside.nu", module.join("linked.nu")).unwrap();
            std::fs::write(module.join("tola-module.toml"), "[nu]\nentry='linked.nu'\n").unwrap();
            assert!(
                read_nu_modules(
                    directory.path(),
                    None,
                    &declared,
                    InputMode::new(InputScope::Pure, false),
                    &Default::default()
                )
                .is_err()
            );
        }
    }

    #[test]
    fn initializer_discovery_loads_nothing() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("tola-module.toml"),
            r#"
[build]
entry = "not-created-yet.typ"
[init]
argv = ["not-installed", "{module}/init.nu"]
"#,
        )
        .unwrap();
        assert!(matches!(
            find_initializer(
                directory.path(),
                None,
                InputMode::new(InputScope::Pure, false),
            ),
            Err(ManifestError::InitializerName),
        ));
        let initializer = find_initializer(
            directory.path(),
            Some("notes"),
            InputMode::new(InputScope::Pure, false),
        )
        .unwrap();
        assert_eq!(initializer.name, "notes");
        assert_eq!(
            initializer.action,
            tola_module::Action::Argv(vec!["not-installed".into(), "{module}/init.nu".into()]),
        );
        let declarations = BTreeMap::from([(
            "notes".into(),
            ModuleDeclaration {
                source: ModuleSource::Local {
                    path: PathBuf::from("."),
                },
                ..Default::default()
            },
        )]);
        let help =
            read_available_declarations(directory.path(), None, &declarations, InputScope::Pure)
                .unwrap();
        assert!(help["notes"].commands.contains_key("init"));
    }
}
