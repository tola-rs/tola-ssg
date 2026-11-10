//! Complete icon outputs derived from replayable native request observations.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;
use tola_address::OutputPath;
use tola_packages::{PublishedIcon, published_icon_path};
use tola_typst::ReadEvidence;

use crate::cancellation::BuildCancellation;
use crate::output::graph::OutputGraphBuilder;
use crate::output::semantics::{OutputDeclaration, ResponseMediaType};

/// Every icon a document asked to publish, with the bytes it serves.
#[derive(Default)]
pub(crate) struct IconOutputs {
    published: BTreeMap<OutputPath, Arc<[u8]>>,
}

impl IconOutputs {
    /// The icons one compilation published, with the bytes each serves.
    pub(crate) fn prepare<'a>(
        evidence: impl IntoIterator<Item = &'a ReadEvidence>,
        collections: &tola_icons::IconCollections,
        cancellation: &BuildCancellation,
    ) -> Result<Arc<Self>> {
        let mut published = BTreeMap::new();
        for read in evidence {
            cancellation.ensure_active()?;
            let Some(request) = PublishedIcon::from_read(read.locator()) else {
                continue;
            };
            let icon = collections
                .get(request.namespace(), request.name())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "the icon `{}:{}` is not in the site's icon collections; run `tola vendor --refresh`",
                        request.namespace(),
                        request.name()
                    )
                })?;
            let bytes = icon.svg().as_bytes();
            published.insert(published_icon_path(bytes), Arc::from(bytes));
        }
        cancellation.ensure_active()?;
        Ok(Arc::new(Self { published }))
    }

    /// Declare every collected icon as a published SVG file.
    pub(crate) fn insert_into(
        &self,
        outputs: &mut OutputGraphBuilder,
        cancellation: &BuildCancellation,
    ) -> Result<()> {
        for (path, bytes) in &self.published {
            cancellation.ensure_active()?;
            outputs.insert_system(
                "icon",
                path.as_str(),
                OutputDeclaration::image(ResponseMediaType::SVG),
                Arc::clone(bytes),
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::build::BuildMode;
    use crate::config::tests::OwnedSiteConfig;

    const MARK: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="currentColor" d="M0 0h24v24H0z"/></svg>"#;
    const DOT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="20" cy="20" r="2" fill="red"/></svg>"#;

    fn config_with_icons(program: &str) -> OwnedSiteConfig {
        let site_config = OwnedSiteConfig::new(
            r#"
            [icons.collections.brand]
            source-type = "local-svg-dir"
            path = "icons"
            [icons.collections.copy]
            source-type = "local-svg-dir"
            path = "icons"
            "#,
        );
        let root = site_config.config.get_root();
        fs::create_dir_all(root.join("icons")).unwrap();
        fs::write(root.join("icons/mark.svg"), MARK).unwrap();
        fs::write(root.join("icons/dot.svg"), DOT).unwrap();
        fs::create_dir_all(&site_config.config.build.content_dir).unwrap();
        fs::write(&site_config.config.build.entry, program).unwrap();
        site_config
    }

    fn output(build: &crate::build::SiteBuild, path: &str) -> Vec<u8> {
        build
            .graph()
            .outputs()
            .iter()
            .find(|output| output.path().as_str() == path)
            .unwrap_or_else(|| panic!("`{path}` is published"))
            .bytes()
            .to_vec()
    }

    #[test]
    fn only_published_icons_are_outputs() {
        let site_config = config_with_icons(
            r#"#import "@tola/icon:0.0.0": icon-url
#document("index.html")[#html.img(src: icon-url("brand:mark"), alt: "Mark")
#html.img(src: icon-url("copy:mark"), alt: "Mark again")]"#,
        );
        let build = crate::build::build_site(&site_config.config, BuildMode::Production).unwrap();
        // `mark` is requested twice under two namespaces and serves identical bytes, so the
        // site publishes one file; the configured `dot` is never requested and stays unpublished.
        let published = build
            .graph()
            .outputs()
            .iter()
            .filter(|output| output.path().as_str().starts_with("_tola/icons/"))
            .collect::<Vec<_>>();
        let [icon] = published.as_slice() else {
            panic!("expected one published icon: {published:#?}")
        };
        assert_eq!(icon.path(), &published_icon_path(icon.bytes()));
        assert!(
            std::str::from_utf8(icon.bytes()).unwrap().contains("<path"),
            "the published bytes are the requested icon"
        );
        let html = std::str::from_utf8(&output(&build, "index.html"))
            .unwrap()
            .to_owned();
        assert_eq!(
            html.matches(icon.path().as_str()).count(),
            2,
            "both requests answer the published URL: {html}"
        );
    }

    #[test]
    fn unpublished_icons_write_nothing() {
        let site_config = config_with_icons(
            r#"#import "@tola/icon:0.0.0": icon
#document("index.html")[#icon("brand:dot")]"#,
        );
        let build = crate::build::build_site(&site_config.config, BuildMode::Production).unwrap();
        assert!(
            !build
                .graph()
                .outputs()
                .iter()
                .any(|output| output.path().as_str().starts_with("_tola/icons/")),
            "an inline icon is not a published file"
        );
    }

    fn compilation_diagnostics(site_config: &OwnedSiteConfig) -> tola_typst::Diagnostics {
        let error = crate::build::build_site(&site_config.config, BuildMode::Production)
            .err()
            .expect("an invalid icon prevents compilation");
        anyhow::Error::new(error)
            .chain()
            .find_map(|cause| cause.downcast_ref::<tola_typst::CompileError>())
            .and_then(tola_typst::CompileError::diagnostics)
            .expect("the build retains Typst diagnostics")
            .clone()
    }

    #[test]
    fn missing_icon_keeps_helper_callers() {
        let site_config = config_with_icons(
            "#import \"preferences.typ\": preferences\n#document(\"index.html\")[#preferences()]",
        );
        let root = site_config.config.get_root();
        fs::write(root.join("helper.typ"), "#import \"@tola/icon:0.0.0\": icon as render-icon\n#let icon(name) = render-icon(\"brand:\" + name)").unwrap();
        fs::write(
            root.join("preferences.typ"),
            "#import \"helper.typ\": icon\n#let preferences() = [#icon(\"mrk\")]",
        )
        .unwrap();
        let diagnostics = compilation_diagnostics(&site_config);
        let diagnostic = diagnostics.errors().next().unwrap();
        assert_eq!(diagnostic.location.path.as_deref(), Some("helper.typ"));
        assert_eq!(diagnostic.location.line, Some(2));
        assert!(
            diagnostic
                .traces
                .iter()
                .any(
                    |trace| trace.location.path.as_deref() == Some("preferences.typ")
                        && trace.location.line == Some(2)
                )
        );
        assert!(
            diagnostic
                .hints
                .iter()
                .any(|hint| hint.message.contains("brand:mark"))
        );
        let raw = diagnostics
            .raw()
            .iter()
            .find(|native| native.source().message.contains("brand:mrk"))
            .unwrap();
        assert!(
            matches!(raw.source().span.id().unwrap().root(), typst::syntax::VirtualRoot::Package(package) if package == &tola_packages::TolaPackage::Icon.spec())
        );
        assert!(raw.source().trace.iter().any(|trace| matches!(
            trace.span.id().unwrap().root(),
            typst::syntax::VirtualRoot::Project
        )));
    }

    #[test]
    fn unused_icons_validate_ids() {
        for (id, message, suggestion) in [
            ("brand", "not a valid icon id", false),
            ("absent:mark", "not declared in `icons.collections`", false),
            ("brand:mrk", "not in collection `brand`", true),
            ("brand:palette", "not in collection `brand`", false),
        ] {
            let mut site_config = config_with_icons(&format!(
                "#import \"@tola/icon:0.0.0\": icon\n#let unused = icon(\"{id}\")\n#document(\"index.html\")[Body]"
            ));
            let other = site_config.config.get_root().join("other-icons");
            fs::create_dir(&other).unwrap();
            fs::write(other.join("palette.svg"), MARK).unwrap();
            site_config.config.icons.collections.insert(
                "copy".into(),
                crate::config::section::IconCollectionSource::LocalSvgDir { path: other },
            );
            let diagnostics = compilation_diagnostics(&site_config);
            let diagnostic = diagnostics.errors().next().unwrap();
            assert!(
                diagnostic.message.contains(message),
                "{}",
                diagnostic.message
            );
            assert_eq!(diagnostic.location.path.as_deref(), Some("site.typ"));
            assert_eq!(diagnostic.location.line, Some(2));
            assert_eq!(
                diagnostic
                    .hints
                    .iter()
                    .any(|hint| hint.message.contains("Did you mean")),
                suggestion
            );
        }
    }

    #[test]
    fn alias_names_remain_callable() {
        let mut site_config = config_with_icons(
            "#import \"@tola/icon:0.0.0\": icon, icon-bytes\n#let alias = icon-bytes(\"brand:alias\")\n#assert.eq(alias, icon-bytes(\"brand:e001\"))\n#let unused = icon(\"brand:alais\")\n#document(\"index.html\")[Body]",
        );
        let path = site_config.config.get_root().join("icons.json");
        fs::write(&path, r#"{"prefix":"other","icons":{"palette":{"body":"<path/>"}},"aliases":{"alias":{"parent":"palette"}},"chars":{"e001":"alias"}}"#).unwrap();
        site_config.config.icons.collections.insert(
            "brand".into(),
            crate::config::section::IconCollectionSource::LocalJson { path },
        );
        let diagnostics = compilation_diagnostics(&site_config);
        let diagnostic = diagnostics.errors().next().unwrap();
        assert!(
            diagnostic.message.contains("brand:alais"),
            "{}",
            diagnostic.message
        );
        assert!(
            diagnostic
                .hints
                .iter()
                .any(|hint| hint.message.contains("brand:alias"))
        );
    }
}
