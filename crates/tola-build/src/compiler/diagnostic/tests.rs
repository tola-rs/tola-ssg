use std::path::{Path, PathBuf};

use super::location::{relativize_message, source_line};
use super::*;

fn location(path: Option<&str>) -> tola_typst::SourceLocation {
    tola_typst::SourceLocation {
        path: path.map(str::to_owned),
        line: None,
        column: None,
        range: None,
        source_lines: Vec::new(),
        location_failure: None,
        truncation: tola_typst::SourceTruncation::default(),
    }
}

fn warning_at(msg: &str, loc: tola_typst::SourceLocation) -> tola_typst::Diagnostic {
    tola_typst::Diagnostic {
        origin: tola_typst::DiagnosticOrigin::Typst,
        severity: tola_typst::DiagnosticSeverity::Warning,
        message: msg.into(),
        location: loc,
        hints: Vec::new(),
        traces: Vec::new(),
        imported_by: Vec::new(),
        package_failure: None,
    }
}

#[test]
fn failures_stable_codes() {
    let site = Path::new("/site");
    let outside = anyhow::Error::new(tola_typst::CompileError::World(
        tola_typst::WorldBuildError::MainOutsideRoot {
            main: "/other/site.typ".into(),
            root: "/site".into(),
        },
    ));
    let src_load = anyhow::Error::new(tola_typst::CompileError::SourceLoad(
        tola_typst::world::SourceLoadError::File {
            path: "/site/content/post.typ".into(),
            error: typst::diag::FileError::AccessDenied,
        },
    ));
    let font = anyhow::Error::new(tola_typst::CompileError::Font(
        tola_typst::FontLoadError::File {
            path: "/site/static/typst-fonts/Inter.ttf".into(),
            error: typst::diag::FileError::NotFound("/site/static/typst-fonts/Inter.ttf".into()),
        },
    ));
    let cancelled = anyhow::Error::new(tola_typst::CompileError::Cancelled);
    let export = anyhow::Error::new(tola_typst::CompileError::bundle_export([
        typst::diag::SourceDiagnostic::error(typst::syntax::Span::detached(), "broken").into(),
    ]));

    for (label, err, code) in [
        ("world", &outside, tola_diagnostic::codes::typst::WORLD),
        (
            "source load",
            &src_load,
            tola_diagnostic::codes::typst::SNAPSHOT,
        ),
        ("font", &font, tola_diagnostic::codes::typst::FONT),
        (
            "export",
            &export,
            tola_diagnostic::codes::typst::BUNDLE_EXPORT,
        ),
    ] {
        let diags = error_diagnostics(err, site)
            .unwrap_or_else(|| panic!("{label} produced no diagnostic"));
        assert_eq!(diags[0].code, code, "{label}");
        for diag in &diags {
            assert!(
                !diag.message.contains("/other/") && !diag.message.contains("/site/"),
                "{label} leaked a host path: {}",
                diag.message
            );
        }
    }

    assert!(error_diagnostics(&cancelled, site).is_none());
}

#[test]
fn failure_locations_site_relative() {
    let src_load = |path: &str| {
        anyhow::Error::new(tola_typst::CompileError::SourceLoad(
            tola_typst::world::SourceLoadError::File {
                path: path.into(),
                error: typst::diag::FileError::AccessDenied,
            },
        ))
    };

    let inside = error_diagnostics(&src_load("/site/content/post.typ"), Path::new("/site"))
        .expect("an inside path is described");
    assert_eq!(
        inside[0].location.as_ref().map(|loc| loc.path.as_str()),
        Some("content/post.typ")
    );

    let outside = error_diagnostics(&src_load("/elsewhere/post.typ"), Path::new("/site"))
        .expect("an outside path is still described");
    assert!(outside[0].location.is_none(), "{outside:#?}");
}

#[test]
fn package_failure_site_directory() {
    use tola_typst::{PackageFailure, PackageFailureReason, PackageSearch};

    let root = Path::new("/site");
    let site_pkgs = |pkg: &str, ver: &str| PackageSearch {
        root: root.join("packages"),
        candidate: root.join("packages").join(pkg).join(ver),
    };
    let host_cache = |pkg: &str, ver: &str| PackageSearch {
        root: PathBuf::from("/home/author/.cache/typst/packages"),
        candidate: PathBuf::from("/home/author/.cache/typst/packages")
            .join(pkg)
            .join(ver),
    };

    for (pkg, reason, searched, names_site) in [
        (
            "@cawemo/report:1.2.0",
            PackageFailureReason::NotInstalled,
            vec![
                site_pkgs("cawemo/report", "1.2.0"),
                host_cache("cawemo/report", "1.2.0"),
            ],
            true,
        ),
        (
            "@preview/cetz:0.3.4",
            PackageFailureReason::Unavailable,
            vec![
                site_pkgs("preview/cetz", "0.3.4"),
                host_cache("preview/cetz", "0.3.4"),
            ],
            true,
        ),
        (
            "@preview/cetz:0.3.4",
            PackageFailureReason::NotInstalled,
            vec![host_cache("preview/cetz", "0.3.4")],
            false,
        ),
    ] {
        let failure = PackageFailure {
            package: pkg.into(),
            reason,
            searched,
        };
        let guidance = package_failure_guidance(&failure, root);

        assert!(guidance.message.contains(pkg), "{pkg} {reason:?}");
        let help = &guidance.help;
        assert!(!help.message.contains("author"), "{}", help.message);
        assert!(!help.message.contains("/home/"), "{}", help.message);
        assert_eq!(
            help.message.contains("`packages/`"),
            names_site,
            "{pkg} {reason:?}: {}",
            help.message
        );
    }
}

#[test]
fn failure_retains_warnings() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = crate::config::tests::load_test_config(dir.path(), "");
    let mut warnings = Diagnostics::new();
    warnings.extend_distinct(&Diagnostics::from_vec(vec![
        warning_at("first warning", location(None)),
        warning_at("second warning", location(None)),
    ]));
    assert_eq!(warning_diagnostics(&cfg, &mut warnings).len(), 2);

    let compiler_warning = warning_at("upstream warning", location(None));
    let export_warning =
        typst::diag::SourceDiagnostic::warning(typst::syntax::Span::detached(), "upstream warning");
    for err in [
        anyhow::Error::new(tola_typst::CompileError::Compilation {
            diagnostics: Diagnostics::from_vec(vec![compiler_warning]),
        }),
        anyhow::Error::new(tola_typst::CompileError::bundle_export([
            export_warning.into()
        ])),
    ] {
        let diags = error_diagnostics(&err, Path::new("/site")).unwrap();
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].severity, tola_diagnostic::Severity::Error);
        assert_eq!(diags[1].severity, tola_diagnostic::Severity::Warning);
    }
}

#[test]
fn relativize_site_root_prefix() {
    let mut msg = String::from("cannot read /site/content/post.typ");
    relativize_message(&mut msg, Path::new("/site"));
    assert_eq!(msg, "cannot read content/post.typ");

    for untouched in [
        "cannot read /other/site/content/post.typ",
        "cannot read /site-sibling/post.typ",
    ] {
        let mut msg = untouched.to_owned();
        relativize_message(&mut msg, Path::new("/site"));
        assert_eq!(msg, untouched);
    }
}

#[test]
fn resolved_paths_stay_relative() {
    let line = source_line(tola_typst::SourceLine {
        line_num: 1,
        start_column: 0,
        start_character: 0,
        ends_line: true,
        text: "éx".into(),
        highlight: Some((1, 2)),
    });
    assert_eq!(line.highlight, Some((2, 3)));

    let resolved = tola_typst::Diagnostic {
        hints: vec![tola_typst::Hint {
            message: "inspect the import".into(),
            location: tola_typst::SourceLocation {
                line: Some(1),
                column: Some(1),
                ..location(Some("/site/templates/document.typ"))
            },
        }],
        traces: vec![tola_typst::Trace {
            kind: tola_typst::TraceKind::Include("content/index.typ".into()),
            message: "while including `content/index.typ`".into(),
            location: location(Some("/site/site.typ")),
        }],
        ..warning_at(
            "warning",
            tola_typst::SourceLocation {
                line: Some(2),
                column: Some(3),
                ..location(Some("/site/content/index.typ"))
            },
        )
    };
    let diag = convert_diagnostic(
        resolved,
        Path::new("/site"),
        tola_diagnostic::codes::typst::COMPILE,
    );

    assert_eq!(diag.code, tola_diagnostic::codes::typst::COMPILE);
    assert_eq!(diag.location.unwrap().path, "content/index.typ");
    assert_eq!(
        diag.help[0].location.as_ref().unwrap().path,
        "templates/document.typ"
    );
    assert_eq!(diag.trace[0].location.as_ref().unwrap().path, "site.typ");
}

#[test]
fn unknown_variable_highlight() {
    let highlighted = tola_typst::Diagnostic {
        origin: tola_typst::DiagnosticOrigin::Typst,
        severity: tola_typst::DiagnosticSeverity::Error,
        message: "unknown variable: absent-name".into(),
        location: tola_typst::SourceLocation {
            line: Some(1),
            column: Some(15),
            source_lines: vec![tola_typst::SourceLine {
                line_num: 1,
                start_column: 0,
                start_character: 0,
                ends_line: true,
                text: "#let missing = absent-name".into(),
                highlight: Some((15, 26)),
            }],
            ..location(Some("/site/content/index.typ"))
        },
        hints: Vec::new(),
        traces: Vec::new(),
        imported_by: Vec::new(),
        package_failure: None,
    };
    let resolved = convert_diagnostic(
        highlighted,
        Path::new("/site"),
        tola_diagnostic::codes::typst::COMPILE,
    );
    assert_eq!(
        resolved.cause,
        Some(tola_diagnostic::DiagnosticCause::UnknownVariable {
            name: "absent-name".to_owned(),
        })
    );

    let no_highlight = tola_typst::Diagnostic {
        location: location(Some("/site/content/index.typ")),
        ..warning_at("unknown variable: absent-name", location(None))
    };
    let panic = tola_typst::Diagnostic {
        message: "panicked with: unknown variable: absent-name".into(),
        ..no_highlight.clone()
    };
    for diag in [no_highlight, panic] {
        let resolved = convert_diagnostic(
            diag,
            Path::new("/site"),
            tola_diagnostic::codes::typst::COMPILE,
        );
        assert_eq!(resolved.cause, None, "{resolved:?}");
    }
}

#[test]
fn missing_clock_help() {
    let diag = tola_typst::Diagnostic {
        origin: tola_typst::DiagnosticOrigin::Typst,
        severity: tola_typst::DiagnosticSeverity::Error,
        message: MISSING_CLOCK.into(),
        location: location(Some("/site/site.typ")),
        hints: Vec::new(),
        traces: Vec::new(),
        imported_by: Vec::new(),
        package_failure: None,
    };

    let resolved = convert_diagnostic(
        diag,
        Path::new("/site"),
        tola_diagnostic::codes::typst::COMPILE,
    );

    assert!(resolved.message.contains("datetime.today()"));
    assert!(
        resolved
            .help
            .iter()
            .any(|h| h.message.contains("datetime(year:")),
        "{resolved:#?}"
    );
}
