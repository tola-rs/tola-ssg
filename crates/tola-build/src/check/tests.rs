use super::*;
use std::fs;
use tola_diagnostic::Severity;

fn site() -> (tempfile::TempDir, Arc<SiteConfig>) {
    let dir = tempfile::TempDir::new().unwrap();
    fs::create_dir(dir.path().join("content")).unwrap();
    fs::write(
        dir.path().join("site.typ"),
        r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": join-route, output-of
#for source in all-sources() {
  document(output-of(join-route(source.route-segments)))[#include source.file]
}"#,
    )
    .unwrap();
    let cfg = Arc::new(crate::config::tests::load_test_config(dir.path(), ""));
    (dir, cfg)
}

fn documents_workspace() -> (tempfile::TempDir, Arc<SiteConfig>) {
    let dir = tempfile::TempDir::new().unwrap();
    fs::write(
        dir.path().join("page.typ"),
        "#let extra = read(\"/data.txt\")\nBody: #extra\n",
    )
    .unwrap();
    fs::write(dir.path().join("data.txt"), "payload\n").unwrap();
    let src =
        crate::config::ConfigSource::defaults(dir.path(), tola_input::InputScope::Online).unwrap();
    let cfg = Arc::new(
        crate::config::loading::resolve_parsed_site_config(
            src.decode::<crate::config::SiteConfigSchema>().unwrap(),
            tola_typst::PackageLocations::default(),
            &crate::config::loading::BuildOverrides::default(),
        )
        .unwrap()
        .into_config(),
    );
    (dir, cfg)
}

#[cfg(unix)]
#[test]
fn pure_check_rejects_outside_overlay() {
    let (_dir, cfg) = site();
    let outside = tempfile::TempDir::new().unwrap();
    let alias = cfg.root().join("linked");
    std::os::unix::fs::symlink(outside.path(), &alias).unwrap();
    let mut session = SourceDiagnosticSession::with_resources(
        cfg,
        crate::BuildResources::new().with_input_scope(tola_input::InputScope::Pure),
    );
    let diag = session
        .check(
            vec![(alias.join("new.typ"), Arc::from("unsaved host text"))],
            &BuildCancellation::new(),
        )
        .unwrap();
    assert!(diag.iter().any(|d| d.severity == Severity::Error));
    assert!(!outside.path().join("new.typ").exists());
}

#[test]
fn unsaved_overlay_preserves_disk() {
    for (name, disk, src, overlay, needle) in [
        (
            "metadata and body share the editor revision",
            Some("#missing_on_disk"),
            "document.typ",
            r#"#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: [Draft], pin: true))
#import "@tola/document:0.0.0": current-document
#context [#current-document().route]
= Unsaved body"#,
            None,
        ),
        (
            "new content is discovered and reports its own error",
            None,
            "new.typ",
            "#unsaved_error",
            Some("unsaved_error"),
        ),
    ] {
        let (_dir, cfg) = site();
        let mut session = SourceDiagnosticSession::new(Arc::clone(&cfg));
        let path = cfg.build.content_dir.join(src);
        if let Some(body) = disk {
            fs::write(&path, body).unwrap();
        }
        let diag = session
            .check(
                vec![(path.clone(), Arc::from(overlay))],
                &BuildCancellation::new(),
            )
            .unwrap();

        match needle {
            Some(needle) => assert!(
                diag.iter().any(|d| {
                    d.message.contains(needle)
                        && d.location
                            .as_ref()
                            .is_some_and(|loc| loc.path == "content/new.typ")
                }),
                "{name}: {diag:#?}"
            ),
            None => assert!(
                diag.iter().all(|d| d.severity != Severity::Error),
                "{name}: {diag:#?}"
            ),
        }
        assert_eq!(
            fs::read_to_string(&path).ok().as_deref(),
            disk,
            "{name} must leave the disk file unchanged"
        );
        assert!(
            !cfg.build.publish_dir.exists(),
            "{name} must not write output"
        );
    }
}

#[test]
fn compiled_document_records_reads() {
    let (dir, cfg) = documents_workspace();
    let cancel = BuildCancellation::new();
    let mut session = SourceDiagnosticSession::for_documents(cfg, crate::BuildResources::default());
    let rev = session
        .inspect_source("page.typ", Vec::new(), &cancel)
        .unwrap();
    assert!(rev.compiled(), "{:#?}", rev.diagnostics());
    let data = fs::canonicalize(dir.path().join("data.txt")).unwrap();
    let reads: Vec<_> = rev
        .checked()
        .expect("the document resolved a world")
        .file_reads()
        .iter()
        .filter_map(|r| match r.origin() {
            tola_typst::ReadOrigin::Disk(p) => Some(p.as_path().to_path_buf()),
            _ => None,
        })
        .collect();
    assert!(reads.contains(&data), "{reads:?}");
}

#[test]
fn site_session_requires_content_dir() {
    let (_dir, cfg) = documents_workspace();
    let mut session = SourceDiagnosticSession::new(cfg);
    let cancel = BuildCancellation::new();
    for rev in [
        session.inspect(Vec::new(), &cancel).unwrap(),
        session
            .inspect_source("page.typ", Vec::new(), &cancel)
            .unwrap(),
    ] {
        assert!(!rev.compiled());
        assert!(rev.checked().is_none());
        assert!(
            rev.diagnostics()
                .iter()
                .any(|d| d.severity == Severity::Error)
        );
    }
}

#[test]
fn report_keeps_unsaved_evidence() {
    let (_dir, cfg) = site();
    let src = cfg.build.content_dir.join("reports.typ");
    fs::write(&src, "Saved body").unwrap();
    let text = r#"#import "@tola/diagnostic:0.0.0": report
#let value = emph[unsaved 🧭]
#report("unknown variable", severity: "error", code: "check.unused_import/中文", at: value,
  hints: ("A correction",), related: ((message: "Source context", at: value), (message: "No location", at: none)))
#report("later info", severity: "info", code: "info")
#report("later hint", severity: "hint", code: "hint")
"#;
    let mut session = SourceDiagnosticSession::new(Arc::clone(&cfg));
    let cancel = BuildCancellation::new();
    for _ in 0..2 {
        let rev = session
            .inspect(vec![(src.clone(), Arc::from(text))], &cancel)
            .unwrap();
        assert!(!rev.compiled(), "an accumulated error prevents success");
        let reports: Vec<_> = rev
            .diagnostics()
            .iter()
            .filter(|d| d.producer.is_some())
            .collect();
        assert_eq!(reports.len(), 3, "all reports survive: {reports:#?}");
        let issue = reports
            .iter()
            .find(|d| d.severity == Severity::Error)
            .unwrap();
        assert_eq!(
            issue.producer.as_ref().unwrap().name.as_deref(),
            Some("project")
        );
        assert_eq!(
            issue.producer.as_ref().unwrap().code.as_deref(),
            Some("check.unused_import/中文")
        );
        assert!(issue.cause.is_none());
        assert_eq!(issue.help.len(), 1);
        assert_eq!(issue.related.len(), 2);
        assert!(issue.related[1].location.is_none());
        let at = issue.location.as_ref().unwrap();
        assert_eq!(at.path, "content/reports.typ");
        assert!(at.range.is_some());
        assert!(
            at.source_lines
                .iter()
                .any(|line| line.text.contains("unsaved 🧭"))
        );
        assert!(reports.iter().any(|d| d.severity == Severity::Info));
        assert!(reports.iter().any(|d| d.severity == Severity::Hint));
        assert_eq!(fs::read_to_string(&src).unwrap(), "Saved body");
    }
    assert!(!cfg.build.publish_dir.exists());
}

#[test]
fn realization_error_blocks_publish() {
    let (_dir, cfg) = site();
    fs::write(
        &cfg.build.entry,
        r#"#import "@tola/diagnostic:0.0.0": report
#document("index.html")[#context {
  report("stop publication", severity: "error", code: "publish", at: none)
  report("still collected", severity: "hint", code: "context")
}]
"#,
    )
    .unwrap();
    let mut session = crate::BuildSession::new();
    let failed = session
        .build_and_write(&cfg, crate::build::BuildRequest::default())
        .expect_err("a report error must fail the candidate");
    let diag = tola_diagnostic::attached(&failed).expect("structured reports remain attached");
    assert!(diag.iter().any(|d| {
        d.message == "stop publication" && d.severity == Severity::Error && d.location.is_none()
    }));
    assert!(
        diag.iter()
            .any(|d| d.message == "still collected" && d.severity == Severity::Hint)
    );
    assert!(!cfg.build.publish_dir.exists());
}
