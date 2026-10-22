//! Evaluate a module's own Typst options schema without compiling the site.

#[path = "diagnostics.rs"]
mod diagnostics;

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use anyhow::{Result, anyhow, ensure};
use tola_packages::library::{HostInputs, ModuleValidation, SiteLibrary};
use tola_typst::{
    AccessedDeps, Diagnostics, ExplicitSources, FileMap, scan_world_observed, virtual_file_id,
};
use typst::foundations::{Array, Dict, Value};

use crate::cancellation::BuildCancellation;
use crate::compiler::TypstHost;
use crate::config::ResolvedSiteConfig;
use crate::modules::ConfiguredModule;

/// Validate declared options without discovering sources, running hooks, or publishing files.
pub fn validate_options(
    config: &ResolvedSiteConfig,
    resources: &crate::resources::BuildResources,
    cancellation: &BuildCancellation,
) -> Result<Vec<tola_diagnostic::Diagnostic>> {
    let host = TypstHost::for_module_options(config, resources, cancellation)?;
    match evaluate_all(config, &host, cancellation) {
        Ok(mut validated) => Ok(crate::compiler::warning_diagnostics(
            config,
            &mut validated.diagnostics,
        )),
        Err(failure) => {
            if crate::cancellation::is_cancelled(&failure.error) {
                return Err(failure.error);
            }
            let mut diagnostics = tola_diagnostic::attached(&failure.error)
                .map(<[tola_diagnostic::Diagnostic]>::to_vec)
                .or_else(|| crate::compiler::error_diagnostics(&failure.error, config.get_root()))
                .unwrap_or_default();
            if let Some(mut retained) = failure.diagnostics {
                for diagnostic in crate::compiler::warning_diagnostics(config, &mut retained) {
                    if !diagnostics.contains(&diagnostic) {
                        diagnostics.push(diagnostic);
                    }
                }
            }
            if !diagnostics
                .iter()
                .any(|item| item.severity == tola_diagnostic::Severity::Error)
            {
                diagnostics.push(tola_diagnostic::fallback(
                    tola_diagnostic::codes::config::INVALID,
                    &failure.error,
                ));
            }
            Err(tola_diagnostic::DiagnosticError::attach(failure.error, diagnostics).into())
        }
    }
}

/// Normalized options and every file read during schema evaluation.
#[derive(Debug)]
pub(crate) struct OptionsEvaluation {
    pub options: Dict,
    pub accessed: AccessedDeps,
    pub diagnostics: Diagnostics,
}

/// One evaluation shared by hook stage resolution and the resulting site compilation.
#[derive(Debug, Clone)]
pub(crate) struct ValidatedOptions {
    pub values: BTreeMap<String, Dict>,
    pub accessed: AccessedDeps,
    pub diagnostics: Diagnostics,
}

pub(crate) fn evaluate_all(
    config: &ResolvedSiteConfig,
    host: &TypstHost,
    cancellation: &BuildCancellation,
) -> std::result::Result<ValidatedOptions, OptionsFailure> {
    let mut validated = ValidatedOptions {
        values: BTreeMap::new(),
        accessed: AccessedDeps::default(),
        diagnostics: Diagnostics::new(),
    };
    for (alias, module) in &config.module_inputs.configured {
        let evaluate_module = || -> std::result::Result<OptionsEvaluation, OptionsFailure> {
            let raw = toml::Value::Table(module.options.clone().into_iter().collect());
            let Value::Dict(raw_options) = raw.try_into().map_err(anyhow::Error::msg)? else {
                unreachable!("a TOML table converts to a Typst dictionary");
            };
            evaluate(config, host, module, raw_options, cancellation)
        };
        let evaluated = match evaluate_module() {
            Ok(evaluated) => evaluated,
            Err(mut failure) => {
                failure.prepend_context(validated.accessed, validated.diagnostics);
                return Err(failure);
            }
        };
        validated.values.insert(alias.clone(), evaluated.options);
        validated.accessed = merge_accessed(validated.accessed, evaluated.accessed);
        validated
            .diagnostics
            .extend_distinct(&evaluated.diagnostics);
    }
    // Each schema wrapper has a unique transient ID, not a reusable input.
    validated.accessed.reads.retain(|read| {
        !matches!(
            read.evidence().locator(),
            tola_typst::ReadLocator::NonPersistent(_)
        )
    });
    if let Err(error) =
        crate::compiler::diagnostic::ensure_native_reports_succeed(config, &validated.diagnostics)
    {
        return Err(OptionsFailure {
            error,
            accessed: Box::new(validated.accessed),
            diagnostics: None,
        });
    }
    Ok(validated)
}

/// A failed evaluation and the dependencies observed before it stopped.
#[derive(Debug)]
pub struct OptionsFailure {
    pub error: anyhow::Error,
    pub accessed: Box<AccessedDeps>,
    pub diagnostics: Option<Diagnostics>,
}

impl fmt::Display for OptionsFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.error)
    }
}
impl std::error::Error for OptionsFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.error.as_ref())
    }
}

impl OptionsFailure {
    /// Retain observations from earlier successful modules before propagating this failure.
    pub(crate) fn prepend_context(
        &mut self,
        mut accessed: AccessedDeps,
        mut diagnostics: Diagnostics,
    ) {
        accessed.reads.append(&mut self.accessed.reads);
        accessed.disk_reads.append(&mut self.accessed.disk_reads);
        accessed
            .package_checks
            .append(&mut self.accessed.package_checks);
        *self.accessed = accessed;
        if let Some(failed) = &self.diagnostics {
            diagnostics.extend_distinct(failed);
        }
        self.diagnostics = (!diagnostics.is_empty()).then_some(diagnostics);
    }
}

impl From<anyhow::Error> for OptionsFailure {
    fn from(error: anyhow::Error) -> Self {
        Self {
            error,
            accessed: Box::default(),
            diagnostics: None,
        }
    }
}

/// Evaluate the exported schema under the already selected local package bindings.
/// The host must bind `tola-modules` packages before calling this function.
/// The caller supplies this module's raw options for the private schema pass.
/// No sources are discovered and no hooks, site program, or output are run.
pub(crate) fn evaluate(
    config: &ResolvedSiteConfig,
    host: &TypstHost,
    module: &ConfiguredModule,
    raw_options: Dict,
    cancellation: &BuildCancellation,
) -> std::result::Result<OptionsEvaluation, OptionsFailure> {
    cancellation.ensure_active().map_err(anyhow::Error::new)?;
    let Some(schema) = &module.module.options_schema else {
        return Ok(OptionsEvaluation {
            options: Dict::new(),
            accessed: AccessedDeps::default(),
            diagnostics: Diagnostics::new(),
        });
    };
    let path = crate::modules::prepared::package_path(&module.module.name, &schema.schema)?;
    let validation = ModuleValidation {
        schema: path,
        options: raw_options,
    };
    let source = wrapper(&schema.export)?;
    // A unique FileId cannot shadow a project source, even if its virtual path matches one.
    let main = virtual_file_id("<tola-module-options>");
    let mut files = FileMap::new();
    files.insert(main, source.into_bytes());
    let host = host.with_virtual_files(files);
    let resolver = host.file_resolver();
    let loaded =
        ExplicitSources::build_with_files(&[], config.get_root(), resolver).map_err(|failure| {
            let (error, accessed) = failure.into_error_and_accessed();
            OptionsFailure {
                error: anyhow!(error),
                accessed: Box::new(accessed),
                diagnostics: None,
            }
        })?;
    let (sources, loaded_accessed) = loaded.into_sources_and_accessed();
    let view = host.candidate_files(Arc::new(sources));
    let library = SiteLibrary::new(HostInputs {
        site: crate::package::site_values(config),
        modules: Dict::new(),
        module_validation: Some(validation),
        asset_urls: Dict::new(),
        asset_origins: Dict::new(),
        source_records: Array::new(),
        source_records_by_file: Dict::new(),
        source_origins: Dict::new(),
    });
    let world = host
        .world_with_id(
            config.get_root(),
            main,
            &library,
            view,
            &cancellation.bundle_cancellation(),
        )
        .map_err(|error| OptionsFailure {
            error: anyhow!(error),
            accessed: Box::new(loaded_accessed.clone()),
            diagnostics: None,
        })?;
    let (_, scanned) = scan_world_observed(&world, []).into_parts();
    let result = match scanned {
        Ok(result) => result,
        Err(failure) => {
            let diagnostics = failure.diagnostics().cloned();
            let (accessed, error) = failure.into_parts();
            let accessed = merge_accessed(loaded_accessed, accessed);
            return Err(OptionsFailure {
                error: anyhow!(error),
                accessed: Box::new(accessed),
                diagnostics,
            });
        }
    };
    let accessed = merge_accessed(loaded_accessed, result.accessed().clone());
    if let Err(error) = cancellation.ensure_active() {
        return Err(OptionsFailure {
            error: error.into(),
            accessed: Box::new(accessed),
            diagnostics: None,
        });
    }
    let value = result
        .module()
        .scope()
        .get("tola-options-result")
        .map(|binding| binding.read().clone());
    let outcome = (|| -> Result<Dict> {
        let Some(Value::Dict(parsed)) = value else {
            anyhow::bail!(UNREADABLE_RESULT)
        };
        match parsed.get("ok") {
            Ok(Value::Bool(true)) => match parsed.get("value") {
                Ok(Value::Dict(options)) => Ok(options.clone()),
                _ => {
                    let schema = module
                        .module
                        .options_schema
                        .as_ref()
                        .expect("the evaluated schema exists");
                    let path = crate::filesystem::display_path(
                        &module.module.root.join(&schema.schema),
                        config.get_root(),
                    );
                    let diagnostic = tola_diagnostic::Diagnostic::at_path(
                        tola_diagnostic::codes::config::INVALID,
                        tola_diagnostic::Severity::Error,
                        path,
                        "the options schema must return a dictionary",
                    );
                    Err(tola_diagnostic::DiagnosticError::new(
                        "the options schema returned an unexpected result",
                        vec![diagnostic],
                    )
                    .into())
                }
            },
            Ok(Value::Bool(false)) => match parsed.get("issues") {
                Ok(Value::Array(issues)) => Err(diagnostics::issues(config, module, issues)?),
                _ => anyhow::bail!(UNREADABLE_RESULT),
            },
            _ => anyhow::bail!(UNREADABLE_RESULT),
        }
    })();
    let options = match outcome {
        Ok(options) => options,
        Err(error) => {
            return Err(OptionsFailure {
                error,
                accessed: Box::new(accessed),
                diagnostics: Some(result.diagnostics().clone()),
            });
        }
    };

    Ok(OptionsEvaluation {
        options,
        accessed,
        diagnostics: result.diagnostics().clone(),
    })
}

fn merge_accessed(mut first: AccessedDeps, second: AccessedDeps) -> AccessedDeps {
    first.reads.extend(second.reads);
    first.disk_reads.extend(second.disk_reads);
    first.package_checks.extend(second.package_checks);
    first
}

/// The schema's return value never matched the shape Tola reads options from.
const UNREADABLE_RESULT: &str = "the options schema returned a value Tola cannot read";

fn wrapper(export: &str) -> Result<String> {
    ensure!(
        typst::syntax::is_ident(export),
        "`options.export` must name a Typst identifier"
    );
    let key = serde_json::to_string(tola_packages::MODULE_VALIDATION_MEMBER)?;
    Ok(format!(
        "#let tola-validation-input = sys.inputs.at({key})\n#import (tola-validation-input.schema): {export} as tola-options-schema\n#import \"@tola/schema:0.0.0\": try-parse\n#let tola-options-result = try-parse(tola-validation-input.options, tola-options-schema)\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preceding_warnings_survive_later_failure() {
        use tola_typst::{
            Diagnostic, DiagnosticOrigin, DiagnosticSeverity, SourceLocation, SourceTruncation,
        };
        let diagnostic = |severity, message: &str| Diagnostic {
            origin: DiagnosticOrigin::Typst,
            severity,
            message: message.into(),
            location: SourceLocation {
                path: None,
                line: None,
                column: None,
                range: None,
                source_lines: Vec::new(),
                location_failure: None,
                truncation: SourceTruncation::default(),
            },
            hints: Vec::new(),
            traces: Vec::new(),
            imported_by: Vec::new(),
            package_failure: None,
        };
        let warning = Diagnostics::from_vec(vec![diagnostic(
            DiagnosticSeverity::Warning,
            "first warning",
        )]);
        let error =
            Diagnostics::from_vec(vec![diagnostic(DiagnosticSeverity::Error, "later error")]);
        let mut failure = OptionsFailure {
            error: anyhow!("failed"),
            accessed: Box::default(),
            diagnostics: Some(error),
        };
        failure.prepend_context(AccessedDeps::default(), warning);
        let messages = failure
            .diagnostics
            .unwrap()
            .iter()
            .map(|item| item.message.clone())
            .collect::<Vec<_>>();
        assert_eq!(messages, ["first warning", "later error"]);
    }

    #[test]
    fn validates_schema_defaults_failure_reads() {
        use std::fs;
        use tola_typst::{LocalPackage, LocalPackages};
        use typst::foundations::IntoValue;
        use typst::syntax::VirtualPath;

        let site = tempfile::tempdir().unwrap();
        let root = site.path();
        let theme = root.join("theme");
        fs::create_dir(&theme).unwrap();
        fs::write(theme.join("lib.typ"), "").unwrap();
        fs::write(theme.join("tola-module.toml"), "schema-version = 1\nname = \"forest\"\nversion = \"1.2.3\"\n[typst]\nentry = \"lib.typ\"\n[options]\nschema = \"options.typ\"\nexport = \"schema\"\n").unwrap();
        fs::write(
            theme.join("options.typ"),
            r#"
#import "@tola/schema:0.0.0" as s
#import "@tola/site:0.0.0": site
#assert.eq(sys.inputs.at("__tola_modules"), (:))
#let schema = s.schema((
  enabled: s.optional(bool, default: true),
  items: s.optional(s.array-of(str), default: ("fallback",)),
  nested: s.schema((count: s.optional(int, default: 4),)),
  label: s.optional(str, default: site.title),
))
"#,
        )
        .unwrap();
        let config = crate::config::tests::load_test_config(
            root,
            "[modules.notes]\npath = \"theme\"\n[modules.notes.options]\nenabled = false\nitems = []\nnested = {}\n[site]\ntitle = \"Site title\"\n",
        );
        let module =
            crate::modules::resolve_modules(root, config.modules(), crate::InputScope::Online)
                .unwrap()
                .remove("notes")
                .unwrap();
        let module = crate::modules::configure_module(&config, module).unwrap();
        let package = LocalPackage::new(
            "@tola-modules/notes:0.0.0".parse().unwrap(),
            &theme,
            VirtualPath::new("lib.typ").unwrap(),
        )
        .unwrap();
        let packages = Arc::new(LocalPackages::new([package], ["tola-modules"]).unwrap());
        let host = crate::compiler::tests::compiler_host(&config)
            .unwrap()
            .with_local_packages(packages);
        let raw = |enabled: Value| {
            let mut options = Dict::new();
            options.insert("enabled".into(), enabled);
            options.insert("items".into(), Array::new().into_value());
            options.insert("nested".into(), Dict::new().into_value());
            options
        };
        let cancellation = BuildCancellation::new();
        let failed = |options: Dict| {
            evaluate(&config, &host, &module, options, &cancellation)
                .expect_err("invalid module options must fail validation")
        };
        let result = evaluate(
            &config,
            &host,
            &module,
            raw(false.into_value()),
            &cancellation,
        )
        .unwrap();
        assert_eq!(result.options.get("enabled").unwrap(), &false.into_value());
        assert_eq!(
            result.options.get("items").unwrap(),
            &Array::new().into_value()
        );
        assert_eq!(
            result.options.get("label").unwrap(),
            &"Site title".into_value()
        );
        let Value::Dict(nested) = result.options.get("nested").unwrap() else {
            panic!("nested dictionary")
        };
        assert_eq!(nested.get("count").unwrap(), &4i64.into_value());
        assert!(format!("{:?}", result.accessed.reads()).contains("options.typ"));

        let failure = failed(raw("wrong".into_value()));
        assert!(format!("{:?}", failure.accessed.reads()).contains("options.typ"));
        assert!(failure.diagnostics.is_some());
        let reported = tola_diagnostic::attached(&failure.error).unwrap();
        assert_eq!(reported[0].location.as_ref().unwrap().path, "tola.toml");
        assert_eq!(reported[0].location.as_ref().unwrap().line, Some(4));
        assert!(!reported[0].message.contains("tola-module-options"));

        let mut wrong_nested = Dict::new();
        wrong_nested.insert("count".into(), "wrong".into_value());
        let mut bad_options = Dict::new();
        bad_options.insert("enabled".into(), false.into_value());
        bad_options.insert("items".into(), Array::new().into_value());
        bad_options.insert("nested".into(), wrong_nested.into_value());
        let nested = failed(bad_options);
        let reported = tola_diagnostic::attached(&nested.error).unwrap();
        assert_eq!(reported[0].location.as_ref().unwrap().path, "tola.toml");
        // Inline nested keys without their own range fall back to the containing key.
        assert_eq!(reported[0].location.as_ref().unwrap().line, Some(6));

        let schema_path = theme.join("options.typ");
        let original = fs::read_to_string(&schema_path).unwrap();
        fs::write(
            &schema_path,
            original.replace("default: site.title", "default: 42"),
        )
        .unwrap();
        let default_failure = failed(raw(false.into_value()));
        let reported = tola_diagnostic::attached(&default_failure.error).unwrap();
        assert!(reported.iter().any(|item| {
            item.location
                .as_ref()
                .unwrap()
                .path
                .ends_with("theme/options.typ")
        }));
        assert!(reported.iter().all(|item| {
            !item
                .location
                .as_ref()
                .unwrap()
                .path
                .contains("tola-module-options")
        }));

        fs::write(
            &schema_path,
            original.replace(
                "enabled: s.optional(bool, default: true)",
                "enabled: s.map(bool, value => \"bad\", output: int)",
            ),
        )
        .unwrap();
        let generated = failed(raw(false.into_value()));
        let reported = tola_diagnostic::attached(&generated.error).unwrap();
        assert!(reported.iter().any(|item| {
            item.location
                .as_ref()
                .unwrap()
                .path
                .ends_with("theme/options.typ")
        }));

        fs::write(
            &schema_path,
            r#"
#import "@tola/schema:0.0.0" as s
#let schema = s.schema((enabled: s.refine(bool, value => panic("callback boom"), message: "invalid"),))
"#,
        )
        .unwrap();
        let panic_failure = failed(raw(false.into_value()));
        assert!(tola_diagnostic::attached(&panic_failure.error).is_none());
        let diagnostics = panic_failure.diagnostics.as_ref().unwrap();
        assert!(
            diagnostics.iter().any(|item| {
                item.message.contains("callback boom")
                    && item
                        .location
                        .path
                        .as_deref()
                        .is_some_and(|path| path.ends_with("options.typ"))
            }),
            "callback diagnostics: {diagnostics:#?}"
        );
        for expression in [
            "get-module()",
            "get-module(auto)",
            "get-module(\"notes\")",
            "get-module(\"another\")",
        ] {
            let schema = format!(
                "#import \"@tola/module:0.0.0\": get-module\n#let impossible = {expression}\n#let schema = str\n"
            );
            fs::write(&schema_path, schema).unwrap();
            let failure = failed(raw(false.into_value()));
            let diagnostics = failure.diagnostics.as_ref().unwrap();
            assert!(
                diagnostics.iter().any(|item| item
                    .message
                    .contains("unavailable while validating options schemas")
                    && item
                        .location
                        .path
                        .as_deref()
                        .is_some_and(|path| path.ends_with("options.typ"))),
                "schema call {expression}: {diagnostics:#?}"
            );
        }
        fs::write(&schema_path, &original).unwrap();

        // A missing transitive schema input is a failed read, then becomes available on retry.
        fs::write(&schema_path, "#import \"missing.typ\": schema\n").unwrap();
        let missing = theme.join("missing.typ");
        let failure = failed(raw(false.into_value()));
        assert!(
            failure
                .accessed
                .disk_reads()
                .iter()
                .any(|read| read.as_path().ends_with("theme/missing.typ"))
        );
        fs::write(&missing, original).unwrap();
        let repaired = evaluate(
            &config,
            &host,
            &module,
            raw(false.into_value()),
            &cancellation,
        )
        .unwrap();
        assert_eq!(
            repaired.options.get("label").unwrap(),
            &"Site title".into_value()
        );
        assert!(
            repaired
                .accessed
                .disk_reads()
                .iter()
                .any(|read| read.as_path().ends_with("theme/missing.typ"))
        );
    }

    #[test]
    fn generated_private_input_safe_export() {
        let entry = wrapper("schema").unwrap();
        assert!(entry.contains(tola_packages::MODULE_VALIDATION_MEMBER));
        assert!(entry.contains("tola-validation-input.options"));
        assert!(wrapper("schema\n#panic() ").is_err());
    }
}
