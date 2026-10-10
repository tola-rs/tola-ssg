//! CLI requests and help URLs locate the same pages.

use super::model::Anchor;

pub(crate) const CROSS_REFERENCE_SCHEME: &str = "tola-help://";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HelpCategory {
    Config,
    Packages,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PageId {
    Overview,
    Index(HelpCategory),
    Config { section: String },
    Package { name: String },
    PackageSelection { name: String, exports: Vec<String> },
}

impl PageId {
    pub(crate) fn uri(&self) -> String {
        let host = match self {
            Self::Overview => "overview",
            Self::Index(HelpCategory::Config) | Self::Config { .. } => "config",
            Self::Index(HelpCategory::Packages)
            | Self::Package { .. }
            | Self::PackageSelection { .. } => "packages",
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
        [package, exports @ ..] if package.starts_with("@tola/") => {
            let name = package.strip_prefix("@tola/")?;
            if !identifier(name) {
                return None;
            }
            if exports.is_empty() {
                PageId::Package {
                    name: name.to_owned(),
                }
            } else if exports.iter().all(|name| identifier(name)) {
                PageId::PackageSelection {
                    name: name.to_owned(),
                    exports: exports.to_vec(),
                }
            } else {
                return None;
            }
        }
        [target] => {
            if let Some(section) = target
                .strip_prefix('[')
                .and_then(|section| section.strip_suffix(']'))
            {
                if !section.split('.').all(identifier) {
                    return None;
                }
                PageId::Config {
                    section: section.to_owned(),
                }
            } else {
                match target.as_str() {
                    "config" => PageId::Index(HelpCategory::Config),
                    "package" => PageId::Index(HelpCategory::Packages),
                    _ => return None,
                }
            }
        }
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

fn fragment_anchor(fragment: &str) -> Option<Anchor> {
    if fragment.is_empty() {
        return None;
    }
    let reference = tola_address::SiteReference::parse(&format!("/#{}", fragment)).ok()?;
    Some(Anchor::from_fragment(
        reference.decoded_fragment()?.into_owned(),
    ))
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
            (
                vec!["[build.hooks]"],
                PageId::Config {
                    section: "build.hooks".into(),
                },
            ),
            (
                vec!["@tola/document"],
                PageId::Package {
                    name: "document".into(),
                },
            ),
            (
                vec!["@tola/document", "headings"],
                PageId::PackageSelection {
                    name: "document".into(),
                    exports: vec!["headings".into()],
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
                exports: vec!["slugify".into(), "url-of".into()],
            },
        ] {
            let target = LinkTarget::Page(page.clone());
            assert_eq!(LinkTarget::from_uri(&target.uri(), &current), Some(target));
            let target = LinkTarget::PageAnchor(page, anchor("Details 名称"));
            assert_eq!(LinkTarget::from_uri(&target.uri(), &current), Some(target));
        }
    }
}
