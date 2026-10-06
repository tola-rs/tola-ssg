//! Server-to-browser development messages.

use serde::Serialize;

use tola_build::diagnostic::{Diagnostic, Severity};
use tola_build::output::PageAvailability;
use tola_build::output::manifest::{RevisionDiff, RevisionId};

/// Errors and warnings the browser indicator counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DiagnosticCounts {
    errors: usize,
    warnings: usize,
}

impl DiagnosticCounts {
    pub(crate) fn of(diagnostics: &[Diagnostic]) -> Self {
        let mut counts = Self::default();
        for diagnostic in diagnostics {
            counts.count(diagnostic);
        }
        counts
    }

    pub(crate) fn count(&mut self, diagnostic: &Diagnostic) {
        match diagnostic.severity {
            Severity::Error => self.errors += 1,
            Severity::Warning => self.warnings += 1,
        }
    }

    pub(crate) fn merge(&mut self, later: Self) {
        self.errors += later.errors;
        self.warnings += later.warnings;
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum HotReloadMessage<'a> {
    Revision {
        diff: &'a RevisionDiff,
        page_availability: PageAvailability,
    },
    Connected {
        revision: &'a RevisionId,
        page_availability: PageAvailability,
    },
    /// The development server has published no site yet.
    Awaiting,
    Status {
        rebuilding: bool,
        errors: usize,
        warnings: usize,
    },
}

impl<'a> HotReloadMessage<'a> {
    pub(crate) const fn connected(
        revision: &'a RevisionId,
        page_availability: PageAvailability,
    ) -> Self {
        Self::Connected {
            revision,
            page_availability,
        }
    }

    pub(crate) const fn awaiting() -> Self {
        Self::Awaiting
    }

    pub(crate) const fn revision(
        diff: &'a RevisionDiff,
        page_availability: PageAvailability,
    ) -> Self {
        Self::Revision {
            diff,
            page_availability,
        }
    }

    pub(crate) const fn status(rebuilding: bool, counts: DiagnosticCounts) -> Self {
        Self::Status {
            rebuilding,
            errors: counts.errors,
            warnings: counts.warnings,
        }
    }

    pub(crate) fn to_json(&self) -> String {
        serde_json::to_string(self).expect("hot-reload protocol message must serialize")
    }
}

#[cfg(test)]
mod tests {
    use super::{DiagnosticCounts, HotReloadMessage};
    use tola_build::output::PageAvailability;
    use tola_build::output::manifest::SiteManifest;

    fn site_manifest(source: &str) -> SiteManifest {
        let directory = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(directory.path().join("content")).unwrap();
        std::fs::write(directory.path().join("site.typ"), source).unwrap();
        let config = tola_build::config::SiteConfigSchema::default()
            .resolve(
                &directory.path().join("tola.toml"),
                tola_typst::PackageLocations::from_absolute_roots(None, None).unwrap(),
                &tola_build::config::loading::BuildOverrides::default(),
            )
            .unwrap();
        let build =
            tola_build::build::build_site(&config, tola_build::build::BuildMode::Development)
                .unwrap();
        build
            .into_unchecked_revision(None)
            .site()
            .manifest()
            .clone()
    }

    #[test]
    fn revision_message_names_output_deltas() {
        let asset = "body { color: green; }";
        let before = site_manifest(r#"#document("index.html")[Site]"#);
        let after = site_manifest(&format!(
            "#document(\"index.html\")[Site]\n#asset(\"styles/site.css\", {})",
            serde_json::to_string(asset).unwrap()
        ));

        let diff = before.diff(&after);
        let json = HotReloadMessage::revision(&diff, PageAvailability::Present).to_json();
        let value = serde_json::from_str::<serde_json::Value>(&json).unwrap();

        assert_eq!(value["type"], "revision");
        assert_eq!(value["page_availability"], "present");
        assert_eq!(value["diff"]["from"], before.revision().as_str());
        assert_eq!(value["diff"]["to"], after.revision().as_str());
        let added = value["diff"]["changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|change| change["output"]["path"] == "styles/site.css")
            .expect("the asset appears among the output changes");
        assert_eq!(added["operation"], "added");
        assert_eq!(added["output"]["kind"], "asset");
        assert_eq!(added["output"]["size"], asset.len());
    }

    #[test]
    fn connected_message_has_identity_only() {
        let manifest = site_manifest(r#"#document("index.html")[Site]"#);
        let json =
            HotReloadMessage::connected(manifest.revision(), PageAvailability::Present).to_json();
        let value = serde_json::from_str::<serde_json::Value>(&json).unwrap();

        assert_eq!(value["type"], "connected");
        assert_eq!(value["revision"], manifest.revision().as_str());
        assert_eq!(value["page_availability"], "present");
        assert_eq!(value.as_object().unwrap().len(), 3);
    }

    #[test]
    fn status_message_counts_severities() {
        let diagnostics = [
            tola_build::diagnostic::Diagnostic::new(
                tola_build::codes::typst::COMPILE,
                tola_build::diagnostic::Severity::Error,
                "unclosed delimiter",
            ),
            tola_build::diagnostic::Diagnostic::new(
                tola_build::codes::typst::COMPILE,
                tola_build::diagnostic::Severity::Warning,
                "layout was ignored during HTML export",
            ),
            tola_build::diagnostic::Diagnostic::new(
                tola_build::codes::typst::COMPILE,
                tola_build::diagnostic::Severity::Error,
                "unknown variable",
            ),
        ];

        let json = HotReloadMessage::status(true, DiagnosticCounts::of(&diagnostics)).to_json();

        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json).unwrap(),
            serde_json::json!({"type": "status", "rebuilding": true, "errors": 2, "warnings": 1})
        );
    }
}
