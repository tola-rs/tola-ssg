//! Select a Nu environment without running hooks or compiling the page driver.

pub(super) mod diagnostic;
mod hook;

use crate::cancellation::Cancellation;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use tola_build::BuildResources;
use tola_build::cancellation::BuildCancellation;
use tola_build::config::ResolvedSiteConfig;
use tola_build::context::{Context as CallContext, Module};
use tola_build::report::ReportSources;
use tola_diagnostic::{Diagnostic, DiagnosticError};
use tola_nu::Host;

use anyhow::{Context, Result, bail, ensure};
use tola_build::config::ConfigSource;
use tola_build::modules::read_nu_modules;
use tola_input::{InputMode, InputScope};
use tola_module::manifest::NuModule;
use tola_nu::{
    BuiltinModule, ConfiguredModule, InvocationContext, Runtime, SourceOverlay, serve_lsp,
};

use crate::cli::args::NuArgs;

pub(in crate::cli) const BUILTIN_MODULES: &[BuiltinModule] = &[BuiltinModule {
    name: "nu-helpers",
    summary: "Pick original values from a searchable table",
    executable: false,
    entry: "mod.nu",
    sources: &[
        (
            "mod.nu",
            include_bytes!("../../../modules/nu-helpers/mod.nu"),
        ),
        (
            "pick.nu",
            include_bytes!("../../../modules/nu-helpers/pick.nu"),
        ),
    ],
}];

pub(super) fn prepare_runtime(
    modules: Vec<ConfiguredModule>,
    dependencies: Vec<PathBuf>,
    context: InvocationContext,
    overlays: Vec<SourceOverlay>,
) -> Result<Runtime> {
    Ok(Runtime::for_invocation_with_builtins(
        modules,
        dependencies,
        context,
        overlays,
        BUILTIN_MODULES,
    )?)
}

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
    if let Some(path) = &args.hook_worker {
        return hook::run_worker(path);
    }
    let call = if args.no_site {
        None
    } else {
        CallContext::read()?
    };
    let scope = effective_scope(
        input_scope.or_else(|| call.as_ref().and_then(|call| call.input_scope.as_deref())),
    )?;
    let current = InvocationContext::current()?;
    let explicit = if args.no_site {
        None
    } else {
        args.config
            .path
            .clone()
            .or_else(|| {
                call.as_ref()?
                    .site
                    .as_ref()
                    .map(|site| site.config_path.clone())
            })
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
        return Ok(runtime.lsp(args.builtin_dev));
    }
    if args.lsp {
        let executable = crate::cli::external::current_executable()?;
        return Ok(serve_lsp(
            &executable,
            args.builtin_dev,
            |workspace, overlays| {
                prepare(workspace, explicit.as_deref(), scope, None, overlays, true)
            },
        ));
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
    let inherited = call_host(call.as_ref())?;
    let before_pack = call
        .as_ref()
        .and_then(|call| call.hook.as_ref())
        .is_some_and(|hook| hook.stage == "before-pack");
    let site = if args.no_site
        || entry == "@tola-scripts/context"
        || inherited.init.is_some()
        || before_pack
        || (entry.starts_with("@tola-scripts/") && explicit.is_none())
    {
        None
    } else {
        site_modules(&current.directory, explicit.as_deref(), scope, &token)?
    };
    let mut env = environment(scope);
    if call.is_some() {
        let path = std::env::var(tola_build::context::PATH_ENV)
            .context("the Tola call context path must be valid UTF-8")?;
        env.insert(tola_build::context::PATH_ENV.into(), Some(path));
    }
    let runtime = prepare_invocation(
        &current.directory,
        scope,
        env,
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
    let module_root = call.as_ref().and_then(|call| call.module.as_ref());
    let path = file_path(
        entry,
        &current.directory,
        module_root.map(|module| module.root.as_path()),
    );
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
    let name = explicit.unwrap_or("online");
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
    env.insert(tola_build::context::PATH_ENV.into(), None);
    env
}

pub(super) fn call_environment(
    scope: InputScope,
    call: &CallContext,
    directory: &Path,
) -> Result<BTreeMap<String, Option<String>>> {
    let mut env = environment(scope);
    let path = call.write(directory)?;
    let path = path
        .to_str()
        .context("the Tola call context path must be valid UTF-8")?;
    env.insert(tola_build::context::PATH_ENV.into(), Some(path.into()));
    Ok(env)
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

fn runtime_inputs(
    config: &ResolvedSiteConfig,
    modules: &BTreeMap<String, NuModule>,
) -> (Vec<ConfiguredModule>, Vec<PathBuf>) {
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
    (libraries, dependencies)
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
        environment(scope),
        entry,
        overlays,
        site,
        host(),
        &token,
    )
}

fn prepare_invocation(
    workspace: &Path,
    scope: InputScope,
    environment: BTreeMap<String, Option<String>>,
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
        return Ok(prepare_runtime(
            Vec::new(),
            Vec::new(),
            InvocationContext {
                directory: workspace.to_owned(),
                environment,
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
    let (libraries, dependencies) = runtime_inputs(&site.config, &site.modules);
    let runtime = prepare_runtime(
        libraries,
        dependencies,
        InvocationContext {
            directory: directory.to_owned(),
            environment,
        },
        overlays.to_vec(),
    )?;
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

pub(super) fn host() -> Host {
    Host {
        render_diagnostics: Some(Arc::new(|directory, issues, color| {
            diagnostic::render(directory, issues, color).map_err(|error| error.to_string())
        })),
        ..Host::default()
    }
}

pub(super) fn call_host(call: Option<&CallContext>) -> Result<Host> {
    let mut host = host();
    let Some(call) = call else { return Ok(host) };
    host.context = Some(serde_json::to_value(call)?);
    host.module = call.module.as_ref().map(|module| module.name.clone());
    host.hook = call.hook.as_ref().map(|hook| {
        json!({
            "name": hook.name, "stage": hook.stage, "mode": hook.mode,
            "input-dir": hook.input_dir, "output-dir": hook.output_dir,
            "cache-dir": hook.cache_dir, "temp-dir": hook.temp_dir,
        })
    });
    if let Some(init) = &call.init {
        host.init = Some(json!({
            "target-dir": init.target_dir, "output-dir": init.output_dir,
            "interactive": init.interactive, "module": call.module,
        }));
    }
    if let Some(path) = &call.report_path {
        let path = path.clone();
        host.submit_diagnostics = Some(Arc::new(move |issues| {
            diagnostic::submit_file(&path, issues)
        }));
    }
    Ok(host)
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

pub(crate) fn hook_runner() -> tola_build::hooks::HookRunner {
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
        let call = &hook.context;
        let current = call.hook.as_ref().context("missing build hook context")?;
        let stage = &current.stage;
        let name = &current.name;
        let producer = match &call.module {
            Some(module) => format!("hook:{stage}:{}.{name}", module.name),
            None => format!("hook:{stage}:{name}"),
        };
        let (submit, diagnostics) = diagnostic::collector(producer, ReportSources::new());
        let host = Host {
            site: Some(site_record(&config)),
            modules: module_records(&modules, directory, hook.assets)?,
            query: Some(query_operation(config.clone(), resources, token.clone())),
            submit_diagnostics: Some(submit),
            ..call_host(Some(call))?
        };
        let (libraries, dependencies) = runtime_inputs(&config, &modules);
        let completion = hook::run(&hook, libraries, dependencies, host);
        token.ensure_active()?;
        let diagnostics = diagnostics
            .lock()
            .expect("diagnostic collector")
            .take()
            .unwrap_or_default();
        if let Err(error) = completion {
            return if diagnostics.is_empty() {
                Err(error)
            } else {
                Err(DiagnosticError::attach(error, diagnostics).into())
            };
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
    let module = modules
        .get(alias)
        .context("the calling module is missing")?;
    let mut call = CallContext::new(crate::cli::external::current_executable()?, Some(scope));
    call.site = Some(config.as_ref().into());
    call.module = Some(Module {
        name: module.name.clone(),
        root: module.root.clone(),
        manifest: module.manifest.clone(),
        options: Some(serde_json::to_value(&module.options)?),
    });
    let (submit, diagnostics) =
        diagnostic::collector(format!("command:{alias}.{command}"), ReportSources::new());
    let temporary = tempfile::tempdir().context("could not prepare the module command context")?;
    let (libraries, dependencies) = runtime_inputs(&config, &modules);
    let runtime = prepare_runtime(
        libraries,
        dependencies,
        InvocationContext {
            directory: directory.to_owned(),
            environment: call_environment(scope, &call, temporary.path())?,
        },
        Vec::new(),
    )?
    .with_host(Host {
        site: Some(site_record(&config)),
        modules: module_records(&modules, directory, None)?,
        query: Some(query_operation(config.clone(), resources, token.clone())),
        submit_diagnostics: Some(submit),
        ..call_host(Some(&call))?
    })?
    .with_cancellation(token.signal_flag());
    let status = runtime.run_embedded(path, arguments)?;
    token.ensure_active()?;
    let diagnostics = diagnostics
        .lock()
        .expect("diagnostic collector")
        .take()
        .unwrap_or_default();
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

    const CONTEXT_CLI: &str = "cli::commands::nu::tests::context_cli_child";
    const CONTEXT_HOOK: &str = "cli::commands::nu::tests::context_hook_child";
    const CONTEXT_ROLE: &str = "TOLA_CONTEXT_TEST_ROLE";

    #[test]
    fn hook_can_read_context_without_tola_on_path() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let executable = std::env::current_exe().unwrap();
        std::fs::create_dir(root.join("content")).unwrap();
        std::fs::create_dir(root.join("empty-path")).unwrap();
        std::fs::write(root.join("site.typ"), r#"#document("index.html")[Site]"#).unwrap();
        let argv = std::iter::once(executable.to_str().unwrap())
            .chain(["--exact", CONTEXT_HOOK, "--nocapture"])
            .collect::<Vec<_>>();
        std::fs::write(
            root.join("tola.toml"),
            format!(
                r#"
[[hooks.generate-outputs]]
name = "context"
argv = {}
outputs = [{{ file = "context.txt" }}]
"#,
                serde_json::to_string(&argv).unwrap()
            ),
        )
        .unwrap();

        let result = std::process::Command::new(&executable)
            .args(["--exact", CONTEXT_CLI, "--nocapture"])
            .current_dir(&root)
            .env("PATH", root.join("empty-path"))
            .env(CONTEXT_ROLE, "build")
            .env_remove("TOLA_EXECUTABLE")
            .env_remove(tola_build::context::PATH_ENV)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}{}",
            String::from_utf8_lossy(&result.stderr),
            String::from_utf8_lossy(&result.stdout),
        );
        assert_eq!(
            std::fs::read_to_string(root.join("public/context.txt")).unwrap(),
            "generate-outputs",
        );
    }

    #[test]
    fn context_cli_child() {
        let arguments: &[&str] = match std::env::var(CONTEXT_ROLE).as_deref() {
            Ok("build") => &["tola", "build", "--offline"],
            Ok("context") => &["tola", "nu", "@tola-scripts/context", "--json"],
            _ => return,
        };
        let arguments = arguments
            .iter()
            .copied()
            .map(Into::into)
            .collect::<Vec<_>>();
        assert_eq!(crate::cli::run(&arguments), ExitCode::SUCCESS);
    }

    #[test]
    fn context_hook_child() {
        if std::env::var(CONTEXT_ROLE).as_deref() != Ok("build") {
            return;
        }
        let executable = PathBuf::from(std::env::var_os("TOLA_EXECUTABLE").unwrap());
        assert!(executable.is_absolute());
        let result = std::process::Command::new(executable)
            .args(["--exact", CONTEXT_CLI, "--nocapture"])
            .env(CONTEXT_ROLE, "context")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        // The test runner also writes its progress to stdout.
        let stdout = String::from_utf8(result.stdout).unwrap();
        let context: Value = stdout
            .lines()
            .find_map(|line| serde_json::from_str(line).ok())
            .expect("context prints a JSON object");
        assert_eq!(context["inputScope"], "offline");
        let hook = &context["hook"];
        assert_eq!(hook["name"], "context");
        let input = Path::new(hook["inputDir"].as_str().unwrap());
        assert!(input.join("index.html").is_file());
        let output = Path::new(hook["outputDir"].as_str().unwrap());
        std::fs::write(output.join("context.txt"), hook["stage"].as_str().unwrap()).unwrap();
    }

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
        let current_config = std::env::var_os(tola_build::context::PATH_ENV);
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
        assert_eq!(
            std::env::var_os(tola_build::context::PATH_ENV),
            current_config
        );
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
        assert!(argument_values(&runtime, tola_build::context::PATH_ENV).is_none());
        let script = root.join("pick.nu");
        std::fs::write(
            &script,
            "use std/assert\nuse tola nu-helpers\nassert equal ([] | nu-helpers pick) []\n",
        )
        .unwrap();
        assert_eq!(runtime.run_embedded(&script, &[]).unwrap(), 0);
    }
}
