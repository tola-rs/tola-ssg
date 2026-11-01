//! Select a Nu environment without running hooks or compiling the page driver.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail, ensure};
use tola_build::InputScope;
use tola_build::config::ConfigSource;
use tola_build::modules::{NuModule, resolve_nu_modules};
use tola_nu::{ConfiguredModule, InvocationContext, Runtime, SourceOverlay, serve_lsp};

use crate::cli::args::NuArgs;

pub(in crate::cli) fn run(args: &NuArgs, input_scope: Option<&str>) -> ExitCode {
    match execute(args, input_scope) {
        Ok(status) => ExitCode::from(u8::try_from(status).unwrap_or(1)),
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn execute(args: &NuArgs, input_scope: Option<&str>) -> Result<i32> {
    let scope = effective_scope(input_scope)?;
    let current = InvocationContext::current()?;
    let explicit = if args.no_site {
        None
    } else {
        args.config
            .path
            .clone()
            .or_else(|| std::env::var_os("TOLA_CONFIG_PATH").map(PathBuf::from))
            .map(std::path::absolute)
            .transpose()?
    };
    if args.lsp_worker {
        let overlays = read_snapshot(
            args.lsp_snapshot
                .as_deref()
                .context("Nu LSP worker requires a buffer snapshot")?,
        )?;
        let runtime = prepare(
            &current.directory,
            explicit.as_deref(),
            scope,
            None,
            &overlays,
            true,
        )?;
        return Ok(runtime.lsp());
    }
    if args.lsp {
        let executable = crate::cli::external::current_executable()?;
        return Ok(serve_lsp(&executable, |workspace, overlays| {
            prepare(workspace, explicit.as_deref(), scope, None, overlays, true)
        }));
    }
    let entry = args.invocation[0]
        .to_str()
        .context("Nu entry must be valid UTF-8")?;
    if entry.starts_with('@') {
        ensure!(
            entry.starts_with("@tola/") || entry.starts_with("@tola-modules/"),
            "`{entry}` is not a configured Nu module; use `./` for a file whose name begins with `@`"
        );
    }
    let runtime = prepare(
        &current.directory,
        explicit.as_deref(),
        scope,
        Some(entry),
        &[],
        !args.no_site,
    )?;
    let arguments = args.invocation[1..]
        .iter()
        .map(|argument| {
            argument
                .to_str()
                .map(str::to_owned)
                .context("script arguments must be valid UTF-8")
        })
        .collect::<Result<Vec<_>>>()?;
    if entry.starts_with('@') {
        let module = runtime.resolve_locator(entry)?;
        return Ok(runtime.run_module(&module, &arguments, args.stdin, input_scope));
    }
    let module_root = std::env::var_os("TOLA_MODULE_ROOT").map(PathBuf::from);
    let path = file_path(entry, &current.directory, module_root.as_deref());
    Ok(runtime.run_file(&path, &arguments, args.stdin, input_scope))
}

pub(super) fn command_signatures(
    invocations: &[(Vec<std::ffi::OsString>, PathBuf)],
    project: &Path,
    config: &Path,
    scope: InputScope,
) -> Vec<Result<Option<tola_nu::Signature>>> {
    use crate::cli::args::{Cli, Commands};
    use clap::Parser;

    let mut runtimes = BTreeMap::new();
    invocations
        .iter()
        .map(|(argv, module_root)| {
            let cli = Cli::try_parse_from(argv)?;
            let Commands::Nu(args) = cli.command else {
                return Ok(None);
            };
            if args.lsp || args.lsp_worker {
                return Ok(None);
            }
            let entry = args.invocation[0]
                .to_str()
                .context("Nu entry must be valid UTF-8")?;
            let explicit = args
                .config
                .path
                .as_deref()
                .map(|path| {
                    if path.is_absolute() {
                        path.to_owned()
                    } else {
                        project.join(path)
                    }
                })
                .unwrap_or_else(|| config.to_owned());
            let locator = entry
                .starts_with("@tola-modules/")
                .then(|| entry.to_owned());
            let key = (explicit.clone(), args.no_site, locator);
            if !runtimes.contains_key(&key) {
                let runtime = prepare(
                    project,
                    Some(&explicit),
                    scope,
                    Some(entry),
                    &[],
                    !args.no_site,
                )?;
                runtimes.insert(key.clone(), runtime);
            }
            let runtime = &runtimes[&key];
            let path = if entry.starts_with('@') {
                runtime.resolve_locator(entry)?.path
            } else {
                file_path(entry, project, Some(module_root))
            };
            let prefix = args.invocation[1..]
                .iter()
                .map(|arg| {
                    arg.to_str()
                        .map(str::to_owned)
                        .context("script arguments must be valid UTF-8")
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(runtime.command_signature(&path, &prefix)?)
        })
        .collect()
}

fn read_snapshot(path: &Path) -> Result<Vec<SourceOverlay>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("read Nu LSP snapshot {}", path.display()))?;
    serde_json::from_reader(file)
        .with_context(|| format!("parse Nu LSP snapshot {}", path.display()))
}

fn file_path(entry: &str, directory: &Path, module_root: Option<&Path>) -> PathBuf {
    let path = Path::new(entry);
    if path.is_absolute() {
        path.to_owned()
    } else {
        module_root.unwrap_or(directory).join(path)
    }
}

fn effective_scope(explicit: Option<&str>) -> Result<InputScope> {
    let inherited = std::env::var("TOLA_INPUT_SCOPE").ok();
    let name = explicit.or(inherited.as_deref()).unwrap_or("online");
    InputScope::parse(name).with_context(|| {
        format!("`{name}` is not an input scope; use `online`, `offline`, or `pure`")
    })
}

struct SiteModules {
    root: PathBuf,
    config: PathBuf,
    config_digest: String,
    content: PathBuf,
    modules: BTreeMap<String, NuModule>,
}

fn site_modules(
    directory: &Path,
    explicit: Option<&Path>,
    scope: InputScope,
) -> Result<Option<SiteModules>> {
    let config = explicit
        .map(Path::to_owned)
        .or_else(|| tola_build::config::loading::find_default_config(directory));
    let Some(config) = config else {
        return Ok(None);
    };
    let source = ConfigSource::load(Some(&config), scope)?;
    let (parsed, _) = source.decode_with_host::<crate::config::ConfigSchema>()?;
    let modules = resolve_nu_modules(source.root(), &parsed.schema().modules, scope)?;
    Ok(Some(SiteModules {
        root: source.root().to_owned(),
        config: source.path().to_owned(),
        config_digest: format!("blake3:{}", source.source_hash().to_hex()),
        content: source.root().join(&parsed.schema().build.content_dir),
        modules,
    }))
}

fn prepare(
    workspace: &Path,
    explicit: Option<&Path>,
    scope: InputScope,
    entry: Option<&str>,
    overlays: &[SourceOverlay],
    load_site: bool,
) -> Result<Runtime> {
    // Built-in tools remain usable without a valid site's configuration.
    let builtin = entry.is_some_and(|entry| entry.starts_with("@tola/"));
    let site = if !load_site || (builtin && explicit.is_none()) {
        None
    } else {
        site_modules(workspace, explicit, scope)?
    };
    let mut context = InvocationContext {
        directory: workspace.to_owned(),
        environment: BTreeMap::from([("TOLA_INPUT_SCOPE".into(), Some(scope.as_str().into()))]),
    };
    for (name, value) in crate::cli::theme::child_environment() {
        context.environment.insert(name.into(), Some(value));
    }
    let mut modules = Vec::new();
    let mut dependencies = Vec::new();
    if let Some(site) = &site {
        dependencies.push(site.config.clone());
        for (name, value) in [
            ("TOLA_PROJECT_ROOT", &site.root),
            ("TOLA_CONFIG_PATH", &site.config),
            ("TOLA_CONTENT_ROOT", &site.content),
        ] {
            context.environment.insert(
                name.into(),
                Some(
                    value
                        .to_str()
                        .with_context(|| format!("{name} must be valid UTF-8"))?
                        .into(),
                ),
            );
        }
        for module in site.modules.values() {
            dependencies.push(module.manifest.clone());
            if let Some(entry) = &module.entry {
                modules.push(ConfiguredModule {
                    alias: module.name.clone(),
                    root: module.root.clone(),
                    entry: entry.clone(),
                    manifest: module.manifest.clone(),
                });
            }
        }
        if let Some(alias) = entry.and_then(|entry| entry.strip_prefix("@tola-modules/")) {
            let module = site
                .modules
                .get(alias)
                .with_context(|| format!("no configured module is named `{alias}`"))?;
            ensure!(
                module.entry.is_some(),
                "module `{alias}` declares no `[nu]` entry"
            );
            context.directory = site.root.clone();
            for (name, value) in [
                ("TOLA_MODULE_ROOT", &module.root),
                ("TOLA_MODULE_MANIFEST", &module.manifest),
            ] {
                context.environment.insert(
                    name.into(),
                    Some(
                        value
                            .to_str()
                            .with_context(|| format!("{name} must be valid UTF-8"))?
                            .into(),
                    ),
                );
            }
            context
                .environment
                .insert("TOLA_MODULE_NAME".into(), Some(alias.into()));
            context
                .environment
                .insert("TOLA_MODULE_OPTIONS".into(), None);
        }
    } else if entry.is_some_and(|entry| entry.starts_with("@tola-modules/")) {
        bail!("a configured Nu module requires a site with tola.toml");
    }
    let runtime =
        Runtime::for_invocation_with_overlays(modules, dependencies, context, overlays.to_vec())?;
    if let Some(site) = site {
        ensure!(
            runtime
                .dependencies()
                .iter()
                .any(|dependency| dependency.path == site.config
                    && dependency.digest == site.config_digest),
            "site configuration changed while preparing Nu modules; run the command again"
        );
    }
    Ok(runtime)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{Cli, Commands};
    use clap::Parser;

    fn module(root: &Path, directory: &str, nu: bool) {
        let module = root.join(directory);
        std::fs::create_dir_all(&module).unwrap();
        std::fs::write(
            module.join("tola-module.toml"),
            if nu {
                "schema-version = 1\nname = \"tooling\"\nversion = \"1.0.0\"\n[nu]\nentry = \"public tools.nu\"\n"
            } else {
                "schema-version = 1\n"
            },
        )
        .unwrap();
        if nu {
            std::fs::write(
                module.join("public tools.nu"),
                "export def message [] { 'saved' }\nexport def main [] { message }\n",
            )
            .unwrap();
        }
    }

    fn selected_config(root: &Path, alias: &str, directory: &str) -> PathBuf {
        let config = root.join("selected site.toml");
        std::fs::write(&config, format!("[build]\ncontent-dir = \"article sources\"\n[modules.{alias}]\npath = {directory:?}\n")).unwrap();
        config
    }

    fn argument_values<'a>(runtime: &'a Runtime, name: &str) -> Option<&'a str> {
        runtime
            .invocation()
            .environment
            .get(name)
            .and_then(Option::as_deref)
    }

    #[test]
    fn preparation_keeps_alias_and_config() {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        module(&root, "module tools", true);
        let config = selected_config(&root, "catalog", "module tools");
        std::fs::write(root.join("tola.toml"), "invalid = [").unwrap();
        let current_directory = std::env::current_dir().unwrap();
        let current_config = std::env::var_os("TOLA_CONFIG_PATH");
        let runtime = prepare(
            &root,
            Some(&config),
            InputScope::Pure,
            Some("@tola-modules/catalog"),
            &[],
            true,
        )
        .unwrap();
        let entry = runtime.resolve_locator("@tola-modules/catalog").unwrap();
        assert_eq!(entry.path, root.join("module tools/public tools.nu"));
        assert_eq!(
            argument_values(&runtime, "TOLA_PROJECT_ROOT"),
            root.to_str()
        );
        assert_eq!(
            argument_values(&runtime, "TOLA_CONFIG_PATH"),
            config.to_str()
        );
        assert_eq!(
            argument_values(&runtime, "TOLA_CONTENT_ROOT"),
            root.join("article sources").to_str()
        );
        assert_eq!(
            argument_values(&runtime, "TOLA_MODULE_NAME"),
            Some("catalog")
        );
        assert_eq!(argument_values(&runtime, "TOLA_INPUT_SCOPE"), Some("pure"));
        assert_eq!(std::env::current_dir().unwrap(), current_directory);
        assert_eq!(std::env::var_os("TOLA_CONFIG_PATH"), current_config);
    }

    #[test]
    fn worker_reads_saved_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        module(&root, "module tools", true);
        let config = selected_config(&root, "catalog", "module tools");
        let source = root.join("module tools/public tools.nu");
        let snapshot = root.join("buffer snapshot.json");
        std::fs::write(
            &snapshot,
            serde_json::to_vec(&vec![SourceOverlay {
                path: source.clone(),
                text: "export def message [] { 'unsaved' }\nexport def main [] { message }\n"
                    .into(),
                version: 7,
            }])
            .unwrap(),
        )
        .unwrap();
        let overlays = read_snapshot(&snapshot).unwrap();
        let runtime = prepare(
            &root,
            Some(&config),
            InputScope::Pure,
            None,
            &overlays,
            true,
        )
        .unwrap();
        assert!(
            runtime
                .origins()
                .iter()
                .any(|origin| origin.path == source && origin.version == Some(7))
        );
        assert_eq!(
            argument_values(&runtime, "TOLA_CONFIG_PATH"),
            config.to_str()
        );
        assert_eq!(
            argument_values(&runtime, "TOLA_PROJECT_ROOT"),
            root.to_str()
        );
        assert_eq!(
            argument_values(&runtime, "TOLA_CONTENT_ROOT"),
            root.join("article sources").to_str()
        );
        assert!(std::fs::read_to_string(source).unwrap().contains("'saved'"));
        std::fs::write(&snapshot, "[broken").unwrap();
        assert!(read_snapshot(&snapshot).is_err());
    }

    #[test]
    fn reload_forwards_config_aliases() {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        module(&root, "module tools", true);
        let config = selected_config(&root, "before", "module tools");
        let previous = prepare(&root, Some(&config), InputScope::Offline, None, &[], true).unwrap();
        assert!(previous.resolve_locator("@tola-modules/before").is_ok());
        selected_config(&root, "after", "module tools");
        assert!(previous.ensure_fresh().is_err());
        let forwarded = Path::new(argument_values(&previous, "TOLA_CONFIG_PATH").unwrap());
        let current =
            prepare(&root, Some(forwarded), InputScope::Offline, None, &[], true).unwrap();
        assert!(current.resolve_locator("@tola-modules/after").is_ok());
        assert!(current.resolve_locator("@tola-modules/before").is_err());
        assert_eq!(
            argument_values(&current, "TOLA_INPUT_SCOPE"),
            Some("offline")
        );
    }

    #[test]
    fn missing_entries_never_fall_back() {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        module(&root, "module tools", false);
        let config = selected_config(&root, "catalog", "module tools");
        let error = prepare(
            &root,
            Some(&config),
            InputScope::Pure,
            Some("@tola-modules/catalog"),
            &[],
            true,
        )
        .err()
        .unwrap();
        assert!(format!("{error:#}").contains("declares no `[nu]` entry"));
        std::fs::create_dir_all(root.join("@tola")).unwrap();
        std::fs::write(root.join("@tola/missing"), "export def main [] { 42 }").unwrap();
        let builtin = prepare(
            &root,
            None,
            InputScope::Pure,
            Some("@tola/image"),
            &[],
            true,
        )
        .unwrap();
        assert!(builtin.resolve_locator("@tola/image").is_ok());
        assert!(builtin.resolve_locator("@tola/missing").is_err());
        assert!(builtin.resolve_locator("@other/image").is_err());
        assert_eq!(
            file_path("./@tola/missing", &root, None),
            root.join("./@tola/missing")
        );
    }

    #[test]
    fn script_resolution_keeps_caller_directory() {
        let caller = Path::new("/caller with spaces");
        let module = Path::new("/module with spaces");
        assert_eq!(
            file_path("tools/my script.nu", caller, None),
            caller.join("tools/my script.nu")
        );
        assert_eq!(
            file_path("tools/my script.nu", caller, Some(module)),
            module.join("tools/my script.nu")
        );
        assert_eq!(
            file_path("/absolute path/script.nu", caller, Some(module)),
            PathBuf::from("/absolute path/script.nu")
        );
    }

    #[test]
    fn internal_ignores_unusable_site_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let config = root.join("tola.toml");
        std::fs::write(&config, "invalid = [").unwrap();
        assert!(prepare(&root, None, InputScope::Pure, Some("main.nu"), &[], true).is_err());
        let runtime = prepare(
            &root,
            Some(&config),
            InputScope::Pure,
            Some("main.nu"),
            &[],
            false,
        )
        .unwrap();
        assert!(runtime.resolve_locator("@tola/image").is_ok());
        assert!(runtime.resolve_locator("@tola/diagnostic").is_ok());
        assert!(argument_values(&runtime, "TOLA_CONFIG_PATH").is_none());
    }

    #[test]
    fn arguments_distinguish_worker_from_script() {
        let cli = Cli::try_parse_from(["tola", "nu", "--no-site", "script.nu"]).unwrap();
        let Commands::Nu(internal) = cli.command else {
            panic!("Nu command");
        };
        assert!(internal.no_site);
        let cli = Cli::try_parse_from([
            "tola",
            "nu",
            "--lsp-worker",
            "--lsp-snapshot",
            "/buffer snapshot.json",
            "--config",
            "/selected site.toml",
        ])
        .unwrap();
        let Commands::Nu(worker) = cli.command else {
            panic!("Nu command");
        };
        assert!(worker.lsp_worker);
        assert_eq!(
            worker.lsp_snapshot.as_deref(),
            Some(Path::new("/buffer snapshot.json"))
        );
        assert_eq!(
            worker.config.path.as_deref(),
            Some(Path::new("/selected site.toml"))
        );
        for args in [
            vec!["tola", "nu", "--lsp-worker"],
            vec!["tola", "nu", "--lsp-snapshot", "snapshot.json"],
            vec![
                "tola",
                "nu",
                "--lsp",
                "--lsp-worker",
                "--lsp-snapshot",
                "snapshot.json",
            ],
            vec!["tola", "nu", "--no-site", "--lsp"],
            vec![
                "tola",
                "nu",
                "--no-site",
                "--config",
                "site.toml",
                "script.nu",
            ],
            vec![
                "tola",
                "nu",
                "--no-site",
                "--lsp-worker",
                "--lsp-snapshot",
                "snapshot.json",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
        let cli = Cli::try_parse_from([
            "tola",
            "nu",
            "./script path.nu",
            "--lsp-snapshot",
            "literal argument",
            "--no-site",
        ])
        .unwrap();
        let Commands::Nu(script) = cli.command else {
            panic!("Nu command");
        };
        assert!(!script.lsp_worker);
        assert!(!script.no_site);
        assert!(script.lsp_snapshot.is_none());
        assert_eq!(
            script.invocation,
            [
                "./script path.nu",
                "--lsp-snapshot",
                "literal argument",
                "--no-site"
            ]
            .map(std::ffi::OsString::from)
        );
    }
}
