//! One prepared Nushell environment with built-in sources and configured site modules.
//!
//! A [`Runtime`] owns nothing about the site itself. The caller decides which configuration,
//! modules, overlays, and invocation directory apply, and passes them in as [`ScriptModule`],
//! [`SourceOverlay`], and [`InvocationContext`]; the caller also reads the dependency evidence and
//! source origins back out. This layer does not read site configuration, resolve routes, or publish
//! output, and it reads the process environment only to place the child evaluation (`PWD`, the
//! inherited `TOLA_*` context, and the running executable's path). It loads no personal Nushell
//! configuration or plugins, so the path fields of `$nu` remain empty.

#![expect(
    clippy::result_large_err,
    reason = "Nushell's ShellError is the engine's own named error type, matched by callers and required by its command API"
)]

mod error;
mod execution;
mod highlight;
mod host;
mod image;
mod lsp;
mod modules;
mod palette;
mod resources;
mod signatures;

pub use error::NuError;
pub use host::{GetAssets, Host, Query, SubmitDiagnostics};
pub use lsp::serve_lsp;
pub use modules::{Dependency, ModuleEntry, Origin, ScriptModule, SourceOrigin, SourceOverlay};
pub use nu_protocol::{Flag, PositionalArg, Signature, Value};

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::error::Result;

use nu_protocol::{
    ByteStream, Config, Handlers, IntoValue, ModuleId, PipelineData, SignalAction, Signals, Span,
    Type,
    engine::{EngineState, Stack, StateWorkingSet},
    record, report_shell_error,
};

/// The embedded Nushell release every nu-* dependency is pinned to.
pub const NU_VERSION: &str = "0.117.0";

/// A built-in Nu library and how it appears in command help.
pub struct BuiltinModule {
    /// The name in `use tola NAME`.
    pub name: &'static str,
    /// A short description shown in `tola nu --help`.
    pub summary: &'static str,
    /// Whether this built-in publishes a runnable `main`.
    pub executable: bool,
    /// The entry file within this module's virtual directory.
    pub entry: &'static str,
    /// Source files relative to the module's virtual directory.
    pub sources: &'static [(&'static str, &'static [u8])],
}

/// Built-in Nu modules available through `use tola`.
pub const BUILTIN_MODULES: &[BuiltinModule] = &[
    BuiltinModule {
        name: "context",
        summary: "Show the context Tola supplied to this invocation",
        executable: true,
        entry: "context.nu",
        sources: &[
            ("context.nu", include_bytes!("../nu/context.nu")),
            ("context_core.nu", include_bytes!("../nu/context_core.nu")),
        ],
    },
    BuiltinModule {
        name: "image",
        summary: "Inspect, resize, and convert images",
        executable: true,
        entry: "image.nu",
        sources: &[
            ("image.nu", include_bytes!("../nu/image.nu")),
            ("image_core.nu", include_bytes!("../nu/image_core.nu")),
        ],
    },
    BuiltinModule {
        name: "diagnostic",
        summary: "Construct, render, and submit diagnostics",
        executable: true,
        entry: "diagnostic.nu",
        sources: &[
            ("diagnostic.nu", include_bytes!("../nu/diagnostic.nu")),
            (
                "diagnostic_core.nu",
                include_bytes!("../nu/diagnostic_core.nu"),
            ),
        ],
    },
];

/// One prepared Nu environment for CLI evaluation or LSP service.
pub struct Runtime {
    engine: EngineState,
    native_module: ModuleId,
    stack: Stack,
    entries: BTreeMap<String, ModuleEntry>,
    dependencies: Vec<Dependency>,
    origins: Vec<SourceOrigin>,
    context: InvocationContext,
    overlays: Vec<SourceOverlay>,
    missing_sources: Vec<PathBuf>,
    source_paths: Vec<PathBuf>,
    host: Arc<OnceLock<Host>>,
    embedded_signals: bool,
}

/// The directory and environment one evaluation runs in, chosen by the caller instead of the
/// process.
#[derive(Clone, Debug)]
pub struct InvocationContext {
    /// Working directory the script sees as `PWD`.
    pub directory: PathBuf,
    /// Variables to set, or `None` to remove from the inherited environment.
    pub environment: BTreeMap<String, Option<String>>,
}

impl InvocationContext {
    /// The process's own directory and environment, without mutating either.
    pub fn current() -> Result<Self> {
        let mut directory = std::env::current_dir()?;
        if let Some(pwd) = std::env::var_os("PWD")
            && same_file::is_same_file(&directory, Path::new(&pwd)).unwrap_or(false)
        {
            directory = pwd.into();
        }
        Ok(Self {
            directory,
            environment: BTreeMap::new(),
        })
    }
}

impl Runtime {
    /// Prepare a session in the process's own directory and environment.
    pub fn new(modules: Vec<ScriptModule>, dependencies: Vec<PathBuf>) -> Result<Self> {
        Self::for_invocation(modules, dependencies, InvocationContext::current()?)
    }

    /// Prepare a session for `context`, with no in-memory source replacements.
    pub fn for_invocation(
        modules: Vec<ScriptModule>,
        dependencies: Vec<PathBuf>,
        context: InvocationContext,
    ) -> Result<Self> {
        Self::for_invocation_with_overlays(modules, dependencies, context, Vec::new())
    }

    /// Prepare a session for `context`, parsing `overlays` in place of the files they stand in
    /// for.
    pub fn for_invocation_with_overlays(
        modules: Vec<ScriptModule>,
        dependencies: Vec<PathBuf>,
        context: InvocationContext,
        overlays: Vec<SourceOverlay>,
    ) -> Result<Self> {
        Self::for_invocation_with_builtins(modules, dependencies, context, overlays, &[])
    }

    /// Prepare an invocation with additional built-in modules supplied by the application.
    pub fn for_invocation_with_builtins(
        modules: Vec<ScriptModule>,
        dependencies: Vec<PathBuf>,
        context: InvocationContext,
        overlays: Vec<SourceOverlay>,
        builtins: &[BuiltinModule],
    ) -> Result<Self> {
        if !(context.directory.is_absolute() && context.directory.is_dir()) {
            return Err(NuError::InvocationDirectory);
        }
        let sources = collect_overlays(overlays)?;
        let overlay_paths = sources.keys().cloned().collect();
        let builtins = BUILTIN_MODULES.iter().chain(builtins);
        let binding_paths: Vec<_> = modules
            .iter()
            .flat_map(|m| [m.root.join(&m.entry), m.manifest.clone()])
            .collect();
        let configured = configure_libraries(modules, &overlay_paths)?;
        let mut captured =
            capture_initial_deps(&dependencies, &binding_paths, &context.directory, &sources)?;
        let (mut engine, stack) =
            initialize_engine(false, &context).map_err(|()| NuError::Engine)?;
        let supplied: Vec<_> = sources.values().cloned().collect();
        let host = Arc::new(OnceLock::new());
        let (entries, native_module) =
            load_modules(&mut engine, builtins.clone(), &configured, &supplied, &host)?;
        let (used_sources, missing_sources) =
            collect_used_sources(&engine, &sources, &configured, &mut captured)?;
        let dependencies: Vec<_> = captured.into_values().collect();
        let origins = collect_origins(&configured, builtins, &used_sources);
        let runtime = Self {
            engine,
            native_module,
            stack,
            entries,
            dependencies,
            origins,
            context,
            overlays: sources
                .into_values()
                .filter(|o| used_sources.contains_key(&o.path))
                .collect(),
            missing_sources,
            source_paths: used_sources.keys().cloned().collect(),
            host,
            embedded_signals: false,
        };
        runtime.ensure_fresh()?;
        Ok(runtime)
    }

    /// Bind the objects and operations this invocation can access.
    pub fn with_host(self, host: Host) -> Result<Self> {
        self.host.set(host).map_err(|_| NuError::Engine)?;
        Ok(self)
    }

    /// Use the host's cancellation flag for embedded evaluation.
    pub fn with_cancellation(mut self, flag: Arc<AtomicBool>) -> Self {
        self.engine.set_signals(Signals::new(flag));
        self.embedded_signals = true;
        self
    }

    /// Run a script using the signal already installed by the host.
    pub fn run_embedded(&self, path: &Path, args: &[String]) -> Result<i32> {
        self.ensure_fresh()?;
        let mut engine = self.engine.clone();
        let mut stack = self.stack.clone();
        Ok(evaluate_file(
            path,
            args,
            false,
            None,
            &mut engine,
            &mut stack,
        ))
    }

    fn site_config(&self) -> Option<&Path> {
        self.host
            .get()?
            .site
            .as_ref()?
            .get("config-path")?
            .as_str()
            .map(Path::new)
    }

    /// Resolve only the two explicit module namespaces. There is no filesystem fallback.
    pub fn resolve_locator(&self, locator: &str) -> Result<ModuleEntry> {
        self.ensure_fresh()?;
        self.entries
            .get(locator)
            .cloned()
            .ok_or(NuError::UnknownLocator {
                locator: locator.to_owned(),
            })
    }

    /// The inputs the session was prepared from, each with the digest it validates against.
    pub fn dependencies(&self) -> &[Dependency] {
        &self.dependencies
    }

    /// The module sources the session actually read, attributed to the module that owns them.
    pub fn origins(&self) -> &[SourceOrigin] {
        &self.origins
    }

    /// The directory and environment this session's evaluation runs in.
    pub fn invocation(&self) -> &InvocationContext {
        &self.context
    }

    /// The in-memory source replacements this session was prepared with.
    pub fn overlays(&self) -> &[SourceOverlay] {
        &self.overlays
    }

    /// Sources supplied only as buffers, which a later session must not find on disk.
    pub fn missing_sources(&self) -> &[PathBuf] {
        &self.missing_sources
    }

    /// Check the captured inputs before starting a new session. This is not live LSP reload.
    pub fn ensure_fresh(&self) -> Result<()> {
        for path in &self.missing_sources {
            if path.try_exists()? {
                return Err(NuError::BufferAppeared { path: path.clone() });
            }
        }
        for dependency in &self.dependencies {
            if std::fs::canonicalize(&dependency.path)? != dependency.canonical_path {
                return Err(NuError::InputTargetChanged {
                    path: dependency.path.clone(),
                });
            }
            let bytes =
                std::fs::read(&dependency.path).map_err(|source| NuError::InputDisappeared {
                    path: dependency.path.clone(),
                    source,
                })?;
            if modules::digest(&bytes) != dependency.digest {
                return Err(NuError::InputChanged {
                    path: dependency.path.clone(),
                });
            }
        }
        Ok(())
    }

    fn check_fresh(&self) -> bool {
        if let Err(err) = self.ensure_fresh() {
            eprintln!("{err:#}");
            return false;
        }
        true
    }

    /// Run one public module entry, reporting a failure on stderr and answering its exit code.
    pub fn run_module(
        &self,
        entry: &ModuleEntry,
        args: &[String],
        read_stdin: bool,
        input_scope: Option<&str>,
    ) -> i32 {
        if !self.check_fresh() {
            return 1;
        }
        if !entry.has_main {
            let import = match &entry.origin {
                Origin::Builtin { name } => format!("use tola {name}"),
                Origin::Module { alias, .. } => format!("use tola-modules {alias}"),
            };
            eprintln!(
                "`{}` is a Nu library; import it with `{import}`",
                entry.locator
            );
            return 1;
        }
        self.run_file(&entry.path, args, read_stdin, input_scope)
    }

    /// Run before another Ctrl-C handler is installed. Official Nu owns script/main evaluation.
    pub fn run_file(
        &self,
        path: &Path,
        args: &[String],
        read_stdin: bool,
        input_scope: Option<&str>,
    ) -> i32 {
        if !self.check_fresh() {
            return 1;
        }
        let mut engine = self.engine.clone();
        let mut stack = self.stack.clone();
        if !self.embedded_signals && install_ctrlc_handler(&mut engine).is_err() {
            return 1;
        }
        evaluate_file(path, args, read_stdin, input_scope, &mut engine, &mut stack)
    }

    /// Serve one prepared project context. Restart/replay after its inventory changes.
    pub fn lsp(&self, builtin_dev: bool) -> i32 {
        if !self.check_fresh() {
            return 1;
        }
        let mut engine = self.engine.clone();
        engine.is_lsp = true;
        if builtin_dev {
            let mut ws = StateWorkingSet::new(&engine);
            ws.use_modules(vec![(
                modules::NATIVE_MODULE.as_bytes().to_vec(),
                self.native_module,
            )]);
            if let Err(err) = engine.merge_delta(ws.render()) {
                eprintln!("{err:#}");
                return 1;
            }
        }
        if install_ctrlc_handler(&mut engine).is_err() {
            return 1;
        }
        match nu_lsp::LanguageServer::initialize_stdio_connection(engine)
            .and_then(|server| server.serve_requests())
        {
            Ok(()) => 0,
            Err(err) => {
                eprintln!("{err:?}");
                1
            }
        }
    }
}

pub(crate) fn path_str<'a>(path: &'a Path, what: &'static str) -> Result<&'a str> {
    path.to_str().ok_or(NuError::NotUtf8 { what })
}

type UsedSources = BTreeMap<PathBuf, (String, Option<i64>)>;

fn collect_overlays(overlays: Vec<SourceOverlay>) -> Result<BTreeMap<PathBuf, SourceOverlay>> {
    let mut sources = BTreeMap::<PathBuf, SourceOverlay>::new();
    for mut overlay in overlays {
        overlay.path = modules::source_path(&overlay.path)?;
        if let Some(prev) = sources.get(&overlay.path)
            && (prev.text != overlay.text || prev.version != overlay.version)
        {
            return Err(NuError::OverlayConflict {
                path: overlay.path.clone(),
            });
        }
        sources.insert(overlay.path.clone(), overlay);
    }
    Ok(sources)
}

fn configure_libraries(
    modules: Vec<ScriptModule>,
    overlay_paths: &BTreeSet<PathBuf>,
) -> Result<Vec<modules::Library>> {
    let configured = modules
        .into_iter()
        .map(|m| modules::Library::configured(m, overlay_paths))
        .collect::<Result<Vec<_>>>()?;
    let mut aliases = BTreeSet::new();
    for lib in &configured {
        if !aliases.insert(&lib.alias) {
            return Err(NuError::DuplicateAlias {
                alias: lib.alias.clone(),
            });
        }
    }
    Ok(configured)
}

fn capture_initial_deps(
    dependencies: &[PathBuf],
    binding_paths: &[PathBuf],
    dir: &Path,
    sources: &BTreeMap<PathBuf, SourceOverlay>,
) -> Result<BTreeMap<PathBuf, Dependency>> {
    let mut captured = BTreeMap::new();
    for path in dependencies.iter().chain(binding_paths) {
        let path = if path.is_absolute() {
            path.clone()
        } else {
            dir.join(path)
        };
        if !sources.contains_key(&modules::source_path(&path)?) {
            insert_dependency(&mut captured, modules::capture(&path)?)?;
        }
    }
    Ok(captured)
}

fn load_modules<'a>(
    engine: &mut EngineState,
    builtins: impl Iterator<Item = &'a BuiltinModule> + Clone,
    configured: &[modules::Library],
    supplied: &[SourceOverlay],
    host: &Arc<OnceLock<Host>>,
) -> Result<(BTreeMap<String, ModuleEntry>, ModuleId)> {
    let base = engine.clone();
    let mut dirs = BTreeSet::new();
    let mut observed = BTreeSet::new();
    let mut attempts = 0;
    loop {
        attempts += 1;
        let count = dirs.len();
        *engine = base.clone();
        let res = modules::register(
            engine,
            builtins.clone(),
            configured,
            supplied,
            &mut dirs,
            &mut observed,
            host,
        );
        if !supplied.is_empty() && dirs.len() > count {
            if attempts > observed.len() + 1 {
                return Err(NuError::ImportsUnstable);
            }
            continue;
        }
        return res;
    }
}

fn file_consumed(engine: &EngineState, covered: Span) -> bool {
    let in_file =
        |span: Option<Span>| span.is_some_and(|s| s.start >= covered.start && s.end <= covered.end);
    (0..engine.num_blocks()).any(|i| in_file(engine.get_block(nu_protocol::BlockId::new(i)).span))
        || (0..engine.num_modules())
            .any(|i| in_file(engine.get_module(nu_protocol::ModuleId::new(i)).span))
}

fn collect_used_sources(
    engine: &EngineState,
    sources: &BTreeMap<PathBuf, SourceOverlay>,
    configured: &[modules::Library],
    captured: &mut BTreeMap<PathBuf, Dependency>,
) -> Result<(UsedSources, Vec<PathBuf>)> {
    let mut used = BTreeMap::new();
    let mut missing = Vec::new();
    for file in engine.files() {
        let path = Path::new(file.name.as_ref());
        if !(path.is_absolute() && file_consumed(engine, file.covered_span)) {
            continue;
        }
        let identity = modules::source_path(path)?;
        if let Some(overlay) = sources.get(&identity) {
            if file.content.as_ref() != overlay.text.as_bytes() {
                return Err(NuError::BufferNotUsed {
                    path: path.to_path_buf(),
                });
            }
            if path.exists() {
                insert_dependency(captured, modules::capture(path)?)?;
            } else {
                if !configured.iter().any(|m| identity.starts_with(&m.root)) {
                    return Err(NuError::BufferOutsideModules);
                }
                missing.push(identity.clone());
            }
            used.insert(
                identity,
                (modules::digest(&file.content), Some(overlay.version)),
            );
            continue;
        }
        if !path.is_file() {
            return Err(NuError::SourceDisappeared {
                path: path.to_path_buf(),
            });
        }
        let digest = modules::digest(&file.content);
        insert_dependency(
            captured,
            Dependency {
                path: path.to_path_buf(),
                canonical_path: std::fs::canonicalize(path)?,
                digest: digest.clone(),
            },
        )?;
        used.insert(identity, (digest, None));
    }
    Ok((used, missing))
}

fn collect_origins<'a>(
    configured: &[modules::Library],
    builtins: impl Iterator<Item = &'a BuiltinModule>,
    used_sources: &UsedSources,
) -> Vec<SourceOrigin> {
    let mut origins = Vec::new();
    for lib in configured {
        for (path, (digest, version)) in used_sources {
            if let Ok(relative) = path.strip_prefix(&lib.root) {
                origins.push(SourceOrigin {
                    path: path.clone(),
                    relative: relative.to_path_buf(),
                    origin: lib.origin.clone(),
                    digest: digest.clone(),
                    version: *version,
                });
            }
        }
    }
    for builtin in builtins {
        let lib = modules::Library::builtin(builtin);
        for &(name, bytes) in builtin.sources {
            origins.push(SourceOrigin {
                path: lib.root.join(name),
                relative: name.into(),
                origin: lib.origin.clone(),
                digest: modules::digest(bytes),
                version: None,
            });
        }
    }
    origins
}

fn insert_dependency(inputs: &mut BTreeMap<PathBuf, Dependency>, dep: Dependency) -> Result<()> {
    if let Some(prev) = inputs.get(&dep.path)
        && (prev.digest != dep.digest || prev.canonical_path != dep.canonical_path)
    {
        return Err(NuError::InputChangedDuringSetup {
            path: dep.path.clone(),
        });
    }
    inputs.insert(dep.path.clone(), dep);
    Ok(())
}

fn evaluate_file(
    path: &Path,
    args: &[String],
    read_stdin: bool,
    input_scope: Option<&str>,
    engine: &mut EngineState,
    stack: &mut Stack,
) -> i32 {
    if let Some(scope) = input_scope {
        engine.add_env_var(
            "TOLA_INPUT_SCOPE".into(),
            Value::string(scope, Span::unknown()),
        );
    }

    let input = if read_stdin {
        match ByteStream::stdin(Span::unknown()) {
            Ok(stream) => PipelineData::byte_stream(stream, None),
            Err(err) => {
                report_shell_error(Some(stack), engine, &err);
                return 1;
            }
        }
    } else {
        PipelineData::empty()
    };
    execution::evaluate(path, args, engine, stack, input)
}

fn initialize_engine(
    is_lsp: bool,
    ctx: &InvocationContext,
) -> std::result::Result<(EngineState, Stack), ()> {
    // Nu's environment merges (including display hooks) must keep PWD engine-local.
    nu_protocol::engine::UPDATE_CWD.store(false, Ordering::Relaxed);
    let engine = nu_cmd_lang::add_default_context(EngineState::new());
    let engine = nu_command::add_shell_command_context(engine);
    let engine = nu_cmd_extra::add_extra_command_context(engine);
    let engine = nu_cli::add_cli_context(engine);
    let engine = nu_explore::add_explore_context(engine);
    let mut engine = nu_tui::add_tui_context(engine);
    engine.is_lsp = is_lsp;
    if let Err(err) = nu_command::init_shared_memory_db() {
        report_shell_error(None, &engine, &err);
        return Err(());
    }
    let _ = nu_cmd_lang::VERSION_NU_FEATURES.set(
        ["network", "rustls-tls", "sqlite", "trash-support"]
            .into_iter()
            .map(Into::into)
            .collect(),
    );
    // `rustls-tls` reads its provider from this static, not from rustls's process default;
    // without it every `http` command fails, plain HTTP included.
    nu_command::tls::CRYPTO_PROVIDER.default();

    let mut stack = Stack::new();
    if initialize_environment(&mut engine, &mut stack, ctx).is_err() {
        return Err(());
    }
    Ok((engine, stack))
}

/// `--`-prefixed arguments are passed through verbatim by `escape_for_script_arg`; a flag name
/// carrying a separator would splice a second pipeline into `main …`.
fn flag_name_is_unsafe(name: &str) -> bool {
    name.chars().any(|c| {
        c.is_whitespace()
            || c.is_control()
            || matches!(
                c,
                '(' | ')' | '[' | ']' | '{' | '}' | '\'' | '`' | '"' | '\\' | ';' | '|' | '#' | '$'
            )
    })
}

fn escape_argument(arg: &str) -> String {
    if let Some(opt) = arg.strip_prefix("--") {
        let name = opt.split_once('=').map_or(opt, |(name, _)| name);
        if flag_name_is_unsafe(name) {
            return nu_utils::escape_quote_string(arg);
        }
    }
    nu_parser::escape_for_script_arg(arg)
}

fn install_ctrlc_handler(engine: &mut EngineState) -> std::result::Result<(), ctrlc::Error> {
    let interrupt = Arc::new(AtomicBool::new(false));
    engine.set_signals(Signals::new(interrupt.clone()));
    let handlers = Handlers::new();
    let jobs = engine.jobs.clone();
    handlers
        .register_unguarded(Box::new(move |action| {
            if action == SignalAction::Interrupt
                && let Ok(mut jobs) = jobs.lock()
            {
                let _ = jobs.kill_all();
            }
        }))
        .expect("failed to register Nushell interrupt handler");
    engine.signal_handlers = Some(handlers.clone());
    ctrlc::set_handler(move || {
        interrupt.store(true, Ordering::Relaxed);
        handlers.run(SignalAction::Interrupt);
    })
    .inspect_err(|err| eprintln!("Could not install the script interrupt handler: {err}"))
}

fn initialize_environment(
    engine: &mut EngineState,
    stack: &mut Stack,
    ctx: &InvocationContext,
) -> std::result::Result<(), ()> {
    engine.add_env_var(
        "config".into(),
        Config::default().into_value(Span::unknown()),
    );
    engine.add_env_var(
        "ENV_CONVERSIONS".into(),
        Value::record(record! {}, Span::unknown()),
    );
    nu_cli::gather_parent_env_vars(engine, &ctx.directory);
    let exe = std::env::current_exe().map_err(|err| {
        eprintln!("Could not determine the Tola executable: {err}");
    })?;
    let exe = exe.to_str().ok_or_else(|| {
        eprintln!("The Tola executable path is not valid UTF-8");
    })?;
    engine.add_env_var(
        "TOLA_EXECUTABLE".into(),
        Value::string(exe, Span::unknown()),
    );
    for (name, val) in &ctx.environment {
        match val {
            Some(val) => engine.add_env_var(name.clone(), Value::string(val, Span::unknown())),
            None => {
                for env in Arc::make_mut(&mut engine.env_vars).values_mut() {
                    env.retain(|key, _| key.as_str() != name);
                }
            }
        }
    }
    let dir = ctx
        .directory
        .to_str()
        .ok_or_else(|| eprintln!("Nu invocation directory is not UTF-8"))?;
    engine.add_env_var("PWD".into(), Value::string(dir, Span::unknown()));
    if let Err(err) = nu_engine::convert_env_values(engine, stack) {
        report_shell_error(None, engine, &err);
        return Err(());
    }
    palette::apply(engine);
    let lib_dirs = Value::list(Vec::new(), Span::unknown());
    engine.add_env_var("NU_LIB_DIRS".into(), lib_dirs.clone());
    let mut ws = StateWorkingSet::new(engine);
    let id = ws.add_variable(
        b"$NU_LIB_DIRS".into(),
        Span::unknown(),
        Type::List(Box::new(Type::String)),
        false,
    );
    ws.set_variable_const_val(id, lib_dirs);
    if let Err(err) = engine.merge_delta(ws.render()) {
        report_shell_error(None, engine, &err);
        return Err(());
    }
    engine.add_env_var(
        "NU_VERSION".into(),
        Value::string(NU_VERSION, Span::unknown()),
    );
    if let Err(err) = nu_std::load_standard_library(engine) {
        eprintln!("{err:?}");
        return Err(());
    }
    engine.is_interactive = false;
    engine.is_login = false;
    engine.history_enabled = false;
    engine.finish_startup();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scenario(tempfile::TempDir);

    impl Scenario {
        fn new() -> Self {
            Self(tempfile::tempdir().unwrap())
        }
        fn root(&self) -> &Path {
            self.0.path()
        }
    }

    fn evaluate(runtime: &Runtime, src: &str) -> Value {
        let mut engine = runtime.engine.clone();
        let mut stack = runtime.stack.clone();
        let mut ws = StateWorkingSet::new(&engine);
        let block = nu_parser::parse(&mut ws, Some("runtime test"), src.as_bytes(), false);
        assert!(ws.parse_errors.is_empty(), "{:?}", ws.parse_errors);
        assert!(ws.compile_errors.is_empty(), "{:?}", ws.compile_errors);
        engine.merge_delta(ws.render()).unwrap();
        nu_engine::eval_block::<nu_protocol::debugger::WithoutDebug>(
            &engine,
            &mut stack,
            &block,
            PipelineData::empty(),
        )
        .unwrap()
        .body
        .into_value(Span::unknown())
        .unwrap()
    }

    fn module_root(scenario: &Scenario, alias: &str, src: &str) -> PathBuf {
        let root = scenario.root().join(alias);
        std::fs::create_dir_all(root.join("tools")).unwrap();
        std::fs::write(
            root.join("tola-module.toml"),
            "[nu]\nentry='tools/mod.nu'\n",
        )
        .unwrap();
        std::fs::write(root.join("tools/mod.nu"), src).unwrap();
        root
    }

    fn library(root: &Path, alias: &str) -> ScriptModule {
        ScriptModule {
            alias: alias.into(),
            root: root.into(),
            entry: "tools/mod.nu".into(),
            manifest: root.join("tola-module.toml"),
        }
    }

    #[test]
    fn syntax_highlight_native() {
        let runtime = Runtime::new(Vec::new(), Vec::new()).unwrap();
        let val = evaluate(
            &runtime,
            r#"use tola
"fn main() {}" | tola syntax-highlight rust | str contains '[38;2;'"#,
        );
        assert!(val.as_bool().unwrap());
    }

    #[test]
    fn native_cmd_isolation() {
        let scenario = Scenario::new();
        let runtime = Runtime::for_invocation(
            Vec::new(),
            Vec::new(),
            InvocationContext {
                directory: scenario.root().to_path_buf(),
                environment: BTreeMap::new(),
            },
        )
        .unwrap();
        let mut ws = StateWorkingSet::new(&runtime.engine);
        nu_parser::parse(&mut ws, None, b"use tola-native", false);
        assert!(matches!(
            ws.parse_errors.first(),
            Some(nu_protocol::ParseError::ModuleNotFound(..))
        ));
        for cmd in [
            "inspect-image",
            "resize-image",
            "render-diagnostics",
            "submit",
            "syntax-highlight",
        ] {
            assert!(runtime.engine.find_decl(cmd.as_bytes(), &[]).is_none());
            assert!(nu_engine::find_builtin_decl(&runtime.engine, cmd).is_none());
        }
    }

    #[test]
    fn diagnostic_host_roundtrip() {
        let submitted = Arc::new(OnceLock::new());
        let rendered = Arc::new(OnceLock::new());
        let submit = submitted.clone();
        let render = rendered.clone();
        let mut runtime = Runtime::new(Vec::new(), Vec::new())
            .unwrap()
            .with_host(Host {
                submit_diagnostics: Some(Arc::new(move |issues| {
                    submit.set(issues).unwrap();
                    Ok(())
                })),
                render_diagnostics: Some(Arc::new(move |_, issues, _| {
                    render.set(issues).unwrap();
                    Ok(String::new())
                })),
                ..Host::default()
            })
            .unwrap();
        evaluate(
            &runtime,
            "use tola diagnostic\n[(diagnostic make 'test')] | diagnostic submit",
        );
        let issues = serde_json::json!([{ "severity": "warning", "message": "test" }]);
        assert_eq!(submitted.get(), Some(&issues));

        let entry = runtime.resolve_locator("@tola-scripts/diagnostic").unwrap();
        let input = Value::string(issues.to_string(), Span::test_data());
        assert_eq!(
            execution::evaluate(
                &entry.path,
                &["render".into()],
                &mut runtime.engine,
                &mut runtime.stack,
                PipelineData::value(input, None),
            ),
            0
        );
        assert_eq!(rendered.get(), Some(&issues));
    }

    #[test]
    fn script_main_dot_path() {
        let scenario = Scenario::new();
        let script = scenario.root().join("./main.nu");
        std::fs::write(&script, "def main [code: int] { exit $code }").unwrap();
        let runtime = Runtime::new(Vec::new(), Vec::new()).unwrap();
        assert_eq!(runtime.run_embedded(&script, &["7".into()]).unwrap(), 7);
    }

    #[test]
    fn overlay_overrides_disk() {
        let scenario = Scenario::new();
        let root = module_root(&scenario, "catalog", "export use ../shared.nu name\n");
        let disk = "export def broken [ {";
        let buffer = "export def name [] { 'unsaved glossary' }";
        std::fs::write(root.join("shared.nu"), disk).unwrap();
        let overlay = SourceOverlay {
            path: root.join("shared.nu"),
            text: buffer.into(),
            version: 7,
        };
        let runtime = Runtime::for_invocation_with_overlays(
            vec![library(&root, "catalog")],
            Vec::new(),
            InvocationContext::current().unwrap(),
            vec![overlay.clone()],
        )
        .unwrap();
        assert_eq!(
            evaluate(&runtime, "use tola-modules catalog\ncatalog name")
                .as_str()
                .unwrap(),
            "unsaved glossary"
        );
        let origin = runtime
            .origins()
            .iter()
            .find(|origin| origin.relative == Path::new("shared.nu"))
            .unwrap();
        assert_eq!(origin.version, Some(7));
        assert_eq!(origin.digest, modules::digest(buffer.as_bytes()));
        assert!(
            runtime
                .dependencies()
                .iter()
                .any(|dep| dep.path.ends_with("shared.nu")
                    && dep.digest == modules::digest(disk.as_bytes()))
        );
        runtime.ensure_fresh().unwrap();
        let conflict = SourceOverlay {
            text: "export def name [] { 'conflict' }".into(),
            ..overlay.clone()
        };
        assert!(
            Runtime::for_invocation_with_overlays(
                vec![library(&root, "catalog")],
                Vec::new(),
                InvocationContext::current().unwrap(),
                vec![overlay, conflict]
            )
            .is_err()
        );
    }

    #[test]
    fn unsaved_import_invalidates() {
        let scenario = Scenario::new();
        let root = module_root(&scenario, "catalog", "export use ./unsaved.nu name");
        let path = root.join("tools/unsaved.nu");
        let text = "export def name [] { 'new helper' }";
        let runtime = Runtime::for_invocation_with_overlays(
            vec![library(&root, "catalog")],
            Vec::new(),
            InvocationContext::current().unwrap(),
            vec![SourceOverlay {
                path: path.clone(),
                text: text.into(),
                version: 3,
            }],
        )
        .unwrap();
        assert_eq!(
            evaluate(&runtime, "use tola-modules catalog\ncatalog name")
                .as_str()
                .unwrap(),
            "new helper"
        );
        assert_eq!(runtime.missing_sources().len(), 1);
        assert!(
            runtime
                .origins()
                .iter()
                .any(|origin| origin.relative == Path::new("tools/unsaved.nu")
                    && origin.version == Some(3))
        );
        runtime.ensure_fresh().unwrap();
        std::fs::write(path, text).unwrap();
        assert!(runtime.ensure_fresh().is_err());
    }

    #[test]
    fn quote_flag_values() {
        assert_eq!(escape_argument("--two words"), r#""--two words""#);
        assert_eq!(
            escape_argument("--x; print injected"),
            r#""--x; print injected""#
        );
        assert_eq!(escape_argument("--name=a b"), r#"--name="a b""#);
        assert_eq!(escape_argument("--help"), "--help");
    }
}
