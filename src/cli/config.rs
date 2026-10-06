//! Shared configuration loading and invocation overrides.

use crate::cancellation::Cancellation;
use crate::cli::log::destination;
use anyhow::{Result, ensure};
use tola_build::InputScope;

use super::{BuildOverrideArgs, ConfigFileArgs, ServerArgs, TypstPackageArgs};
use crate::cli::output::CommandOutput;
use crate::config::{ConfigOverrides, ServerOverrides};
use tola_build::config::loading::BuildOverrides;

pub(super) fn build_resources(scope: InputScope) -> tola_build::BuildResources {
    tola_build::BuildResources::new().with_input_scope(scope)
}

pub(super) fn build_overrides(build: &BuildOverrideArgs) -> BuildOverrides {
    BuildOverrides {
        publish_dir: build.publish_dir.clone(),
        minify: build.minify,
        origin: build.origin.clone(),
        base_path: build.base_path.clone(),
    }
}

pub(super) fn server_overrides(server: &ServerArgs) -> ServerOverrides {
    ServerOverrides {
        interface: server.interface,
        port: server.port,
    }
}

pub(super) fn package_locations(
    packages: &TypstPackageArgs,
    scope: InputScope,
) -> Result<tola_typst::PackageLocations> {
    if scope == InputScope::Pure {
        ensure!(
            packages.package_path.is_none() && packages.package_cache_path.is_none(),
            "`--pure` cannot use `--package-path` or `--package-cache-path`"
        );
        return tola_typst::PackageLocations::from_absolute_roots(None, None).map_err(Into::into);
    }
    tola_typst::PackageLocations::discover(
        packages.package_path.clone(),
        packages.package_cache_path.clone(),
    )
    .map_err(Into::into)
}

pub(super) fn load_config(
    config_file: &ConfigFileArgs,
    packages: &TypstPackageArgs,
    scope: InputScope,
    overrides: &ConfigOverrides,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<crate::config::LoadedConfig> {
    let loaded = (|| {
        crate::config::load(
            config_file.path.as_deref(),
            scope,
            package_locations(packages, scope)?,
            overrides,
        )
    })();
    match loaded {
        Ok(loaded) => Ok(loaded),
        Err(error) => {
            start_error_log(config_file, output, cancellation);
            Err(error)
        }
    }
}

/// The site configuration a command works on, when its directory has one.
///
/// A configuration named with `--config` always loads, so a path that does not exist stays an
/// error. Without one, a directory that holds no `tola.toml` in itself or above it is not a site,
/// and the command works on the directory itself.
pub(super) fn load_optional_config(
    config_file: &ConfigFileArgs,
    packages: &TypstPackageArgs,
    scope: InputScope,
    overrides: &ConfigOverrides,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<Option<crate::config::LoadedConfig>> {
    match load_config(
        config_file,
        packages,
        scope,
        overrides,
        output,
        cancellation,
    ) {
        Ok(loaded) => Ok(Some(loaded)),
        Err(error)
            if config_file.path.is_none()
                && error
                    .chain()
                    .any(|cause| cause.is::<tola_build::config::loading::ConfigNotFound>()) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

pub(super) fn load_site(
    config_file: &ConfigFileArgs,
    packages: &TypstPackageArgs,
    scope: InputScope,
    overrides: &ConfigOverrides,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<crate::config::LoadedConfig> {
    let loaded = load_config(
        config_file,
        packages,
        scope,
        overrides,
        output,
        cancellation,
    )?;
    output.apply_diagnostic_limits(loaded.diagnostics());
    let config = loaded.config();
    output.install_source_files(
        config.get_root().to_path_buf(),
        config.package_locations().clone(),
    );
    destination::start_site(output, config, &cancellation.token())?;
    output.diagnostics(&tola_build::config::diagnostic::warning_diagnostics(config))?;
    Ok(loaded)
}

pub(super) fn start_error_log(
    config_file: &ConfigFileArgs,
    output: &CommandOutput,
    cancellation: &Cancellation,
) {
    if output.log().is_none() {
        return;
    }
    match source_root(config_file) {
        Ok(root) => destination::start_after_error(output, &root, &cancellation.token()),
        Err(_) => {
            let _ = output.diagnostic(
                &tola_build::diagnostic::Diagnostic::new(
                    crate::codes::log::UNAVAILABLE,
                    tola_build::diagnostic::Severity::Warning,
                    "cannot determine the site directory for logging",
                )
                .with_help("Run the command inside the site"),
            );
        }
    }
}

pub(crate) fn source_root(config_file: &ConfigFileArgs) -> Result<std::path::PathBuf> {
    tola_build::config::loading::config_site_root(config_file.path.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_refuses_host_package_flags() {
        for packages in [
            TypstPackageArgs {
                package_path: Some("/tmp/packages".into()),
                ..TypstPackageArgs::default()
            },
            TypstPackageArgs {
                package_cache_path: Some("/tmp/packages".into()),
                ..TypstPackageArgs::default()
            },
        ] {
            assert!(package_locations(&packages, InputScope::Pure).is_err());
        }
    }
}
