//! The configured site language: one language tag, written whole or as its parts.

use serde::{Deserialize, Serialize};

/// A configured site language, written as one tag or as its parts.
///
/// `language = "zh-Hans-CN"` and `language = { lang = "zh", script = "Hans", region = "CN" }`
/// declare the same language. No other shape is a language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum LanguageDeclaration {
    /// A language tag such as `en`, `ja`, `zh-Hans`, or `zh-Hant-TW`.
    Tag(String),
    /// The subtags of one; `lang` is required, `script` and `region` are optional.
    Parts {
        lang: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        script: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<String>,
    },
}

impl<'de> Deserialize<'de> for LanguageDeclaration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        /// The subtags of one language, named after the parameters that take them.
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Parts {
            lang: String,
            #[serde(default)]
            script: Option<String>,
            #[serde(default)]
            region: Option<String>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Declared {
            Tag(String),
            Parts(Parts),
        }

        match Declared::deserialize(deserializer) {
            Ok(Declared::Tag(tag)) => Ok(Self::Tag(tag)),
            Ok(Declared::Parts(parts)) => Ok(Self::Parts {
                lang: parts.lang,
                script: parts.script,
                region: parts.region,
            }),
            Err(_) => Err(serde::de::Error::custom(
                "`site.language` is neither one language tag nor the table of its parts: write a tag such as `zh-Hans-CN`, or `{ lang = \"zh\", script = \"Hans\", region = \"CN\" }`",
            )),
        }
    }
}

impl Default for LanguageDeclaration {
    fn default() -> Self {
        Self::Tag("en".to_owned())
    }
}

/// One validated site language: the canonical tag and the subtags Typst takes.
///
/// The subtag domains are the ones `set text` accepts: `lang` is 2–3 letters (ISO 639), `script`
/// 3–4 (ISO 15924), and `region` exactly 2 (ISO 3166-1 alpha-2). Canonical casing is a lowercase
/// language, a titlecase script, and an uppercase region, which is the tag's own spelling.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SiteLanguage {
    tag: String,
    lang: String,
    script: Option<String>,
    region: Option<String>,
}

impl SiteLanguage {
    /// Validate one declaration and canonicalize its subtags.
    ///
    /// The error names the failing part, so each caller spells the option path it reads from.
    pub fn parse(declaration: &LanguageDeclaration) -> Result<Self, UnusableLanguage> {
        let (lang, script, region) = match declaration {
            LanguageDeclaration::Tag(tag) => parse_tag(tag)?,
            LanguageDeclaration::Parts {
                lang,
                script,
                region,
            } => (
                language_subtag(lang)?,
                script.as_deref().map(script_subtag).transpose()?,
                region.as_deref().map(region_subtag).transpose()?,
            ),
        };
        let mut tag = lang.clone();
        if let Some(script) = &script {
            tag.push('-');
            tag.push_str(script);
        }
        if let Some(region) = &region {
            tag.push('-');
            tag.push_str(region);
        }
        Ok(Self {
            tag,
            lang,
            script,
            region,
        })
    }

    /// The canonical language tag, as a document declares it.
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// The ISO 639 language subtag, which `text.lang` takes.
    pub fn lang(&self) -> &str {
        &self.lang
    }

    /// The ISO 15924 script subtag, which `text.script` takes.
    pub fn script(&self) -> Option<&str> {
        self.script.as_deref()
    }

    /// The ISO 3166-1 alpha-2 region subtag, which `text.region` takes.
    pub fn region(&self) -> Option<&str> {
        self.region.as_deref()
    }
}

/// Why a declared site language is unusable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnusableLanguage {
    /// The whole declaration is not a language tag.
    Tag { declared: String },
    /// One subtag is outside the forms `set text` accepts.
    Subtag {
        key: LanguageSubtag,
        declared: String,
    },
}

/// One named part of a language tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageSubtag {
    /// The language, which `text.lang` takes.
    Lang,
    /// The script, which `text.script` takes.
    Script,
    /// The region, which `text.region` takes.
    Region,
}

impl LanguageSubtag {
    /// The subtag's name, as configuration and `set text` spell it.
    pub fn spelling(self) -> &'static str {
        match self {
            Self::Lang => "lang",
            Self::Script => "script",
            Self::Region => "region",
        }
    }

    /// The form `set text` accepts for this subtag.
    fn accepted_form(self) -> &'static str {
        match self {
            Self::Lang => {
                "a two- or three-letter ISO 639 language code such as `en`, `zh`, or `ja`"
            }
            Self::Script => {
                "a three- or four-letter ISO 15924 script code such as `Hans`, `Hant`, or `Latn`"
            }
            Self::Region => "a two-letter ISO 3166-1 alpha-2 region code such as `US` or `CN`",
        }
    }
}

impl std::fmt::Display for UnusableLanguage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tag { declared } => write!(
                formatter,
                "`{declared}` is not a language tag such as `en`, `ja`, `zh-Hans`, or `zh-Hant-TW`"
            ),
            Self::Subtag { key, declared } => {
                write!(formatter, "`{declared}` is not {}", key.accepted_form())
            }
        }
    }
}

/// Split one language tag into its canonical subtags.
fn parse_tag(tag: &str) -> Result<(String, Option<String>, Option<String>), UnusableLanguage> {
    let invalid = || UnusableLanguage::Tag {
        declared: tag.to_owned(),
    };
    let mut parts = tag.split('-');
    let language = parts.next().unwrap_or_default();
    let lang = language_subtag(language).map_err(|_| invalid())?;
    let mut script = None;
    let mut region = None;
    for part in parts {
        let alphabetic = part
            .chars()
            .all(|character| character.is_ascii_alphabetic());
        if script.is_none() && region.is_none() && (3..=4).contains(&part.len()) && alphabetic {
            script = Some(script_subtag(part).map_err(|_| invalid())?);
        } else if region.is_none() && part.len() == 2 && alphabetic {
            region = Some(region_subtag(part).map_err(|_| invalid())?);
        } else {
            return Err(invalid());
        }
    }
    Ok((lang, script, region))
}

/// One 2–3 letter ISO 639 language subtag.
fn language_subtag(value: &str) -> Result<String, UnusableLanguage> {
    ok_subtag(value, 2..=3)
        .then(|| value.to_ascii_lowercase())
        .ok_or_else(|| UnusableLanguage::Subtag {
            key: LanguageSubtag::Lang,
            declared: value.to_owned(),
        })
}

/// One 3–4 letter ISO 15924 script subtag, in its canonical titlecase spelling.
fn script_subtag(value: &str) -> Result<String, UnusableLanguage> {
    ok_subtag(value, 3..=4)
        .then(|| {
            let lowercase = value.to_ascii_lowercase();
            let mut characters = lowercase.chars();
            match characters.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + characters.as_str(),
                None => lowercase,
            }
        })
        .ok_or_else(|| UnusableLanguage::Subtag {
            key: LanguageSubtag::Script,
            declared: value.to_owned(),
        })
}

/// One 2 letter ISO 3166-1 alpha-2 region subtag.
fn region_subtag(value: &str) -> Result<String, UnusableLanguage> {
    ok_subtag(value, 2..=2)
        .then(|| value.to_ascii_uppercase())
        .ok_or_else(|| UnusableLanguage::Subtag {
            key: LanguageSubtag::Region,
            declared: value.to_owned(),
        })
}

fn ok_subtag(value: &str, lengths: std::ops::RangeInclusive<usize>) -> bool {
    lengths.contains(&value.len())
        && value
            .chars()
            .all(|character| character.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(value: &str) -> LanguageDeclaration {
        LanguageDeclaration::Tag(value.to_owned())
    }

    #[test]
    fn spellings_declare_one_language() {
        let tag = SiteLanguage::parse(&declared("zh-hant-tw")).unwrap();
        let parts = SiteLanguage::parse(&LanguageDeclaration::Parts {
            lang: "ZH".to_owned(),
            script: Some("hant".to_owned()),
            region: Some("tw".to_owned()),
        })
        .unwrap();
        assert_eq!(tag, parts);
        assert_eq!(tag.tag(), "zh-Hant-TW");
        assert_eq!(tag.lang(), "zh");
        assert_eq!(tag.script(), Some("Hant"));
        assert_eq!(tag.region(), Some("TW"));
    }

    #[test]
    fn omitted_subtags_shorten_the_tag() {
        let language = SiteLanguage::parse(&declared("ja")).unwrap();
        assert_eq!(language.tag(), "ja");
        assert_eq!(language.lang(), "ja");
        assert_eq!(language.script(), None);
        assert_eq!(language.region(), None);

        let script = SiteLanguage::parse(&LanguageDeclaration::Parts {
            lang: "zh".to_owned(),
            script: Some("Hans".to_owned()),
            region: None,
        })
        .unwrap();
        assert_eq!(script.tag(), "zh-Hans");
    }

    fn declared_in_toml(source: &str) -> Result<LanguageDeclaration, toml::de::Error> {
        #[derive(Deserialize)]
        struct Holder {
            language: LanguageDeclaration,
        }

        toml::from_str::<Holder>(&format!("language = {source}")).map(|holder| holder.language)
    }

    #[test]
    fn wrong_subtags_name_their_key() {
        let error = SiteLanguage::parse(&LanguageDeclaration::Parts {
            lang: "zh".to_owned(),
            script: None,
            region: Some("Hans".to_owned()),
        })
        .expect_err("a region is exactly two letters");
        assert_eq!(
            error,
            UnusableLanguage::Subtag {
                key: LanguageSubtag::Region,
                declared: "Hans".to_owned(),
            }
        );
        assert_eq!(
            error.to_string(),
            "`Hans` is not a two-letter ISO 3166-1 alpha-2 region code such as `US` or `CN`"
        );

        let tag =
            SiteLanguage::parse(&declared("zh-CN-Hans")).expect_err("a tag orders its subtags");
        assert_eq!(
            tag,
            UnusableLanguage::Tag {
                declared: "zh-CN-Hans".to_owned(),
            }
        );
    }

    #[test]
    fn only_the_two_spellings_deserialize() {
        assert_eq!(
            declared_in_toml("\"zh-Hans-CN\"").unwrap(),
            LanguageDeclaration::Tag("zh-Hans-CN".to_owned())
        );
        assert_eq!(
            declared_in_toml("{ lang = \"zh\", script = \"Hans\", region = \"CN\" }").unwrap(),
            LanguageDeclaration::Parts {
                lang: "zh".to_owned(),
                script: Some("Hans".to_owned()),
                region: Some("CN".to_owned()),
            }
        );

        for source in [
            "5",
            "{}",
            "{ script = \"Hans\" }",
            "{ lang = \"zh\", tag = \"zh-Hans-CN\" }",
            "{ lang = \"zh\", region = \"CN\", fallback = false }",
        ] {
            assert!(declared_in_toml(source).is_err(), "{source} deserialized");
        }
    }

    #[test]
    fn malformed_tags_are_rejected() {
        for value in [
            "",
            "-",
            "zh-Hans-CN-extra",
            "zh-CN-Hans",
            "Hans",
            "zh-Hans-Hant",
            "zh-",
            "en-US-",
        ] {
            assert!(
                SiteLanguage::parse(&declared(value)).is_err(),
                "{value} parsed"
            );
        }
        assert!(SiteLanguage::parse(&declared("zhx")).is_ok());
    }
}
