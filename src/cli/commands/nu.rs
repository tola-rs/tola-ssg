//! Select a Nu environment without running hooks or compiling the page driver.

use crate::cancellation::Cancellation;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use tola_build::BuildResources;
use tola_build::cancellation::BuildCancellation;
use tola_build::config::ResolvedSiteConfig;
use tola_build::report::{ReportBatch, ReportSources};
use tola_diagnostic::{Diagnostic, DiagnosticError};
use tola_nu::Host;

use anyhow::{Context, Result, bail, ensure};
use tola_build::config::ConfigSource;
use tola_build::modules::read_nu_modules;
use tola_input::{InputMode, InputScope};
use tola_module::manifest::NuModule;
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
            entry.starts_with("@tola-scripts/") || entry.starts_with("@tola-modules/"),
            "`{entry}` is not a configured Nu module; use `./` for a file whose name begins with `@`"
        );
    }
    let cancellation = Cancellation::install()?;
    let token = cancellation.token();
    let inherited = if args.no_site {
        Host::default()
    } else {
        external_host()
    };
    let author_hook = inherited
        .hook
        .as_ref()
        .and_then(|hook| hook.get("stage"))
        .and_then(Value::as_str)
        == Some("before-pack");
    let site = if args.no_site
        || inherited.init.is_some()
        || author_hook
        || (entry.starts_with("@tola-scripts/") && explicit.is_none())
    {
        None
    } else {
        site_modules(&current.directory, explicit.as_deref(), scope, &token)?
    };
    let runtime = prepare_invocation(
        &current.directory,
        scope,
        Some(entry),
        &[],
        site,
        inherited,
        &token,
    )?
    .with_cancellation(token.signal_flag());
    let arguments = utf8_arguments(&args.invocation[1..])?;
    if entry.starts_with('@') {
        let module = runtime.resolve_locator(entry)?;
        return Ok(runtime.run_module(&module, &arguments, args.stdin, input_scope));
    }
    let module_root = (!args.no_site)
        .then(|| std::env::var_os("TOLA_MODULE_ROOT"))
        .flatten()
        .map(PathBuf::from);
    let path = file_path(entry, &current.directory, module_root.as_deref());
    Ok(runtime.run_file(&path, &arguments, args.stdin, input_scope))
}

pub(super) fn utf8_arguments(arguments: &[std::ffi::OsString]) -> Result<Vec<String>> {
    arguments
        .iter()
        .map(|argument| {
            argument
                .to_str()
                .map(str::to_owned)
                .context("script arguments must be valid UTF-8")
        })
        .collect()
}

pub(super) fn command_signatures(
    invocations: &[(PathBuf, Vec<String>)],
    project: &Path,
    config: &Path,
    scope: InputScope,
) -> Vec<Result<Option<tola_nu::Signature>>> {
    if invocations.is_empty() {
        return Vec::new();
    }
    let runtime = prepare(project, Some(config), scope, None, &[], true);
    invocations
        .iter()
        .map(|(path, args)| {
            Ok(runtime
                .as_ref()
                .map_err(|error| anyhow::anyhow!("{error:#}"))?
                .command_signature(path, args)?)
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
    config: Arc<ResolvedSiteConfig>,
    config_digest: String,
    modules: BTreeMap<String, NuModule>,
}

fn site_modules(
    directory: &Path,
    explicit: Option<&Path>,
    scope: InputScope,
    cancellation: &BuildCancellation,
) -> Result<Option<SiteModules>> {
    cancellation.ensure_active()?;
    let config = explicit
        .map(Path::to_owned)
        .or_else(|| tola_build::config::loading::find_default_config(directory));
    let Some(config) = config else {
        return Ok(None);
    };
    let source = ConfigSource::load(Some(&config), scope)?;
    let config_digest = format!("blake3:{}", source.source_hash().to_hex());
    let config = Arc::new(
        crate::config::resolve(
            source,
            crate::cli::config::package_locations(&Default::default(), scope)?,
            &Default::default(),
        )?
        .into_config(),
    );
    let modules = read_nu_modules(
        config.get_root(),
        config.vendor().path.as_deref(),
        config.modules(),
        InputMode::new(scope, true),
        cancellation,
    )?;
    Ok(Some(SiteModules {
        config,
        config_digest,
        modules,
    }))
}

pub(super) fn environment(scope: InputScope) -> BTreeMap<String, Option<String>> {
    let mut env = BTreeMap::from([("TOLA_INPUT_SCOPE".into(), Some(scope.as_str().into()))]);
    for (name, value) in crate::cli::color::child_environment() {
        env.insert(name.into(), Some(value));
    }
    for name in [
        "TOLA_PROJECT_ROOT",
        "TOLA_CONFIG_PATH",
        "TOLA_CONTENT_ROOT",
        "TOLA_MODULE_ROOT",
        "TOLA_MODULE_MANIFEST",
        "TOLA_MODULE_NAME",
        "TOLA_MODULE_OPTIONS",
        "TOLA_HOOK_NAME",
        "TOLA_HOOK_STAGE",
        "TOLA_BUILD_MODE",
        "TOLA_HOOK_INPUT_DIR",
        "TOLA_HOOK_OUTPUT_DIR",
        "TOLA_HOOK_CACHE_DIR",
        "TOLA_HOOK_TEMP_DIR",
        "TOLA_INIT_TARGET_DIR",
        "TOLA_INIT_OUTPUT_DIR",
        "TOLA_INIT_INTERACTIVE",
        "TOLA_REPORT_PATH",
    ] {
        env.insert(name.into(), None);
    }
    env
}

pub(crate) fn site_record(config: &ResolvedSiteConfig) -> Value {
    use tola_build::config::section::site::SiteLanguage;

    let site = config.site();
    let language = |declaration| {
        let language = SiteLanguage::parse(declaration).expect("validated site language");
        json!({ "tag": language.tag(), "lang": language.lang(),
            "script": language.script(), "region": language.region() })
    };
    json!({
        "origin": site.origin, "base-path": site.base_path.base_path(), "url": config.site_url(),
        "title": site.title, "authors": site.authors.iter().map(|author| json!({
            "name": author.name, "email": author.email, "url": author.url,
        })).collect::<Vec<_>>(),
        "description": site.description, "language": language(&site.language),
        "languages": site.languages.iter().map(language).collect::<Vec<_>>(),
        "options": config.options(), "root": config.get_root(), "config-path": config.config_path(),
        "content-root": config.build().content_dir,
    })
}

fn runtime(
    directory: &Path,
    config: &ResolvedSiteConfig,
    modules: &BTreeMap<String, NuModule>,
    overlays: &[SourceOverlay],
    scope: InputScope,
) -> Result<Runtime> {
    let mut dependencies = vec![config.config_path().to_owned()];
    let mut libraries = Vec::new();
    for module in modules.values() {
        dependencies.push(module.manifest.clone());
        if let Some(entry) = &module.entry {
            libraries.push(ConfiguredModule {
                alias: module.name.clone(),
                root: module.root.clone(),
                entry: entry.clone(),
                manifest: module.manifest.clone(),
            });
        }
    }
    Ok(Runtime::for_invocation_with_overlays(
        libraries,
        dependencies,
        InvocationContext {
            directory: directory.to_owned(),
            environment: environment(scope),
        },
        overlays.to_vec(),
    )?)
}

fn module_records(
    modules: &BTreeMap<String, NuModule>,
    root: &Path,
    published: Option<&tola_build::AssetUrls>,
) -> Result<BTreeMap<String, Value>> {
    use tola_build::ModuleAssetInput;

    let mut assets = BTreeMap::<
        &str,
        (
            serde_json::Map<String, Value>,
            serde_json::Map<String, Value>,
        ),
    >::new();
    for (asset, url) in published
        .into_iter()
        .flat_map(tola_build::AssetUrls::module_assets)
    {
        let path = match &asset.input {
            ModuleAssetInput::Module(path) => modules
                .get(&asset.alias)
                .with_context(|| format!("no configured module is named `{}`", asset.alias))?
                .root
                .join(path),
            ModuleAssetInput::Project(path) => root.join(path),
        };
        let path = path
            .to_str()
            .context("a module asset path is not valid UTF-8")?;
        let (files, urls) = assets.entry(&asset.alias).or_default();
        files.insert(asset.name.clone(), path.into());
        urls.insert(asset.name.clone(), url.into());
    }
    modules
        .iter()
        .map(|(name, module)| {
            let assets = if module.assets_enabled && published.is_none() {
                Value::Null
            } else {
                let (files, urls) = assets.remove(name.as_str()).unwrap_or_default();
                json!({ "enabled": module.assets_enabled, "files": files, "urls": urls })
            };
            let root = module
                .root
                .to_str()
                .with_context(|| format!("the root of module `{name}` is not valid UTF-8"))?;
            let manifest = module.manifest.to_str().with_context(|| {
                format!("the manifest path of module `{name}` is not valid UTF-8")
            })?;
            Ok((
                name.clone(),
                json!({
                    "name": name, "options": module.options, "root": root, "manifest": manifest,
                    "assets": assets,
                }),
            ))
        })
        .collect()
}

fn prepare(
    workspace: &Path,
    explicit: Option<&Path>,
    scope: InputScope,
    entry: Option<&str>,
    overlays: &[SourceOverlay],
    load_site: bool,
) -> Result<Runtime> {
    let token = BuildCancellation::default();
    let builtin = entry.is_some_and(|entry| entry.starts_with("@tola-scripts/"));
    let site = if !load_site || (builtin && explicit.is_none()) {
        None
    } else {
        site_modules(workspace, explicit, scope, &token)?
    };
    prepare_invocation(
        workspace,
        scope,
        entry,
        overlays,
        site,
        Host::default(),
        &token,
    )
}

fn prepare_invocation(
    workspace: &Path,
    scope: InputScope,
    entry: Option<&str>,
    overlays: &[SourceOverlay],
    site: Option<SiteModules>,
    mut inherited: Host,
    token: &BuildCancellation,
) -> Result<Runtime> {
    let alias = entry.and_then(|entry| entry.strip_prefix("@tola-modules/"));
    let Some(site) = site else {
        ensure!(
            alias.is_none(),
            "a configured Nu module requires a site with tola.toml"
        );
        return Ok(Runtime::for_invocation_with_overlays(
            Vec::new(),
            Vec::new(),
            InvocationContext {
                directory: workspace.to_owned(),
                environment: environment(scope),
            },
            overlays.to_vec(),
        )?
        .with_host(inherited)?);
    };
    if let Some(alias) = alias {
        let module = site
            .modules
            .get(alias)
            .with_context(|| format!("no configured module is named `{alias}`"))?;
        ensure!(
            module.entry.is_some(),
            "module `{alias}` declares no `[nu]` entry"
        );
    }
    let directory = if alias.is_some() {
        site.config.get_root()
    } else {
        workspace
    };
    let runtime = runtime(directory, &site.config, &site.modules, overlays, scope)?;
    ensure!(
        runtime
            .dependencies()
            .iter()
            .any(|dependency| dependency.path == site.config.config_path()
                && dependency.digest == site.config_digest),
        "site configuration changed while preparing Nu modules; run the command again"
    );
    inherited.site = Some(site_record(&site.config));
    inherited.modules = module_records(&site.modules, site.config.get_root(), None)?;
    if let Some(alias) = alias {
        inherited.module = Some(alias.to_owned());
    }
    inherited.query = Some(query_operation(
        site.config.clone(),
        crate::cli::config::build_resources(InputMode::new(scope, false)),
        token.clone(),
    ));
    Ok(runtime.with_host(inherited)?)
}

fn external_host() -> Host {
    let mut host = Host::default();
    let variable = |name| std::env::var(name).ok().filter(|value| !value.is_empty());
    if let Some(stage) = variable("TOLA_HOOK_STAGE") {
        host.hook = Some(json!({
            "name": variable("TOLA_HOOK_NAME"), "stage": stage, "mode": variable("TOLA_BUILD_MODE"),
            "input-dir": variable("TOLA_HOOK_INPUT_DIR"), "output-dir": variable("TOLA_HOOK_OUTPUT_DIR"),
            "cache-dir": variable("TOLA_HOOK_CACHE_DIR"), "temp-dir": variable("TOLA_HOOK_TEMP_DIR"),
        }));
    }
    if let Some(output) = variable("TOLA_INIT_OUTPUT_DIR") {
        host.init = Some(json!({
            "target-dir": variable("TOLA_INIT_TARGET_DIR"), "output-dir": output,
            "interactive": variable("TOLA_INIT_INTERACTIVE").as_deref() == Some("true"),
            "module": {
                "name": variable("TOLA_MODULE_NAME"), "root": variable("TOLA_MODULE_ROOT"),
                "manifest": variable("TOLA_MODULE_MANIFEST"),
            },
        }));
    } else if variable("TOLA_CONFIG_PATH").is_some() {
        host.module = variable("TOLA_MODULE_NAME");
    }
    if let Some(path) = variable("TOLA_REPORT_PATH") {
        host.diagnostic = Some(Arc::new(move |issues| {
            let batch = json!({ "schemaVersion": 1, "diagnostics": issues });
            let bytes = serde_json::to_vec(&batch).map_err(|error| error.to_string())?;
            std::fs::write(&path, bytes)
                .map_err(|error| format!("write diagnostic report: {error}"))
        }));
    }
    host
}

fn query_operation(
    config: Arc<ResolvedSiteConfig>,
    resources: BuildResources,
    cancellation: BuildCancellation,
) -> tola_nu::Query {
    Arc::new(move |directory, target, override_path, label| {
        let result = (|| -> Result<Value> {
            cancellation.ensure_active()?;
            let config = if let Some(path) = override_path {
                let scope = resources.input_scope();
                let source = ConfigSource::load(Some(&directory.join(path)), scope)?;
                Arc::new(
                    crate::config::resolve(
                        source,
                        config.package_locations().clone(),
                        &Default::default(),
                    )?
                    .into_config(),
                )
            } else {
                config.clone()
            };
            let result = super::query::evaluate(
                config,
                directory,
                target,
                label,
                resources.clone(),
                Vec::new(),
                &cancellation,
            )?;
            if !result.success {
                let detail = result
                    .diagnostics
                    .iter()
                    .map(|issue| issue.message.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                bail!("model query `{}` failed: {detail}", result.query);
            }
            result
                .data
                .context("the model query did not produce a result")
        })();
        result.map_err(|error| format!("{error:#}"))
    })
}

pub(super) fn diagnostic_collector(
    producer: String,
    sources: ReportSources,
) -> (tola_nu::Diagnostic, Arc<Mutex<Vec<Diagnostic>>>) {
    let diagnostics = Arc::new(Mutex::new(Vec::new()));
    let collected = diagnostics.clone();
    let submit: tola_nu::Diagnostic = Arc::new(move |issues| {
        let diagnostics = serde_json::from_value(issues).map_err(|error| error.to_string())?;
        let batch = ReportBatch {
            schema_version: 1,
            diagnostics,
        };
        let issues = batch
            .validate(&producer, &sources)
            .map_err(|error| error.to_string())?;
        *collected.lock().expect("diagnostic collector") = issues;
        Ok(())
    });
    (submit, diagnostics)
}

pub(crate) fn hook_runner() -> tola_build::hooks::HookRunner {
    use tola_build::build::BuildMode;
    tola_build::hooks::HookRunner::new(|hook| {
        let config = Arc::new(hook.config.clone());
        let directory = config.get_root();
        let token = hook.cancellation.cloned().unwrap_or_default();
        let resources = hook.resources.clone();
        let mut modules = BTreeMap::new();
        if let Some(prepared) = hook.modules {
            let enabled = prepared
                .assets
                .iter()
                .map(|assets| assets.alias.as_str())
                .collect::<std::collections::BTreeSet<_>>();
            for (name, module) in &prepared.modules {
                modules.insert(
                    name.clone(),
                    NuModule {
                        name: name.clone(),
                        root: module.root.clone(),
                        manifest: module.manifest.clone(),
                        entry: module.nu.as_ref().map(|nu| nu.entry.clone()),
                        options: module.options.clone(),
                        assets_enabled: enabled.contains(name.as_str()),
                    },
                );
            }
        }
        let mode = match hook.mode {
            BuildMode::Production => "prod",
            BuildMode::Development => "dev",
        };
        let stage = hook.stage.as_str();
        let name = hook.name;
        let producer = match hook.origin {
            Some(origin) => format!("hook:{stage}:{}.{}", origin.name, name),
            None => format!("hook:{stage}:{name}"),
        };
        let (submit, diagnostics) = diagnostic_collector(producer, ReportSources::new());
        let runtime = runtime(directory, &config, &modules, &[], hook.scope)?.with_host(Host {
            site: Some(site_record(&config)), modules: module_records(&modules, directory, hook.assets)?,
            module: hook.origin.map(|origin| origin.name.clone()),
            hook: Some(json!({
                "name": name, "stage": stage, "mode": mode, "input-dir": hook.input_dir,
                "output-dir": hook.output_dir, "cache-dir": hook.cache_dir, "temp-dir": hook.temp_dir,
            })),
            query: Some(query_operation(config.clone(), resources, token.clone())),
            diagnostic: Some(submit), ..Default::default()
        })?.with_cancellation(token.signal_flag());
        let status = runtime.run_embedded(hook.path, hook.args)?;
        token.ensure_active()?;
        let diagnostics = std::mem::take(&mut *diagnostics.lock().expect("diagnostic collector"));
        if status != 0 {
            return Err(DiagnosticError::new(
                format!("{stage} hook `{name}` exited with status {status}"),
                diagnostics,
            )
            .into());
        }
        Ok(diagnostics)
    })
}

pub(super) fn run_action(
    path: &Path,
    arguments: &[String],
    alias: &str,
    command: &str,
    config: Arc<ResolvedSiteConfig>,
    resources: BuildResources,
    cancellation: &Cancellation,
) -> Result<Vec<Diagnostic>> {
    let scope = resources.input_scope();
    let token = cancellation.token();
    let directory = config.get_root();
    let modules = read_nu_modules(
        directory,
        config.vendor().path.as_deref(),
        config.modules(),
        resources.input_mode(),
        &token,
    )?;
    let (submit, diagnostics) =
        diagnostic_collector(format!("command:{alias}.{command}"), ReportSources::new());
    let runtime = runtime(directory, &config, &modules, &[], scope)?
        .with_host(Host {
            site: Some(site_record(&config)),
            modules: module_records(&modules, directory, None)?,
            module: Some(alias.into()),
            query: Some(query_operation(config.clone(), resources, token.clone())),
            diagnostic: Some(submit),
            ..Default::default()
        })?
        .with_cancellation(token.signal_flag());
    let status = runtime.run_embedded(path, arguments)?;
    token.ensure_active()?;
    let diagnostics = std::mem::take(&mut *diagnostics.lock().expect("diagnostic collector"));
    if status != 0 || super::holds_error(&diagnostics) {
        let message = if status != 0 {
            format!("module command `{alias} {command}` exited with status {status}")
        } else {
            format!("module command `{alias} {command}` reported an error")
        };
        return Err(DiagnosticError::new(message, diagnostics).into());
    }
    Ok(diagnostics)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(root: &Path, directory: &str, nu: bool) {
        let module = root.join(directory);
        std::fs::create_dir_all(&module).unwrap();
        std::fs::write(
            module.join("tola-module.toml"),
            if nu {
                "\n[nu]\nentry = \"public tools.nu\"\n"
            } else {
                "\n"
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
        let current = prepare(&root, Some(&config), InputScope::Offline, None, &[], true).unwrap();
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
        std::fs::create_dir_all(root.join("@tola-scripts")).unwrap();
        std::fs::write(
            root.join("@tola-scripts/missing"),
            "export def main [] { 42 }",
        )
        .unwrap();
        let builtin = prepare(
            &root,
            None,
            InputScope::Pure,
            Some("@tola-scripts/image"),
            &[],
            true,
        )
        .unwrap();
        assert!(builtin.resolve_locator("@tola-scripts/image").is_ok());
        assert!(builtin.resolve_locator("@tola-scripts/missing").is_err());
        assert!(builtin.resolve_locator("@other/image").is_err());
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
        assert!(runtime.resolve_locator("@tola-scripts/image").is_ok());
        assert!(runtime.resolve_locator("@tola-scripts/diagnostic").is_ok());
        assert!(argument_values(&runtime, "TOLA_CONFIG_PATH").is_none());
    }
}
