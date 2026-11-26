//! Languages supported by `tola help`.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum HelpLanguage {
    #[default]
    English,
    SimplifiedChinese,
}

impl HelpLanguage {
    pub(crate) fn related(self) -> &'static str {
        match self {
            Self::English => "Related",
            Self::SimplifiedChinese => "相关条目",
        }
    }

    pub(crate) fn tag(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::SimplifiedChinese => "zh-Hans",
        }
    }

    pub(crate) fn colon(self) -> &'static str {
        match self {
            Self::English => ": ",
            Self::SimplifiedChinese => "：",
        }
    }

    pub(crate) fn parentheses(self) -> (&'static str, &'static str) {
        match self {
            Self::English => (" (", ")"),
            Self::SimplifiedChinese => ("（", "）"),
        }
    }

    /// Accept locale spellings such as `zh_CN.UTF-8` and `zh-Hans-CN` as well as `en` and `zh`.
    pub(crate) fn parse(tag: &str) -> Option<Self> {
        let tag = tag.trim();
        let tag = tag.split(['.', '@']).next().unwrap_or(tag);
        let tag = tag.replace('_', "-");
        let language = tag
            .split('-')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match language.as_str() {
            "en" | "c" | "posix" => Some(Self::English),
            "zh" => Some(Self::SimplifiedChinese),
            _ => None,
        }
    }
}
