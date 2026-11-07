//! Freeze build-selected dependencies without exposing an incomplete vendor tree.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use tola_build::cancellation::BuildCancellation;
use tola_build::config::ResolvedSiteConfig;
use tola_build::filesystem::{SwapError, SwapMember, TreeSwap};
use tola_build::{BuildResources, InputScope};
use tola_diagnostic::{Diagnostic, DiagnosticError, Severity};

use crate::cancellation::Cancellation;
use crate::cli::output::CommandOutput;
use crate::cli::{ConfigFileArgs, TypstPackageArgs, VendorArgs};
use crate::terminal::{InputCancelled, display_path_within};

const VENDOR_TREES: [&str; 3] = ["modules", "typst-packages", "icons"];

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
    let Some(vendor_path) = config.vendor().path.as_deref() else {
        anyhow::bail!(
            "this site does not declare a `[vendor] path`; add `path = \"vendor\"` to `{}`",
            config_name.display()
        );
    };
    let _lock = tola_build::build::SiteBuildLock::acquire(&config, &cancellation, || {
        let _ = output.waiting_for_build();
    })?;
    let replacement = VendorReplacement::locate(&config, vendor_path)?;
    if vendor.dry_run {
        replacement.refuse_unfinished_install()?;
    }
    // Earlier releases froze host fonts under `fonts/`; Tola reads no vendored fonts now.
    let legacy = vendor_path.join("fonts");
    if legacy.is_dir() {
        let display = tola_build::filesystem::display_path(&legacy, config.get_root());
        if vendor.dry_run {
            output.status(format!("Would remove the obsolete `{display}`"))?;
        } else {
            fs::remove_dir_all(&legacy)
                .with_context(|| format!("could not remove the obsolete `{display}`"))?;
            output.status(format!(
                "Removed the obsolete `{display}`; fonts are site inputs under `[typst.fonts] paths`"
            ))?;
        }
    }
    // Settle what an earlier run left before any build reads the vendored inputs.
    replacement.recover()?;
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
        replacement.open()?;
        let candidate = replacement.candidate_root();
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
        copy.modules(&selection.modules)?;
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
            replacement.commit(&cancellation)?;
        }
        Ok(())
    })();
    if let Err(error) = prepared {
        if let Err(recovery) = replacement.recover() {
            // The recovery failure is not part of the returned chain, so the file log keeps it.
            tracing::warn!(
                target: "tola::vendor",
                error = %format!("{recovery:#}"),
                "vendor recovery did not finish"
            );
            if !is_cancellation(&error) {
                let note = "an unfinished replacement is left behind, so the next build refuses the vendored inputs";
                let diagnostics =
                    crate::cli::output::attached_or_fallback(&error, crate::codes::command::FAILED)
                        .into_iter()
                        .map(|diagnostic| diagnostic.with_note(note))
                        .collect();
                return Err(tola_diagnostic::DiagnosticError::attach(error, diagnostics).into());
            }
        }
        return Err(error);
    }
    if let Err(error) = replacement.finish() {
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

struct SelectedInputs {
    site: tola_build::build::SiteBuild,
    packages: Vec<SelectedPackage>,
    modules: Vec<SelectedModule>,
}

/// One remote module directory `tola vendor` freezes below `modules/`.
struct SelectedModule {
    alias: String,
    source: PathBuf,
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
    // Resolve frozen inputs, not site validity: references belong to `tola check`.
    request.reference_checks = tola_build::build::ReferenceChecks::Skip;
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
    let mode = resources.input_mode();
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
    let roots = tola_build::inputs::module_roots(
        config.get_root(),
        mode,
        None,
        config.modules(),
        cancellation,
    )?;
    let mut modules = Vec::new();
    for (alias, declaration) in config.modules() {
        if !matches!(
            declaration.source,
            tola_build::modules::ModuleSource::Remote { .. }
        ) {
            continue;
        }
        let source = roots
            .get(alias)
            .with_context(|| format!("module `{alias}` has no resolved directory"))?
            .clone();
        modules.push(SelectedModule {
            alias: alias.clone(),
            source,
        });
    }
    Ok(SelectedInputs {
        site,
        packages: packages.into_values().collect(),
        modules,
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
            "the build still reads `{}` from the previous vendor inputs; keep site-owned sources outside `modules`, `typst-packages`, and `icons`",
            replacement.destination(path)
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
            replacement.destination(&destination)
        ))?;
    }
    for module in &selection.modules {
        output.status(format!("{action} module `{}`", module.alias))?;
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
        if physical.starts_with(self.replacement.workspace()) || destination.starts_with(&physical)
        {
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

    /// Freeze each resolved remote module directory into the prepared replacement.
    fn modules(&mut self, modules: &[SelectedModule]) -> Result<()> {
        for module in modules {
            self.cancellation.ensure_active()?;
            let destination = self
                .replacement
                .candidate_root()
                .join("modules")
                .join(&module.alias);
            self.directory(&module.source, &destination, &mut Vec::new())?;
        }
        Ok(())
    }

    /// Freeze the verified icon collections into the prepared replacement.
    fn icons(&self, icons: &[SelectedIconCollection]) -> Result<()> {
        for collection in icons {
            self.cancellation.ensure_active()?;
            let destination = self
                .replacement
                .candidate_root()
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
        self.replacement.step_failure(step, destination, source)
    }

    /// Report a refusal Tola states itself about one copy step.
    fn refusal(&self, destination: &Path, reason: &str, advice: &str) -> anyhow::Error {
        self.replacement.refusal(destination, reason, advice)
    }
}

/// One step of the vendor operation, stated for the site author who must act on its failure.
#[derive(Clone, Copy)]
enum VendorStep {
    /// Claiming the workspace and creating the trees the replacement is prepared in.
    Preparing,
    /// Replacing the vendored trees with the prepared ones.
    Installing,
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
            Self::Installing => "replace the vendored inputs in",
            Self::ReadingSource => "read the source of",
            Self::WritingCopy => "prepare",
        }
    }

    /// The reader's next step.
    fn advice(self) -> &'static str {
        match self {
            Self::Preparing => {
                "Check that Tola can create and remove directories beside the vendored inputs, then rerun `tola vendor`"
            }
            Self::Installing => {
                "Check that Tola can write inside the vendored directory and the site directory, then rerun `tola vendor`"
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

/// The vendored trees one run replaces, prepared in a swap workspace beside them.
struct VendorReplacement {
    site: PathBuf,
    root: PathBuf,
    swap: TreeSwap,
}

impl VendorReplacement {
    /// Resolves the site's vendor destinations without touching them.
    fn locate(config: &ResolvedSiteConfig, vendor_path: &Path) -> Result<Self> {
        let site = fs::canonicalize(config.get_root()).with_context(|| {
            format!(
                "could not resolve the site directory `{}`",
                crate::terminal::display_path(config.get_root())
            )
        })?;
        let root = site.join(config.root_relative(vendor_path));
        let replacement = Self {
            swap: TreeSwap::beside(&site, &root)?,
            site,
            root,
        };
        replacement.validate_destinations()?;
        Ok(replacement)
    }

    /// Refuse a dry run while an earlier replacement still needs `tola vendor` to finish it.
    fn refuse_unfinished_install(&self) -> Result<()> {
        ensure!(
            !self
                .swap
                .is_claimed()
                .map_err(|error| self.swap_failure(error))?,
            "an interrupted vendor installation needs recovery; run `tola vendor` to finish it"
        );
        Ok(())
    }

    /// Claim the workspace and create the trees the verification build reads.
    fn open(&self) -> Result<()> {
        self.swap
            .claim()
            .map_err(|error| self.swap_failure(error))?;
        for name in VENDOR_TREES {
            let tree = self.candidate_root().join(name);
            fs::create_dir_all(&tree)
                .with_context(|| format!("could not create `{}`", tree.display()))
                .map_err(|error| self.step_failure(VendorStep::Preparing, &tree, error))?;
        }
        Ok(())
    }

    /// Complete a replacement an earlier run committed, or discard one it never did.
    fn recover(&self) -> Result<()> {
        self.swap
            .recover()
            .map_err(|error| self.swap_failure(error))?;
        self.swap.finish().map_err(|error| self.swap_failure(error))
    }

    /// Replace both vendored trees with the verified ones.
    fn commit(&self, cancellation: &BuildCancellation) -> Result<()> {
        cancellation.ensure_active()?;
        let staged = self.swap.staged(0);
        let staged_directory = staged
            .parent()
            .expect("a staged tree lives below the workspace");
        fs::create_dir_all(staged_directory)
            .with_context(|| format!("could not prepare `{}`", staged_directory.display()))
            .map_err(|error| self.step_failure(VendorStep::Installing, &self.root, error))?;
        let mut members = Vec::with_capacity(VENDOR_TREES.len());
        for (index, name) in VENDOR_TREES.iter().enumerate() {
            let prepared = self.candidate_root().join(name);
            fs::rename(&prepared, self.swap.staged(index))
                .with_context(|| format!("could not prepare `{}`", prepared.display()))
                .map_err(|error| self.step_failure(VendorStep::Installing, &prepared, error))?;
            members.push(SwapMember {
                staged: self.swap.staged(index),
                destination: self.root.join(name),
            });
        }
        fs::create_dir_all(&self.root)
            .with_context(|| format!("could not create `{}`", self.root.display()))
            .map_err(|error| self.step_failure(VendorStep::Installing, &self.root, error))?;
        self.swap
            .commit(&members)
            .map_err(|error| self.swap_failure(error))
    }

    /// Remove the workspace a completed replacement leaves behind.
    fn finish(&self) -> Result<()> {
        self.swap.finish()?;
        Ok(())
    }

    /// The vendor root the verification build reads, holding both prepared trees by name.
    fn candidate_root(&self) -> PathBuf {
        self.swap.workspace().join("candidate")
    }

    fn workspace(&self) -> &Path {
        self.swap.workspace()
    }

    /// The site author's spelling of a path Tola uses while it prepares the replacement.
    fn destination(&self, path: &Path) -> String {
        match path.strip_prefix(self.candidate_root()) {
            Ok(relative) => display_path_within(&self.root.join(relative), &self.site),
            Err(_) => display_path_within(path, &self.site),
        }
    }

    /// Report the failure of one vendor step on `path`.
    fn step_failure(&self, step: VendorStep, path: &Path, source: anyhow::Error) -> anyhow::Error {
        let destination = self.destination(path);
        vendor_failure(
            &destination,
            step.message(&destination),
            step.advice(),
            Some(source),
        )
    }

    /// Report a refusal Tola states itself about `path`.
    fn refusal(&self, path: &Path, reason: &str, advice: &str) -> anyhow::Error {
        let destination = self.destination(path);
        vendor_failure(
            &destination,
            format!("Tola cannot prepare `{destination}`: {reason}"),
            advice,
            None,
        )
    }

    /// Report a failed replacement step, refusing another site's workspace in Tola's words.
    fn swap_failure(&self, error: SwapError) -> anyhow::Error {
        match error {
            SwapError::ForeignWorkspace { .. } => self.refusal(
                &self.root,
                "the working files beside the vendored inputs belong to another site",
                VENDOR_PATH_CONFLICT_ADVICE,
            ),
            error => anyhow::Error::new(error),
        }
    }

    /// Refuse a vendored tree a replacement would write through: a link or a plain file at any
    /// name below the vendor path.
    fn validate_destinations(&self) -> Result<()> {
        self.validate_directory_chain(&self.root)?;
        for name in VENDOR_TREES {
            self.validate_directory_chain(&self.root.join(name))?;
        }
        Ok(())
    }

    /// Whether `path` is a real directory, refusing a link or a file at that name.
    fn directory_exists(&self, path: &Path) -> Result<bool> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                let destination = self.destination(path);
                return Err(vendor_failure(
                    &destination,
                    format!("Tola could not inspect `{destination}`"),
                    "Check that Tola can read the vendored directory, then rerun `tola vendor`",
                    Some(anyhow::Error::new(error)),
                ));
            }
        };
        if metadata.is_dir() && !metadata.is_symlink() {
            return Ok(true);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vendor replacement of this site's declared `vendor` directory, workspace claimed.
    fn replacement(site: &Path) -> VendorReplacement {
        // The resolved site root is canonical, so the test spells the target the same way.
        let site = site.canonicalize().unwrap();
        let replacement =
            VendorReplacement::locate(&site_config(&site), &site.join("vendor")).unwrap();
        replacement.open().unwrap();
        replacement
    }

    /// A site configuration whose `[vendor] path` names this root's `vendor` directory.
    fn site_config(root: &Path) -> ResolvedSiteConfig {
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

    #[test]
    fn foreign_swap_workspace_is_refused() {
        let site = tempfile::TempDir::new().unwrap();
        let workspace = site.path().join(".vendor-swap");
        fs::create_dir(&workspace).unwrap();
        fs::write(workspace.join("owner"), b"another site").unwrap();
        fs::write(workspace.join("author.txt"), b"author bytes").unwrap();

        let site_root = site.path().canonicalize().unwrap();
        let replacement =
            VendorReplacement::locate(&site_config(&site_root), &site_root.join("vendor")).unwrap();
        assert!(replacement.open().is_err());

        assert_eq!(
            fs::read(workspace.join("author.txt")).unwrap(),
            b"author bytes"
        );
    }

    /// A replacement the last run committed is completed by the next run, not undone.

    /// Both vendored trees are installed as one set; a site file beside them stays.

    /// A cancelled run keeps the installed trees and removes its workspace.

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
    fn prepared_paths_reported_vendored_destinations() {
        let site = tempfile::TempDir::new().unwrap();
        let replacement = replacement(site.path());

        assert_eq!(
            replacement.destination(&replacement.candidate_root().join("icons/lucide.json")),
            "vendor/icons/lucide.json"
        );
        assert_eq!(
            replacement.destination(&replacement.root.join("typst-packages")),
            "vendor/typst-packages"
        );
    }
}
