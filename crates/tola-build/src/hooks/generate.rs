//! Hook-generated files added to the candidate output graph.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::cancellation::BuildCancellation;
use crate::config::ResolvedSiteConfig;
use crate::config::section::build::hooks::{CommandOutput, HookStage, OutputCommandConfig};
use crate::filesystem::TemporaryDirectory;
use crate::mode::BuildMode;
use crate::output::files::HookOutputFiles;
use crate::output::graph::OutputFile;
use crate::output::owner::{OutputOwner, OutputRootOwnership};
use crate::output::semantics::OutputDeclaration;
use tola_address::OutputPath;

use super::command::{HookCall, HookDirectories, HookInvocation};
use super::report::{hook_command_error, report_reused, run_hook_entry};
use super::reuse::{ReusableHookOutput, ReusableHookOutputs, observe};

/// Files and exclusive directory ownerships produced by this candidate's commands.
#[derive(Debug, Default)]
pub(crate) struct GeneratedOutputs {
    pub(crate) outputs: Vec<OutputFile>,
    pub(crate) root_ownerships: Vec<OutputRootOwnership>,
    pub(crate) reuse: ReusableHookOutputs,
}

/// Run each participating producer against the same immutable upstream files.
///
/// A `dev = "reuse"` command runs only when its observed inputs differ from the previous
/// successful development revision; otherwise its outputs are reused unchanged.
pub(crate) fn generate_outputs(
    config: &ResolvedSiteConfig,
    upstream: &[OutputFile],
    mode: BuildMode,
    previous: Option<&ReusableHookOutputs>,
    cancellation: &BuildCancellation,
) -> Result<GeneratedOutputs> {
    cancellation.ensure_active().map_err(anyhow::Error::new)?;
    let mut participating = config
        .build
        .hooks
        .generate_outputs
        .iter()
        .enumerate()
        .filter(|(_, command)| super::hook_participates(command.enable, command.dev, mode))
        .peekable();
    if participating.peek().is_none() {
        return Ok(GeneratedOutputs::default());
    }
    // Materialize the input view only when a command actually needs to run: a round whose
    // generators all reuse their outputs must not copy the built site into a temporary tree.
    let mut candidate = None;
    let mut generated_outputs = GeneratedOutputs::default();
    for (index, command) in participating {
        cancellation.ensure_active().map_err(anyhow::Error::new)?;
        let reuses_outputs = mode == BuildMode::Development && command.dev.reuses_outputs();
        let inputs = if reuses_outputs {
            Some(observe(config.get_root(), command, upstream)?)
        } else {
            None
        };
        if let (Some(inputs), Some(previous)) = (inputs.as_ref(), previous)
            && let Some(previous) = previous.matching(&command.name, inputs)
        {
            let owner = OutputOwner::command(index, command.name.as_str());
            for output in &previous.outputs {
                generated_outputs
                    .outputs
                    .push(output.with_owner(owner.clone()));
            }
            for root in &previous.roots {
                generated_outputs
                    .root_ownerships
                    .push(OutputRootOwnership::new(root.root().clone(), owner.clone()));
            }
            generated_outputs.reuse.insert(
                &command.name,
                ReusableHookOutput {
                    inputs: inputs.clone(),
                    outputs: previous.outputs.clone(),
                    roots: previous.roots.clone(),
                },
            );
            report_reused(HookStage::GenerateOutputs, &command.name);
            continue;
        }
        if candidate.is_none() {
            candidate = Some(HookOutputFiles::materialize_outputs(
                config.get_root(),
                upstream,
                Some(cancellation),
                crate::compiler::minified_languages(config),
            )?);
        }
        let candidate = candidate.as_ref().expect("materialized above");
        let output_start = generated_outputs.outputs.len();
        let root_start = generated_outputs.root_ownerships.len();
        run_hook_entry(HookStage::GenerateOutputs, &command.name, || {
            generate_entry(
                &mut generated_outputs,
                command,
                index,
                candidate,
                config,
                mode,
                cancellation,
            )
        })?;
        if let Some(inputs) = inputs {
            generated_outputs.reuse.insert(
                &command.name,
                ReusableHookOutput {
                    inputs,
                    outputs: generated_outputs.outputs[output_start..].to_vec(),
                    roots: generated_outputs.root_ownerships[root_start..].to_vec(),
                },
            );
        }
    }
    cancellation.ensure_active().map_err(anyhow::Error::new)?;
    Ok(generated_outputs)
}

/// The declaration is a contract: a generated file no declaration owns, and a declared
/// file the command never wrote, both fail this entry.
fn generate_entry(
    generated_outputs: &mut GeneratedOutputs,
    command: &OutputCommandConfig,
    index: usize,
    candidate: &HookOutputFiles,
    config: &ResolvedSiteConfig,
    mode: BuildMode,
    cancellation: &BuildCancellation,
) -> Result<()> {
    if command.outputs.is_empty() {
        return Err(super::hook_contract_error(
            format!("`build.hooks.generate-outputs[{index}].outputs` is empty"),
            "declare each output with `{ file = … }` or `{ tree = … }`",
        ));
    }
    let generated = TemporaryDirectory::create(
        &config
            .get_root()
            .join(crate::filesystem::INTERNAL_DIR)
            .join("hook-generated"),
        "outputs",
    )?;
    let invocation = HookInvocation::new(
        &command.command,
        HookCall {
            site_root: config.get_root(),
            name: &command.name,
            mode,
            directories: HookDirectories::GenerateOutputs {
                input: candidate.root(),
                output: generated.path(),
            },
        },
    )?;
    let identity = super::hook_identity(HookStage::GenerateOutputs, &command.name);
    let completion = invocation.run(Some(cancellation));

    cancellation.ensure_active().map_err(anyhow::Error::new)?;
    completion
        .map(|_output| ())
        .map_err(|error| hook_command_error(HookStage::GenerateOutputs, &identity, error))?;
    let files = read_generated_files(generated.path(), cancellation)
        .with_context(|| format!("Tola could not read the files written by {identity}"))?;
    let declarations = DeclaredOutputs::new(&command.outputs, cancellation)?;
    for path in files.keys() {
        cancellation.ensure_active()?;
        if !declarations.contains(path) {
            return Err(super::hook_contract_error(
                format!("{identity} produced `{path}`, which `outputs` does not declare"),
                "declare it in `outputs`, or stop writing it",
            ));
        }
    }
    for declaration in &command.outputs {
        cancellation.ensure_active()?;
        match declaration {
            CommandOutput::File(path) if !files.contains_key(path) => {
                return Err(super::hook_contract_error(
                    format!(
                        "`outputs` of {identity} declares `{path}`, but the command wrote nothing"
                    ),
                    "write it, or remove it from `outputs`",
                ));
            }
            CommandOutput::Tree(root) => {
                if !fs::symlink_metadata(generated.path().join(root.as_str()))
                    .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
                {
                    return Err(super::hook_contract_error(
                        format!(
                            "`outputs` of {identity} declares the directory `{root}`, but the command created none"
                        ),
                        "create it, or declare the files instead",
                    ));
                }
            }
            CommandOutput::File(_) => {}
        }
    }
    let owner = OutputOwner::command(index, command.name.as_str());
    for declaration in &command.outputs {
        if let CommandOutput::Tree(root) = declaration {
            generated_outputs
                .root_ownerships
                .push(OutputRootOwnership::new(root.clone(), owner.clone()));
        }
    }
    for (path, bytes) in files {
        cancellation.ensure_active()?;
        let media = OutputDeclaration::from_filesystem_source(Path::new(path.as_str()))
            .media_type()
            .clone();
        generated_outputs.outputs.push(OutputFile::new(
            path,
            OutputDeclaration::opaque(media),
            owner.clone(),
            bytes,
        ));
    }
    Ok(())
}

/// Exact spelling remains significant here; portable collisions are rejected by
/// the output graph. Tree prefixes are sorted and disjoint, so only the nearest
/// predecessor can contain a file, even when declarations themselves overlap.
struct DeclaredOutputs<'a> {
    files: BTreeSet<&'a OutputPath>,
    trees: Vec<String>,
}

impl<'a> DeclaredOutputs<'a> {
    fn new(declarations: &'a [CommandOutput], cancellation: &BuildCancellation) -> Result<Self> {
        let mut files = BTreeSet::new();
        let mut sorted_trees = BTreeSet::new();
        for declaration in declarations {
            cancellation.ensure_active()?;
            match declaration {
                CommandOutput::File(path) => {
                    files.insert(path);
                }
                CommandOutput::Tree(root) => {
                    sorted_trees.insert(format!("{}/", root.as_str()));
                }
            }
        }
        let mut trees: Vec<String> = Vec::new();
        for root in sorted_trees {
            cancellation.ensure_active()?;
            if !trees.last().is_some_and(|parent| root.starts_with(parent)) {
                trees.push(root);
            }
        }
        Ok(Self { files, trees })
    }

    fn contains(&self, path: &OutputPath) -> bool {
        if self.files.contains(path) {
            return true;
        }
        let end = self
            .trees
            .partition_point(|root| root.as_str() <= path.as_str());
        end.checked_sub(1)
            .is_some_and(|index| path.as_str().starts_with(&self.trees[index]))
    }
}

fn read_generated_files(
    root: &Path,
    cancellation: &BuildCancellation,
) -> Result<BTreeMap<OutputPath, Vec<u8>>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        cancellation.ensure_active().map_err(anyhow::Error::new)?;
        let mut children = fs::read_dir(&directory)
            .with_context(|| "Tola could not read what the command generated")?
            .collect::<std::io::Result<Vec<_>>>()
            .with_context(|| "Tola could not read what the command generated")?;
        children.sort_unstable_by_key(|child| child.file_name());
        for child in children {
            cancellation.ensure_active().map_err(anyhow::Error::new)?;
            let absolute = child.path();
            let relative = absolute
                .strip_prefix(root)
                .expect("generated child remains below its root");
            let generated_display = crate::filesystem::display_path(&absolute, root);
            let metadata = fs::symlink_metadata(&absolute).with_context(|| {
                format!("Tola could not read the generated file `{generated_display}`")
            })?;
            if metadata.file_type().is_symlink() {
                return Err(super::hook_contract_error(
                    format!("the command generated `{generated_display}` as a symbolic link"),
                    "generate a regular file or directory",
                ));
            }
            if metadata.is_dir() {
                pending.push(absolute);
            } else if metadata.is_file() {
                let path = relative
                    .components()
                    .map(|component| {
                        component.as_os_str().to_str().ok_or_else(|| {
                            super::hook_contract_error(
                                format!(
                                    "the command generated `{generated_display}`, whose name is not valid UTF-8"
                                ),
                                "rename it to valid UTF-8",
                            )
                        })
                    })
                    .collect::<Result<Vec<_>>>()?
                    .join("/");
                let path = OutputPath::parse(&path)
                    .with_context(|| format!("`{path}` cannot be used as a published path"))?;
                let mut bytes = Vec::new();
                crate::filesystem::read_file_chunks(&absolute, cancellation, |chunk| {
                    bytes.extend_from_slice(chunk);
                })
                .map_err(crate::filesystem::FileReadError::into_anyhow)
                .with_context(|| format!("Tola could not read the generated file `{path}`"))?;
                files.insert(path, bytes);
            } else {
                return Err(super::hook_contract_error(
                    format!(
                        "the command generated `{generated_display}`, which is not a regular file or directory"
                    ),
                    "generate a regular file or directory",
                ));
            }
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::tests::{hook_child_command, is_hook_child_for};
    use crate::config::section::build::hooks::{DevParticipation, OutputCommandConfig};
    use crate::output::graph::{OutputGraphBuilder, OutputKind};
    use crate::output::semantics::{DeclaredOutputSemantics, ResponseMediaType};
    use std::io::{Read, Write};
    use std::sync::mpsc;

    fn generated_root() -> std::path::PathBuf {
        std::env::var_os("TOLA_HOOK_OUTPUT_DIR").unwrap().into()
    }

    fn output_command_config(
        child: &str,
        outputs: &[&str],
    ) -> (tempfile::TempDir, ResolvedSiteConfig, OutputGraphBuilder) {
        let directory = tempfile::TempDir::new().unwrap();
        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.build.hooks.generate_outputs = vec![OutputCommandConfig {
            name: "search".into(),
            command: hook_child_command(&format!("hooks::generate::tests::{child}")),
            outputs: outputs
                .iter()
                .map(|path| CommandOutput::File(OutputPath::parse(path).unwrap()))
                .collect(),
            ..OutputCommandConfig::default()
        }];
        let mut upstream = OutputGraphBuilder::new();
        upstream
            .insert(OutputFile::new(
                OutputPath::parse("index.html").unwrap(),
                OutputDeclaration::html_document(),
                OutputOwner::bundle("site.typ"),
                b"<h1>Home</h1>".to_vec(),
            ))
            .unwrap();
        (directory, config, upstream)
    }

    #[test]
    fn child_writes_search_index() {
        if !is_hook_child_for("generate-outputs") {
            return;
        }
        crate::build::tests::assert_hook_environment("generate-outputs");
        let candidate = std::path::PathBuf::from(std::env::var_os("TOLA_HOOK_INPUT_DIR").unwrap());
        let html = fs::read_to_string(candidate.join("index.html")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(candidate.join("index.html"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o222,
                0
            );
        }
        fs::write(
            generated_root().join("search.json"),
            serde_json::to_vec(&serde_json::json!({"html": html})).unwrap(),
        )
        .unwrap();
    }

    /// Records how many times the command actually ran, beside the site root.
    #[test]
    fn child_counts_runs() {
        if !is_hook_child_for("generate-outputs") {
            return;
        }
        let path = std::path::Path::new("runs.txt");
        let count = fs::read_to_string(path)
            .ok()
            .and_then(|runs| runs.trim().parse::<u32>().ok())
            .unwrap_or(0);
        fs::write(path, (count + 1).to_string()).unwrap();
        let candidate = std::path::PathBuf::from(std::env::var_os("TOLA_HOOK_INPUT_DIR").unwrap());
        let html = fs::read_to_string(candidate.join("index.html")).unwrap();
        fs::write(
            generated_root().join("search.json"),
            serde_json::to_vec(&serde_json::json!({"html": html})).unwrap(),
        )
        .unwrap();
    }

    /// A command that keeps its index in `TOLA_HOOK_CACHE_DIR` and restores it into every
    /// fresh generated root; a cache hit still has to write the declared outputs.
    #[test]
    fn child_restores_cached_search_index() {
        if !is_hook_child_for("generate-outputs") {
            return;
        }
        crate::build::tests::record_hook_environment();
        let cache = std::path::PathBuf::from(std::env::var_os("TOLA_HOOK_CACHE_DIR").unwrap());
        let cached = cache.join("search/index.json");
        let target = generated_root().join("search/index.json");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        match fs::read(&cached) {
            Ok(bytes) => {
                assert!(!target.exists(), "Tola reused a generated root");
                fs::write(target, bytes).unwrap();
            }
            Err(_) => {
                let candidate =
                    std::path::PathBuf::from(std::env::var_os("TOLA_HOOK_INPUT_DIR").unwrap());
                let html = fs::read_to_string(candidate.join("index.html")).unwrap();
                let bytes = serde_json::to_vec(&serde_json::json!({"html": html})).unwrap();
                fs::create_dir_all(cached.parent().unwrap()).unwrap();
                fs::write(&cached, &bytes).unwrap();
                fs::write(target, bytes).unwrap();
            }
        }
    }

    #[test]
    fn child_writes_extra_file() {
        if !is_hook_child_for("generate-outputs") {
            return;
        }
        fs::write(generated_root().join("search.json"), "[]").unwrap();
        fs::write(generated_root().join("extra.json"), "[]").unwrap();
    }

    #[test]
    fn child_writes_nothing() {}

    #[cfg(unix)]
    #[test]
    fn child_writes_symlink() {
        if !is_hook_child_for("generate-outputs") {
            return;
        }
        let candidate = std::path::PathBuf::from(std::env::var_os("TOLA_HOOK_INPUT_DIR").unwrap());
        std::os::unix::fs::symlink(
            candidate.join("index.html"),
            generated_root().join("search.json"),
        )
        .unwrap();
    }

    #[test]
    fn child_waits_for_cancellation() {
        if !is_hook_child_for("generate-outputs") {
            return;
        }
        let address = fs::read_to_string("signal.txt").unwrap();
        let mut connection = std::net::TcpStream::connect(address).unwrap();
        connection.write_all(b"R").unwrap();
        let mut byte = [0];
        let _ = connection.read_exact(&mut byte);
    }

    #[test]
    fn generated_outputs_join_the_graph() {
        let (_directory, mut config, mut upstream) =
            output_command_config("child_writes_search_index", &["search.json"]);
        let mut disabled = config.build.hooks.generate_outputs[0].clone();
        disabled.enable = false;
        config.build.hooks.generate_outputs.insert(0, disabled);
        let outputs = generate_outputs(
            &config,
            upstream.outputs(),
            BuildMode::Production,
            None,
            &BuildCancellation::new(),
        )
        .unwrap();
        assert_eq!(outputs.outputs.len(), 1);
        let generated = &outputs.outputs[0];
        assert_eq!(generated.path().as_str(), "search.json");
        assert_eq!(generated.kind(), OutputKind::Asset);
        assert_eq!(
            generated.declaration().semantics(),
            DeclaredOutputSemantics::Opaque
        );
        assert_eq!(
            generated.declaration().media_type(),
            &ResponseMediaType::JSON
        );
        assert!(
            matches!(generated.owner(), OutputOwner::Command { index: 1, name } if name.as_ref() == "search")
        );
        let content: serde_json::Value = serde_json::from_slice(generated.bytes()).unwrap();
        assert_eq!(content["html"], "<h1>Home</h1>");
        assert!(!config.build.publish_dir.exists());
        upstream
            .insert(outputs.outputs.into_iter().next().unwrap())
            .unwrap();
        assert_eq!(upstream.finish().outputs().len(), 2);
    }

    #[test]
    fn output_declarations_are_enforced() {
        for (child, declared) in [
            ("child_writes_nothing", vec!["search.json"]),
            ("child_writes_extra_file", vec!["search.json"]),
            ("child_writes_nothing", vec![]),
        ] {
            let (_directory, config, upstream) = output_command_config(child, &declared);
            let error = generate_outputs(
                &config,
                upstream.outputs(),
                BuildMode::Production,
                None,
                &BuildCancellation::new(),
            )
            .unwrap_err();
            assert!(crate::diagnostic::attached(&error).is_some());
            assert_eq!(upstream.outputs()[0].bytes(), b"<h1>Home</h1>");
        }
    }

    #[cfg(unix)]
    #[test]
    fn generated_symlinks_are_rejected() {
        let (_directory, config, upstream) =
            output_command_config("child_writes_symlink", &["search.json"]);
        generate_outputs(
            &config,
            upstream.outputs(),
            BuildMode::Production,
            None,
            &BuildCancellation::new(),
        )
        .unwrap_err();
        assert_eq!(upstream.outputs()[0].bytes(), b"<h1>Home</h1>");
    }

    #[test]
    fn cancellation_stops_output_command() {
        let (directory, config, upstream) =
            output_command_config("child_waits_for_cancellation", &["search.json"]);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        fs::write(directory.path().join("signal.txt"), address.to_string()).unwrap();
        let (ready_sender, ready_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let observer = std::thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            connection
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut ready = [0];
            let received = connection.read_exact(&mut ready).map(|()| ready);
            let _ = ready_sender.send(received);
            let _ = release_receiver.recv();
        });
        let canceller = crate::cancellation::BuildCanceller::new();
        let worker_cancellation = canceller.token();
        let worker = std::thread::spawn(move || {
            generate_outputs(
                &config,
                upstream.outputs(),
                BuildMode::Production,
                None,
                &worker_cancellation,
            )
        });
        let ready = ready_receiver.recv_timeout(std::time::Duration::from_secs(5));
        canceller.cancel();
        if ready.is_err() {
            let _ = std::net::TcpStream::connect(address)
                .and_then(|mut connection| connection.write_all(b"W"));
        }
        let completion = worker.join().unwrap();
        let _ = release_sender.send(());
        observer.join().unwrap();
        assert_eq!(ready.unwrap().unwrap(), [b'R']);
        let error = completion.unwrap_err();
        assert!(
            matches!(
                error.downcast_ref::<crate::cancellation::BuildCancelled>(),
                Some(crate::cancellation::BuildCancelled)
            ),
            "{error:#}"
        );
    }

    #[test]
    fn output_command_receives_build_mode() {
        let (_directory, mut config, upstream) =
            output_command_config("child_restores_cached_search_index", &[]);
        config.build.hooks.generate_outputs[0].outputs =
            vec![CommandOutput::Tree(OutputPath::parse("search").unwrap())];

        for (mode, expected) in [
            (BuildMode::Development, "dev"),
            (BuildMode::Production, "prod"),
        ] {
            generate_outputs(
                &config,
                upstream.outputs(),
                mode,
                None,
                &BuildCancellation::new(),
            )
            .unwrap();
            assert_eq!(
                crate::build::tests::recorded_build_mode(config.get_root()),
                expected
            );
        }
    }

    fn run_count(directory: &tempfile::TempDir) -> u32 {
        std::fs::read_to_string(directory.path().join("runs.txt"))
            .ok()
            .and_then(|runs| runs.trim().parse().ok())
            .unwrap_or(0)
    }

    /// Whether the previous revision's outputs are reused decides whether the command runs:
    /// a reuse-policy command in development skips an unchanged input, and every other
    /// (mode, policy, changed-input) combination runs it again.
    #[test]
    fn reuse_policy_decides_whether_command_runs() {
        struct Case {
            mode: BuildMode,
            dev: DevParticipation,
            changes_upstream: bool,
            expected_runs: u32,
        }
        for case in [
            Case {
                mode: BuildMode::Development,
                dev: DevParticipation::Reuse,
                changes_upstream: false,
                expected_runs: 1,
            },
            Case {
                mode: BuildMode::Development,
                dev: DevParticipation::Reuse,
                changes_upstream: true,
                expected_runs: 2,
            },
            Case {
                mode: BuildMode::Development,
                dev: DevParticipation::Run,
                changes_upstream: false,
                expected_runs: 2,
            },
            Case {
                mode: BuildMode::Production,
                dev: DevParticipation::Reuse,
                changes_upstream: false,
                expected_runs: 2,
            },
        ] {
            let (directory, mut config, upstream) =
                output_command_config("child_counts_runs", &["search.json"]);
            config.build.hooks.generate_outputs[0].dev = case.dev;
            let cancellation = BuildCancellation::new();
            let first =
                generate_outputs(&config, upstream.outputs(), case.mode, None, &cancellation)
                    .unwrap();
            assert_eq!(run_count(&directory), 1);

            let next = if case.changes_upstream {
                let mut changed = OutputGraphBuilder::new();
                changed
                    .insert(OutputFile::new(
                        OutputPath::parse("index.html").unwrap(),
                        OutputDeclaration::html_document(),
                        OutputOwner::bundle("site.typ"),
                        b"<h1>Changed</h1>".to_vec(),
                    ))
                    .unwrap();
                changed
            } else {
                let mut same = OutputGraphBuilder::new();
                same.insert(OutputFile::new(
                    OutputPath::parse("index.html").unwrap(),
                    OutputDeclaration::html_document(),
                    OutputOwner::bundle("site.typ"),
                    b"<h1>Home</h1>".to_vec(),
                ))
                .unwrap();
                same
            };
            let second = generate_outputs(
                &config,
                next.outputs(),
                case.mode,
                Some(&first.reuse),
                &cancellation,
            )
            .unwrap();

            let label = format!(
                "{:?}/{:?}/changed={}",
                case.mode, case.dev, case.changes_upstream
            );
            assert_eq!(run_count(&directory), case.expected_runs, "{label}");
            if case.expected_runs == 1 {
                assert_eq!(
                    first.outputs[0].bytes(),
                    second.outputs[0].bytes(),
                    "{label}"
                );
            }
        }
    }
}
