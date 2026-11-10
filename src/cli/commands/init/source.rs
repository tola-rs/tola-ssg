//! Run a source-owned initializer in a disposable candidate directory.

use anyhow::{Context, Result, ensure};
use std::fs;
use std::path::{Component, Path, PathBuf};
use tola_input::InputScope;
use tola_subprocess::Command;

use crate::cancellation::Cancellation;
use crate::cli::args::InitArgs;
use crate::writes::FileWrites;

pub(super) fn generate(
    source: &Path,
    args: &InitArgs,
    target: &Path,
    scope: InputScope,
    _output: &crate::cli::output::CommandOutput,
    cancellation: &Cancellation,
) -> Result<FileWrites> {
    let initializer = tola_build::modules::find_initializer(
        source,
        args.alias.as_deref(),
        tola_input::InputMode::new(scope, true),
    )?;
    crate::cli::commands::extensions::check_namespaces(&std::collections::BTreeMap::from([(
        initializer.name.clone(),
        (),
    )]))?;
    ensure!(
        scope != InputScope::Pure || initializer.root.starts_with(target),
        "the initializer module is outside the site `tola init` is creating, which `--pure` does not allow"
    );
    let temporary =
        tempfile::tempdir().context("Tola could not create its initializer workspace")?;
    let workspace = temporary.path().canonicalize()?;
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
    let schema = crate::config::validate_settings(&target.join("tola.toml"), text, scope)?;
    crate::cli::commands::extensions::check_namespaces(&schema.modules)?;
    ensure!(
        !writes
            .file_paths()
            .any(|path| path.starts_with(&initializer.root)),
        "initializer must not write into its source module"
    );
    super::validate::scaffold(&writes, args.force)?;

    // Existing author inputs and proposed files form one disposable validation
    // tree. Only `writes`, collected above, can be committed to the target.
    let validation = workspace.join("validation");
    fs::create_dir(&validation)?;
    if target.exists() {
        snapshot(target, target, &validation, &validation, cancellation)?;
    }
    let mut proposed = FileWrites::new(&validation)?;
    for path in writes.directory_paths() {
        proposed.add_directory(writes.relative_path(path))?;
    }
    for path in writes.file_paths() {
        let relative = writes.relative_path(path);
        proposed.create_file(
            relative,
            writes.file_contents(relative).expect("planned file exists"),
        )?;
    }
    proposed.apply(&cancellation.token())?;

    let source =
        tola_build::config::ConfigSource::parse(&validation.join("tola.toml"), text, scope)?;
    let (parsed, _) = source.decode_with_host::<crate::config::ConfigSchema>()?;
    let mut module_paths = std::collections::BTreeMap::new();
    for (name, module) in &schema.modules {
        let tola_module::ModuleSource::Local { path } = &module.source else {
            anyhow::bail!(
                "remote modules are not supported during `tola init`; add them after creating the site"
            )
        };
        let final_path = resolve_module_path(target, &validation, path)
            .with_context(|| format!("Tola could not read the directory of module `{name}`"))?;
        ensure!(
            scope != InputScope::Pure || final_path.starts_with(target),
            "module `{name}` is outside the site `tola init` is creating, which `--pure` does not allow"
        );
        let path = final_path
            .strip_prefix(target)
            .map(|relative| validation.join(relative))
            .unwrap_or(final_path);
        module_paths.insert(name.clone(), path);
    }
    let selected = schema
        .modules
        .get(&initializer.name)
        .context("initializer must register its source module under the selected alias")?;
    let tola_module::ModuleSource::Local { path } = &selected.source else {
        anyhow::bail!("remote modules are not supported during `tola init`")
    };
    ensure!(
        resolve_module_path(target, &validation, path)? == initializer.root,
        "initializer module path must refer to the source module from the final target directory"
    );
    let parsed = parsed.map_schema(|mut schema| {
        for (name, path) in module_paths {
            schema
                .modules
                .get_mut(&name)
                .expect("parsed module exists")
                .source = tola_module::ModuleSource::Local { path };
        }
        schema
    });
    let loaded = tola_build::config::loading::resolve_parsed_site_config(
        parsed,
        crate::cli::config::package_locations(&Default::default(), scope)?,
        &Default::default(),
    )?
    .into_config();
    tola_build::modules::prepare_site(
        &loaded,
        tola_input::InputMode::new(scope, true),
        &cancellation.token(),
    )?;
    Ok(writes)
}

/// Walk the final declaration using candidate inputs inside the target and real
/// inputs outside it. Only the target itself may be absent from the real tree.
fn resolve_module_path(target: &Path, validation: &Path, declared: &Path) -> Result<PathBuf> {
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
        fs::metadata(actual(&path))?.is_dir(),
        "module path is not a directory: {}",
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
        let output = destination.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            fs::create_dir(&output)?;
            snapshot(source_root, &path, destination_root, &output, cancellation)?;
        } else if kind.is_file() {
            fs::copy(&path, &output)?;
        } else if kind.is_symlink() {
            let original = source.join(fs::read_link(&path)?);
            let physical = path.canonicalize().unwrap_or_else(|_| original.clone());
            // Keep unresolved components intact: `file/../x` and `missing/../x`
            // must not become valid merely because this is a validation copy.
            let internal = tola_input::lexical_path_identity(&physical).starts_with(source_root);
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
            resolve_module_path(&target, &validation, Path::new("link/../module")).unwrap(),
            target.join("actual/module")
        );
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
