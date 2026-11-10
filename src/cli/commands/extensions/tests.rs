use super::*;
use crate::cli::external::{CommandExit, run as run_process};
use std::path::{Path, PathBuf};
use tola_diagnostic::Severity;
use tola_input::InputScope;
use tola_module::ModuleCommands;

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
        commands: BTreeMap::new(),
        initializer: None,
        options: Default::default(),
    }
}

#[test]
fn builtins_cannot_be_module_namespaces() {
    let mut module = module();
    module.name = "build".into();
    assert!(check_namespaces(&BTreeMap::from([("build".into(), module)])).is_err());
}

#[cfg(unix)]
#[test]
fn declared_argv_reaches_the_process() {
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
printf '%s\n' "$@" "$TOLA_PROJECT_ROOT" "$TOLA_MODULE_NAME" "$TOLA_INPUT_SCOPE" > "$record"
exit 37
"#,
    )
    .unwrap();
    let argv = vec![
        "sh".into(),
        "{module}/record.sh".into(),
        "{project}/arguments.txt".into(),
    ];
    let (command, _options) = process_command(
        &argv,
        &["a title".into(), "--".into(), "$(literal)".into()],
        project,
        &project.join("tola.toml"),
        &module,
        InputScope::Offline,
    )
    .unwrap();
    let error = run_process(command, &Cancellation::default()).unwrap_err();
    assert_eq!(
        error.downcast_ref::<CommandExit>().unwrap().0.code(),
        Some(37)
    );
    let recorded = std::fs::read_to_string(&record).unwrap();
    let lines = recorded.lines().collect::<Vec<_>>();
    assert_eq!(&lines[..3], ["a title", "--", "$(literal)"]);
    assert_eq!(Path::new(lines[3]), project);
    assert_eq!(lines[4], "notes");
    assert_eq!(lines[5], "offline");
}
