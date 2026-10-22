//! Locate normalized schema Issues in the user's configuration or the module schema.

use anyhow::{Result, ensure};
use typst::foundations::{Array, Value};

use crate::config::ResolvedSiteConfig;
use crate::modules::ConfiguredModule;
use tola_diagnostic::{Diagnostic, DiagnosticError, Severity};

pub(super) fn issues(
    config: &ResolvedSiteConfig,
    module: &ConfiguredModule,
    issues: &Array,
) -> Result<anyhow::Error> {
    let projected = tola_packages::schema_issue::project_issues(issues).map_err(|error| {
        anyhow::anyhow!("the options schema returned an issue Tola cannot read: {error}")
    })?;
    ensure!(
        !projected.is_empty(),
        "the options schema reported a failure without naming an issue"
    );
    let mut diagnostics = Vec::new();
    for problem in projected {
        let path = if problem.generated {
            let schema = module
                .module
                .options_schema
                .as_ref()
                .expect("the evaluated schema exists");
            crate::filesystem::display_path(
                &module.module.root.join(&schema.schema),
                config.get_root(),
            )
        } else {
            crate::filesystem::display_path(config.config_path(), config.get_root())
        };
        let mut diagnostic = Diagnostic::at_path(
            tola_diagnostic::codes::config::INVALID,
            Severity::Error,
            path,
            format!(
                "module `{}` options: {}",
                module.module.name, problem.message
            ),
        );
        if !problem.generated {
            let mut keys = vec![
                "modules".to_owned(),
                module.module.name.clone(),
                "options".to_owned(),
            ];
            for part in &problem.path {
                match part {
                    Value::Str(key) if !key.as_str().contains('.') => keys.push(key.to_string()),
                    Value::Int(index) if *index >= 0 => keys.push(index.to_string()),
                    _ => break,
                }
            }
            if let Some(positions) = config.positions() {
                while !keys.is_empty() {
                    let key = keys.join(".");
                    if let Some(excerpt) = positions.excerpt(&key) {
                        diagnostic = diagnostic.with_source_excerpt(
                            excerpt.line,
                            excerpt.column,
                            excerpt.text,
                            Some(excerpt.highlight),
                        );
                        if let Some(range) = positions.written(&key) {
                            diagnostic = diagnostic.with_source_range(range);
                        }
                        break;
                    }
                    keys.pop();
                }
            }
        }
        for note in problem.notes {
            diagnostic = diagnostic.with_note(note);
        }
        diagnostics.push(diagnostic);
    }
    Ok(DiagnosticError::new("module options did not satisfy their schema", diagnostics).into())
}
