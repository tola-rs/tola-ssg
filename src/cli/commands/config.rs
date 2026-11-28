//! The resolved configuration and the paths it selects, as one JSON document for
//! `tola inspect config`.
//!
//! Preparing modules resolves their declared paths without executing a driver, so the document
//! states what a build would use while running none of the site's code.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;
use tola_build::config::section::build::HookJobs;
use tola_build::config::section::build::hooks::{DevParticipation, HookStage, ModuleHookOrigin};
use tola_build::modules::LoadedSite;
use tola_diagnostic::{SourcePositions, SourceRange, display_path};
use tola_typst_syntax::style::ProseWrap;

use crate::cli::BuildOverrideArgs;
use crate::cli::args::InspectConfigMode;

#[derive(Serialize)]
struct ModuleDocument<'a> {
    root: &'a Path,
    manifest: &'a Path,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_tola_version: Option<&'a tola_module::TolaVersion>,
    capabilities: Vec<tola_module::ModuleCapability>,
    library: Option<PathBuf>,
    build_entry: Option<PathBuf>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    modules: BTreeMap<String, ModuleDocument<'a>>,
}

fn module_document<'a>(
    module: &'a tola_module::LoadedModule,
    all: &'a BTreeMap<String, tola_module::LoadedModule>,
) -> ModuleDocument<'a> {
    let children = module
        .child_aliases
        .iter()
        .filter_map(|(sub_alias, scoped)| {
            all.get(scoped.as_str())
                .map(|child| (sub_alias.as_str().to_owned(), module_document(child, all)))
        })
        .collect();
    ModuleDocument {
        root: module.root.as_path(),
        manifest: module.manifest.as_path(),
        min_tola_version: module.min_tola_version.as_ref(),
        capabilities: module.capabilities(),
        library: module
            .typst
            .as_ref()
            .and_then(|typst| typst.entry.as_ref())
            .map(|entry| module.root.join(entry)),
        build_entry: module
            .build
            .as_ref()
            .map(|entry| module.root.join(&entry.entry)),
        modules: children,
    }
}

#[derive(Serialize)]
struct MinifyDocument {
    html: bool,
    css: bool,
    javascript: bool,
}

#[derive(Serialize)]
struct SiteDocument<'a> {
    origin: Option<&'a str>,
    base_path: String,
    url: Option<String>,
}

#[derive(Serialize)]
struct FormatterDocument<'a> {
    indent: usize,
    print_width: usize,
    prose_wrap: &'a ProseWrap,
}

#[derive(Serialize)]
struct ServerDocument {
    interface: String,
    port: u16,
}

#[derive(Serialize)]
struct DevDocument {
    watch: bool,
}

#[derive(Serialize)]
struct ConfigDocument<'a> {
    config: &'a Path,
    root: &'a Path,
    content: &'a Path,
    entry: &'a Path,
    entry_module: Option<&'a str>,
    modules: BTreeMap<String, ModuleDocument<'a>>,
    output: &'a Path,
    minify: MinifyDocument,
    site: SiteDocument<'a>,
    formatter: FormatterDocument<'a>,
    package_path: Option<&'a Path>,
    package_cache_path: Option<&'a Path>,
    vendored_package_path: Option<PathBuf>,
    icon_cache_directory: PathBuf,
    system_fonts: bool,
    server: ServerDocument,
    dev: DevDocument,
    hooks: HookDocument<'a>,
}

pub(super) fn value(
    request: &super::SiteRequest<'_>,
    build: &BuildOverrideArgs,
    mode: InspectConfigMode,
) -> Result<serde_json::Value> {
    let overrides = crate::cli::config::config_overrides(build);
    let loaded = request.load_config(&overrides)?;
    let mut prepared = tola_build::modules::prepare_site(
        loaded.config(),
        tola_input::InputMode::new(request.scope(), true),
        &request.cancellation.token(),
    )?;
    tola_build::build::install_hooks(&mut prepared, &request.resources)?;
    let config = &prepared;
    let all_modules = &config.prepared().modules;
    let modules = config
        .loaded_modules()
        .map(|(alias, module)| (alias.to_owned(), module_document(module, all_modules)))
        .collect();
    let document = ConfigDocument {
        config: config.config_path(),
        root: config.root(),
        content: config.build().content_dir.as_path(),
        entry: config.build().entry.as_path(),
        entry_module: config.module_entry(),
        modules,
        output: config.build().publish_dir.as_path(),
        minify: MinifyDocument {
            html: config.build().minify.html,
            css: config.build().minify.css,
            javascript: config.build().minify.javascript,
        },
        site: SiteDocument {
            origin: config.site().origin.as_deref(),
            base_path: config.site().base_path.base_path(),
            url: config.site_url(),
        },
        formatter: FormatterDocument {
            indent: config.formatter().indent,
            print_width: config.formatter().print_width,
            prose_wrap: &config.formatter().prose_wrap,
        },
        package_path: config
            .package_locations()
            .data()
            .map(|location| location.root()),
        package_cache_path: config
            .package_locations()
            .cache()
            .map(|location| location.root()),
        vendored_package_path: config.vendor().typst_packages(),
        icon_cache_directory: tola_build::BuildResources::icon_cache_directory(config.root()),
        system_fonts: config.system_fonts_allowed(&request.resources),
        server: ServerDocument {
            interface: loaded.server().interface.to_string(),
            port: loaded.server().port,
        },
        dev: DevDocument {
            watch: loaded.dev().watch,
        },
        hooks: HookDocument::new(config, mode),
    };
    let mut value = serde_json::to_value(document)?;
    if let Some(source) = config.source() {
        value["config_sources"] = serde_json::Value::Array(source.applications().iter().map(|app| {
            serde_json::json!({
                "id": app.id,
                "path": display_path(&app.path, config.root()),
                "state": match &app.state {
                    tola_build::config::ConfigFileState::Loaded => "loaded",
                    tola_build::config::ConfigFileState::OptionalMissing => "optional-missing",
                },
                "apply_order": app.apply_order,
                "included_by": app.included_by.as_ref().map(|parent| serde_json::json!({
                    "source_id": parent.source_id, "line": parent.line,
                    "column": parent.column, "range": parent.range,
                })),
            })
        }).collect());
        value["field_origins"] = field_origins(source, config, build)?;
    }
    Ok(value)
}

#[derive(Default)]
struct OriginList {
    origins: Vec<serde_json::Value>,
    overridden: Vec<serde_json::Value>,
}

type OriginMap = BTreeMap<Vec<tola_config::ValueStep>, OriginList>;

fn field_origins(
    source: &tola_build::config::ConfigSource,
    site: &LoadedSite,
    build: &BuildOverrideArgs,
) -> Result<serde_json::Value> {
    let (core, host) = source.decode_with_host::<crate::config::ConfigSchema>()?;
    let mut entries = OriginMap::new();
    for (path, origins) in source.origins() {
        if path.is_empty() {
            continue;
        }
        entries.insert(
            path.clone(),
            OriginList {
                origins: origins
                    .origins
                    .iter()
                    .map(|origin| file_origin(source, origin))
                    .collect(),
                overridden: origins
                    .overridden
                    .iter()
                    .map(|origin| file_origin(source, origin))
                    .collect(),
            },
        );
    }
    add_defaults(
        &toml::Value::try_from(core.schema())?,
        &mut Vec::new(),
        &mut entries,
    );
    add_defaults(
        &toml::Value::try_from(host.schema())?,
        &mut Vec::new(),
        &mut entries,
    );
    for (alias, module) in site.loaded_modules() {
        let Some(evidence) = &module.manifest_evidence else {
            continue;
        };
        let manifest: toml::Value = toml::from_str(evidence.source())?;
        let defaults = manifest.get("options");
        let positions = evidence.positions();
        let overrides = core
            .schema()
            .modules
            .get(alias)
            .map(|declaration| toml::Value::Table(declaration.options.clone()));
        let mut path = vec![step("modules"), step(alias), step("options")];
        let mut original = vec![step("options")];
        let values = toml::Value::Table(module.options.clone());
        module_origins(
            &values,
            defaults,
            overrides.as_ref(),
            &mut path,
            &mut original,
            &ModuleOrigin {
                alias,
                module,
                site_root: site.root(),
                positions: positions.as_ref(),
            },
            &mut entries,
        );
    }
    for (enabled, flag, paths) in [
        (
            build.publish_dir.is_some(),
            "--publish-dir",
            &["build.publish-dir"][..],
        ),
        (
            build.minify.is_some(),
            "--minify",
            &[
                "build.minify.html",
                "build.minify.css",
                "build.minify.javascript",
            ][..],
        ),
        (build.origin.is_some(), "--origin", &["site.origin"][..]),
        (
            build.base_path.is_some(),
            "--base-path",
            &["site.base-path"][..],
        ),
    ] {
        if enabled {
            for path in paths {
                let key = path.split('.').map(step).collect::<Vec<_>>();
                let entry = entries.entry(key).or_default();
                entry.overridden.append(&mut entry.origins);
                entry
                    .origins
                    .push(serde_json::json!({"kind": "cli", "argument": flag}));
            }
        }
    }
    Ok(serde_json::Value::Array(entries.into_iter().map(|(path, entry)| serde_json::json!({
        "key_path": key_path(&path), "origins": entry.origins, "overridden": entry.overridden,
    })).collect()))
}

fn step(key: &str) -> tola_config::ValueStep {
    tola_config::ValueStep::Key(key.to_owned())
}

fn add_defaults(
    value: &toml::Value,
    path: &mut Vec<tola_config::ValueStep>,
    entries: &mut OriginMap,
) {
    if !path.is_empty() {
        entries.entry(path.clone()).or_insert_with(|| OriginList {
            origins: vec![serde_json::json!({"kind": "default", "key_path": key_path(path)})],
            overridden: Vec::new(),
        });
    }
    match value {
        toml::Value::Table(table) => {
            for (key, value) in table {
                path.push(step(key));
                add_defaults(value, path, entries);
                path.pop();
            }
        }
        toml::Value::Array(array) => {
            for (index, value) in array.iter().enumerate() {
                path.push(tola_config::ValueStep::Index(index));
                add_defaults(value, path, entries);
                path.pop();
            }
        }
        _ => {}
    }
}

struct ModuleOrigin<'a> {
    alias: &'a str,
    module: &'a tola_module::LoadedModule,
    site_root: &'a Path,
    positions: Option<&'a SourcePositions>,
}

fn module_origins(
    value: &toml::Value,
    default: Option<&toml::Value>,
    override_value: Option<&toml::Value>,
    path: &mut Vec<tola_config::ValueStep>,
    original: &mut Vec<tola_config::ValueStep>,
    module: &ModuleOrigin<'_>,
    entries: &mut OriginMap,
) {
    let mut pointer = String::new();
    for step in original.iter() {
        match step {
            tola_config::ValueStep::Key(key) => tola_diagnostic::push_pointer(&mut pointer, key),
            tola_config::ValueStep::Index(index) => {
                tola_diagnostic::push_pointer(&mut pointer, &index.to_string())
            }
        }
    }
    let written = module
        .positions
        .and_then(|position| position.at_pointer(&pointer));
    let excerpt = written.as_ref().and_then(|position| position.excerpt());
    let mut origin = serde_json::json!({"kind": "module", "alias": module.alias,
        "path": display_path(&module.module.manifest, module.site_root), "key_path": key_path(original)});
    if let Some(excerpt) = excerpt {
        origin["line"] = serde_json::json!(excerpt.line);
        origin["column"] = serde_json::json!(excerpt.column);
    }
    if let Some(written) = written {
        origin["range"] = serde_json::json!(written.range());
    }
    if default.is_some() {
        let entry = entries.entry(path.clone()).or_default();
        let merge_tables = default.and_then(toml::Value::as_table).is_some()
            && override_value.and_then(toml::Value::as_table).is_some();
        if override_value.is_some() && !merge_tables {
            entry.overridden.insert(0, origin);
        } else {
            entry.origins.retain(|item| item["kind"] != "default");
            entry.origins.insert(0, origin);
        }
    }
    if let toml::Value::Table(table) = value {
        for (key, child) in table {
            path.push(step(key));
            original.push(step(key));
            module_origins(
                child,
                default.and_then(|value| value.get(key)),
                override_value.and_then(|value| value.get(key)),
                path,
                original,
                module,
                entries,
            );
            original.pop();
            path.pop();
        }
    } else if let toml::Value::Array(array) = value {
        for (index, child) in array.iter().enumerate() {
            path.push(tola_config::ValueStep::Index(index));
            original.push(tola_config::ValueStep::Index(index));
            module_origins(
                child,
                default.and_then(|value| value.get(index)),
                override_value.and_then(|value| value.get(index)),
                path,
                original,
                module,
                entries,
            );
            original.pop();
            path.pop();
        }
    }
}

fn key_path(path: &[tola_config::ValueStep]) -> Vec<serde_json::Value> {
    path.iter()
        .map(|step| match step {
            tola_config::ValueStep::Key(key) => serde_json::json!(key),
            tola_config::ValueStep::Index(index) => serde_json::json!(index),
        })
        .collect()
}

fn file_origin(
    source: &tola_build::config::ConfigSource,
    origin: &tola_build::config::ConfigOrigin,
) -> serde_json::Value {
    let position = source.origin_positions(origin);
    let excerpt = position.as_ref().and_then(|position| position.excerpt());
    let mut result = serde_json::json!({
        "kind": "file", "source_id": origin.source_id, "key_path": key_path(&origin.key_path),
    });
    if let Some(excerpt) = excerpt {
        result["line"] = serde_json::json!(excerpt.line);
        result["column"] = serde_json::json!(excerpt.column);
    }
    if let Some(position) = position {
        result["range"] = serde_json::json!(position.range());
    }
    result
}

#[derive(Serialize)]
struct HookDocument<'a> {
    mode: &'static str,
    par_hooks: bool,
    hook_jobs: HookJobsDocument,
    stages: BTreeMap<&'static str, HookStageDocument<'a>>,
}

#[derive(Serialize)]
struct HookJobsDocument {
    requested: HookJobs,
    resolved: usize,
    effective: usize,
}

#[derive(Serialize)]
struct HookStageDocument<'a> {
    declarations: Vec<HookDeclaration<'a>>,
    batches: Vec<HookBatch<'a>>,
    reuse: HookReuse,
}

#[derive(Serialize)]
struct HookDeclaration<'a> {
    index: usize,
    owner: HookOwner<'a>,
    name: &'a str,
    order: u64,
    source: serde_json::Value,
    participates: bool,
    skip_reason: Option<&'static str>,
    reuse: HookReuse,
}

#[derive(Clone, Copy, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum HookOwner<'a> {
    Site,
    Module { alias: &'a str },
}

#[derive(Serialize)]
struct HookSource<'a> {
    kind: &'static str,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    alias: Option<&'a str>,
    key_path: (&'static str, &'static str, usize),
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    column: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    range: Option<SourceRange>,
}

#[derive(Serialize)]
struct HookBatch<'a> {
    index: usize,
    owner: HookOwner<'a>,
    order: u64,
    declared: Vec<usize>,
    participants: Vec<usize>,
}

#[derive(Serialize)]
struct HookReuse {
    eligible: bool,
    hit: Option<bool>,
}

impl HookReuse {
    fn new(eligible: bool) -> Self {
        Self {
            eligible,
            hit: None,
        }
    }
}

/// Project the fields shared by all three configured hook types for display.
struct Hook<'a> {
    name: &'a str,
    origin: Option<&'a ModuleHookOrigin>,
    order: u64,
    enable: bool,
    option_disabled: bool,
    dev: DevParticipation,
}

impl<'a> HookDocument<'a> {
    fn new(site: &'a LoadedSite, mode: InspectConfigMode) -> Self {
        let development = mode == InspectConfigMode::Dev;
        let build = site.build();
        let auto_limit = std::thread::available_parallelism().map_or(1, |cores| cores.get());
        let hooks = site.hooks();
        let module_positions: BTreeMap<_, _> = site
            .loaded_modules()
            .filter_map(|(alias, module)| {
                Some((alias, module.manifest_evidence.as_ref()?.positions()?))
            })
            .collect();
        let mut stages = BTreeMap::new();
        macro_rules! add_stage {
            ($stage:ident, $hooks:ident) => {{
                let stage = HookStage::$stage;
                let hooks = hooks.$hooks.iter().map(|hook| Hook {
                    name: hook.declared_name.as_deref().unwrap_or(&hook.name),
                    origin: hook.origin.as_ref(),
                    order: hook.order,
                    enable: hook.enable,
                    option_disabled: hook.option_disabled,
                    dev: hook.dev,
                });
                stages.insert(
                    stage.as_str(),
                    HookStageDocument::new(
                        site,
                        &module_positions,
                        stage,
                        hooks.collect(),
                        development,
                    ),
                );
            }};
        }
        add_stage!(BeforeBuild, before_build);
        add_stage!(GenerateOutputs, generate_outputs);
        add_stage!(AfterPublish, after_publish);
        Self {
            mode: if development { "dev" } else { "prod" },
            par_hooks: build.par_hooks,
            hook_jobs: HookJobsDocument {
                requested: build.hook_jobs,
                resolved: build.hook_jobs.count(auto_limit),
                effective: tola_build::hooks::hook_limit(
                    build.par_hooks,
                    build.hook_jobs,
                    auto_limit,
                ),
            },
            stages,
        }
    }
}

impl<'a> HookStageDocument<'a> {
    fn new(
        site: &'a LoadedSite,
        module_positions: &BTreeMap<&str, SourcePositions>,
        stage: HookStage,
        hooks: Vec<Hook<'a>>,
        development: bool,
    ) -> Self {
        let stage_name = stage.as_str();
        let mut owner_indices = BTreeMap::new();
        let declarations: Vec<_> = hooks
            .iter()
            .enumerate()
            .map(|(index, hook)| {
                let alias = hook.origin.map(|origin| origin.name.as_str());
                let local_index = owner_indices.entry(alias).or_insert(0);
                let source = hook.source(site, module_positions, stage_name, *local_index);
                *local_index += 1;
                let skip_reason = hook.skip_reason(development);
                let participates = skip_reason.is_none();
                HookDeclaration {
                    index,
                    owner: alias.map_or(HookOwner::Site, |alias| HookOwner::Module { alias }),
                    name: hook.name,
                    order: hook.order,
                    source,
                    participates,
                    skip_reason,
                    reuse: HookReuse::new(development && participates && hook.dev.reuses_outputs()),
                }
            })
            .collect();
        let batches = tola_build::hooks::hook_batches(
            &hooks,
            stage == HookStage::BeforeBuild,
            |hook| hook.origin.map(|origin| origin.name.as_str()),
            |hook| hook.order,
        )
        .into_iter()
        .enumerate()
        .map(|(index, declared)| {
            let first = &declarations[declared[0]];
            let participants = declared
                .iter()
                .copied()
                .filter(|&index| declarations[index].participates)
                .collect();
            HookBatch {
                index,
                owner: first.owner,
                order: first.order,
                declared,
                participants,
            }
        })
        .collect();
        let reuse = HookReuse::new(
            development
                && stage == HookStage::AfterPublish
                && declarations.iter().any(|hook| hook.participates),
        );
        Self {
            declarations,
            batches,
            reuse,
        }
    }
}

impl<'a> Hook<'a> {
    fn skip_reason(&self, development: bool) -> Option<&'static str> {
        if !self.enable {
            Some("enable-false")
        } else if self.option_disabled {
            Some("enabled-by-false")
        } else if development && !self.dev.participates_in_development() {
            Some("dev-skip")
        } else {
            None
        }
    }

    fn source(
        &self,
        site: &LoadedSite,
        module_positions: &BTreeMap<&str, SourcePositions>,
        stage: &'static str,
        index: usize,
    ) -> serde_json::Value {
        if self.origin.is_none() {
            let path = vec![
                tola_config::ValueStep::Key("hooks".into()),
                tola_config::ValueStep::Key(stage.into()),
                tola_config::ValueStep::Index(index),
            ];
            if let Some(source) = site.source()
                && let Some(origin) = source
                    .origins()
                    .get(&path)
                    .and_then(|entry| entry.origins.last())
            {
                return file_origin(source, origin);
            }
        }
        let (kind, path, alias, positions) = match self.origin {
            Some(origin) => (
                "module",
                origin.manifest.as_path(),
                Some(origin.name.as_str()),
                module_positions.get(origin.name.as_str()),
            ),
            None => ("file", site.config_path(), None, site.positions()),
        };
        let key = format!("hooks.{stage}.{index}");
        let excerpt = positions.and_then(|positions| positions.excerpt(&key));
        serde_json::to_value(HookSource {
            kind,
            path: display_path(path, site.root()),
            alias,
            key_path: ("hooks", stage, index),
            line: excerpt.as_ref().map(|excerpt| excerpt.line),
            column: excerpt.as_ref().map(|excerpt| excerpt.column),
            range: positions.and_then(|positions| positions.written(&key)),
        })
        .expect("hook source is serializable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancellation::Cancellation;
    use crate::cli::{ConfigFileArgs, TypstPackageArgs};
    use tola_input::InputScope;

    #[test]
    fn theme_resolved_without_driver() {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("project");
        let theme = dir.path().join("theme");
        std::fs::create_dir_all(proj.join("content")).unwrap();
        std::fs::create_dir(&theme).unwrap();
        std::fs::write(theme.join("lib.typ"), "").unwrap();
        std::fs::write(theme.join("site.typ"), "#panic(\"must not run\")").unwrap();
        std::fs::write(
            theme.join("tola-module.toml"),
            r#"
[typst]
entry = "lib.typ"
[build]
entry = "site.typ"
[[hooks.generate-outputs]]
name = "catalog"
argv = ["must-not-run"]
dev = "skip"
outputs = [{ file = "catalog.json" }]
[options]
shared = "default"
only-module = true
"#,
        )
        .unwrap();
        std::fs::write(
            proj.join("tola.toml"),
            r#"
[modules.notes]
path = "../theme"
[build]
entry = { module = "notes" }
par-hooks = false
hook-jobs = 2
[[hooks.before-build]]
name = "prepare"
nu = "prepare.nu"
dev = "reuse"
generates = ["content/generated.typ"]
[[hooks.generate-outputs]]
name = "search"
argv = ["must-not-run"]
dev = "reuse"
outputs = [{ file = "search.json" }]
[modules.notes.options]
shared = "site"
"#,
        )
        .unwrap();
        let out = super::super::testing::buffered_output();
        let cancel = Cancellation::default();
        let req = super::super::SiteRequest::new(
            crate::cli::SiteArgs {
                config: ConfigFileArgs {
                    path: Some(proj.join("tola.toml")),
                },
                packages: TypstPackageArgs::default(),
            },
            crate::cli::BuildArgs::default(),
            tola_input::InputMode::new(InputScope::Offline, false),
            &out,
            &cancel,
        )
        .unwrap();
        let shown = value(
            &req,
            &BuildOverrideArgs {
                minify: Some(false),
                ..Default::default()
            },
            InspectConfigMode::Dev,
        )
        .unwrap();
        let mod_doc = &shown["modules"]["notes"];
        assert_eq!(shown["entry_module"], "notes");
        assert_eq!(
            shown["entry"],
            serde_json::json!(theme.canonicalize().unwrap().join("site.typ"))
        );
        assert_eq!(
            mod_doc["library"],
            serde_json::json!(theme.canonicalize().unwrap().join("lib.typ"))
        );
        assert_eq!(mod_doc["build_entry"], shown["entry"]);
        assert_eq!(
            shown["content"],
            serde_json::json!(proj.canonicalize().unwrap().join("content"))
        );
        let origins = shown["field_origins"].as_array().unwrap();
        let field =
            |path: serde_json::Value| origins.iter().find(|e| e["key_path"] == path).unwrap();
        assert_eq!(
            field(serde_json::json!([
                "modules",
                "notes",
                "options",
                "only-module"
            ]))["origins"][0]["kind"],
            "module"
        );
        let shared = field(serde_json::json!(["modules", "notes", "options", "shared"]));
        assert_eq!(shared["origins"][0]["kind"], "file");
        assert_eq!(shared["overridden"][0]["kind"], "module");
        let minify = field(serde_json::json!(["build", "minify", "html"]));
        assert_eq!(minify["origins"][0]["argument"], "--minify");
        assert_eq!(minify["overridden"][0]["kind"], "default");
        let hooks = &shown["hooks"];
        assert_eq!(hooks["hook_jobs"]["effective"], 1);
        assert_eq!(
            hooks["stages"]["before-build"]["declarations"][0]["reuse"],
            serde_json::json!({"eligible": true, "hit": null})
        );
        let gens = &hooks["stages"]["generate-outputs"];
        let mod_hook = &gens["declarations"][0];
        assert_eq!(mod_hook["name"], "catalog");
        assert_eq!(mod_hook["owner"]["alias"], "notes");
        assert_eq!(mod_hook["source"]["line"], 6);
        assert_eq!(mod_hook["skip_reason"], "dev-skip");
        assert_eq!(gens["batches"][0]["participants"], serde_json::json!([]));
        assert_eq!(gens["batches"][1]["participants"], serde_json::json!([1]));
        assert_eq!(
            gens["declarations"][1]["reuse"],
            serde_json::json!({"eligible": true, "hit": null})
        );
        assert!(!proj.join("site.typ").exists());
        assert!(!proj.join("public").exists());
    }
}
