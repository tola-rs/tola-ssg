//! Run a source-owned initializer in a disposable candidate directory.

use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use tola_input::{InputLock, InputMode, InputScope, LockedModule, TarballLocation};
use tola_module::{ModuleDeclaration, ModuleSource};
use tola_subprocess::Command;

use crate::cancellation::Cancellation;
use crate::cli::args::InitArgs;
use crate::writes::FileWrites;

pub(super) struct Generated {
    pub(super) writes: FileWrites,
    _workspace: tempfile::TempDir,
    caches: Vec<(PathBuf, PathBuf)>,
}

impl Generated {
    pub(super) fn install_cache(
        &self,
        target: &Path,
        output: &crate::cli::output::CommandOutput,
        cancellation: &Cancellation,
    ) -> Result<()> {
        for (source, relative) in &self.caches {
            let destination = target.join(relative);
            let installed = (|| -> Result<()> {
                if destination.is_dir() {
                    return Ok(());
                }
                fs::create_dir_all(destination.parent().expect("cache directory has a parent"))?;
                match fs::rename(source, &destination) {
                    Ok(()) => return Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {}
                    Err(error) => return Err(error.into()),
                }
                let stage = tola_fs::DirectoryReplacement::new(&destination)?;
                fs::create_dir(stage.staged())?;
                copy_module(source, &stage.staged(), cancellation)?;
                stage.publish()?;
                Ok(())
            })();
            if let Err(error) = installed {
                tracing::warn!(%error, "could not retain the initializer module cache");
                output.status(
                    "The site is ready; a later build will fetch its locked modules again",
                )?;
                break;
            }
        }
        Ok(())
    }
}

pub(super) fn generate(
    input: &super::Source,
    args: &InitArgs,
    target: &Path,
    mode: InputMode,
    cancellation: &Cancellation,
) -> Result<Generated> {
    let scope = mode.scope();
    let alias = args
        .alias
        .as_deref()
        .context("choose an initializer module alias with --alias")?;
    crate::cli::commands::extensions::check_namespaces(&BTreeMap::from([(alias.to_owned(), ())]))?;
    ensure!(
        tola_module::declaration::valid_name(alias),
        "invalid initializer module alias"
    );
    let temporary =
        tempfile::tempdir().context("Tola could not create its initializer workspace")?;
    let workspace = temporary.path().canonicalize()?;
    let validation = workspace.join("validation");
    fs::create_dir(&validation)?;
    if target.exists() {
        snapshot(target, target, &validation, &validation, cancellation)?;
    }
    let source = relocate_source(&input.declaration, target, &validation, scope)?;
    let mut lock = InputLock::read(&validation)?.unwrap_or_default();
    if let Some(LockedModule::Tarball { tarball, .. }) = lock.modules.get_mut(alias)
        && let ModuleSource::Tarball { tarball: original } = &input.declaration
        && *tarball == original.location
        && let ModuleSource::Tarball { tarball: relocated } = &source
    {
        *tarball = relocated.location.clone();
    }
    if let Some((directory, pinned)) = &input.cached {
        let mut pinned = pinned.clone();
        set_lock_source(&mut pinned, &source);
        ensure!(
            mode.may_write_lock() || lock.modules.get(alias) == Some(&pinned),
            "`--locked` initialization needs the selected module in the destination's `tola.lock`"
        );
        lock.modules.insert(alias.into(), pinned);
        fs::write(InputLock::path(&validation), lock.to_toml()?)?;
        let cached = tola_module::inputs::declaration_root(&validation, None, alias, &source)?;
        if !cached.exists() {
            fs::create_dir_all(&cached)?;
            copy_module(directory, &cached, cancellation)?;
        }
    }
    let module_root = match &source {
        ModuleSource::Local { .. } => {
            let ModuleSource::Local { path } = &input.declaration else {
                unreachable!()
            };
            path.clone()
        }
        _ => {
            let declarations = BTreeMap::from([(
                alias.to_owned(),
                ModuleDeclaration {
                    source: source.clone(),
                    ..Default::default()
                },
            )]);
            let roots = tola_module::inputs::record_module_roots(
                &validation,
                mode,
                None,
                &declarations,
                &mut lock,
                &tola_typst::BundleCancellation::from_flag(cancellation.token().signal_flag()),
            )?;
            roots[alias].clone()
        }
    };
    let pinned = lock.modules.get(alias).cloned();
    let initializer = tola_build::modules::find_initializer(&module_root, Some(alias), mode)?;
    let candidate = workspace.join("site");
    fs::create_dir(&candidate)?;
    match &initializer.action {
        tola_module::Action::Nu { path, args: prefix } => {
            let path = tola_module::module_set::validate_module_path(
                &initializer.root,
                &initializer.name,
                &initializer.root,
                path,
                false,
            )?;
            let mut arguments = prefix.clone();
            arguments.extend(super::super::nu::utf8_arguments(&args.args)?);
            let context = tola_nu::InvocationContext {
                directory: candidate.clone(),
                environment: super::super::nu::environment(scope),
            };
            let runtime = tola_nu::Runtime::for_invocation(Vec::new(), vec![initializer.manifest.clone()], context)?
                .with_host(tola_nu::Host {
                    init: Some(serde_json::json!({
                        "target-dir": target, "output-dir": candidate, "interactive": args.interactive,
                        "module": { "name": initializer.name, "root": initializer.root, "manifest": initializer.manifest },
                    })),
                    ..Default::default()
                })?
                .with_cancellation(cancellation.token().signal_flag());
            let status = runtime.run_embedded(&path, &arguments)?;
            cancellation.token().ensure_active()?;
            ensure!(
                status == 0,
                "module initializer exited with status {status}"
            );
        }
        tola_module::Action::Argv(argv) => {
            ensure!(
                !argv.iter().any(|word| word.contains("{project}")),
                "initializer argv cannot use `{}` before a site exists; use TOLA_INIT_TARGET_DIR",
                "{project}"
            );
            let executable = crate::cli::external::current_executable()?;
            let expand = |word: &str| {
                tola_module::expand_argument(
                    word,
                    &[
                        ("{module}", initializer.root.as_os_str()),
                        ("{tola}", executable.as_os_str()),
                        ("{name}", std::ffi::OsStr::new(&initializer.name)),
                    ],
                )
            };
            let command = Command::new(expand(&argv[0]))
                .args(argv[1..].iter().map(|word| expand(word)))
                .args(args.args.iter().cloned())
                .cwd(&candidate)
                .env("TOLA_INIT_OUTPUT_DIR", candidate.as_os_str())
                .env("TOLA_INIT_TARGET_DIR", target.as_os_str())
                .env(
                    "TOLA_INIT_INTERACTIVE",
                    if args.interactive { "true" } else { "false" },
                )
                .env("TOLA_MODULE_NAME", initializer.name.as_str())
                .env("TOLA_MODULE_ROOT", initializer.root.as_os_str())
                .env("TOLA_MODULE_MANIFEST", initializer.manifest.as_os_str())
                .env("TOLA_EXECUTABLE", executable)
                .env("TOLA_INPUT_SCOPE", scope.as_str())
                .env_remove("TOLA_PROJECT_ROOT")
                .env_remove("TOLA_CONFIG_PATH")
                .env_remove("TOLA_CONTENT_ROOT")
                .env_remove("TOLA_MODULE_OPTIONS")
                .env_remove("TOLA_HOOK_STAGE")
                .env_remove("TOLA_HOOK_NAME")
                .env_remove("TOLA_REPORT_PATH")
                .env_remove("TOLA_BUILD_MODE")
                .env_remove("TOLA_HOOK_INPUT_DIR")
                .env_remove("TOLA_HOOK_OUTPUT_DIR")
                .env_remove("TOLA_HOOK_TEMP_DIR")
                .env_remove("TOLA_HOOK_CACHE_DIR");
            crate::cli::external::run(command, cancellation)?;
        }
    }
    cancellation.token().ensure_active()?;

    let mut writes = FileWrites::new(target)?;
    collect(&candidate, &candidate, &mut writes, cancellation)?;
    let config = writes
        .file_contents(Path::new(super::config::CONFIG_PATH))
        .context("the module initializer did not write `tola.toml` to its output directory")?;
    let text = std::str::from_utf8(config)
        .context("the initializer wrote a `tola.toml` that is not UTF-8")?;
    let text = register_source(text, alias, &input.declaration)?;
    let schema = crate::config::validate_settings(&target.join("tola.toml"), &text, scope)?;
    crate::cli::commands::extensions::check_namespaces(&schema.modules)?;
    ensure!(
        !writes
            .file_paths()
            .any(|path| path.starts_with(&initializer.root)),
        "initializer must not write into its source module"
    );
    super::validate::scaffold(&writes, args.force)?;

    let mut proposed = FileWrites::new(&validation)?;
    for path in writes.directory_paths() {
        proposed.add_directory(writes.relative_path(path))?;
    }
    for path in writes.file_paths() {
        let relative = writes.relative_path(path);
        let contents = if relative == Path::new("tola.toml") {
            text.as_bytes()
        } else {
            writes.file_contents(relative).expect("planned file exists")
        };
        proposed.create_file(relative, contents)?;
    }
    proposed.apply(&cancellation.token())?;

    let source =
        tola_build::config::ConfigSource::parse(&validation.join("tola.toml"), &text, scope)?;
    let (parsed, _) = source.decode_with_host::<crate::config::ConfigSchema>()?;
    let mut module_sources = BTreeMap::new();
    for (name, module) in &schema.modules {
        module_sources.insert(
            name.clone(),
            relocate_source(&module.source, target, &validation, scope)?,
        );
    }
    let mut staged_lock = InputLock::read(&validation)?.unwrap_or_default();
    let before = staged_lock.clone();
    if let Some(pinned) = pinned {
        staged_lock.modules.insert(alias.to_owned(), pinned);
    }
    for (name, source) in &module_sources {
        if let Some(entry) = staged_lock.modules.get_mut(name) {
            let matches = match (&*entry, &schema.modules[name].source) {
                (
                    LockedModule::Tarball { tarball, .. },
                    ModuleSource::Tarball { tarball: declared },
                ) => tarball == &declared.location,
                _ => true,
            };
            if matches {
                set_lock_source(entry, source);
            }
        }
    }
    if staged_lock != before {
        staged_lock.commit(&validation, &before)?;
    }
    let parsed = parsed.map_schema(|mut schema| {
        for (name, source) in module_sources {
            schema
                .modules
                .get_mut(&name)
                .expect("parsed module exists")
                .source = source;
        }
        schema
    });
    let loaded = tola_build::config::loading::resolve_parsed_site_config(
        parsed,
        crate::cli::config::package_locations(&Default::default(), scope)?,
        &Default::default(),
    )?
    .into_config();
    let prepared = tola_build::modules::prepare_site(&loaded, mode, &cancellation.token())?;
    let caches = prepared
        .loaded_modules()
        .filter(|(_, module)| {
            module
                .root
                .starts_with(tola_input::layout::modules(&validation))
        })
        .map(|(_, module)| {
            (
                module.root.clone(),
                module
                    .root
                    .strip_prefix(&validation)
                    .expect("module cache is inside validation")
                    .to_path_buf(),
            )
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut final_lock = InputLock::read(&validation)?.unwrap_or(staged_lock);
    for (name, entry) in &mut final_lock.modules {
        if let Some(module) = schema.modules.get(name) {
            set_lock_source(entry, &module.source);
        }
    }
    let mut result = FileWrites::new(target)?;
    for path in writes.directory_paths() {
        result.add_directory(writes.relative_path(path))?;
    }
    for path in writes.file_paths() {
        let relative = writes.relative_path(path);
        if relative == Path::new(tola_input::LOCK_FILE) && mode.may_write_lock() {
            continue;
        }
        let contents = if relative == Path::new("tola.toml") {
            text.as_bytes()
        } else {
            writes.file_contents(relative).expect("planned file exists")
        };
        result.create_file(relative, contents)?;
    }
    if mode.may_write_lock() {
        result.create_file(tola_input::LOCK_FILE, final_lock.to_toml()?)?;
    }
    Ok(Generated {
        writes: result,
        _workspace: temporary,
        caches,
    })
}

/// Preserve the selected initializer's declared source in the generated configuration.
fn register_source(text: &str, alias: &str, source: &ModuleSource) -> Result<String> {
    let mut document: toml_edit::DocumentMut =
        text.parse().context("initializer wrote invalid TOML")?;
    let modules = document
        .entry("modules")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_like_mut()
        .context("initializer configuration needs a modules table")?;
    let module = modules
        .entry(alias)
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_like_mut()
        .context("initializer module must be a table")?;
    for key in ["path", "git", "tarball"] {
        module.remove(key);
    }
    let fields = toml_edit::ser::to_document(source)?;
    for (key, value) in fields.iter() {
        module.insert(key, value.clone());
    }
    Ok(document.to_string())
}

/// Relocate site-owned inputs into the validation snapshot; retain outside local inputs.
fn relocate_source(
    source: &ModuleSource,
    target: &Path,
    validation: &Path,
    scope: InputScope,
) -> Result<ModuleSource> {
    let mut source = source.clone();
    let (path, directory) = match &mut source {
        ModuleSource::Local { path } => (path, true),
        ModuleSource::Tarball { tarball } => match &mut tarball.location {
            TarballLocation::File { path } => (path, false),
            TarballLocation::Url { .. } => return Ok(source),
        },
        ModuleSource::Git { .. } => return Ok(source),
    };
    ensure!(
        !tola_input::normalize_existing_prefix(&target.join(&*path))
            .starts_with(validation.parent().expect("validation has a workspace")),
        "initializer module paths must refer to the final site, not temporary output; use `get-init`'s target-dir"
    );
    let final_path = if !directory && !target.join(&*path).exists() {
        tola_input::normalize_existing_prefix(&target.join(&*path))
    } else {
        resolve_input_path(target, validation, path, directory)?
    };
    ensure!(
        scope != InputScope::Pure || final_path.starts_with(target),
        "an initializer input is outside the site, which --pure does not allow"
    );
    *path = final_path
        .strip_prefix(target)
        .map(|relative| validation.join(relative))
        .unwrap_or(final_path);
    Ok(source)
}

fn set_lock_source(entry: &mut LockedModule, source: &ModuleSource) {
    match (entry, source) {
        (LockedModule::Local { path, .. }, ModuleSource::Local { path: declared }) => {
            *path = declared.clone()
        }
        (LockedModule::Tarball { tarball, .. }, ModuleSource::Tarball { tarball: declared }) => {
            *tarball = declared.location.clone()
        }
        _ => {}
    }
}

/// Copy an already-selected cache while preserving Git's links and executable permissions.
fn copy_module(source: &Path, destination: &Path, cancellation: &Cancellation) -> Result<()> {
    for entry in fs::read_dir(source)? {
        cancellation.token().ensure_active()?;
        let entry = entry?;
        let path = entry.path();
        let output = destination.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            fs::create_dir(&output)?;
            copy_module(&path, &output, cancellation)?;
        } else if kind.is_symlink() {
            let link = fs::read_link(&path)?;
            if path.is_dir() {
                tola_fs::link_directory(&link, &output)?;
            } else {
                tola_fs::link_file(&link, &output)?;
            }
        } else {
            ensure!(kind.is_file(), "module cache contains a special file");
            fs::copy(path, output)?;
        }
    }
    Ok(())
}

/// Walk the final declaration using candidate inputs inside the target and real
/// inputs outside it. Only the target itself may be absent from the real tree.
fn resolve_input_path(
    target: &Path,
    validation: &Path,
    declared: &Path,
    directory: bool,
) -> Result<PathBuf> {
    let actual = |path: &Path| {
        path.strip_prefix(target)
            .map(|relative| validation.join(relative))
            .unwrap_or_else(|_| path.to_owned())
    };
    let mut path = if declared.is_absolute() {
        PathBuf::new()
    } else {
        target.to_owned()
    };
    for component in declared.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                path.push(component.as_os_str());
                continue;
            }
            _ => {
                ensure!(
                    fs::metadata(actual(&path))?.is_dir(),
                    "module path traverses a non-directory: {}",
                    path.display()
                );
            }
        }
        match component {
            Component::ParentDir => {
                path.pop();
            }
            Component::CurDir => {}
            component => path.push(component.as_os_str()),
        }
        let physical = actual(&path).canonicalize()?;
        path = physical
            .strip_prefix(validation)
            .map(|relative| target.join(relative))
            .unwrap_or(physical);
    }
    ensure!(
        if directory {
            fs::metadata(actual(&path))?.is_dir()
        } else {
            fs::metadata(actual(&path))?.is_file()
        },
        "initializer input has the wrong file type: {}",
        path.display()
    );
    Ok(path)
}

fn snapshot(
    source_root: &Path,
    source: &Path,
    destination_root: &Path,
    destination: &Path,
    cancellation: &Cancellation,
) -> Result<()> {
    for entry in fs::read_dir(source)? {
        cancellation.token().ensure_active()?;
        let entry = entry?;
        let path = entry.path();
        if destination_root.starts_with(&path) {
            continue;
        }
        let output = destination.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            fs::create_dir(&output)?;
            snapshot(source_root, &path, destination_root, &output, cancellation)?;
        } else if kind.is_file() {
            fs::copy(&path, &output)?;
        } else if kind.is_symlink() {
            let relative = path.strip_prefix(source_root)?;
            if relative == Path::new(tola_input::LOCK_FILE) {
                fs::copy(&path, &output)?;
                continue;
            }
            let original = source.join(fs::read_link(&path)?);
            let physical = path.canonicalize().unwrap_or_else(|_| original.clone());
            // Keep unresolved components intact: `file/../x` and `missing/../x`
            // must not become valid merely because this is a validation copy.
            let internal = tola_input::lexical_path_identity(&physical).starts_with(source_root);
            ensure!(
                internal || !relative.starts_with(tola_input::INTERNAL_DIR),
                "cannot initialize with a `.tola` link outside the site; move the cache aside first"
            );
            let relocated = if internal {
                physical
                    .strip_prefix(source_root)
                    .map(|relative| destination_root.join(relative))
                    .unwrap_or(physical)
            } else {
                physical
            };
            if path.is_dir() {
                tola_fs::link_directory(&relocated, &output)?;
            } else {
                tola_fs::link_file(&relocated, &output)?;
            }
        } else {
            anyhow::bail!(
                "cannot validate a site containing a special file: {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn collect(
    root: &Path,
    directory: &Path,
    writes: &mut FileWrites,
    cancellation: &Cancellation,
) -> Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        cancellation.token().ensure_active()?;
        let path = entry.path();
        let relative = path.strip_prefix(root)?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            writes.add_directory(relative)?;
            collect(root, &path, writes, cancellation)?;
        } else {
            ensure!(
                kind.is_file(),
                "initializer output must contain only regular files and directories: {}",
                relative.display()
            );
            writes.create_file(relative, fs::read(&path)?)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_source_replaces_temporary_paths_and_keeps_options() {
        let declaration: ModuleDeclaration = toml::from_str(
            "git = { url = 'https://github.com/example/theme', branch = 'preview', subdir = 'theme' }",
        ).unwrap();
        let text = register_source(
            "[modules.theme]\npath = '/temporary/download'\n[modules.theme.options]\ntitle = 'My site'\n",
            "theme", &declaration.source,
        ).unwrap();
        let value: toml::Value = toml::from_str(&text).unwrap();
        let module = &value["modules"]["theme"];
        assert!(module.get("path").is_none());
        assert_eq!(module["options"]["title"].as_str(), Some("My site"));
        let selected: ModuleDeclaration = module.clone().try_into().unwrap();
        assert_eq!(selected.source, declaration.source);
    }

    #[cfg(unix)]
    #[test]
    fn validation_keeps_external_state_read_only() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let site = root.join("site");
        let validation = root.join("validation");
        let external = root.join("external");
        for path in [&site, &validation, &external] {
            fs::create_dir(path).unwrap();
        }
        let lock = external.join("lock");
        fs::write(&lock, "original").unwrap();
        std::os::unix::fs::symlink(&lock, site.join("tola.lock")).unwrap();
        snapshot(
            &site,
            &site,
            &validation,
            &validation,
            &Cancellation::default(),
        )
        .unwrap();
        fs::write(validation.join("tola.lock"), "changed").unwrap();
        assert_eq!(fs::read_to_string(&lock).unwrap(), "original");
        std::os::unix::fs::symlink(&external, site.join(".tola")).unwrap();
        assert!(
            snapshot(
                &site,
                &site,
                &validation,
                &validation,
                &Cancellation::default()
            )
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn existing_resolved_before_parent_components() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let target = root.join("target");
        let validation = root.join("validation");
        fs::create_dir_all(validation.join("actual/child")).unwrap();
        fs::create_dir(validation.join("actual/module")).unwrap();
        std::os::unix::fs::symlink("actual/child", validation.join("link")).unwrap();
        assert_eq!(
            resolve_input_path(&target, &validation, Path::new("link/../module"), true).unwrap(),
            target.join("actual/module")
        );
    }

    #[test]
    fn snapshot_omits_its_own_workspace() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let validation = root.join("workspace/validation");
        fs::create_dir_all(&validation).unwrap();
        fs::write(root.join("input.txt"), "source").unwrap();
        snapshot(
            &root,
            &root,
            &validation,
            &validation,
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(validation.join("input.txt")).unwrap(),
            "source"
        );
        assert!(!validation.join("workspace").exists());
    }

    #[cfg(unix)]
    #[test]
    fn candidate_rejected_before_target_writes() {
        let temporary = tempfile::tempdir().unwrap();
        let candidate = temporary.path().join("candidate");
        fs::create_dir(&candidate).unwrap();
        fs::write(candidate.join("actual"), "value").unwrap();
        std::os::unix::fs::symlink("actual", candidate.join("alias")).unwrap();
        let target = temporary.path().join("target");
        let mut writes = FileWrites::new(&target).unwrap();
        assert!(
            collect(
                &candidate,
                &candidate,
                &mut writes,
                &Cancellation::default()
            )
            .is_err()
        );
        assert!(!target.exists());
    }
}
