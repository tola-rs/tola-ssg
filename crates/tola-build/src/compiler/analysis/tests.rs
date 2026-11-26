use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tempfile::TempDir;
use tola_input::normalize_path;
use tola_typst::FileRead;
use typst::foundations::IntoValue;

use crate::compiler::tests::{compiler_host, configure_site, metadata_field};
use crate::config::SiteConfig;
use crate::content::ContentUnit;

use super::round::AnalyzedSource;
use super::settlement::SourceReads;
use super::*;

fn content_units(
    cfg: &SiteConfig,
) -> Result<Vec<ContentUnit>, crate::content::ContentDiscoveryError> {
    crate::content::discover_content_units_for_config_with_cancellation(
        cfg,
        &crate::cancellation::BuildCancellation::default(),
    )
}

fn analyzed_entry<'a>(scan: &'a SourceScan, src: &Path) -> &'a Arc<AnalyzedSource> {
    scan.entries
        .get(&normalize_path(src))
        .expect("analyzed entry exists")
}

fn scan_default(
    cfg: &SiteConfig,
    host: &SiteHost,
    content: &[ContentUnit],
    reuse: Option<&SourceAnalysisCache>,
) -> anyhow::Result<SourceScan> {
    let deps = reuse.map(|_| {
        crate::compiler::CompilationDependencies::new(
            cfg.root(),
            content.iter().map(|unit| unit.source.clone()),
            std::iter::once(cfg.build.entry.clone()),
            std::iter::empty(),
            std::iter::empty(),
            host,
            None,
        )
        .unwrap()
    });
    let reuse = match (reuse, deps.as_ref()) {
        (Some(cache), Some(deps)) => SourceAnalysisReuse::retained(cache, deps),
        (None, None) => SourceAnalysisReuse::None,
        _ => unreachable!("source analysis test reuse is paired"),
    };
    let mut inputs = crate::compiler::BuildInputs::default();
    scan(
        cfg,
        host,
        content,
        crate::package::SiteBindings::from_config(cfg, Default::default(), None),
        reuse,
        &tola_typst::BundleCancellation::default(),
        &mut inputs,
    )
    .map_err(SourceAnalysisFailure::into_error)
}

fn rendered_failure(scan: anyhow::Result<SourceScan>) -> String {
    match scan {
        Ok(_) => panic!("the scan was expected to fail"),
        Err(err) => format!("{err:#}"),
    }
}

fn failure_diagnostics(scan: anyhow::Result<SourceScan>) -> Vec<tola_diagnostic::Diagnostic> {
    let err = scan.err().expect("the scan was expected to fail");
    tola_diagnostic::attached(&err)
        .expect("a failed scan carries diagnostics")
        .to_vec()
}

fn setup_site(srcs: &[(&str, &str)]) -> (TempDir, SiteConfig, Vec<ContentUnit>) {
    let dir = TempDir::new().unwrap();
    let content = dir.path().join("content");
    fs::create_dir_all(&content).unwrap();
    for (name, src) in srcs {
        let path = content.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, src).unwrap();
    }
    let entry = dir.path().join("site.typ");
    fs::write(&entry, "").unwrap();
    let mut cfg = crate::config::tests::load_test_config(dir.path(), "");
    configure_site(&mut cfg, &content, &entry);
    let units = content_units(&cfg).unwrap();
    (dir, cfg, units)
}

fn scan_result(srcs: &[(&str, &str)]) -> (TempDir, anyhow::Result<SourceScan>) {
    let (dir, cfg, units) = setup_site(srcs);
    let res = scan_default(&cfg, &compiler_host(&cfg).unwrap(), &units, None);
    (dir, res)
}

fn scan_content(srcs: &[(&str, &str)]) -> (TempDir, SourceScan) {
    let (dir, res) = scan_result(srcs);
    (dir, res.unwrap())
}

fn prepare_cache_for_changes(
    cache: SourceAnalysisCache,
    cfg: &SiteConfig,
    content: &[ContentUnit],
    reads: Vec<SourceReads>,
    changed: &[PathBuf],
    inventory_may_change: bool,
) -> SourceAnalysisCache {
    let host = compiler_host(cfg).unwrap();
    let pkg_checks = reads
        .iter()
        .flat_map(|(_, _, checks)| checks.iter().cloned());
    let deps = crate::compiler::CompilationDependencies::new(
        cfg.root(),
        content.iter().map(|unit| unit.source.clone()),
        std::iter::once(cfg.build.entry.clone()),
        reads.iter().map(|(src, reads, checks)| {
            (
                crate::compiler::TypstDependencyReader::ContentSource(src.clone()),
                reads.clone(),
                checks.clone(),
            )
        }),
        pkg_checks,
        &host,
        None,
    )
    .unwrap();
    let decision =
        crate::compiler::RebuildDecision::for_paths(Some(&deps), changed, inventory_may_change);
    cache.prepare_for_rebuild(&decision, cfg.project_entry_path().as_deref())
}

#[test]
fn declarations_call_site() {
    for (name, srcs, declares, warns) in [
        (
            "plain label",
            &[("post.typ", "#metadata((title: \"A\")) <tola-meta>")][..],
            false,
            true,
        ),
        (
            "parenthesized",
            &[("post.typ", "#(metadata((title: \"A\"))) <tola-meta>")][..],
            false,
            true,
        ),
        (
            "aliased binding",
            &[(
                "post.typ",
                "#let m = metadata\n#m((title: \"A\")) <tola-meta>",
            )][..],
            false,
            true,
        ),
        (
            "shadowed native",
            &[(
                "post.typ",
                "#let tola-meta = metadata\n#tola-meta((title: \"A\")) <tola-meta>",
            )][..],
            false,
            true,
        ),
        (
            "wrapper body",
            &[(
                "post.typ",
                "#let decl(d) = metadata(d)\n#decl((title: \"A\")) <tola-meta>",
            )][..],
            false,
            false,
        ),
        (
            "show rule",
            &[(
                "post.typ",
                "#show heading: it => [#metadata((show: true)) <tola-meta>#it]\n= Heading",
            )][..],
            false,
            false,
        ),
        (
            "placed binding",
            &[(
                "post.typ",
                "#let marker = metadata((title: \"A\"))\n#marker <tola-meta>",
            )][..],
            false,
            false,
        ),
        (
            "top level",
            &[(
                "post.typ",
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"A\"))",
            )][..],
            true,
            false,
        ),
        (
            "own function",
            &[(
                "post.typ",
                "#import \"@tola/source:0.0.0\": tola-meta\n#let declare() = tola-meta((title: \"Fn\"))\n#declare()",
            )][..],
            true,
            false,
        ),
    ] {
        let (dir, scan) = scan_content(srcs);
        let entry = analyzed_entry(&scan, &dir.path().join("content/post.typ"));
        assert_eq!(entry.declaration.is_some(), declares, "{name} declares");
        assert_eq!(
            scan.declaration_warnings.len(),
            usize::from(warns),
            "{name}"
        );
        if warns {
            let warn = &scan.declaration_warnings[0];
            assert_eq!(warn.severity, tola_diagnostic::Severity::Warning);
            assert_eq!(
                warn.code,
                tola_diagnostic::codes::source::DECLARATION_DEPRECATED
            );
        }
    }
}

#[test]
fn declarations_deferred_ignored() {
    for src in [
        r#"#import "@tola/document:0.0.0": current-document
#context {
  let ctx = current-document()
  [#import "@tola/source:0.0.0": tola-meta
#tola-meta((contextual: true))]
}"#,
        r#"#show heading: it => [#import "@tola/source:0.0.0": tola-meta
#tola-meta((selector-show: true))#it]
= Heading"#,
    ] {
        let (_dir, res) = scan_content(&[("post.typ", src)]);
        assert!(res.source_set.sources()[0].metadata().is_none());
    }
}

#[test]
fn declarations_native_values() {
    let dir = TempDir::new().unwrap();
    let content = dir.path().join("content");
    fs::create_dir(&content).unwrap();
    fs::write(
        dir.path().join("theme.typ"),
        r#"#let attributes() = (
  title: [Document],
  callback: value => value,
  card: [#metadata((nested: true))],
)"#,
    )
    .unwrap();
    fs::write(
        content.join("post.typ"),
        r#"#import "../theme.typ": attributes
#import "@tola/source:0.0.0": tola-meta
#tola-meta(attributes())
= Body"#,
    )
    .unwrap();
    fs::write(dir.path().join("site.typ"), "").unwrap();
    let mut cfg = crate::config::tests::load_test_config(dir.path(), "");
    configure_site(&mut cfg, &content, &dir.path().join("site.typ"));
    let units = content_units(&cfg).unwrap();
    let scanned = scan_default(&cfg, &compiler_host(&cfg).unwrap(), &units, None).unwrap();

    assert_eq!(
        metadata_field(&scanned, "post.typ", "title"),
        typst::text::TextElem::packed("Document").into_value()
    );
    assert!(matches!(
        metadata_field(&scanned, "post.typ", "callback"),
        typst::foundations::Value::Func(_)
    ));
    assert!(matches!(
        metadata_field(&scanned, "post.typ", "card"),
        typst::foundations::Value::Content(_)
    ));
}

#[test]
fn physical_reads_owned_by_source() {
    let (dir, cfg, units) = setup_site(&[
        ("a.typ", "#read(\"/data/a.txt\")"),
        ("b.typ", "#read(\"/data/b.txt\")"),
    ]);
    let data = dir.path().join("data");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("a.txt"), "A").unwrap();
    fs::write(data.join("b.txt"), "B").unwrap();

    let scan = scan_default(&cfg, &compiler_host(&cfg).unwrap(), &units, None).unwrap();
    let reads_for = |src: &Path| {
        scan.dependency_reads()
            .into_iter()
            .find(|(path, _, _)| path == &normalize_path(src))
            .map(|(_, reads, _)| reads)
            .unwrap()
    };
    let reads_path = |reads: &[FileRead], expected: &str| {
        reads.iter().any(|read| {
            matches!(
                read.evidence().locator(),
                tola_typst::ReadLocator::Root(path) if path == Path::new(expected)
            )
        })
    };

    let content = dir.path().join("content");
    for (src, own, other) in [
        ("a.typ", "data/a.txt", "data/b.txt"),
        ("b.typ", "data/b.txt", "data/a.txt"),
    ] {
        let reads = reads_for(&content.join(src));
        assert!(reads_path(&reads, own), "{src} read {own}");
        assert!(!reads_path(&reads, other), "{src} claimed {other}");
    }
}

#[test]
fn cache_reanalyzes_affected_only() {
    let (dir, cfg, units) = setup_site(&[
        ("a.typ", "#read(\"/data/shared.txt\")"),
        ("b.typ", "#read(\"/data/b.txt\")"),
        ("plain.typ", "Plain"),
    ]);
    let root = dir.path();
    let content = root.join("content");
    let data = root.join("data");
    let (a, b, plain) = (
        content.join("a.typ"),
        content.join("b.typ"),
        content.join("plain.typ"),
    );
    let shared = data.join("shared.txt");
    fs::create_dir_all(&data).unwrap();
    fs::write(&shared, "one").unwrap();
    fs::write(data.join("b.txt"), "B").unwrap();

    let host = compiler_host(&cfg).unwrap();
    let first = scan_default(&cfg, &host, &units, None).unwrap();
    let ids: BTreeMap<_, _> = [&a, &b, &plain]
        .into_iter()
        .map(|path| (path.clone(), Arc::clone(analyzed_entry(&first, path))))
        .collect();
    let cache = SourceAnalysisCache::from_scan(&first);

    fs::write(&shared, "two").unwrap();
    let prepared = prepare_cache_for_changes(
        cache,
        &cfg,
        &units,
        first.dependency_reads(),
        std::slice::from_ref(&shared),
        false,
    );
    let changed = scan_default(&cfg, &host, &units, Some(&prepared)).unwrap();

    assert!(!Arc::ptr_eq(analyzed_entry(&changed, &a), &ids[&a]));
    assert!(Arc::ptr_eq(analyzed_entry(&changed, &b), &ids[&b]));
    assert!(Arc::ptr_eq(analyzed_entry(&changed, &plain), &ids[&plain]));
}

#[test]
fn cache_reuses_unchanged() {
    let files: Vec<_> = (0..8)
        .map(|i| (format!("post-{i}.typ"), "Plain".to_owned()))
        .collect();
    let refs: Vec<_> = files
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let (_dir, cfg, units) = setup_site(&refs);
    let host = compiler_host(&cfg).unwrap();
    let first = scan_default(&cfg, &host, &units, None).unwrap();
    let ids: Vec<_> = first
        .entries
        .ordered_paths
        .iter()
        .map(|path| (path.clone(), Arc::clone(analyzed_entry(&first, path))))
        .collect();
    let cache = SourceAnalysisCache::from_scan(&first);

    let prepared =
        prepare_cache_for_changes(cache, &cfg, &units, first.dependency_reads(), &[], false);
    let unchanged = scan_default(&cfg, &host, &units, Some(&prepared)).unwrap();

    for (path, entry) in ids {
        assert!(
            Arc::ptr_eq(analyzed_entry(&unchanged, &path), &entry),
            "{} was re-derived",
            path.display()
        );
    }
}

#[test]
fn errors_reported_per_source() {
    let (_dir, scan) = scan_result(&[("one.typ", "#let broken ="), ("two.typ", "#let broken =")]);
    let err = scan
        .err()
        .expect("source errors unexpectedly passed analysis");
    let diags = tola_diagnostic::attached(&err).expect("source diagnostics should remain attached");
    assert_eq!(diags.len(), 2);
    assert_eq!(
        diags
            .iter()
            .map(|diag| diag.location.as_ref().unwrap().path.as_str())
            .collect::<Vec<_>>(),
        ["content/one.typ", "content/two.typ"]
    );
    assert!(
        diags
            .iter()
            .all(|diag| diag.severity == tola_diagnostic::Severity::Error)
    );
}

#[test]
fn errors_retain_warnings() {
    let (dir, cfg, units) = setup_site(&[
        ("document.typ", "#let d = decimal(0.1)"),
        ("other.typ", "#let d = decimal(0.2)"),
    ]);
    let doc = dir.path().join("content/document.typ");
    let host = compiler_host(&cfg).unwrap();
    let successful = scan_default(&cfg, &host, &units, None).unwrap();
    let cache = SourceAnalysisCache::from_scan(&successful);
    fs::write(&doc, "#let d = decimal(0.1)\n#missing").unwrap();
    let cache = prepare_cache_for_changes(
        cache,
        &cfg,
        &units,
        successful.dependency_reads(),
        std::slice::from_ref(&doc),
        false,
    );

    for reuse in [None, Some(&cache)] {
        let diags = failure_diagnostics(scan_default(&cfg, &host, &units, reuse));
        let warnings: BTreeSet<_> = diags
            .iter()
            .filter(|diag| diag.severity == tola_diagnostic::Severity::Warning)
            .map(|warn| warn.location.as_ref().unwrap().path.clone())
            .collect();
        assert_eq!(
            warnings,
            BTreeSet::from([
                "content/document.typ".to_owned(),
                "content/other.typ".to_owned()
            ])
        );
        assert_eq!(
            diags
                .iter()
                .filter(|diag| diag.severity == tola_diagnostic::Severity::Error)
                .count(),
            1
        );
    }
}

#[test]
fn settlement_mutual_converges() {
    let (_dir, scan) = scan_content(&[
        (
            "alpha.typ",
            "#import \"@tola/source:0.0.0\": all-sources, tola-meta\n\
             #tola-meta((title: [Alpha]))\n\
             #let beta = all-sources().find(source => source.id == \"beta.typ\")\n\
             #let stopped = beta.meta.title",
        ),
        (
            "beta.typ",
            "#import \"@tola/source:0.0.0\": all-sources, tola-meta\n\
             #let alpha = all-sources().find(source => source.id == \"alpha.typ\")\n\
             #tola-meta((title: alpha.meta.title))",
        ),
    ]);

    assert!(scan.metadata_rounds > 1);
    for name in ["alpha.typ", "beta.typ"] {
        assert_eq!(
            metadata_field(&scan, name, "title"),
            typst::text::TextElem::packed("Alpha").into_value(),
            "{name}"
        );
    }
}

#[test]
fn settlement_chain_converges() {
    let mut srcs = vec![(
        "source-0.typ".to_owned(),
        "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((rank: 0))".to_owned(),
    )];
    for i in 1..20 {
        srcs.push((
            format!("source-{i}.typ"),
            format!(
                r#"#import "@tola/source:0.0.0": all-sources
#let previous = all-sources().find(source => source.id == "source-{}.typ")
#import "@tola/source:0.0.0": tola-meta
#tola-meta((rank: previous.meta.rank + 1))"#,
                i - 1
            ),
        ));
    }
    let refs: Vec<_> = srcs
        .iter()
        .map(|(name, src)| (name.as_str(), src.as_str()))
        .collect();
    let (_dir, scanned) = scan_content(&refs);
    assert_eq!(
        metadata_field(&scanned, "source-19.typ", "rank"),
        19.into_value()
    );
}

#[test]
fn settlement_cycle_fails() {
    let (_dir, scan) = scan_result(&[(
        "toggle.typ",
        r#"#import "@tola/source:0.0.0": all-sources
#let previous = all-sources().first().meta
#import "@tola/source:0.0.0": tola-meta
#tola-meta((flag: previous == none or not previous.flag))"#,
    )]);
    let rendered = rendered_failure(scan);
    assert!(rendered.contains("content/toggle.typ"), "{rendered}");
}

#[test]
fn settlement_unsettled_fails() {
    let (_dir, scan) = scan_result(&[(
        "post.typ",
        r#"#import "@tola/source:0.0.0": all-sources
#let captured = all-sources()
#import "@tola/source:0.0.0": tola-meta
#tola-meta((count: () => captured.len()))"#,
    )]);
    let rendered = rendered_failure(scan);
    assert!(rendered.contains("content/post.typ"), "{rendered}");
}

#[test]
fn declarations_duplicate_fails() {
    let (_dir, scan) = scan_result(&[(
        "post.typ",
        "#import \"@tola/source:0.0.0\": tola-meta\n\
         #tola-meta((first: true))\n\
         #tola-meta((second: true))",
    )]);
    let err = scan
        .err()
        .expect("duplicate source metadata unexpectedly succeeded");
    let diags = tola_diagnostic::attached(&err).unwrap();
    assert_eq!(diags[0].location.as_ref().unwrap().line, Some(3));
}

#[test]
fn legacy_label_warns_per_source() {
    let (_dir, scan) = scan_content(&[
        ("alpha.typ", "#metadata((title: \"Alpha\")) <tola-meta>"),
        ("beta.typ", "#metadata((title: \"Beta\")) <tola-meta>"),
    ]);
    let locs: Vec<_> = scan
        .declaration_warnings
        .iter()
        .map(|warn| {
            let loc = warn.location.as_ref().unwrap();
            assert!(loc.range.is_some());
            loc.path.as_str()
        })
        .collect();
    assert_eq!(locs, ["content/alpha.typ", "content/beta.typ"]);
}

#[test]
fn sources_shared_set() {
    let meta_src = r#"#import "@tola/source:0.0.0": all-sources, tola-meta
#tola-meta((count: all-sources().len()))"#;
    let (_dir, res) = scan_content(&[
        ("docs/guide.typ", meta_src),
        ("api/reference.typ", meta_src),
    ]);
    assert_eq!(res.source_set.sources().len(), 2);
    for src in res.source_set.sources() {
        let file = src.source().file_name().unwrap().to_str().unwrap();
        assert_eq!(metadata_field(&res, file, "count"), 2.into_value());
    }
}

#[test]
fn cache_membership_invalidates_readers() {
    let (dir, cfg, units) = setup_site(&[
        ("plain.typ", "Plain"),
        (
            "reader.typ",
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/source:0.0.0": tola-meta
#tola-meta((count: all-sources().len()))"#,
        ),
    ]);
    let content = dir.path().join("content");
    let (plain, reader) = (content.join("plain.typ"), content.join("reader.typ"));
    let host = compiler_host(&cfg).unwrap();
    let first = scan_default(&cfg, &host, &units, None).unwrap();
    let plain_entry = Arc::clone(analyzed_entry(&first, &plain));
    let reader_entry = Arc::clone(analyzed_entry(&first, &reader));
    let cache = SourceAnalysisCache::from_scan(&first);

    fs::write(content.join("added.typ"), "Added").unwrap();
    let expanded = content_units(&cfg).unwrap();
    let changed = scan_default(&cfg, &host, &expanded, Some(&cache)).unwrap();

    assert!(Arc::ptr_eq(analyzed_entry(&changed, &plain), &plain_entry));
    assert!(!Arc::ptr_eq(
        analyzed_entry(&changed, &reader),
        &reader_entry
    ));
}

#[test]
fn scan_cancelled_reports_error() {
    let (_dir, cfg, units) = setup_site(&[("post.typ", "Post")]);
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let policy = tola_typst::BundleCancellation::default().with_cancellation(cancelled);
    let mut inputs = crate::compiler::BuildInputs::default();
    let err = match scan(
        &cfg,
        &compiler_host(&cfg).unwrap(),
        &units,
        crate::package::SiteBindings::from_config(&cfg, Default::default(), None),
        SourceAnalysisReuse::None,
        &policy,
        &mut inputs,
    ) {
        Ok(_) => panic!("cancelled source analysis unexpectedly succeeded"),
        Err(failure) => failure.into_error(),
    };
    assert!(err.chain().any(|cause| matches!(
        cause.downcast_ref::<tola_typst::CompileError>(),
        Some(tola_typst::CompileError::Cancelled)
    )));
}
