//! CLI requests and help URLs locate the same pages; neither starts a demo operation.

use super::model::Anchor;

pub(crate) const CROSS_REFERENCE_SCHEME: &str = "tola-help://";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HelpCategory {
    Config,
    Packages,
    Demos,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PageId {
    Overview,
    Index(HelpCategory),
    Config { section: String },
    Package { name: String },
    PackageSelection { name: String, exports: Vec<String> },
    Demo { id: String },
    DemoFile { id: String, path: String },
}

impl PageId {
    pub(crate) fn selector(&self) -> Option<String> {
        match self {
            Self::Overview => None,
            Self::Index(HelpCategory::Config) => Some("config".into()),
            Self::Index(HelpCategory::Packages) => Some("package".into()),
            Self::Index(HelpCategory::Demos) => Some("demo".into()),
            Self::Config { section } => Some(format!("config {section}")),
            Self::Package { name } => Some(format!("package {name}")),
            Self::PackageSelection { name, exports } => {
                Some(format!("package {name} {}", exports.join(" ")))
            }
            Self::Demo { id } => Some(format!("demo {id}")),
            Self::DemoFile { id, path } => Some(format!("demo {id} {}", shell_argument(path))),
        }
    }

    pub(crate) fn demo(&self) -> Option<&str> {
        match self {
            Self::Demo { id } | Self::DemoFile { id, .. } => Some(id),
            _ => None,
        }
    }

    pub(crate) fn uri(&self) -> String {
        let host = match self {
            Self::Overview => "overview",
            Self::Index(HelpCategory::Config) | Self::Config { .. } => "config",
            Self::Index(HelpCategory::Packages)
            | Self::Package { .. }
            | Self::PackageSelection { .. } => "packages",
            _ => "demos",
        };
        let mut uri = url::Url::parse(&format!("{CROSS_REFERENCE_SCHEME}{host}"))
            .expect("help categories are valid URL hosts");
        let segments: Vec<&str> = match self {
            Self::Overview | Self::Index(_) => Vec::new(),
            Self::Config { section } => section.split('.').collect(),
            Self::Package { name } => vec![name],
            Self::PackageSelection { name, exports } if exports.len() == 1 => {
                vec![name, &exports[0]]
            }
            Self::PackageSelection { name, .. } => vec![name],
            Self::Demo { id } => vec![id],
            Self::DemoFile { id, path } => std::iter::once(id.as_str())
                .chain(std::iter::once("files"))
                .chain(path.split('/'))
                .collect(),
        };
        if !segments.is_empty() {
            uri.path_segments_mut()
                .expect("help URLs are hierarchical")
                .extend(segments);
        }
        if let Self::PackageSelection { exports, .. } = self
            && exports.len() > 1
        {
            uri.set_query(Some(&format!("exports={}", exports.join(","))));
        }
        uri.into()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LinkTarget {
    Page(PageId),
    PageAnchor(PageId, Anchor),
    External(String),
}

impl LinkTarget {
    pub(crate) fn page(&self) -> Option<&PageId> {
        match self {
            Self::Page(page) | Self::PageAnchor(page, _) => Some(page),
            Self::External(_) => None,
        }
    }

    pub(crate) fn uri(&self) -> String {
        match self {
            Self::Page(page) => page.uri(),
            Self::PageAnchor(page, anchor) => {
                let mut uri = url::Url::parse(&page.uri()).expect("a page produces a valid URL");
                uri.set_fragment(Some(anchor.as_str()));
                uri.into()
            }
            Self::External(uri) => uri.clone(),
        }
    }

    pub(crate) fn from_uri(uri: &str, current: &PageId) -> Option<Self> {
        if let Some(fragment) = uri.strip_prefix('#') {
            return fragment_anchor(fragment)
                .map(|anchor| Self::PageAnchor(current.clone(), anchor));
        }
        if !uri.starts_with(CROSS_REFERENCE_SCHEME) {
            return Some(Self::External(uri.to_owned()));
        }
        let uri = url::Url::parse(uri).ok()?;
        if !uri.username().is_empty() || uri.password().is_some() || uri.port().is_some() {
            return None;
        }
        let path = if uri.path().is_empty() || uri.path() == "/" {
            String::new()
        } else {
            tola_address::UrlPath::parse(uri.path())
                .ok()?
                .as_str()
                .trim_start_matches('/')
                .to_owned()
        };
        let segments = if path.is_empty() {
            Vec::new()
        } else {
            path.split('/').collect::<Vec<_>>()
        };
        let query = uri.query_pairs().collect::<Vec<_>>();
        let page = match (uri.host_str()?, segments.as_slice()) {
            ("overview", []) if query.is_empty() => PageId::Overview,
            ("config", []) if query.is_empty() => PageId::Index(HelpCategory::Config),
            ("config", segments)
                if query.is_empty() && segments.iter().all(|segment| identifier(segment)) =>
            {
                PageId::Config {
                    section: segments.join("."),
                }
            }
            ("packages", []) if query.is_empty() => PageId::Index(HelpCategory::Packages),
            ("packages", [name]) if identifier(name) && query.is_empty() => PageId::Package {
                name: (*name).into(),
            },
            ("packages", [name, export])
                if identifier(name) && identifier(export) && query.is_empty() =>
            {
                PageId::PackageSelection {
                    name: (*name).into(),
                    exports: vec![(*export).into()],
                }
            }
            ("packages", [name])
                if identifier(name) && query.len() == 1 && query[0].0 == "exports" =>
            {
                let exports = query[0].1.split(',').map(str::to_owned).collect::<Vec<_>>();
                if !exports.iter().all(|name| identifier(name)) {
                    return None;
                }
                PageId::PackageSelection {
                    name: (*name).into(),
                    exports,
                }
            }
            ("demos", []) if query.is_empty() => PageId::Index(HelpCategory::Demos),
            ("demos", [id]) if identifier(id) && query.is_empty() => {
                PageId::Demo { id: (*id).into() }
            }
            ("demos", [id, "files", path @ ..]) if identifier(id) && query.is_empty() => {
                PageId::DemoFile {
                    id: (*id).into(),
                    path: relative_path(&path.join("/"))?,
                }
            }
            _ => return None,
        };
        match uri.fragment() {
            Some(fragment) => {
                fragment_anchor(fragment).map(|anchor| Self::PageAnchor(page, anchor))
            }
            None => Some(Self::Page(page)),
        }
    }
}

pub(crate) fn request(targets: &[String]) -> Option<LinkTarget> {
    let page = match targets {
        [] => PageId::Overview,
        [uri] if uri.starts_with(CROSS_REFERENCE_SCHEME) => {
            return LinkTarget::from_uri(uri, &PageId::Overview);
        }
        [category] => match category.as_str() {
            "config" => PageId::Index(HelpCategory::Config),
            "package" => PageId::Index(HelpCategory::Packages),
            "demo" => PageId::Index(HelpCategory::Demos),
            _ => return None,
        },
        [category, section] if category == "config" && section.split('.').all(identifier) => {
            PageId::Config {
                section: section.clone(),
            }
        }
        [category, name, exports @ ..] if category == "package" && identifier(name) => {
            if exports.is_empty() {
                PageId::Package { name: name.clone() }
            } else if exports.iter().all(|name| identifier(name)) {
                PageId::PackageSelection {
                    name: name.clone(),
                    exports: exports.to_vec(),
                }
            } else {
                return None;
            }
        }
        [category, id] if category == "demo" && identifier(id) => PageId::Demo { id: id.clone() },
        [category, id, path] if category == "demo" && identifier(id) => PageId::DemoFile {
            id: id.clone(),
            path: relative_path(path)?,
        },
        _ => return None,
    };
    Some(LinkTarget::Page(page))
}

fn identifier(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn relative_path(path: &str) -> Option<String> {
    tola_address::OutputPath::parse(path)
        .ok()
        .map(|path| path.as_str().to_owned())
}

fn fragment_anchor(fragment: &str) -> Option<Anchor> {
    if fragment.is_empty() {
        return None;
    }
    let reference = tola_address::SiteReference::parse(&format!("/#{}", fragment)).ok()?;
    Some(Anchor::from_fragment(
        reference.decoded_fragment()?.into_owned(),
    ))
}

fn shell_argument(argument: &str) -> String {
    if argument
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'-' | b'_'))
    {
        argument.into()
    } else {
        format!("'{}'", argument.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::help::model::anchor;

    #[test]
    fn category_requests_locate_pages() {
        for (arguments, page) in [
            (vec![], PageId::Overview),
            (vec!["config"], PageId::Index(HelpCategory::Config)),
            (vec!["package"], PageId::Index(HelpCategory::Packages)),
            (vec!["demo"], PageId::Index(HelpCategory::Demos)),
            (
                vec!["config", "build.hooks"],
                PageId::Config {
                    section: "build.hooks".into(),
                },
            ),
            (
                vec!["package", "document", "headings"],
                PageId::PackageSelection {
                    name: "document".into(),
                    exports: vec!["headings".into()],
                },
            ),
            (
                vec!["demo", "backlinks", "site/page.typ"],
                PageId::DemoFile {
                    id: "backlinks".into(),
                    path: "site/page.typ".into(),
                },
            ),
        ] {
            let arguments = arguments.into_iter().map(str::to_owned).collect::<Vec<_>>();
            assert_eq!(request(&arguments), Some(LinkTarget::Page(page)));
        }
    }

    #[test]
    fn page_urls_preserve_identity() {
        let current = PageId::Overview;
        for page in [
            PageId::Overview,
            PageId::Index(HelpCategory::Config),
            PageId::Index(HelpCategory::Packages),
            PageId::Index(HelpCategory::Demos),
            PageId::Config {
                section: "build.hooks".into(),
            },
            PageId::Package {
                name: "document".into(),
            },
            PageId::PackageSelection {
                name: "document".into(),
                exports: vec!["headings".into()],
            },
            PageId::PackageSelection {
                name: "address".into(),
                exports: vec!["slugify".into(), "output-to-url".into()],
            },
            PageId::Demo {
                id: "backlinks".into(),
            },
            PageId::DemoFile {
                id: "backlinks".into(),
                path: "site/100% notes.typ".into(),
            },
        ] {
            let target = LinkTarget::Page(page.clone());
            assert_eq!(LinkTarget::from_uri(&target.uri(), &current), Some(target));
            let target = LinkTarget::PageAnchor(page, anchor("Details 名称"));
            assert_eq!(LinkTarget::from_uri(&target.uri(), &current), Some(target));
        }
    }
}
