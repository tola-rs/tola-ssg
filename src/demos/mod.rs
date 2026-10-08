//! Complete bundled sites and tutorials, shared by help, preview, export, and verification.

use std::path::Path;

use anyhow::{Result, bail};

use crate::i18n::HelpLanguage;
use crate::tree::{EntryKind, Tree};

pub(crate) mod export;
pub(crate) mod preview;

pub(crate) struct DemoFile {
    pub path: &'static str,
    pub bytes: &'static [u8],
}

pub(crate) struct Demo {
    pub id: &'static str,
    pub title: &'static str,
    pub title_zh: &'static str,
    pub summary: &'static str,
    pub summary_zh: &'static str,
    files: &'static [DemoFile],
    directories: &'static [&'static str],
    guide: &'static str,
    guide_zh: &'static str,
    packages: &'static [(&'static str, &'static [&'static str])],
    tables: &'static [&'static str],
}

include!(concat!(env!("OUT_DIR"), "/demo_sites.rs"));

static DEMOS: &[Demo] = &[
    Demo {
        id: "sources",
        title: "From sources to selected pages",
        title_zh: "从内容源到选中的页面",
        summary: "Validate metadata, choose routes, and reuse one ordered page selection.",
        summary_zh: "校验 metadata、选择路由，并复用同一份有序页面列表。",
        files: SOURCES_FILES,
        directories: SOURCES_DIRECTORIES,
        guide: include_str!("guides/sources.md"),
        guide_zh: include_str!("../i18n/zh-Hans/demos/sources.md"),
        packages: &[
            ("source", &["all-sources", "tola-meta", "parse-sources"]),
            (
                "schema",
                &["schema", "optional", "nullable", "trim", "non-empty"],
            ),
            ("address", &["route", "route-to-output", "decode-url-path"]),
        ],
        tables: &["build", "site"],
    },
    Demo {
        id: "toc",
        title: "A table of contents for each document",
        title_zh: "每篇文档自己的目录",
        summary: "Query native headings, respect outline visibility, and link their actual locations.",
        summary_zh: "查询原生标题、遵循 outline 可见性，并链接到它们的真实 location。",
        files: TOC_FILES,
        directories: TOC_DIRECTORIES,
        guide: include_str!("guides/toc.md"),
        guide_zh: include_str!("../i18n/zh-Hans/demos/toc.md"),
        packages: &[("document", &["headings", "current-document"])],
        tables: &[],
    },
    Demo {
        id: "backlinks",
        title: "Incoming links from page bodies",
        title_zh: "来自页面正文的反向链接",
        summary: "Select body references and choose one incoming row per page, outside the queried region.",
        summary_zh: "选取正文引用，按来源页去重，并把反向链接列表放在查询区域之外。",
        files: BACKLINKS_FILES,
        directories: BACKLINKS_DIRECTORIES,
        guide: include_str!("guides/backlinks.md"),
        guide_zh: include_str!("../i18n/zh-Hans/demos/backlinks.md"),
        packages: &[("document", &["references", "current-document"])],
        tables: &["build.references"],
    },
    Demo {
        id: "media",
        title: "Local images and icons",
        title_zh: "本地图片与图标",
        summary: "Connect configured assets and SVG collections; inspect and resize an image input.",
        summary_zh: "连接配置的资源与 SVG 集合，读取图片信息并生成缩放版本。",
        files: MEDIA_FILES,
        directories: MEDIA_DIRECTORIES,
        guide: include_str!("guides/media.md"),
        guide_zh: include_str!("../i18n/zh-Hans/demos/media.md"),
        packages: &[
            ("image", &["image-metadata", "resize-image"]),
            ("icon", &["icon", "icon-url"]),
            ("address", &["asset-url"]),
        ],
        tables: &["assets", "icons", "typst.fonts"],
    },
    Demo {
        id: "feeds",
        title: "Four ways to supply a feed body",
        title_zh: "给 feed 正文的四种方式",
        summary: "Compare a summary-only entry, portable content, the whole document body, and a selected HTML subtree.",
        summary_zh: "比较仅摘要、可移植内容、整篇文档正文和选定的 HTML 子树。",
        files: FEEDS_FILES,
        directories: FEEDS_DIRECTORIES,
        guide: include_str!("guides/feeds.md"),
        guide_zh: include_str!("../i18n/zh-Hans/demos/feeds.md"),
        packages: &[("web", &["feed"]), ("source", &["tola-meta"])],
        tables: &["site", "assets"],
    },
    Demo {
        id: "multiple-outputs",
        title: "One input, several outputs",
        title_zh: "同一份输入，多种输出",
        summary: "Reuse a JSON source for two HTML layouts and an explicitly generated download.",
        summary_zh: "复用一份 JSON 输入，生成两套 HTML 页面和明确声明的下载文件。",
        files: MULTIPLE_OUTPUTS_FILES,
        directories: MULTIPLE_OUTPUTS_DIRECTORIES,
        guide: include_str!("guides/multiple-outputs.md"),
        guide_zh: include_str!("../i18n/zh-Hans/demos/multiple-outputs.md"),
        packages: &[
            ("source", &["all-sources", "current-source"]),
            ("document", &["current-document"]),
            ("address", &["output-to-url"]),
        ],
        tables: &["build", "assets"],
    },
];

pub(crate) fn all() -> &'static [Demo] {
    DEMOS
}

pub(crate) fn find(id: &str) -> Option<&'static Demo> {
    all().iter().find(|demo| demo.id == id)
}

impl Demo {
    pub(crate) fn files(&self) -> &'static [DemoFile] {
        self.files
    }

    pub(crate) fn directories(&self) -> &'static [&'static str] {
        self.directories
    }

    pub(crate) fn file(&self, path: &str) -> Option<&'static DemoFile> {
        self.files.iter().find(|file| file.path == path)
    }

    pub(crate) fn title(&self, language: HelpLanguage) -> &'static str {
        match language {
            HelpLanguage::English => self.title,
            HelpLanguage::SimplifiedChinese => self.title_zh,
        }
    }

    pub(crate) fn summary(&self, language: HelpLanguage) -> &'static str {
        match language {
            HelpLanguage::English => self.summary,
            HelpLanguage::SimplifiedChinese => self.summary_zh,
        }
    }

    pub(crate) fn landing_output(&self) -> &'static str {
        "index.html"
    }

    pub(crate) fn related_package(&self, name: &str, exports: &[String]) -> bool {
        let name = name.strip_prefix("@tola/").unwrap_or(name);
        self.packages.iter().any(|(package, members)| {
            *package == name
                && (exports.is_empty()
                    || exports
                        .iter()
                        .any(|export| members.contains(&export.as_str())))
        })
    }

    pub(crate) fn related_table(&self, section: &str) -> bool {
        self.tables.contains(&section)
    }

    pub(crate) fn documentation(&self, language: HelpLanguage) -> Result<String> {
        let heading = match language {
            HelpLanguage::English => "Site files",
            HelpLanguage::SimplifiedChinese => "站点文件",
        };
        let mut document = format!(
            "# {}\n\n{}\n\n## {heading}\n\n```text\n",
            self.title(language),
            self.summary(language)
        );
        let mut tree = Tree::default();
        for directory in self.directories {
            tree.insert(Path::new(directory), EntryKind::Directory);
        }
        for file in self.files {
            tree.insert(Path::new(file.path), EntryKind::File);
        }
        document.push_str(&tree.render());
        document.push_str("\n```\n\n");
        let guide = match language {
            HelpLanguage::English => self.guide,
            HelpLanguage::SimplifiedChinese => self.guide_zh,
        };
        for line in guide.lines() {
            if let Some(path) = line
                .strip_prefix("{{file:")
                .and_then(|path| path.strip_suffix("}}"))
            {
                let Some(file) = self.file(path) else {
                    bail!("demo `{}` has no source file `{path}`", self.id);
                };
                let Ok(source) = std::str::from_utf8(file.bytes) else {
                    bail!("demo source `{path}` is not a text file");
                };
                let maximum = source
                    .split(|character| character != '`')
                    .map(str::len)
                    .max()
                    .unwrap_or(0);
                let fence = "`".repeat(3.max(maximum + 1));
                let language = match path.rsplit('.').next() {
                    Some("typ") => "typst",
                    Some("toml") => "toml",
                    Some("css") => "css",
                    Some("svg") => "xml",
                    Some("json") => "json",
                    _ => "text",
                };
                document.push_str(&format!("### `{path}`\n\n{fence}{language}\n{source}"));
                if !source.ends_with('\n') {
                    document.push('\n');
                }
                document.push_str(&format!("{fence}\n\n"));
            } else {
                document.push_str(line);
                document.push('\n');
            }
        }
        Ok(document)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tempfile::TempDir;
    use tola_build::build::{BuildMode, BuildRequest, SiteBuildGuard};
    use tola_build::output::graph::OutputKind;
    use tola_build::{BuildResources, BuildSession, InputScope};

    use super::*;

    fn demo_build(id: &str) -> (TempDir, tola_build::build::SiteBuild) {
        let directory = TempDir::new().unwrap();
        let demo = find(id).unwrap();
        let root =
            export::write(demo, &directory.path().join("site"), &Default::default()).unwrap();
        let config = crate::config::load(
            Some(&root.join("tola.toml")),
            InputScope::Pure,
            tola_typst::PackageLocations::from_absolute_roots(None, None).unwrap(),
            &crate::config::ConfigOverrides::default(),
        )
        .unwrap()
        .into_config();
        let resources = BuildResources::new().with_input_scope(InputScope::Pure);
        let mut session = BuildSession::with_resources(resources);
        let built = SiteBuildGuard::build(
            &mut session,
            Arc::new(config),
            BuildRequest::new(BuildMode::Production),
            || {},
        )
        .unwrap_or_else(|failure| panic!("demo `{id}` failed: {}", failure.into_error()))
        .release();
        (directory, built)
    }

    fn output<'a>(
        build: &'a tola_build::build::SiteBuild,
        path: &str,
    ) -> &'a tola_build::output::graph::OutputFile {
        build
            .graph()
            .outputs()
            .iter()
            .find(|output| output.path().as_str() == path)
            .unwrap_or_else(|| panic!("demo output `{path}` is missing"))
    }

    fn html(build: &tola_build::build::SiteBuild, path: &str) -> String {
        String::from_utf8(output(build, path).bytes().to_vec()).unwrap()
    }

    #[test]
    fn bundled_sites_build_pure() {
        for demo in all() {
            let (_directory, build) = demo_build(demo.id);
            assert_eq!(
                output(&build, demo.landing_output()).kind(),
                OutputKind::HtmlDocument
            );
            assert_eq!(output(&build, "404.html").kind(), OutputKind::HtmlDocument);
            assert!(
                build.diagnostics().is_empty(),
                "{}: {:?}",
                demo.id,
                build.diagnostics()
            );
        }
    }

    #[test]
    fn tutorials_embed_the_exported_sources() {
        for demo in all() {
            let directory = TempDir::new().unwrap();
            let root =
                export::write(demo, &directory.path().join("site"), &Default::default()).unwrap();
            for file in demo.files() {
                assert_eq!(
                    std::fs::read(root.join(file.path)).unwrap(),
                    file.bytes,
                    "{} / {}",
                    demo.id,
                    file.path
                );
            }
            for language in [HelpLanguage::English, HelpLanguage::SimplifiedChinese] {
                let guide = demo.documentation(language).unwrap();
                assert!(!guide.contains("{{file:"));
                let original = match language {
                    HelpLanguage::English => demo.guide,
                    HelpLanguage::SimplifiedChinese => demo.guide_zh,
                };
                for line in original.lines() {
                    if let Some(path) = line
                        .strip_prefix("{{file:")
                        .and_then(|path| path.strip_suffix("}}"))
                    {
                        let source = std::str::from_utf8(demo.file(path).unwrap().bytes).unwrap();
                        assert!(guide.contains(source), "{} / {path}", demo.id);
                    }
                }
            }
        }
    }

    #[test]
    fn selected_pages_preserve_declared_metadata() {
        let (_directory, build) = demo_build("sources");
        assert!(
            build
                .graph()
                .outputs()
                .iter()
                .all(|output| output.path().as_str() != "draft/index.html")
        );
        assert_eq!(
            output(&build, "chosen/index.html").kind(),
            OutputKind::HtmlDocument
        );
        let declarations: serde_json::Value =
            serde_json::from_slice(output(&build, "source-metadata.json").bytes()).unwrap();
        let start = declarations
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["path"] == "start.typ")
            .unwrap();
        assert_eq!(start["title"], "  Getting started  ");
        let start = build
            .index()
            .html_pages()
            .find(|(page, _)| page.output.as_str() == "start/index.html")
            .unwrap()
            .0;
        assert_eq!(start.properties.title.as_deref(), Some("Getting started"));
        let inventory = build
            .index()
            .html_pages()
            .find(|(page, _)| page.output.as_str() == "index.html")
            .unwrap()
            .1;
        let destinations = inventory
            .references()
            .iter()
            .filter(|reference| {
                reference
                    .span()
                    .id()
                    .is_some_and(|id| id.vpath().get_without_slash() == "site/navigation.typ")
            })
            .map(|reference| reference.destination())
            .collect::<Vec<_>>();
        assert_eq!(destinations, ["/demo/", "/demo/chosen/", "/demo/start/"]);
    }

    #[test]
    fn contents_use_document_headings() {
        let (_directory, build) = demo_build("toc");
        for (path, expected) in [("index.html", 2), ("other/index.html", 1)] {
            let inventory = build
                .index()
                .html_pages()
                .find(|(page, _)| page.output.as_str() == path)
                .unwrap()
                .1;
            let links = inventory
                .references()
                .iter()
                .filter(|reference| {
                    reference
                        .span()
                        .id()
                        .is_some_and(|id| id.vpath().get_without_slash() == "site/toc.typ")
                })
                .collect::<Vec<_>>();
            assert_eq!(links.len(), expected, "{path}");
            for link in links {
                let fragment = link.destination().split_once('#').unwrap().1;
                assert!(
                    inventory
                        .fragments()
                        .iter()
                        .any(|target| target.value() == fragment),
                    "{} / {fragment}",
                    path
                );
            }
        }
    }

    #[test]
    fn backlinks_choose_one_row_per_page() {
        let (_directory, build) = demo_build("backlinks");
        let inventory = build
            .index()
            .html_pages()
            .find(|(page, _)| page.output.as_str() == "topic/index.html")
            .unwrap()
            .1;
        let links = inventory
            .references()
            .iter()
            .filter(|reference| {
                reference
                    .span()
                    .id()
                    .is_some_and(|id| id.vpath().get_without_slash() == "site/backlinks.typ")
            })
            .map(|reference| reference.destination())
            .collect::<Vec<_>>();
        let destinations = links
            .iter()
            .map(|link| {
                let destination = url::Url::parse("https://example.test/demo/topic/")
                    .unwrap()
                    .join(link)
                    .unwrap();
                let browser_path = tola_address::UrlPath::parse(destination.path()).unwrap();
                let mount = tola_address::SiteUrlMount::from_base_path("/demo/").unwrap();
                let route = mount.strip(&browser_path).unwrap();
                tola_address::OutputPath::from_route(&route)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            destinations,
            [
                tola_address::OutputPath::parse("a/index.html").unwrap(),
                tola_address::OutputPath::parse("b/index.html").unwrap(),
            ]
        );
    }

    #[test]
    fn resized_images_preserve_input_dimensions() {
        let (_directory, build) = demo_build("media");
        let png_dimensions = |bytes: &[u8]| {
            assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
            (
                u32::from_be_bytes(bytes[16..20].try_into().unwrap()),
                u32::from_be_bytes(bytes[20..24].try_into().unwrap()),
            )
        };
        assert_eq!(
            png_dimensions(output(&build, "assets/stripes.png").bytes()),
            (128, 64)
        );
        let derived = build
            .graph()
            .outputs()
            .iter()
            .filter(|output| output.path().as_str().starts_with("_tola/images/"))
            .collect::<Vec<_>>();
        assert_eq!(derived.len(), 1);
        assert_eq!(png_dimensions(derived[0].bytes()), (48, 24));
        assert!(
            build
                .graph()
                .outputs()
                .iter()
                .all(|output| !output.path().as_str().contains("image-input"))
        );
        assert!(
            build
                .graph()
                .outputs()
                .iter()
                .any(|output| output.kind() == OutputKind::Asset
                    && output.declaration().media_type().as_str() == "image/svg+xml")
        );
        assert!(html(&build, "index.html").contains("128 by 64"));
    }

    #[test]
    fn feed_bodies_follow_their_selection() {
        let (_directory, build) = demo_build("feeds");
        let read = |path| rss::Channel::read_from(output(&build, path).bytes()).unwrap();
        let summary = read("summary.xml");
        let portable = read("portable.xml");
        let whole = read("whole.xml");
        let selected = read("selected.xml");
        for channel in [&summary, &portable, &whole, &selected] {
            let entry = &channel.items()[0];
            assert_eq!(entry.guid().unwrap().value(), "urn:tola-demo:field-note");
            assert!(
                entry
                    .description()
                    .unwrap()
                    .contains("<strong>short</strong>")
            );
        }
        assert!(summary.items()[0].content().is_none());
        let portable = portable.items()[0].content().unwrap();
        assert!(portable.contains("<strong>body</strong>"));
        assert!(portable.contains("https://typst.app/"));
        let whole = whole.items()[0].content().unwrap();
        let selected = selected.items()[0].content().unwrap();
        assert!(whole.contains("Outside the article"));
        assert!(!selected.contains("Outside the article"));
        assert!(!selected.contains("Feed choices"));
        assert!(selected.contains("Inside the article"));
        assert!(selected.contains("https://example.test/demo/assets/stripes.png"));
    }

    #[test]
    fn generated_download_selects_source_fields() {
        let (_directory, build) = demo_build("multiple-outputs");
        let chapters: serde_json::Value =
            serde_json::from_slice(output(&build, "chapters.json").bytes()).unwrap();
        let chapters = chapters.as_array().unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0]["title"], "Start");
        assert_eq!(chapters[1]["title"], "Write");
        assert!(chapters.iter().all(|chapter| chapter.get("text").is_none()));
        assert!(html(&build, "index.html").contains("Choose the page structure."));
        assert!(html(&build, "table/index.html").contains("Reuse the same source values."));
        assert!(
            build
                .graph()
                .outputs()
                .iter()
                .all(|output| output.path().as_str() != "static/data/chapters.json")
        );
    }

    #[test]
    fn shared_source_reports_containing_documents() {
        let (_directory, build) = demo_build("multiple-outputs");
        let mut shared = None;
        for path in ["index.html", "table/index.html"] {
            let rendered = html(&build, path);
            let words = rendered
                .split('>')
                .skip(1)
                .flat_map(|node| node.split('<').next().unwrap().split_whitespace())
                .collect::<Vec<_>>();
            assert!(
                words
                    .windows(2)
                    .any(|words| words == ["Source:", "index.typ"])
            );
            assert!(words.windows(2).any(|words| words == ["document:", path]));
            let page = build
                .index()
                .html_pages()
                .find(|(page, _)| page.output.as_str() == path)
                .unwrap()
                .0;
            let source = *page
                .sources
                .iter()
                .find(|id| id.vpath().get_without_slash() == "content/index.typ")
                .unwrap();
            if let Some(previous) = shared {
                assert_eq!(source, previous);
            }
            shared = Some(source);
        }
    }
}
