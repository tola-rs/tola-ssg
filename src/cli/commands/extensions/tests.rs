use super::argv::*;
use super::*;
use crate::cli::external::{CommandExit, run as run_process};
use std::path::Path;
use tola_build::modules::ModuleCommands;
use tola_diagnostic::Severity;

fn report_channel(severity: &str) -> ReportChannel {
    let reports = ReportChannel::new("command:catalog.scan", ReportSources::new()).unwrap();
    std::fs::write(
        reports.path(),
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "diagnostics": [{"severity": severity, "message": "catalog issue", "code": "rule"}],
        }))
        .unwrap(),
    )
    .unwrap();
    reports
}

#[test]
fn report_severity_controls_success() {
    let output = super::super::testing::buffered_output();
    complete_report(Ok(()), report_channel("warning"), &output).unwrap();
    let error = complete_report(Ok(()), report_channel("error"), &output).unwrap_err();
    let diagnostics = tola_diagnostic::attached(&error).unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert_eq!(
        diagnostics[0].producer.as_ref().unwrap().name.as_deref(),
        Some("command:catalog.scan"),
    );
}

#[cfg(unix)]
#[test]
fn failed_process_status_survives_reports() {
    use std::os::unix::process::ExitStatusExt;

    let output = super::super::testing::buffered_output();
    for invalid in [false, true] {
        let reports = report_channel("warning");
        if invalid {
            std::fs::write(reports.path(), b"partial report").unwrap();
        }
        let completion = Err(CommandExit(std::process::ExitStatus::from_raw(37 << 8)).into());
        let error = complete_report(completion, reports, &output).unwrap_err();
        assert_eq!(
            error.downcast_ref::<CommandExit>().unwrap().0.code(),
            Some(37)
        );
        let diagnostics = tola_diagnostic::attached(&error).unwrap();
        assert_eq!(diagnostics.len(), 1);
        if invalid {
            assert_eq!(diagnostics[0].severity, Severity::Error);
            assert!(
                diagnostics[0]
                    .message
                    .contains("invalid module command diagnostic report")
            );
        } else {
            assert_eq!(diagnostics[0].severity, Severity::Warning);
        }
    }
}

fn module() -> ModuleCommands {
    ModuleCommands {
        name: "forest".into(),
        root: PathBuf::from("/module with spaces"),
        manifest: PathBuf::from("/module with spaces/tola-module.toml"),
        summary: "Forest author tools".into(),
        commands: BTreeMap::from([(
            "edit".into(),
            tola_build::modules::Command {
                help: "Edit a tree\nLonger instructions.".into(),
                action: CommandAction::Argv {
                    argv: vec!["missing-tool".into()],
                },
            },
        )]),
        initializer: None,
    }
}

#[test]
fn static_help_needs_no_executable() {
    let module = module();
    let help = namespace_help(&module, &BTreeMap::new(), false, true);
    assert!(help.contains("forest"));
    assert!(help.contains("Edit a tree"));
    assert!(
        command_help(&module, "edit", &module.commands["edit"], false, true)
            .contains("Longer instructions.")
    );
    let query = tola_build::modules::Command {
        help: "Find trees".into(),
        action: CommandAction::Query {
            query: "nodes".into(),
            projection: vec![],
        },
    };
    let query_help = command_help(&module, "find", &query, false, true);
    assert!(query_help.contains("--output"));
}

#[test]
fn namespace_config_stops_declared_command() {
    let invocation = Invocation::parse(
        [
            "forest",
            "--config",
            "a site/tola.toml",
            "edit",
            "--config",
            "child.toml",
        ]
        .map(OsString::from)
        .to_vec(),
    )
    .unwrap();
    assert_eq!(
        invocation.config.path.as_deref(),
        Some(Path::new("a site/tola.toml"))
    );
    assert_eq!(
        invocation.arguments,
        ["edit", "--config", "child.toml"].map(OsString::from)
    );
    let equals = Invocation::parse(
        ["forest", "--config=a site/tola.toml", "--help"]
            .map(OsString::from)
            .to_vec(),
    )
    .unwrap();
    assert_eq!(equals.config.path, invocation.config.path);
}

#[test]
fn declared_argv_paths_arguments_separator() {
    let module = module();
    let project = Path::new("/project with spaces");
    let command = process_command(
        &[
            "deno".into(),
            "{module}/tools/cli.ts".into(),
            "--project={project}".into(),
        ],
        &["a title".into(), "--".into(), "$(literal)".into()],
        project,
        &project.join("tola.toml"),
        &module,
        InputScope::Online,
    )
    .unwrap();
    assert_eq!(command.program(), "deno");
    assert_eq!(
        expand(
            "{module}/tools/cli.ts",
            project,
            &module.root,
            Path::new("/tola")
        ),
        "/module with spaces/tools/cli.ts"
    );
    assert_eq!(
        expand(
            "--project={project}",
            project,
            &module.root,
            Path::new("/tola")
        ),
        "--project=/project with spaces"
    );
}

#[test]
fn builtins_cannot_be_module_namespaces() {
    let mut module = module();
    module.name = "build".into();
    assert!(check_namespaces(&BTreeMap::from([("build".into(), module)])).is_err());
}

#[cfg(unix)]
#[test]
fn external_argv_context_exit_status() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path();
    let mut module = module();
    module.root = project.join("module with spaces");
    module.name = "notes".into();
    std::fs::create_dir(&module.root).unwrap();
    let record = project.join("arguments.txt");
    let script = module.root.join("record.sh");
    std::fs::write(
            &script,
            r#"record="$1"
shift
printf '%s\n' "$@" "$TOLA_PROJECT_ROOT" "$TOLA_MODULE_ROOT" "$TOLA_CONFIG_PATH" "$TOLA_EXECUTABLE" "$TOLA_INPUT_SCOPE" "$TOLA_MODULE_NAME" > "$record"
exit 37
"#,
        )
        .unwrap();
    let argv = vec![
        "sh".into(),
        "{module}/record.sh".into(),
        "{project}/arguments.txt".into(),
    ];
    for (scope, expected_scope) in [
        (InputScope::Online, "online"),
        (InputScope::Offline, "offline"),
        (InputScope::Pure, "pure"),
    ] {
        let command = process_command(
            &argv,
            &[
                "a title with spaces".into(),
                "--".into(),
                "$(literal)".into(),
            ],
            project,
            &project.join("custom config.toml"),
            &module,
            scope,
        )
        .unwrap();
        let error = run_process(command, &Cancellation::default()).unwrap_err();
        assert_eq!(
            error.downcast_ref::<CommandExit>().unwrap().0.code(),
            Some(37)
        );
        let recorded = std::fs::read_to_string(&record).unwrap();
        let lines = recorded.lines().collect::<Vec<_>>();
        assert_eq!(&lines[..3], ["a title with spaces", "--", "$(literal)"]);
        assert_eq!(Path::new(lines[3]), project);
        assert_eq!(Path::new(lines[4]), module.root);
        assert_eq!(Path::new(lines[5]), project.join("custom config.toml"));
        assert_eq!(Path::new(lines[6]), std::env::current_exe().unwrap());
        assert_eq!(lines[7], expected_scope);
        assert_eq!(lines[8], "notes");
    }
}

#[test]
fn unavailable_declared_executable_is_an_error() {
    let directory = tempfile::tempdir().unwrap();
    let program = directory.path().join("tool-that-is-not-installed");
    let command = process_command(
        &[program.to_string_lossy().into_owned()],
        &[],
        directory.path(),
        &directory.path().join("tola.toml"),
        &module(),
        InputScope::Online,
    )
    .unwrap();
    let error = run_process(command, &Cancellation::default()).unwrap_err();
    assert!(error.to_string().contains("Tola could not run"), "{error}");
    assert!(error.downcast_ref::<CommandExit>().is_none());
}
