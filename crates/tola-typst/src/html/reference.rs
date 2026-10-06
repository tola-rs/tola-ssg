//! Shared HTML reference classification and attribute token syntax.

use std::borrow::Cow;
use std::collections::HashSet;
use std::ops::Range;

use typst::utils::{PicoStr, ResolvedPicoStr};
use typst_html::HtmlElement;

/// URL-use semantics implied by an HTML element and attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HtmlReferenceUse {
    /// A browser navigation target.
    Navigation,
    /// A browser-loaded resource without a more specific required representation.
    GenericResource,
    /// A document or subresource prefetched by a `<link rel="prefetch">` element.
    Prefetch,
    /// A stylesheet loaded by a `<link rel="stylesheet">` element.
    Stylesheet,
    /// A classic script loaded by a `<script>` element.
    ClassicScript,
    /// A JavaScript module loaded by a `<script type="module">` element.
    ModuleScript,
    /// A JavaScript module dependency preloaded by a `<link rel="modulepreload">` element.
    ModulePreload,
}

/// Lowercase an HTML name only when it has uppercase, so the common already-lowercase
/// spelling keeps borrowing the resolved name.
pub(super) fn ascii_lowercase(value: &str) -> Cow<'_, str> {
    if value.bytes().any(|byte| byte.is_ascii_uppercase()) {
        Cow::Owned(value.to_ascii_lowercase())
    } else {
        Cow::Borrowed(value)
    }
}

pub(super) fn map_url_ranges<E>(
    value: &str,
    ranges: impl IntoIterator<Item = Range<usize>>,
    map: &mut impl FnMut(&str) -> Result<Option<String>, E>,
) -> Result<Option<String>, E> {
    let mut output = None;
    let mut cursor = 0;
    for range in ranges {
        let Some(replacement) = map(&value[range.clone()])? else {
            continue;
        };
        if replacement == value[range.clone()] {
            continue;
        }
        let output = output.get_or_insert_with(|| String::with_capacity(value.len()));
        output.push_str(&value[cursor..range.start]);
        output.push_str(&replacement);
        cursor = range.end;
    }
    if let Some(output) = &mut output {
        output.push_str(&value[cursor..]);
    }
    Ok(output)
}

/// One native HTML attribute whose name decides how its value is interpreted.
pub(super) struct HtmlAttribute<'a> {
    /// Position among the element's native attributes.
    pub(super) index: usize,
    /// Lowercased attribute name.
    pub(super) name: ResolvedPicoStr,
    /// Attribute value.
    pub(super) value: &'a str,
}

/// Native attributes retain their spelling and order; only the first
/// case-insensitive occurrence of a name has browser semantics.
pub(super) fn unique_attributes(element: &HtmlElement) -> impl Iterator<Item = HtmlAttribute<'_>> {
    let mut seen = HashSet::new();
    element
        .attrs
        .0
        .iter()
        .enumerate()
        .filter_map(move |(index, (name, value))| {
            let resolved = name.resolve();
            let name = ascii_lowercase(resolved.as_str());
            let name = PicoStr::intern(&name).resolve();
            seen.insert(name).then_some(HtmlAttribute {
                index,
                name,
                value: value.as_str(),
            })
        })
}

/// `srcset` and `imagesrcset` values yield their candidate URLs, `ping` yields
/// its whitespace-separated tokens, a `meta` refresh `content` value yields its
/// destination when it has one, and every other URL-bearing attribute yields its
/// complete value. Ranges are byte offsets into `value`.
pub(super) fn url_ranges<'a>(tag: &str, attribute: &str, value: &'a str) -> UrlRanges<'a> {
    if attribute == "srcset" || attribute == "imagesrcset" {
        UrlRanges::Srcset { value, rest: value }
    } else if attribute == "ping" {
        UrlRanges::Ping {
            value,
            tokens: value.split_ascii_whitespace(),
        }
    } else if tag == "meta" && attribute == "content" {
        UrlRanges::Single(match refresh_destination(value) {
            RefreshDestination::Absent | RefreshDestination::Declared(None) => None,
            RefreshDestination::Declared(Some(url)) => Some(token_range(value, url)),
        })
    } else {
        UrlRanges::Single(Some(0..value.len()))
    }
}

pub(super) enum UrlRanges<'a> {
    Single(Option<Range<usize>>),
    Ping {
        value: &'a str,
        tokens: std::str::SplitAsciiWhitespace<'a>,
    },
    Srcset {
        value: &'a str,
        rest: &'a str,
    },
}

impl Iterator for UrlRanges<'_> {
    type Item = Range<usize>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Single(range) => range.take(),
            Self::Ping { value, tokens } => tokens.next().map(|token| token_range(value, token)),
            Self::Srcset { value, rest } => loop {
                *rest = rest.trim_start_matches(|character: char| {
                    character.is_ascii_whitespace() || character == ','
                });
                if rest.is_empty() {
                    return None;
                }

                let remaining = *rest;
                let url_start = value.len() - remaining.len();
                let url_end = remaining
                    .find(char::is_whitespace)
                    .unwrap_or(remaining.len());
                let mut url = &remaining[..url_end];
                *rest = &remaining[url_end..];
                let trailing_commas = url.len() - url.trim_end_matches(',').len();
                if trailing_commas > 0 {
                    url = url.trim_end_matches(',');
                } else if !valid_srcset_descriptors(srcset_descriptors(rest)) {
                    continue;
                }

                if !url.is_empty() {
                    return Some(url_start..url_start + url.len());
                }
            },
        }
    }
}

fn token_range(value: &str, token: &str) -> Range<usize> {
    let start = token.as_ptr() as usize - value.as_ptr() as usize;
    start..start + token.len()
}

pub(super) fn attribute<'a>(element: &'a HtmlElement, expected: &str) -> Option<&'a str> {
    element.attrs.0.iter().find_map(|(name, value)| {
        name.resolve()
            .as_str()
            .eq_ignore_ascii_case(expected)
            .then_some(value.as_str())
    })
}
pub(super) fn classify_reference_use(
    element: &HtmlElement,
    tag: &str,
    name: &str,
) -> Option<HtmlReferenceUse> {
    use HtmlReferenceUse::{GenericResource, Navigation};

    let relation = attribute(element, "rel");
    let http_equiv = attribute(element, "http-equiv");
    let element_type = attribute(element, "type");

    match (tag, name) {
        ("a" | "area", "href")
        | ("form", "action")
        | ("blockquote" | "q" | "del" | "ins", "cite") => Some(Navigation),
        ("iframe", "src") if attribute(element, "srcdoc").is_none() => Some(Navigation),
        ("button", "formaction")
            if !element_type.is_some_and(|value| {
                value.eq_ignore_ascii_case("reset") || value.eq_ignore_ascii_case("button")
            }) =>
        {
            Some(Navigation)
        }
        ("input", "formaction")
            if element_type.is_some_and(|value| {
                value.eq_ignore_ascii_case("submit") || value.eq_ignore_ascii_case("image")
            }) =>
        {
            Some(Navigation)
        }
        ("meta", "content")
            if http_equiv.is_some_and(|value| value.eq_ignore_ascii_case("refresh")) =>
        {
            Some(Navigation)
        }
        ("a" | "area", "ping") => Some(GenericResource),
        ("link", "href") => classify_link_reference_use(relation),
        ("object", "data") => Some(GenericResource),
        ("script", "src") => classify_script_reference_use(element),
        ("audio" | "embed" | "img" | "source" | "track" | "video", "src") => Some(GenericResource),
        ("input", "src")
            if element_type.is_some_and(|value| value.eq_ignore_ascii_case("image")) =>
        {
            Some(GenericResource)
        }
        ("img" | "source", "srcset") => Some(GenericResource),
        ("link", "imagesrcset") if relation_has(relation, "preload") => Some(GenericResource),
        ("video", "poster") => Some(GenericResource),
        _ => None,
    }
}

fn classify_script_reference_use(element: &HtmlElement) -> Option<HtmlReferenceUse> {
    let element_type = attribute(element, "type");
    let script_type = match element_type {
        Some("") => Cow::Borrowed("text/javascript"),
        Some(value) => Cow::Borrowed(value),
        None => match attribute(element, "language") {
            Some(language) if !language.is_empty() => Cow::Owned(format!("text/{language}")),
            _ => Cow::Borrowed("text/javascript"),
        },
    };
    // Only a MIME type written in `type` permits surrounding ASCII whitespace.
    if is_javascript_mime_type(&script_type)
        || element_type.is_some_and(|value| is_javascript_mime_type(value.trim_ascii()))
    {
        Some(HtmlReferenceUse::ClassicScript)
    } else if script_type.eq_ignore_ascii_case("module") {
        Some(HtmlReferenceUse::ModuleScript)
    } else {
        None
    }
}

/// Whether a MIME type essence, without parameters, identifies JavaScript.
///
/// Uses the [JavaScript MIME types](https://mimesniff.spec.whatwg.org/#javascript-mime-type).
pub fn is_javascript_mime_type(value: &str) -> bool {
    [
        "application/ecmascript",
        "application/javascript",
        "application/x-ecmascript",
        "application/x-javascript",
        "text/ecmascript",
        "text/javascript",
        "text/javascript1.0",
        "text/javascript1.1",
        "text/javascript1.2",
        "text/javascript1.3",
        "text/javascript1.4",
        "text/javascript1.5",
        "text/jscript",
        "text/livescript",
        "text/x-ecmascript",
        "text/x-javascript",
    ]
    .into_iter()
    .any(|mime_type| value.eq_ignore_ascii_case(mime_type))
}

fn classify_link_reference_use(relation: Option<&str>) -> Option<HtmlReferenceUse> {
    let relation = relation?;
    let tokens = relation.split_ascii_whitespace().collect::<Vec<_>>();
    let has = |expected: &str| {
        tokens
            .iter()
            .any(|token| token.eq_ignore_ascii_case(expected))
    };
    if has("stylesheet") {
        return Some(HtmlReferenceUse::Stylesheet);
    }
    if has("modulepreload") {
        return Some(HtmlReferenceUse::ModulePreload);
    }
    if ["icon", "manifest", "pingback", "preload"]
        .into_iter()
        .any(&has)
    {
        return Some(HtmlReferenceUse::GenericResource);
    }
    if has("prefetch") {
        return Some(HtmlReferenceUse::Prefetch);
    }
    [
        "alternate",
        "author",
        "canonical",
        "expect",
        "help",
        "license",
        "next",
        "prev",
        "privacy-policy",
        "search",
        "terms-of-service",
    ]
    .into_iter()
    .any(has)
    .then_some(HtmlReferenceUse::Navigation)
}

/// What a `meta` refresh `content` value declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RefreshDestination<'a> {
    /// The value is not a refresh directive.
    Absent,
    /// A refresh directive, naming its destination when it has one.
    Declared(Option<&'a str>),
}

pub(super) fn refresh_destination(value: &str) -> RefreshDestination<'_> {
    let value = value.trim();
    let boundary = value.find(|character: char| {
        character == ';' || character == ',' || character.is_ascii_whitespace()
    });
    let (delay, suffix) = boundary.map_or((value, ""), |boundary| {
        (&value[..boundary], &value[boundary..])
    });
    let Ok(delay) = delay.trim().parse::<f64>() else {
        return RefreshDestination::Absent;
    };
    if !delay.is_finite() || delay < 0.0 {
        return RefreshDestination::Absent;
    }
    let directive = suffix
        .trim_start_matches(|character: char| {
            character == ';' || character == ',' || character.is_ascii_whitespace()
        })
        .trim();
    if directive.is_empty() {
        return RefreshDestination::Declared(None);
    }
    let url = directive.split_once('=').map_or(directive, |(name, url)| {
        if name.trim().eq_ignore_ascii_case("url") {
            url
        } else {
            directive
        }
    });
    let url = url.trim();
    let unquoted = url
        .strip_prefix('"')
        .and_then(|url| url.strip_suffix('"'))
        .or_else(|| {
            url.strip_prefix('\'')
                .and_then(|url| url.strip_suffix('\''))
        })
        .unwrap_or(url);
    if unquoted.is_empty() {
        RefreshDestination::Declared(None)
    } else {
        RefreshDestination::Declared(Some(unquoted))
    }
}

fn relation_has(relation: Option<&str>, expected: &str) -> bool {
    relation.is_some_and(|relation| {
        relation
            .split_ascii_whitespace()
            .any(|token| token.eq_ignore_ascii_case(expected))
    })
}

#[derive(Clone, Copy)]
enum SrcsetState {
    Descriptor,
    Parentheses,
    AfterDescriptor,
}

fn srcset_descriptors<'a, 'b>(rest: &'b mut &'a str) -> impl Iterator<Item = &'a str> + 'b {
    let value = *rest;
    let mut start = None;
    let mut end = 0;
    let mut state = SrcsetState::Descriptor;
    let mut ended = false;
    std::iter::from_fn(move || {
        if ended {
            return None;
        }
        while let Some(character) = rest.chars().next() {
            let offset = value.len() - rest.len();
            *rest = &rest[character.len_utf8()..];
            match state {
                SrcsetState::Descriptor => match character {
                    character if character.is_ascii_whitespace() => {
                        if let Some(start) = start.take() {
                            state = SrcsetState::AfterDescriptor;
                            return Some(&value[start..end]);
                        }
                    }
                    ',' => break,
                    '(' => {
                        start.get_or_insert(offset);
                        end = offset + character.len_utf8();
                        state = SrcsetState::Parentheses;
                    }
                    _ => {
                        start.get_or_insert(offset);
                        end = offset + character.len_utf8();
                    }
                },
                SrcsetState::Parentheses => {
                    end = offset + character.len_utf8();
                    if character == ')' {
                        state = SrcsetState::Descriptor;
                    }
                }
                SrcsetState::AfterDescriptor => match character {
                    character if character.is_ascii_whitespace() => {}
                    ',' => break,
                    _ => {
                        start = Some(offset);
                        end = offset + character.len_utf8();
                        state = SrcsetState::Descriptor;
                    }
                },
            }
        }
        ended = true;
        start.take().map(|start| &value[start..end])
    })
}

fn valid_srcset_descriptors<'a>(descriptors: impl IntoIterator<Item = &'a str>) -> bool {
    let mut width = false;
    let mut density = false;
    let mut height = false;
    let mut valid = true;
    for descriptor in descriptors {
        // Even a rejected candidate consumes all descriptors before the next URL starts.
        if !valid {
            continue;
        }
        let Some(suffix) = descriptor.chars().last() else {
            valid = false;
            continue;
        };
        let value = &descriptor[..descriptor.len() - suffix.len_utf8()];
        match suffix {
            'w' if !width && !density => {
                width = value.parse::<u64>().is_ok_and(|value| value > 0);
                valid = width;
            }
            'x' if !width && !density && !height => {
                density = value
                    .parse::<f64>()
                    .is_ok_and(|value| value.is_finite() && value >= 0.0);
                valid = density;
            }
            'h' if !height && !density => {
                height = value.parse::<u64>().is_ok_and(|value| value > 0);
                valid = height;
            }
            _ => valid = false,
        }
    }
    valid && (!height || width)
}
#[cfg(test)]
mod tests {
    use super::*;

    fn srcset_urls(value: &str) -> Vec<&str> {
        url_ranges("img", "srcset", value)
            .map(|range| &value[range])
            .collect()
    }

    #[test]
    fn srcset_retains_commas_inside_urls() {
        assert_eq!(
            srcset_urls("data:image/svg+xml,%3Csvg%3E 1x, /image.png 2x"),
            ["data:image/svg+xml,%3Csvg%3E", "/image.png"]
        );
        assert_eq!(srcset_urls("one.png, two.png,"), ["one.png", "two.png"]);
        assert_eq!(
            srcset_urls("bad.png nope, width.png 480w, density.png 2x"),
            ["width.png", "density.png"]
        );
        assert_eq!(
            srcset_urls("height-only.png 200h, sized.png 480w 200h"),
            ["sized.png"]
        );
    }
    #[test]
    fn refresh_destination_decodes_syntax() {
        for (value, expected) in [
            ("0;url=/next/", "/next/"),
            ("0, /next/", "/next/"),
            ("0 /next/", "/next/"),
            ("0; URL = '/next/'", "/next/"),
            ("0; '/next?theme=dark#top'", "/next?theme=dark#top"),
        ] {
            assert_eq!(
                refresh_destination(value),
                RefreshDestination::Declared(Some(expected))
            );
        }
        assert_eq!(refresh_destination("0"), RefreshDestination::Declared(None));
        assert_eq!(
            refresh_destination("not-a-delay;url=/next/"),
            RefreshDestination::Absent
        );
    }

    #[test]
    fn link_rel_selects_reference_use() {
        for (relation, expected) in [
            (Some("next"), Some(HtmlReferenceUse::Navigation)),
            (
                Some("alternate stylesheet"),
                Some(HtmlReferenceUse::Stylesheet),
            ),
            (Some("modulepreload"), Some(HtmlReferenceUse::ModulePreload)),
            (Some("icon"), Some(HtmlReferenceUse::GenericResource)),
            (
                Some(" \tPrEfEtCh\n prefetch "),
                Some(HtmlReferenceUse::Prefetch),
            ),
            // A more specific relation wins over a lower-priority companion.
            (
                Some("prefetch stylesheet"),
                Some(HtmlReferenceUse::Stylesheet),
            ),
            (
                Some("modulepreload prefetch"),
                Some(HtmlReferenceUse::ModulePreload),
            ),
            (
                Some("icon prefetch"),
                Some(HtmlReferenceUse::GenericResource),
            ),
            (
                Some("prefetch preload"),
                Some(HtmlReferenceUse::GenericResource),
            ),
            (Some("custom-extension"), None),
            (None, None),
        ] {
            assert_eq!(
                classify_link_reference_use(relation),
                expected,
                "{relation:?}"
            );
        }
    }
}
