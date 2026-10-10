//! Freeze build-selected dependencies without exposing an incomplete vendor tree.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use tola_build::cancellation::BuildCancellation;
use tola_build::diagnostic::{Diagnostic, DiagnosticError, Severity};
use tola_build::{BuildResources, InputScope};

use crate::cancellation::Cancellation;
use crate::cli::output::CommandOutput;
use crate::cli::{ConfigFileArgs, TypstPackageArgs, VendorArgs};
use crate::terminal::{InputCancelled, display_path_within};

const VENDOR_TREES: [&str; 3] = ["typst-packages", "fonts", "icons"];

/// The next step for a reader whose `vendor.path` cannot hold a fresh replacement.
const VENDOR_PATH_CONFLICT_ADVICE: &str =
    "Leave what is there intact, then rerun `tola vendor` with a different `vendor.path`";

pub(in crate::cli) fn run(
    config_file: ConfigFileArgs,
    packages: TypstPackageArgs,
    scope: InputScope,
    vendor: VendorArgs,
    resources: BuildResources,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    cancellation.token().ensure_active()?;
    let loaded = crate::cli::config::load_site(
        &config_file,
        &packages,
        scope,
        &crate::config::ConfigOverrides::default(),
        output,
        cancellation,
    )?;
    let cancellation = cancellation.token();
    let config = Arc::new(loaded.into_config());
    let config_name = config_file
        .path
        .as_deref()
        .unwrap_or_else(|| Path::new("tola.toml"));
    ensure!(
        config.vendor().path.is_some(),
        "this site does not declare a `[vendor] path`; add `path = \"vendor\"` to `{}`",
        config_name.display()
    );
    let _lock = tola_build::build::SiteBuildLock::acquire(&config, &cancellation, || {
        let _ = output.waiting_for_build();
    })?;
    if vendor.dry_run {
        VendorReplacement::reject_interrupted_install(&config)?;
    }
    let replacement = VendorReplacement::open(&config)?;
    let resources = resources.with_input_scope(scope);
    let prepared: Result<()> = (|| {
        cancellation.ensure_active()?;
        let selection_config = if vendor.refresh {
            Arc::new(config.with_vendor_root(None)?)
        } else {
            Arc::clone(&config)
        };
        let selection = select_inputs(
            Arc::clone(&selection_config),
            resources.clone(),
            &cancellation,
        )?;
        if vendor.refresh {
            reject_previous_inputs(&selection.site, &replacement)?;
        }
        let icons = selected_icon_collections(&selection_config, &resources, &cancellation)?;
        let candidate = replacement.candidate();
        let mut copy = VendorCopy {
            boundary: resources.source_boundary(&selection_config),
            replacement: &replacement,
            cancellation: &cancellation,
            buffer: [0; 64 * 1024],
        };
        for package in &selection.packages {
            copy.directory(
                &package.source,
                &candidate.join("typst-packages").join(&package.relative),
                &mut Vec::new(),
            )?;
        }
        for font in &selection.fonts {
            copy.file(&font.source, &candidate.join("fonts").join(&font.name))?;
        }
        copy.icons(&icons)?;
        selection.site.ensure_fresh(&cancellation)?;
        let candidate_config = Arc::new(config.with_vendor_root(Some(candidate))?);
        let verified = build_without_hooks(
            candidate_config,
            resources.clone().with_input_scope(InputScope::Pure),
            &cancellation,
        )
        .context("the prepared vendor inputs could not build the site under `--pure`")?;
        reject_previous_inputs(&verified, &replacement)?;
        show_selection(&selection, &icons, &replacement, vendor.dry_run, output)?;
        verified.ensure_fresh(&cancellation)?;
        cancellation.ensure_active()?;
        if !vendor.dry_run {
            replacement.install(&cancellation)?;
        }
        Ok(())
    })();
    if let Err(error) = prepared {
        if let Err(recovery) = replacement.discard() {
            // The recovery failure is not part of the returned chain, so the file log keeps it.
            tracing::warn!(
                target: "tola::vendor",
                error = %format!("{recovery:#}"),
                "vendor recovery did not finish"
            );
            if !is_cancellation(&error) {
                let note =
                    "Tola could not undo its work, so an unfinished replacement is left behind";
                let diagnostics =
                    crate::cli::output::attached_or_fallback(&error, crate::codes::command::FAILED)
                        .into_iter()
                        .map(|diagnostic| diagnostic.with_note(note))
                        .collect();
                return Err(
                    tola_build::diagnostic::DiagnosticError::attach(error, diagnostics).into(),
                );
            }
        }
        return Err(error);
    }
    if let Err(error) = replacement.cleanup() {
        // The replacement is committed; the working files it left behind stay for the next run.
        tracing::warn!(
            target: "tola::vendor",
            error = %format!("{error:#}"),
            "vendor cleanup did not finish"
        );
        let updated = if vendor.dry_run {
            "vendor is unchanged"
        } else {
            "the vendor inputs were updated"
        };
        let _ = output.diagnostic(
            &Diagnostic::new(
                crate::codes::command::FAILED,
                Severity::Warning,
                format!("{updated}, but Tola could not remove the working files it used"),
            )
            .with_help("Run `tola vendor` again"),
        );
    }
    let _ = output.status(if vendor.dry_run {
        "Dry run verified under `--pure`; vendor unchanged"
    } else {
        "Vendored inputs verified under --pure and committed"
    });
    Ok(())
}

struct SelectedIconCollection {
    namespace: String,
    bytes: Vec<u8>,
}

fn selected_icon_collections(
    config: &tola_build::config::ResolvedSiteConfig,
    resources: &BuildResources,
    cancellation: &BuildCancellation,
) -> Result<Vec<SelectedIconCollection>> {
    use tola_build::config::section::IconCollectionSource;
    let mut selected = Vec::new();
    for (namespace, source) in &config.icons().collections {
        cancellation.ensure_active()?;
        if matches!(source, IconCollectionSource::RemoteJson { .. }) {
            selected.push(SelectedIconCollection {
                namespace: namespace.clone(),
                bytes: tola_build::remote_collection_bytes(
                    config,
                    namespace,
                    resources,
                    cancellation,
                )?,
            });
        }
    }
    Ok(selected)
}

struct SelectedPackage {
    package: tola_typst::PackageSpec,
    source: PathBuf,
    relative: PathBuf,
}

struct SelectedFont {
    source: PathBuf,
    name: std::ffi::OsString,
}

struct SelectedInputs {
    site: tola_build::build::SiteBuild,
    packages: Vec<SelectedPackage>,
    fonts: Vec<SelectedFont>,
}

fn build_without_hooks(
    config: Arc<tola_build::config::ResolvedSiteConfig>,
    resources: BuildResources,
    cancellation: &BuildCancellation,
) -> Result<tola_build::build::SiteBuild> {
    let mut session = tola_build::BuildSession::with_resources(resources);
    let mut request =
        tola_build::build::BuildRequest::new(tola_build::build::BuildMode::Production);
    request.cancellation = cancellation.clone();
    request.hook_execution = tola_build::build::HookExecution::Skip;
    // The caller holds the site lock across selection, verification, and replacement.
    session
        .prepare(config, request)
        .run()
        .map_err(|failure| failure.into_error())
}

fn select_inputs(
    config: Arc<tola_build::config::ResolvedSiteConfig>,
    resources: BuildResources,
    cancellation: &BuildCancellation,
) -> Result<SelectedInputs> {
    let site = build_without_hooks(Arc::clone(&config), resources, cancellation)?;
    let observation = site.input_observation();
    let mut packages = BTreeMap::new();
    for check in observation
        .package_checks()
        .iter()
        .filter(|check| check.was_selected())
    {
        cancellation.ensure_active()?;
        let Some(source) = check.canonical_target() else {
            continue;
        };
        let package = check.package().clone();
        let relative = PathBuf::from(package.namespace.as_str())
            .join(package.name.as_str())
            .join(package.version.to_string());
        packages.insert(
            relative.clone(),
            SelectedPackage {
                package,
                source: source.to_path_buf(),
                relative,
            },
        );
    }
    let configured_fonts = config
        .fonts()
        .paths
        .iter()
        .map(|path| tola_build::filesystem::normalize_existing_prefix(path))
        .collect::<Vec<_>>();
    let font_paths = observation
        .paths()
        .iter()
        .filter(|path| path.input() == tola_build::build::InputKind::FontInventory)
        .map(|path| tola_build::filesystem::normalize_existing_prefix(path.path()))
        .filter(|path| {
            !configured_fonts
                .iter()
                .any(|configured| path.starts_with(configured))
        })
        .collect::<BTreeSet<_>>();
    let mut fonts = BTreeMap::new();
    for source in font_paths {
        let name = source
            .file_name()
            .with_context(|| format!("`{}` has no font file name", source.display()))?
            .to_owned();
        if let Some(previous) = fonts.insert(name.clone(), source.clone()) {
            bail!(
                "`{}` and `{}` are both named `{}`; keep one in `typst.fonts.paths` instead",
                previous.display(),
                source.display(),
                name.to_string_lossy()
            );
        }
    }
    Ok(SelectedInputs {
        site,
        packages: packages.into_values().collect(),
        fonts: fonts
            .into_iter()
            .map(|(name, source)| SelectedFont { source, name })
            .collect(),
    })
}

fn reject_previous_inputs(
    site: &tola_build::build::SiteBuild,
    replacement: &VendorReplacement,
) -> Result<()> {
    let observation = site.input_observation();
    let previous = VENDOR_TREES.map(|name| replacement.root.join(name));
    for path in observation
        .physical_read_paths()
        .iter()
        .map(PathBuf::as_path)
        .chain(observation.paths().iter().map(|path| path.path()))
        .chain(
            observation
                .package_checks()
                .iter()
                .filter_map(|check| check.canonical_target()),
        )
    {
        let physical = tola_build::filesystem::normalize_existing_prefix(path);
        ensure!(
            !previous.iter().any(|root| physical.starts_with(root)),
            "the build still reads `{}` from the previous vendor inputs; keep site-owned sources outside `typst-packages`, `fonts`, and `icons`",
            replacement.display_destination(path)
        );
    }
    Ok(())
}

fn show_selection(
    selection: &SelectedInputs,
    icons: &[SelectedIconCollection],
    replacement: &VendorReplacement,
    dry_run: bool,
    output: &CommandOutput,
) -> Result<()> {
    let action = if dry_run { "Would freeze" } else { "Prepared" };
    for package in &selection.packages {
        let destination = replacement
            .root
            .join("typst-packages")
            .join(&package.relative);
        tracing::debug!(
            target: "tola::vendor",
            package = %package.package,
            source = %package.source.display(),
            destination = %destination.display(),
            "selected vendored package"
        );
        output.status(format!(
            "{action} `{}` into `{}`",
            package.package,
            replacement.display_destination(&destination)
        ))?;
    }
    for font in &selection.fonts {
        let destination = replacement.root.join("fonts").join(&font.name);
        tracing::debug!(
            target: "tola::vendor",
            font = %font.name.to_string_lossy(),
            source = %font.source.display(),
            destination = %destination.display(),
            "selected vendored font"
        );
        output.status(format!(
            "{action} font `{}` into `{}`",
            font.name.to_string_lossy(),
            replacement.display_destination(&destination)
        ))?;
    }
    for collection in icons {
        output.status(format!(
            "{action} icon collection `{}`",
            collection.namespace
        ))?;
    }
    Ok(())
}

struct VendorCopy<'a> {
    boundary: tola_typst::SourceBoundary,
    replacement: &'a VendorReplacement,
    cancellation: &'a BuildCancellation,
    buffer: [u8; 64 * 1024],
}

impl VendorCopy<'_> {
    fn source(&self, source: &Path, destination: &Path) -> Result<PathBuf> {
        self.cancellation.ensure_active()?;
        self.boundary.check(source)?;
        let physical = fs::canonicalize(source)
            .with_context(|| {
                format!(
                    "could not resolve `{}`; vendor sources must not contain broken links or link cycles",
                    source.display()
                )
            })
            .map_err(|error| self.failure(VendorStep::ReadingSource, destination, error))?;
        if physical.starts_with(&self.replacement.workspace) || destination.starts_with(&physical) {
            return Err(self.refusal(
                destination,
                "its source and the vendored replacement overlap",
                "Keep the sources of the vendored inputs outside the vendored directory and its working files",
            ));
        }
        Ok(physical)
    }

    fn directory(
        &mut self,
        source: &Path,
        destination: &Path,
        ancestors: &mut Vec<PathBuf>,
    ) -> Result<()> {
        let physical = self.source(source, destination)?;
        if ancestors.contains(&physical) {
            return Err(self.refusal(
                destination,
                "its source holds a link cycle",
                VendorStep::ReadingSource.advice(),
            ));
        }
        let metadata = fs::metadata(&physical)
            .with_context(|| format!("could not read vendor source `{}`", source.display()))
            .map_err(|error| self.failure(VendorStep::ReadingSource, destination, error))?;
        if !metadata.is_dir() {
            return Err(self.refusal(
                destination,
                "its source is not a directory",
                VendorStep::ReadingSource.advice(),
            ));
        }
        fs::create_dir_all(destination)
            .with_context(|| format!("could not prepare `{}`", destination.display()))
            .map_err(|error| self.failure(VendorStep::WritingCopy, destination, error))?;
        ancestors.push(physical.clone());
        let entries = fs::read_dir(&physical)
            .with_context(|| format!("could not read vendor source `{}`", source.display()))
            .map_err(|error| self.failure(VendorStep::ReadingSource, destination, error))?;
        for child in entries {
            self.cancellation.ensure_active()?;
            let child = child
                .with_context(|| format!("could not read vendor source `{}`", source.display()))
                .map_err(|error| self.failure(VendorStep::ReadingSource, destination, error))?;
            let from = child.path();
            let to = destination.join(child.file_name());
            let metadata = fs::metadata(&from)
                .with_context(|| {
                    format!(
                        "could not read `{}`; vendor sources must not contain broken links",
                        from.display()
                    )
                })
                .map_err(|error| self.failure(VendorStep::ReadingSource, &to, error))?;
            if metadata.is_dir() {
                self.directory(&from, &to, ancestors)?;
            } else {
                self.file(&from, &to)?;
            }
        }
        ancestors.pop();
        Ok(())
    }

    fn file(&mut self, source: &Path, destination: &Path) -> Result<()> {
        let physical = self.source(source, destination)?;
        let metadata = fs::metadata(&physical)
            .with_context(|| format!("could not read vendor source `{}`", source.display()))
            .map_err(|error| self.failure(VendorStep::ReadingSource, destination, error))?;
        if !metadata.is_file() {
            return Err(self.refusal(
                destination,
                "its source is not a regular file",
                VendorStep::ReadingSource.advice(),
            ));
        }
        let mut reader = fs::File::open(&physical)
            .with_context(|| format!("could not read `{}`", physical.display()))
            .map_err(|error| self.failure(VendorStep::ReadingSource, destination, error))?;
        let mut writer = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .with_context(|| format!("could not prepare `{}`", destination.display()))
            .map_err(|error| self.failure(VendorStep::WritingCopy, destination, error))?;
        loop {
            self.cancellation.ensure_active()?;
            let count = reader
                .read(&mut self.buffer)
                .with_context(|| format!("could not read `{}`", physical.display()))
                .map_err(|error| self.failure(VendorStep::ReadingSource, destination, error))?;
            if count == 0 {
                break;
            }
            writer
                .write_all(&self.buffer[..count])
                .with_context(|| format!("could not prepare `{}`", destination.display()))
                .map_err(|error| self.failure(VendorStep::WritingCopy, destination, error))?;
        }
        self.cancellation.ensure_active()?;
        Ok(())
    }

    /// Freeze the verified icon collections into the prepared replacement.
    fn icons(&self, icons: &[SelectedIconCollection]) -> Result<()> {
        for collection in icons {
            self.cancellation.ensure_active()?;
            let destination = self
                .replacement
                .candidate()
                .join("icons")
                .join(format!("{}.json", collection.namespace));
            fs::write(&destination, &collection.bytes)
                .with_context(|| {
                    format!(
                        "could not prepare icon collection `{}`",
                        collection.namespace
                    )
                })
                .map_err(|error| self.failure(VendorStep::WritingCopy, &destination, error))?;
        }
        Ok(())
    }

    /// Report the failure of one copy step, naming the vendored destination for the author.
    fn failure(
        &self,
        step: VendorStep,
        destination: &Path,
        source: anyhow::Error,
    ) -> anyhow::Error {
        let vendored = self.vendored_destination(destination);
        vendor_failure(
            &vendored,
            step.message(&vendored),
            step.advice(),
            Some(source),
        )
    }

    /// Report a refusal Tola states itself about one copy step.
    fn refusal(&self, destination: &Path, reason: &str, advice: &str) -> anyhow::Error {
        let vendored = self.vendored_destination(destination);
        vendor_failure(
            &vendored,
            format!("Tola cannot prepare `{vendored}`: {reason}"),
            advice,
            None,
        )
    }

    /// The vendored destination a prepared path becomes, as the site author names it.
    fn vendored_destination(&self, destination: &Path) -> String {
        let staged = destination
            .strip_prefix(self.replacement.candidate())
            .or_else(|_| destination.strip_prefix(self.replacement.workspace.join("previous")));
        match staged {
            Ok(relative) => display_path_within(
                &self.replacement.root.join(relative),
                &self.replacement.site,
            ),
            Err(_) if destination.starts_with(&self.replacement.workspace) => {
                display_path_within(&self.replacement.root, &self.replacement.site)
            }
            Err(_) => display_path_within(destination, &self.replacement.site),
        }
    }
}

/// One step of the vendor operation, stated for the site author who must act on its failure.
#[derive(Clone, Copy)]
enum VendorStep {
    /// Creating and inspecting the working directory that holds the prepared replacement.
    Preparing,
    /// Reading Tola's own working files, including the record of an unfinished replacement.
    Reading,
    /// Replacing the vendored trees with the prepared ones.
    Installing,
    /// Undoing a replacement that stopped before it committed.
    Undoing,
    /// Removing the working files of a finished replacement.
    Cleaning,
    /// Reading one selected input from its source.
    ReadingSource,
    /// Writing one selected input into the prepared replacement.
    WritingCopy,
}

impl VendorStep {
    /// What Tola was doing, as the predicate of the sentence the reader sees.
    fn operation(self) -> &'static str {
        match self {
            Self::Preparing => "prepare the working files it needs for",
            Self::Reading => "read the working files it keeps for",
            Self::Installing => "replace the vendored inputs in",
            Self::Undoing => "undo the replacement of",
            Self::Cleaning => "remove the working files it used for",
            Self::ReadingSource => "read the source of",
            Self::WritingCopy => "prepare",
        }
    }

    /// The reader's next step.
    fn advice(self) -> &'static str {
        match self {
            Self::Preparing | Self::Cleaning => {
                "Check that Tola can create and remove directories beside the vendored inputs, then rerun `tola vendor`"
            }
            Self::Reading => {
                "Check that Tola can read the working files it keeps beside the vendored inputs, then rerun `tola vendor`"
            }
            Self::Installing => {
                "Check that Tola can write inside the vendored directory and the site directory, then rerun `tola vendor`"
            }
            Self::Undoing => {
                "Check that Tola can write inside the vendored directory, then rerun `tola vendor`"
            }
            Self::ReadingSource => {
                "Check that the vendored sources are readable, then rerun `tola vendor --refresh`"
            }
            Self::WritingCopy => {
                "Check that Tola can write inside the vendored directory and that the site's disk has free space, then rerun `tola vendor`"
            }
        }
    }

    /// The sentence a reader sees for this step's failure at `destination`.
    fn message(self, destination: &str) -> String {
        format!("Tola could not {} `{destination}`", self.operation())
    }
}

/// The site author's view of a failed vendor step.
///
/// `destination` is spelled the way the author names it, and `advice` states the next step. The
/// caller keeps the exact path, the operating-system error, and the cause chain in `source`,
/// which only the debug and file logs show.
fn vendor_failure(
    destination: &str,
    message: String,
    advice: &str,
    source: Option<anyhow::Error>,
) -> anyhow::Error {
    let diagnostic = Diagnostic::at_path(
        crate::codes::command::FAILED,
        Severity::Error,
        destination,
        message.clone(),
    )
    .with_help(advice);
    match source {
        Some(source) => anyhow::Error::new(DiagnosticError::attach(source, vec![diagnostic])),
        None => anyhow::Error::new(DiagnosticError::new(message, vec![diagnostic])),
    }
}

/// Whether this failure is the command being stopped, which stays cancellation.
fn is_cancellation(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.is::<tola_build::cancellation::BuildCancelled>() || cause.is::<InputCancelled>()
    })
}

struct VendorReplacement {
    site: PathBuf,
    root: PathBuf,
    workspace: PathBuf,
    owner: Vec<u8>,
}

impl VendorReplacement {
    /// Resolves the site's vendor destinations without touching them.
    fn locate(config: &tola_build::config::ResolvedSiteConfig) -> Result<Self> {
        let site = fs::canonicalize(config.get_root()).with_context(|| {
            format!(
                "could not resolve the site directory `{}`",
                crate::terminal::display_path(config.get_root())
            )
        })?;
        let declared = config
            .vendor()
            .path
            .as_deref()
            .context("vendor path is missing")?;
        let relative = declared
            .strip_prefix(config.get_root())
            .context("vendor path must stay below the site root")?;
        ensure!(
            relative
                .components()
                .all(|part| matches!(part, Component::Normal(_) | Component::CurDir)),
            "vendor path must stay below the site root"
        );
        let root = site.join(relative);
        let workspace = config
            .vendor()
            .workspace_path()
            .context("vendor path must name a directory below the site root")?;
        let workspace = site.join(
            workspace
                .strip_prefix(config.get_root())
                .context("vendor path must name a directory below the site root")?,
        );
        let mut owner = b"tola-vendor-root\nvendor=".to_vec();
        for part in relative.components() {
            if let Component::Normal(name) = part {
                owner.extend_from_slice(name.as_encoded_bytes());
                owner.push(b'/');
            }
        }
        owner.push(b'\n');
        let replacement = Self {
            site,
            root,
            workspace,
            owner,
        };
        replacement.validate_destinations()?;
        Ok(replacement)
    }

    /// Refuses a dry run over an installation interrupted before it committed: recovering it
    /// restores the previous vendor trees, which the dry run must leave alone.
    fn reject_interrupted_install(config: &tola_build::config::ResolvedSiteConfig) -> Result<()> {
        let replacement = Self::locate(config)?;
        if !replacement.workspace_occupied()? {
            return Ok(());
        }
        // A foreign or malformed workspace keeps its own refusal instead of the recovery hint.
        replacement.validate_workspace()?;
        ensure!(
            !replacement.install_interrupted()?,
            "an interrupted vendor installation needs recovery; run `tola vendor` to restore the previous inputs"
        );
        Ok(())
    }

    fn open(config: &tola_build::config::ResolvedSiteConfig) -> Result<Self> {
        let replacement = Self::locate(config)?;
        if replacement.workspace_occupied()? {
            replacement.discard()?;
        } else if replacement.directory_exists(&replacement.workspace)? {
            replacement.remove_workspace(VendorStep::Preparing)?;
        }
        replacement.create_workspace()?;
        replacement.claim_workspace()?;
        let candidate = replacement.candidate();
        replacement.create_prepared_directory(&candidate)?;
        replacement.create_prepared_directory(&replacement.workspace.join("previous"))?;
        for name in VENDOR_TREES {
            replacement.create_prepared_directory(&candidate.join(name))?;
        }
        Ok(replacement)
    }

    /// Whether the workspace holds anything; an empty directory carries no installation state.
    fn workspace_occupied(&self) -> Result<bool> {
        if !self.directory_exists(&self.workspace)? {
            return Ok(false);
        }
        Ok(fs::read_dir(&self.workspace)
            .with_context(|| format!("could not read `{}`", self.workspace.display()))
            .map_err(|error| self.step_failure(VendorStep::Reading, &self.workspace, error))?
            .next()
            .is_some())
    }

    /// Whether the workspace journal records an installation interrupted before it committed.
    fn install_interrupted(&self) -> Result<bool> {
        if self.directory_exists(&self.workspace.join("committed"))? {
            return Ok(false);
        }
        let journal = self.workspace.join("replacing");
        journal
            .try_exists()
            .with_context(|| format!("could not inspect `{}`", journal.display()))
            .map_err(|error| self.step_failure(VendorStep::Reading, &self.workspace, error))
    }

    fn candidate(&self) -> PathBuf {
        self.workspace.join("candidate")
    }

    /// Create the working directory Tola prepares the replacement in.
    fn create_workspace(&self) -> Result<()> {
        fs::create_dir_all(&self.workspace)
            .with_context(|| format!("could not create `{}`", self.workspace.display()))
            .map_err(|error| self.step_failure(VendorStep::Preparing, &self.workspace, error))
    }

    /// Remove the working directory Tola prepared the replacement in.
    fn remove_workspace(&self, step: VendorStep) -> Result<()> {
        fs::remove_dir(&self.workspace)
            .with_context(|| format!("could not remove `{}`", self.workspace.display()))
            .map_err(|error| self.step_failure(step, &self.workspace, error))
    }

    /// Claim the working directory for this site's vendor path.
    fn claim_workspace(&self) -> Result<()> {
        let owner = self.workspace.join("owner");
        let mut marker = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&owner)
            .with_context(|| format!("could not create `{}`", owner.display()))
            .map_err(|error| self.step_failure(VendorStep::Preparing, &self.workspace, error))?;
        marker
            .write_all(&self.owner)
            .with_context(|| format!("could not write `{}`", owner.display()))
            .map_err(|error| self.step_failure(VendorStep::Preparing, &self.workspace, error))?;
        marker
            .sync_all()
            .with_context(|| format!("could not write `{}`", owner.display()))
            .map_err(|error| self.step_failure(VendorStep::Preparing, &self.workspace, error))
    }

    /// Create one directory below the working directory Tola prepares the replacement in.
    fn create_prepared_directory(&self, path: &Path) -> Result<()> {
        fs::create_dir(path)
            .with_context(|| format!("could not create `{}`", path.display()))
            .map_err(|error| self.step_failure(VendorStep::Preparing, &self.workspace, error))
    }

    /// The site author's spelling of a vendor path.
    ///
    /// A path Tola only uses while preparing the replacement is reported as the vendored
    /// destination, whose exact spelling stays in the raw chain.
    fn display_destination(&self, path: &Path) -> String {
        if path.starts_with(&self.workspace) {
            return display_path_within(&self.root, &self.site);
        }
        display_path_within(path, &self.site)
    }

    /// Report the failure of one vendor step on `path`.
    fn step_failure(&self, step: VendorStep, path: &Path, source: anyhow::Error) -> anyhow::Error {
        let destination = self.display_destination(path);
        vendor_failure(
            &destination,
            step.message(&destination),
            step.advice(),
            Some(source),
        )
    }

    /// Report a refusal Tola states itself about `path`.
    fn refusal(&self, path: &Path, reason: &str, advice: &str) -> anyhow::Error {
        let destination = self.display_destination(path);
        vendor_failure(
            &destination,
            format!("Tola cannot prepare `{destination}`: {reason}"),
            advice,
            None,
        )
    }

    fn validate_destinations(&self) -> Result<()> {
        ensure!(
            self.root != self.site,
            "vendor path must name a directory below the site root"
        );
        self.validate_directory_chain(&self.root)?;
        self.validate_directory_chain(&self.workspace)?;
        for name in VENDOR_TREES {
            self.validate_directory_chain(&self.root.join(name))?;
        }
        Ok(())
    }

    fn validate_workspace(&self) -> Result<()> {
        self.validate_directory_chain(&self.workspace)?;
        let marker = self.workspace.join("owner");
        let metadata = fs::symlink_metadata(&marker).map_err(|error| {
            let destination = self.display_destination(&self.root);
            vendor_failure(
                &destination,
                format!(
                    "Tola cannot prepare `{destination}`: the working files beside the vendored inputs are not Tola's own"
                ),
                VENDOR_PATH_CONFLICT_ADVICE,
                Some(
                    anyhow::Error::new(error)
                        .context(format!("could not read `{}`", marker.display())),
                ),
            )
        })?;
        let owned = metadata.is_file()
            && !metadata.is_symlink()
            && fs::read(&marker)
                .with_context(|| format!("could not read `{}`", marker.display()))
                .map_err(|error| self.step_failure(VendorStep::Reading, &self.workspace, error))?
                == self.owner;
        if !owned {
            return Err(self.refusal(
                &self.root,
                "the working files beside the vendored inputs belong to another site",
                VENDOR_PATH_CONFLICT_ADVICE,
            ));
        }
        let entries = fs::read_dir(&self.workspace)
            .with_context(|| format!("could not read `{}`", self.workspace.display()))
            .map_err(|error| self.step_failure(VendorStep::Reading, &self.workspace, error))?;
        for child in entries {
            let child = child
                .with_context(|| format!("could not read `{}`", self.workspace.display()))
                .map_err(|error| self.step_failure(VendorStep::Reading, &self.workspace, error))?;
            let name = child.file_name();
            let path = child.path();
            let metadata = child
                .file_type()
                .with_context(|| format!("could not read `{}`", path.display()))
                .map_err(|error| self.step_failure(VendorStep::Reading, &self.workspace, error))?;
            let directory = ["candidate", "previous", "committed"]
                .iter()
                .any(|known| name == *known);
            let file = ["owner", "replacing", "replacing.pending"]
                .iter()
                .any(|known| name == *known);
            if !((directory && metadata.is_dir()) || (file && metadata.is_file())) {
                return Err(self.refusal(
                    &self.root,
                    "the working files beside the vendored inputs hold a path Tola does not own",
                    VENDOR_PATH_CONFLICT_ADVICE,
                ));
            }
        }
        for base in [self.candidate(), self.workspace.join("previous")] {
            for name in VENDOR_TREES {
                self.validate_directory_chain(&base.join(name))?;
            }
        }
        Ok(())
    }

    fn install(&self, cancellation: &BuildCancellation) -> Result<()> {
        cancellation.ensure_active()?;
        self.validate_destinations()?;
        self.validate_workspace()?;
        let mut previous = [b'0'; VENDOR_TREES.len()];
        for (index, name) in VENDOR_TREES.iter().enumerate() {
            if self.directory_exists(&self.root.join(name))? {
                previous[index] = b'1';
            }
        }
        let pending = self.workspace.join("replacing.pending");
        let mut journal = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pending)
            .with_context(|| format!("could not create `{}`", pending.display()))
            .map_err(|error| self.step_failure(VendorStep::Installing, &self.root, error))?;
        journal
            .write_all(&previous)
            .with_context(|| format!("could not write `{}`", pending.display()))
            .map_err(|error| self.step_failure(VendorStep::Installing, &self.root, error))?;
        journal
            .sync_all()
            .with_context(|| format!("could not write `{}`", pending.display()))
            .map_err(|error| self.step_failure(VendorStep::Installing, &self.root, error))?;
        drop(journal);
        fs::rename(&pending, self.workspace.join("replacing"))
            .with_context(|| format!("could not move `{}` into place", pending.display()))
            .map_err(|error| self.step_failure(VendorStep::Installing, &self.root, error))?;
        fs::create_dir_all(&self.root)
            .with_context(|| format!("could not create `{}`", self.root.display()))
            .map_err(|error| self.step_failure(VendorStep::Installing, &self.root, error))?;
        // The journal precedes every rename. Until `committed` exists, recovery restores all
        // previous trees, including those installed before an interruption between subtrees.
        for (index, name) in VENDOR_TREES.iter().enumerate() {
            cancellation.ensure_active()?;
            self.validate_destinations()?;
            let tree = self.root.join(name);
            if previous[index] == b'1' {
                fs::rename(&tree, self.workspace.join("previous").join(name))
                    .with_context(|| format!("could not retain previous vendor `{name}`"))
                    .map_err(|error| self.step_failure(VendorStep::Installing, &tree, error))?;
            }
            fs::rename(self.candidate().join(name), &tree)
                .with_context(|| format!("could not install prepared vendor `{name}`"))
                .map_err(|error| self.step_failure(VendorStep::Installing, &tree, error))?;
        }
        cancellation.ensure_active()?;
        let committed = self.workspace.join("committed");
        fs::create_dir(&committed)
            .with_context(|| format!("could not create `{}`", committed.display()))
            .map_err(|error| self.step_failure(VendorStep::Installing, &self.root, error))?;
        Ok(())
    }

    fn discard(&self) -> Result<()> {
        self.validate_destinations()?;
        self.validate_workspace()?;
        let journal = self.workspace.join("replacing");
        if self.install_interrupted()? {
            let previous = fs::read(&journal)
                .with_context(|| format!("could not read `{}`", journal.display()))
                .map_err(|error| self.step_failure(VendorStep::Undoing, &self.root, error))?;
            if previous.len() != VENDOR_TREES.len()
                || !previous
                    .iter()
                    .all(|present| matches!(*present, b'0' | b'1'))
            {
                let destination = self.display_destination(&self.root);
                return Err(vendor_failure(
                    &destination,
                    format!(
                        "Tola cannot undo the replacement of `{destination}`: the record of the unfinished replacement is damaged"
                    ),
                    VENDOR_PATH_CONFLICT_ADVICE,
                    None,
                ));
            }
            // Cancellation must not interrupt rollback between input trees.
            for (index, name) in VENDOR_TREES.iter().enumerate() {
                let backup = self.workspace.join("previous").join(name);
                let current = self.root.join(name);
                if self.directory_exists(&backup)? {
                    self.remove_directory(&current, VendorStep::Undoing)?;
                    fs::rename(&backup, &current)
                        .with_context(|| format!("could not restore previous vendor `{name}`"))
                        .map_err(|error| self.step_failure(VendorStep::Undoing, &current, error))?;
                } else if previous[index] == b'0' {
                    self.remove_directory(&current, VendorStep::Undoing)?;
                }
            }
        }
        self.cleanup()
    }

    fn cleanup(&self) -> Result<()> {
        self.validate_workspace()?;
        self.remove_directory(&self.candidate(), VendorStep::Cleaning)?;
        self.remove_directory(&self.workspace.join("previous"), VendorStep::Cleaning)?;
        for name in ["replacing", "replacing.pending"] {
            let path = self.workspace.join(name);
            if path
                .try_exists()
                .with_context(|| format!("could not inspect `{}`", path.display()))
                .map_err(|error| self.step_failure(VendorStep::Cleaning, &self.workspace, error))?
            {
                fs::remove_file(&path)
                    .with_context(|| format!("could not remove `{}`", path.display()))
                    .map_err(|error| {
                        self.step_failure(VendorStep::Cleaning, &self.workspace, error)
                    })?;
            }
        }
        let committed = self.workspace.join("committed");
        if self.directory_exists(&committed)? {
            fs::remove_dir(&committed)
                .with_context(|| format!("could not remove `{}`", committed.display()))
                .map_err(|error| self.step_failure(VendorStep::Cleaning, &self.workspace, error))?;
        }
        let owner = self.workspace.join("owner");
        fs::remove_file(&owner)
            .with_context(|| format!("could not remove `{}`", owner.display()))
            .map_err(|error| self.step_failure(VendorStep::Cleaning, &self.workspace, error))?;
        self.remove_workspace(VendorStep::Cleaning)
    }

    /// Whether `path` is a real directory, refusing a link or a file at that name.
    fn directory_exists(&self, path: &Path) -> Result<bool> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(self.step_failure(
                    VendorStep::Reading,
                    path,
                    anyhow::Error::new(error)
                        .context(format!("could not inspect `{}`", path.display())),
                ));
            }
        };
        if metadata.is_dir() && !metadata.is_symlink() {
            return Ok(true);
        }
        if path.starts_with(&self.workspace) {
            return Err(self.refusal(
                &self.root,
                "the working files Tola keeps beside the vendored inputs are not a directory it can use",
                VENDOR_PATH_CONFLICT_ADVICE,
            ));
        }
        Err(self.refusal(
            path,
            "it must be a real directory, not a link or a file",
            "Move that path aside, then rerun `tola vendor`",
        ))
    }

    /// Refuse a vendor destination whose path leaves the site or crosses a link.
    fn validate_directory_chain(&self, directory: &Path) -> Result<()> {
        let relative = directory
            .strip_prefix(&self.site)
            .context("vendor destination leaves the site")?;
        let mut current = self.site.clone();
        for part in relative.components() {
            ensure!(
                matches!(part, Component::Normal(_) | Component::CurDir),
                "vendor destination leaves the site"
            );
            current.push(part.as_os_str());
            if !self.directory_exists(&current)? {
                break;
            }
        }
        Ok(())
    }

    /// Remove one vendor directory tree, reporting it as `step`.
    fn remove_directory(&self, path: &Path, step: VendorStep) -> Result<()> {
        if !self.directory_exists(path)? {
            return Ok(());
        }
        fs::remove_dir_all(path)
            .with_context(|| format!("could not remove vendor tree `{}`", path.display()))
            .map_err(|error| self.step_failure(step, path, error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replacement(site: &Path) -> VendorReplacement {
        let root = site.join("vendor");
        let workspace = site.join(".vendor-vendor");
        fs::create_dir_all(workspace.join("candidate")).unwrap();
        fs::create_dir(workspace.join("previous")).unwrap();
        let owner = b"vendor owner".to_vec();
        fs::write(workspace.join("owner"), &owner).unwrap();
        for name in VENDOR_TREES {
            fs::create_dir(workspace.join("candidate").join(name)).unwrap();
        }
        VendorReplacement {
            site: site.to_path_buf(),
            root,
            workspace,
            owner,
        }
    }

    /// A site configuration whose `[vendor] path` names this root's `vendor` directory.
    fn site_config(root: &Path) -> tola_build::config::ResolvedSiteConfig {
        tola_build::config::loading::resolve_site_config(
            &root.join("tola.toml"),
            "[vendor]\npath = \"vendor\"\n",
            tola_typst::PackageLocations::default(),
            &tola_build::config::loading::BuildOverrides::default(),
        )
        .unwrap()
        .into_config()
    }

    /// A copy that reads `boundary` and writes through `replacement`.
    fn vendor_copy<'a>(
        boundary: tola_typst::SourceBoundary,
        replacement: &'a VendorReplacement,
        cancellation: &'a BuildCancellation,
    ) -> VendorCopy<'a> {
        VendorCopy {
            boundary,
            replacement,
            cancellation,
            buffer: [0; 64 * 1024],
        }
    }

    /// The superseded `typst-packages` tree a replacement must restore on discard.
    fn seed_old_package(replacement: &VendorReplacement) {
        fs::create_dir_all(replacement.root.join("typst-packages")).unwrap();
        fs::write(
            replacement.root.join("typst-packages/lib.typ"),
            b"old package",
        )
        .unwrap();
    }

    #[test]
    fn interrupted_replace_restores_inputs() {
        let site = tempfile::TempDir::new().unwrap();
        let replacement = replacement(site.path());
        seed_old_package(&replacement);
        fs::write(replacement.root.join("notes.txt"), b"author notes").unwrap();
        fs::write(replacement.workspace.join("replacing"), b"100").unwrap();
        fs::rename(
            replacement.root.join("typst-packages"),
            replacement.workspace.join("previous/typst-packages"),
        )
        .unwrap();
        fs::rename(
            replacement.candidate().join("typst-packages"),
            replacement.root.join("typst-packages"),
        )
        .unwrap();
        fs::write(
            replacement.root.join("typst-packages/lib.typ"),
            b"incomplete replacement",
        )
        .unwrap();
        fs::rename(
            replacement.candidate().join("fonts"),
            replacement.root.join("fonts"),
        )
        .unwrap();
        replacement.discard().unwrap();
        assert_eq!(
            fs::read(replacement.root.join("typst-packages/lib.typ")).unwrap(),
            b"old package"
        );
        assert_eq!(
            fs::read(replacement.root.join("notes.txt")).unwrap(),
            b"author notes"
        );
        assert!(!replacement.root.join("fonts").exists());
    }

    #[test]
    fn cancelled_replace_keeps_previous() {
        let site = tempfile::TempDir::new().unwrap();
        let replacement = replacement(site.path());
        seed_old_package(&replacement);
        let canceller = tola_build::cancellation::BuildCanceller::new();
        canceller.cancel();
        assert!(replacement.install(&canceller.token()).is_err());
        replacement.discard().unwrap();
        assert_eq!(
            fs::read(replacement.root.join("typst-packages/lib.typ")).unwrap(),
            b"old package"
        );
    }

    #[test]
    fn committed_replace_keeps_new_inputs() {
        let site = tempfile::TempDir::new().unwrap();
        let replacement = replacement(site.path());
        seed_old_package(&replacement);
        fs::write(
            replacement.candidate().join("typst-packages/lib.typ"),
            b"new package",
        )
        .unwrap();

        replacement.install(&BuildCancellation::default()).unwrap();
        replacement.discard().unwrap();

        assert_eq!(
            fs::read(replacement.root.join("typst-packages/lib.typ")).unwrap(),
            b"new package"
        );
    }

    #[test]
    fn interrupted_first_install_is_removed() {
        let site = tempfile::TempDir::new().unwrap();
        let replacement = replacement(site.path());
        fs::create_dir(&replacement.root).unwrap();
        fs::write(replacement.workspace.join("replacing"), b"000").unwrap();
        fs::rename(
            replacement.candidate().join("typst-packages"),
            replacement.root.join("typst-packages"),
        )
        .unwrap();
        fs::write(
            replacement.root.join("typst-packages/typst.toml"),
            b"partial",
        )
        .unwrap();

        replacement.discard().unwrap();

        assert!(!replacement.root.join("typst-packages").exists());
    }

    #[test]
    fn foreign_workspace_is_preserved() {
        let site = tempfile::TempDir::new().unwrap();
        let replacement = replacement(site.path());
        fs::write(replacement.workspace.join("owner"), b"another owner").unwrap();
        fs::write(replacement.candidate().join("notes.txt"), b"author notes").unwrap();

        assert!(replacement.discard().is_err());

        assert_eq!(
            fs::read(replacement.candidate().join("notes.txt")).unwrap(),
            b"author notes"
        );
    }

    #[test]
    fn source_copy_cannot_enter_itself() {
        let site = tempfile::TempDir::new().unwrap();
        let source = fs::canonicalize(site.path()).unwrap();
        fs::write(source.join("lib.typ"), b"source bytes").unwrap();
        let replacement = replacement(&source);
        let destination = source.join("candidate");
        let cancellation = BuildCancellation::default();
        let mut copy = vendor_copy(
            tola_typst::SourceBoundary::new(&source, false),
            &replacement,
            &cancellation,
        );

        assert!(
            copy.directory(&source, &destination, &mut Vec::new())
                .is_err()
        );

        assert_eq!(fs::read(source.join("lib.typ")).unwrap(), b"source bytes");
        assert!(!destination.exists());
    }

    #[cfg(unix)]
    #[test]
    fn source_links_become_regular_bytes() {
        let site = tempfile::TempDir::new().unwrap();
        let source = site.path().join("packages");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("lib.typ"), b"package bytes").unwrap();
        std::os::unix::fs::symlink("lib.typ", source.join("linked.typ")).unwrap();
        let replacement = replacement(site.path());
        let destination = site.path().join("candidate");
        let cancellation = BuildCancellation::default();
        let mut copy = vendor_copy(
            tola_typst::SourceBoundary::new(site.path(), false),
            &replacement,
            &cancellation,
        );
        copy.directory(&source, &destination, &mut Vec::new())
            .unwrap();
        assert!(
            !fs::symlink_metadata(destination.join("linked.typ"))
                .unwrap()
                .is_symlink()
        );
        assert_eq!(
            fs::read(destination.join("linked.typ")).unwrap(),
            b"package bytes"
        );
    }

    #[cfg(unix)]
    #[test]
    fn source_cycles_are_refused() {
        let site = tempfile::TempDir::new().unwrap();
        let source = site.path().join("packages");
        fs::create_dir(&source).unwrap();
        std::os::unix::fs::symlink(".", source.join("cycle")).unwrap();
        let replacement = replacement(site.path());
        let destination = site.path().join("candidate");
        let cancellation = BuildCancellation::default();
        let mut copy = vendor_copy(
            tola_typst::SourceBoundary::new(site.path(), false),
            &replacement,
            &cancellation,
        );
        assert!(
            copy.directory(&source, &destination, &mut Vec::new())
                .is_err()
        );
    }

    #[test]
    fn prepared_paths_are_reported_as_vendored_destinations() {
        let site = tempfile::TempDir::new().unwrap();
        let replacement = replacement(site.path());
        let cancellation = BuildCancellation::default();
        let copy = vendor_copy(
            tola_typst::SourceBoundary::new(site.path(), false),
            &replacement,
            &cancellation,
        );

        assert_eq!(
            copy.vendored_destination(&replacement.candidate().join("fonts/Inter.ttf")),
            "vendor/fonts/Inter.ttf"
        );
        assert_eq!(
            copy.vendored_destination(&replacement.workspace.join("previous/fonts/Inter.ttf")),
            "vendor/fonts/Inter.ttf"
        );
        assert_eq!(
            copy.vendored_destination(&replacement.workspace.join("replacing")),
            "vendor"
        );
        assert_eq!(
            replacement.display_destination(&replacement.workspace.join("replacing")),
            "vendor"
        );
        assert_eq!(
            replacement.display_destination(&replacement.root.join("typst-packages")),
            "vendor/typst-packages"
        );
    }

    #[test]
    fn committed_install_needs_no_recovery() {
        let site = tempfile::TempDir::new().unwrap();
        let config = site_config(site.path());
        let replacement = VendorReplacement::open(&config).unwrap();
        fs::create_dir_all(replacement.root.join("typst-packages")).unwrap();
        fs::write(
            replacement.root.join("typst-packages/lib.typ"),
            b"committed inputs",
        )
        .unwrap();
        fs::write(replacement.workspace.join("replacing"), b"100").unwrap();
        fs::create_dir(replacement.workspace.join("committed")).unwrap();

        assert!(VendorReplacement::reject_interrupted_install(&config).is_ok());
        VendorReplacement::open(&config).unwrap();
        assert_eq!(
            fs::read(replacement.root.join("typst-packages/lib.typ")).unwrap(),
            b"committed inputs"
        );
    }
}
