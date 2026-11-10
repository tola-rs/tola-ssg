//! Build-side manifest wrappers: read module manifests for commands and configuration.

use std::collections::BTreeMap;
use std::path::Path;

use tola_input::InputMode;
use tola_module::declaration::ModuleDeclaration;
use tola_module::manifest::{
    InitializerSource, ManifestError, ModuleDeclarations, NuModule, read_one, read_selected,
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
    read_selected(site_root, declarations, &roots, mode)?
        .into_iter()
        .map(|(name, loaded)| {
            let assets_enabled = loaded.manifest.assets.is_some()
                && declarations[&name]
                    .assets
                    .as_ref()
                    .is_none_or(|assets| assets.enable);
            let entry = loaded.manifest.nu.map(|nu| nu.entry);
            if let Some(entry) = &entry {
                tola_module::module_set::validate_module_path(
                    site_root,
                    &name,
                    &loaded.root,
                    entry,
                    false,
                )?;
            }
            let root = loaded.root;
            Ok((
                name.clone(),
                NuModule {
                    name,
                    manifest: loaded.path,
                    root,
                    entry,
                    assets_enabled,
                    options: loaded.options,
                },
            ))
        })
        .collect()
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
    let loaded = read_one(&root, alias, root.clone(), mode)?;
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

/// Read only configuration-selected module manifests, without loading their referenced inputs.
pub fn read_declarations(
    site_root: &Path,
    vendor: Option<&Path>,
    declarations: &BTreeMap<String, ModuleDeclaration>,
    mode: InputMode,
    cancellation: &crate::cancellation::BuildCancellation,
) -> std::result::Result<BTreeMap<String, ModuleDeclarations>, ManifestError> {
    let roots = crate::inputs::module_roots(site_root, mode, vendor, declarations, cancellation)?;
    let mut result = BTreeMap::new();
    for (name, loaded) in read_selected(site_root, declarations, &roots, mode)? {
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
    use tola_input::InputScope;
    use tola_module::declaration::ModuleSource;

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
        let help = read_declarations(
            directory.path(),
            None,
            &declarations,
            InputMode::new(InputScope::Pure, false),
            &Default::default(),
        )
        .unwrap();
        assert!(help["notes"].commands.contains_key("init"));
    }
}
