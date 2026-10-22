//! Validate hook options before generators run, and retain that evaluation for compilation.

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::{Result, ensure};

use super::options::{ValidatedOptions, evaluate_all};
use crate::cancellation::BuildCancellation;
use crate::compiler::{CompilationDependencies, TypstDependencyReader, TypstHost};
use crate::config::{ConfigDiagnostics, ResolvedSiteConfig};
use crate::resources::BuildResources;

pub(crate) struct Bootstrap {
    pub options: Arc<ValidatedOptions>,
    dependencies: CompilationDependencies,
    boundary: tola_typst::SourceBoundary,
}

impl Bootstrap {
    pub(crate) fn prepare(
        config: &ResolvedSiteConfig,
        resources: &BuildResources,
        cancellation: &BuildCancellation,
    ) -> Result<Self> {
        let host = TypstHost::for_module_options(config, resources, cancellation)?;
        let options = Arc::new(evaluate_all(config, &host, cancellation)?);
        let accessed = &options.accessed;
        let dependencies = CompilationDependencies::new(
            config.get_root(),
            [],
            [],
            [(
                TypstDependencyReader::SiteProgram,
                accessed.reads.clone(),
                accessed.package_checks.clone(),
            )],
            accessed.package_checks.clone(),
            &host,
            None,
        )?;
        Ok(Self {
            options,
            dependencies,
            boundary: host.source_boundary().clone(),
        })
    }

    pub(crate) fn install(&self, config: &mut ResolvedSiteConfig) -> Result<()> {
        let modules = config
            .module_inputs
            .configured
            .iter()
            .map(|(alias, configured)| (alias.clone(), configured.module.clone()))
            .collect();
        let mut hooks = super::hooks::hook_stages(&modules, &self.options.values)?;
        let project_hooks = config.build.hooks.clone();
        hooks
            .before_build
            .extend(project_hooks.before_build.iter().cloned());
        hooks
            .generate_outputs
            .extend(project_hooks.generate_outputs.iter().cloned());
        hooks
            .after_publish
            .extend(project_hooks.after_publish.iter().cloned());
        let mut named = BTreeSet::new();
        for hook in crate::hooks::configured_hooks(&hooks) {
            ensure!(
                named.insert((hook.stage.as_str(), hook.label)),
                "`{}` declares two hooks named `{}`",
                hook.stage.as_str(),
                hook.label
            );
        }
        let mut diagnostics = ConfigDiagnostics::new();
        crate::config::loading::validate_hook_output_boundaries(
            &hooks,
            config.get_root(),
            config.config_path(),
            &config.build.publish_dir,
            config.vendor_workspace.as_deref(),
            &mut diagnostics,
        );
        diagnostics.into_result()?;
        for hook in &hooks.before_build {
            if !hook.enable {
                continue;
            }
            for path in &hook.generates {
                let target = crate::filesystem::FilesystemSourceIdentity::from_path(
                    &config.get_root().join(path),
                );
                for module in &config.module_roots {
                    ensure!(
                        !target.intersects(
                            &crate::filesystem::FilesystemSourceIdentity::from_path(module)
                        ),
                        "the `{}` hook generates `{}`, which is inside the module directory `{}`",
                        hook.name,
                        path.display(),
                        crate::filesystem::display_path(module, config.get_root()),
                    );
                }
            }
        }
        config.build.hooks = hooks;
        let inputs = Arc::make_mut(&mut config.module_inputs);
        inputs.project_hooks = Some(project_hooks);
        inputs.validated_options = Some(Arc::clone(&self.options));
        Ok(())
    }

    pub(crate) fn ensure_current(
        &self,
        config: &ResolvedSiteConfig,
        cancellation: &BuildCancellation,
    ) -> Result<()> {
        ensure!(
            config.module_inputs.manifests_are_fresh(cancellation)?
                && self
                    .dependencies
                    .physical_reads_are_fresh(&self.boundary, cancellation)?
                && self.dependencies.package_checks_are_fresh(cancellation)?,
            "module options or manifests changed after hook planning; keep hook inputs separate from the files used to select hooks",
        );
        Ok(())
    }
}
