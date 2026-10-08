//! Export of one compiled Bundle into entries, reusing unchanged bytes.

use crate::diagnostic::{CompileError, Diagnostics};
use crate::session::AccessedDeps;
use crate::world::file::{DiskReadPath, FileRead};
use std::collections::HashMap;

use typst_bundle::{Bundle, BundleOptions};

use super::*;

/// Bound cancellation latency while hashing exported bytes.
const BUNDLE_DIGEST_CHUNK_BYTES: usize = 64 * 1024;

/// One exported Bundle and the compilation evidence that produced it.
#[derive(Debug)]
pub struct BundleExport {
    entries: BundleEntries,
    compilation: BundleCompilation,
}

impl BundleExport {
    /// Entries in Typst's Bundle order.
    pub fn entries(&self) -> &[BundleEntry] {
        self.entries.as_slice()
    }

    /// Number of exported entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no entries were exported.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate over entries in native Bundle order.
    pub fn iter(&self) -> impl Iterator<Item = &BundleEntry> {
        self.entries.iter()
    }

    /// Find an entry by its native virtual path.
    pub fn entry(&self, path: &typst::syntax::VirtualPath) -> Option<&BundleEntry> {
        self.entries.get(path)
    }

    /// Reusable entry bytes and digests for the next export.
    pub fn entry_cache(&self) -> &BundleEntries {
        &self.entries
    }

    /// Compiled documents with native Typst introspection access.
    pub fn documents(&self) -> impl Iterator<Item = CompiledBundleDocument<'_>> {
        self.compilation.documents()
    }

    /// Successful file reads consumed by the compilation.
    pub fn file_reads(&self) -> &[FileRead] {
        self.compilation.file_reads()
    }

    /// All dependency evidence observed during compilation.
    pub fn accessed(&self) -> &AccessedDeps {
        self.compilation.accessed()
    }

    /// Package-directory checks observed during compilation.
    pub fn package_checks(&self) -> &[crate::world::package::PackageCheck] {
        self.compilation.package_checks()
    }

    /// Physical read paths retained by the compilation.
    pub fn disk_reads(&self) -> &[DiskReadPath] {
        self.compilation.disk_reads()
    }

    /// Compilation diagnostics (warnings).
    pub fn diagnostics(&self) -> &Diagnostics {
        self.compilation.diagnostics()
    }

    /// Destructure the export into entries, dependency information, and diagnostics.
    pub fn into_parts(self) -> (Vec<BundleEntry>, AccessedDeps, Diagnostics) {
        (
            self.entries.as_slice().to_vec(),
            self.compilation.accessed,
            self.compilation.diagnostics,
        )
    }
}

impl BundleCompilation {
    /// Export this Bundle with default Typst options.
    pub fn export_default(
        self,
        cancellation: &BundleCancellation,
        previous: Option<&BundleEntries>,
    ) -> Result<BundleExport, CompileError> {
        self.export(&BundleOptions::default(), cancellation, previous)
    }

    /// Export entries without consuming the compiled Bundle.
    ///
    /// Each call exports the whole Bundle. `previous` reuses byte storage for
    /// unchanged entries after export; it does not skip compilation or export.
    pub fn export_entries(
        &self,
        options: &BundleOptions,
        cancellation: &BundleCancellation,
        previous: Option<&BundleEntries>,
    ) -> Result<BundleEntries, CompileError> {
        export_bundle(&self.bundle, options, cancellation, previous)
    }

    /// Export this compiled Bundle and retain the compilation in the result.
    pub fn export(
        self,
        options: &BundleOptions,
        cancellation: &BundleCancellation,
        previous: Option<&BundleEntries>,
    ) -> Result<BundleExport, CompileError> {
        let entries = self.export_entries(options, cancellation, previous)?;
        Ok(BundleExport {
            entries,
            compilation: self,
        })
    }
}

fn export_bundle(
    bundle: &Bundle,
    options: &BundleOptions,
    cancellation: &BundleCancellation,
    previous: Option<&BundleEntries>,
) -> Result<BundleEntries, CompileError> {
    cancellation.ensure_active()?;

    let exported = typst_bundle::export(bundle, options);
    cancellation.ensure_active()?;
    let exported = exported.map_err(|diagnostics| {
        CompileError::bundle_export(
            diagnostics
                .into_iter()
                .map(crate::diagnostic::NativeDiagnostic::from),
        )
    })?;

    let mut previous_by_path = HashMap::new();
    if let Some(previous) = previous {
        for entry in previous.as_slice() {
            cancellation.ensure_active()?;
            previous_by_path.insert(entry.path(), entry);
        }
    }
    let mut entries = Vec::with_capacity(bundle.files.len());
    for (path, file) in bundle.files.iter() {
        cancellation.ensure_active()?;
        let kind = bundle_file_kind(file);
        let bytes = exported
            .get(path)
            .expect("official Bundle export preserves every source file identity")
            .clone();
        let mut hasher = blake3::Hasher::new();
        for chunk in bytes.as_slice().chunks(BUNDLE_DIGEST_CHUNK_BYTES) {
            cancellation.ensure_active()?;
            hasher.update(chunk);
        }
        let digest = crate::world::file::ContentDigest::from_bytes(*hasher.finalize().as_bytes());
        if let Some(entry) = previous_by_path.get(path)
            && entry.kind() == kind
            && entry.digest() == digest
        {
            entries.push((*entry).clone());
        } else {
            entries.push(BundleEntry {
                path: path.clone(),
                kind,
                digest,
                bytes: BundleBytes::new(bytes),
            });
        }
    }
    let entries = BundleEntries(entries.into());
    cancellation.ensure_active()?;
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{compile, entry, entry_from, shares_backing, world_for};
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    #[test]
    fn exported_entry_describes_itself() {
        let result = compile(
            r#"
            #document("page.bin", format: "html")[HTML]
            #document("paper.bin", format: "pdf")[#rect(width: 1pt, height: 1pt)]
            #document("preview.bin", format: "png")[#rect(width: 1pt, height: 1pt)]
            #document("vector.bin", format: "svg")[#rect(width: 1pt, height: 1pt)]
            "#,
        );
        assert_eq!(
            result
                .entries()
                .iter()
                .map(|entry| entry.kind)
                .collect::<Vec<_>>(),
            [
                BundleEntryKind::HtmlDocument,
                BundleEntryKind::PdfDocument,
                BundleEntryKind::PngDocument,
                BundleEntryKind::SvgDocument,
            ]
        );

        let nothing = compile("");
        assert!(nothing.entries().is_empty());
        assert_eq!(nothing.len(), 0);

        let single = compile("#document(\"one.html\")[Hello]");
        assert_eq!(single.entries().len(), 1);

        let only_entry = &single.entries()[0];
        assert_eq!(only_entry.path().get_with_slash(), "/one.html");
        assert_eq!(only_entry.kind(), BundleEntryKind::HtmlDocument);
        assert!(String::from_utf8_lossy(only_entry.bytes().as_slice()).contains("Hello"));
        assert_eq!(
            only_entry.digest(),
            crate::world::file::ContentDigest::of(only_entry.bytes().as_slice())
        );
        assert_eq!(single.entry(only_entry.path()), Some(only_entry));
    }
    #[test]
    fn export_reuses_unchanged_entry_bytes() {
        let dir = TempDir::new().unwrap();
        let world = world_for(
            &dir,
            "main.typ",
            "#document(\"index.html\")[Hello]#asset(\"raw.bin\", bytes(\"raw\"))",
        );
        let cancellation = BundleCancellation::default();

        let first = compile_bundle_world(&world, &cancellation)
            .unwrap()
            .export(&BundleOptions::default(), &cancellation, None)
            .unwrap();
        let repeated = compile_bundle_world(&world, &cancellation)
            .unwrap()
            .export(
                &BundleOptions::default(),
                &cancellation,
                Some(first.entry_cache()),
            )
            .unwrap();
        assert!(shares_backing(
            &first.entries()[0].bytes,
            &repeated.entries()[0].bytes
        ));
        assert_eq!(first.entries()[0].digest, repeated.entries()[0].digest);

        let compilation = compile_bundle_world(&world, &cancellation).unwrap();
        let mut compact = BundleOptions::default();
        compact.html.pretty = false;
        let first = compilation
            .export_entries(&compact, &cancellation, None)
            .unwrap();
        let mut pretty = compact;
        pretty.html.pretty = true;
        let second = compilation
            .export_entries(&pretty, &cancellation, Some(&first))
            .unwrap();
        let first_html = entry_from(first.as_slice(), "/index.html");
        let second_html = entry_from(second.as_slice(), "/index.html");
        assert_ne!(first_html.bytes(), second_html.bytes());
        let first_asset = entry_from(first.as_slice(), "/raw.bin");
        let second_asset = entry_from(second.as_slice(), "/raw.bin");
        assert!(shares_backing(&first_asset.bytes, &second_asset.bytes));
        assert_eq!(first_asset.digest(), second_asset.digest());
    }
    #[test]
    fn unaffected_entries_keep_their_bytes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("main.typ");
        fs::write(dir.path().join("a.typ"), "First A").unwrap();
        fs::write(dir.path().join("b.typ"), "Stable B").unwrap();
        fs::write(
            &path,
            "#document(\"a.html\")[#include \"a.typ\"]\n#document(\"b.html\")[#include \"b.typ\"]",
        )
        .unwrap();
        let cache = Arc::new(crate::world::file::SharedFileCache::new());
        let world = || {
            TypstWorld::builder(&path, dir.path())
                .with_shared_cache(Arc::clone(&cache))
                .no_fonts()
                .build(&crate::BundleCancellation::default())
                .expect("valid test world")
        };
        let cancellation = BundleCancellation::default();
        let options = BundleOptions::default();
        let first = compile_bundle_world(&world(), &cancellation)
            .unwrap()
            .export(&options, &cancellation, None)
            .unwrap();

        fs::write(dir.path().join("a.typ"), "Second A").unwrap();
        let second = compile_bundle_world(&world(), &cancellation)
            .unwrap()
            .export(&options, &cancellation, Some(first.entry_cache()))
            .unwrap();
        let first_a = entry(&first, "/a.html");
        let second_a = entry(&second, "/a.html");
        let first_b = entry(&first, "/b.html");
        let second_b = entry(&second, "/b.html");

        assert_ne!(first_a.digest, second_a.digest);
        assert!(!shares_backing(&first_a.bytes, &second_a.bytes));
        assert_eq!(first_b.digest, second_b.digest);
        assert!(shares_backing(&first_b.bytes, &second_b.bytes));
    }
    #[test]
    fn export_error_keeps_hints_in_diagnostics() {
        use typst::diag::SourceDiagnostic;
        use typst::syntax::Span;

        let first = SourceDiagnostic::error(Span::detached(), "first failure");
        let second = SourceDiagnostic::error(Span::detached(), "second failure")
            .with_hint("try another value");

        let error = CompileError::bundle_export([first.into(), second.into()]);

        let raw = error.raw_diagnostics().unwrap();
        assert_eq!(raw.len(), 2);
        assert_eq!(raw[1].source().hints[0].v, "try another value");
    }
}
