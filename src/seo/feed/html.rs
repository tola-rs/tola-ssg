//! HTML preparation for feed entry bodies.

use crate::utils::html::{escape_attr, is_void_element};

pub struct HtmlOptions<'a> {
    pub site_url: Option<&'a str>,
    pub page_url: &'a str,
    pub no_script: bool,
}

pub fn prepare(html: &str, options: &HtmlOptions<'_>) -> String {
    if options.site_url.is_none() && !options.no_script {
        return html.to_string();
    }

    let Ok(dom) = tl::parse(html, tl::ParserOptions::default()) else {
        return html.to_string();
    };

    let parser = dom.parser();
    let mut out = String::with_capacity(html.len());
    for handle in dom.children() {
        if let Some(node) = handle.get(parser) {
            render_node(node, parser, options, &mut out);
        }
    }
    out
}

fn render_node(
    node: &tl::Node<'_>,
    parser: &tl::Parser<'_>,
    options: &HtmlOptions<'_>,
    out: &mut String,
) {
    match node {
        tl::Node::Tag(tag) => render_tag(tag, parser, options, out),
        tl::Node::Raw(text) => out.push_str(&text.as_utf8_str()),
        tl::Node::Comment(_) => {}
    }
}

fn render_tag(
    tag: &tl::HTMLTag<'_>,
    parser: &tl::Parser<'_>,
    options: &HtmlOptions<'_>,
    out: &mut String,
) {
    let name = tag.name().as_utf8_str();
    if options.no_script && name.eq_ignore_ascii_case("script") {
        return;
    }

    out.push('<');
    out.push_str(&name);

    for (attr, value) in tag.attributes().iter() {
        if options.no_script && attr.to_ascii_lowercase().starts_with("on") {
            continue;
        }

        if let Some(value) = value {
            let Some(value) = rewrite_attr(&attr, &value, options) else {
                continue;
            };
            out.push(' ');
            out.push_str(&attr);
            out.push_str("=\"");
            out.push_str(&escape_attr(&value));
            out.push('"');
        } else {
            out.push(' ');
            out.push_str(&attr);
        }
    }

    if is_void_element(&name) {
        out.push('>');
        return;
    }

    out.push('>');
    for handle in tag.children().top().iter() {
        if let Some(node) = handle.get(parser) {
            render_node(node, parser, options, out);
        }
    }
    out.push_str("</");
    out.push_str(&name);
    out.push('>');
}

fn rewrite_attr(attr: &str, value: &str, options: &HtmlOptions<'_>) -> Option<String> {
    match attr.to_ascii_lowercase().as_str() {
        "href" | "src" | "poster" | "action" => absolute_url(value, options),
        "srcset" => Some(absolute_srcset(value, options)),
        "srcdoc" if options.no_script => None,
        _ => Some(value.to_string()),
    }
}

fn absolute_srcset(value: &str, options: &HtmlOptions<'_>) -> String {
    value
        .split(',')
        .map(|candidate| {
            let leading = candidate.len() - candidate.trim_start().len();
            let candidate = candidate.trim_start();
            let split = candidate
                .find(char::is_whitespace)
                .unwrap_or(candidate.len());
            let (url, descriptor) = candidate.split_at(split);
            format!(
                "{}{}{}",
                " ".repeat(leading),
                absolute_url(url, options).unwrap_or_default(),
                descriptor
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn absolute_url(value: &str, options: &HtmlOptions<'_>) -> Option<String> {
    if options.no_script && has_script_scheme(value) {
        return None;
    }

    let Some(site_url) = options.site_url else {
        return Some(value.to_string());
    };

    if url::Url::parse(value).is_ok() {
        return Some(value.to_string());
    }

    if value.starts_with('/') && !value.starts_with("//") {
        return root_relative_url(value, site_url).or_else(|| Some(value.to_string()));
    }

    url::Url::parse(options.page_url)
        .and_then(|base| base.join(value))
        .map(|url| url.to_string())
        .or_else(|_| {
            url::Url::parse(site_url)
                .and_then(|base| base.join(value))
                .map(|url| url.to_string())
        })
        .ok()
        .or_else(|| Some(value.to_string()))
}

fn root_relative_url(value: &str, site_url: &str) -> Option<String> {
    let base = url::Url::parse(&format!("{}/", site_url.trim_end_matches('/'))).ok()?;
    let reference = url::Url::parse("http://tola.local")
        .ok()?
        .join(value)
        .ok()?;

    let path = reference.path().trim_start_matches('/');

    let mut relative = path.to_string();
    if let Some(query) = reference.query() {
        relative.push('?');
        relative.push_str(query);
    }
    if let Some(fragment) = reference.fragment() {
        relative.push('#');
        relative.push_str(fragment);
    }

    base.join(&relative).map(|url| url.to_string()).ok()
}

fn has_script_scheme(value: &str) -> bool {
    value
        .trim_start()
        .get(.."javascript:".len())
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("javascript:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> HtmlOptions<'static> {
        HtmlOptions {
            site_url: Some("https://example.com/blog"),
            page_url: "https://example.com/blog/posts/%E4%B8%AD%E6%96%87/",
            no_script: false,
        }
    }

    #[test]
    fn makes_root_relative_urls_absolute() {
        let html = r#"<p><a href="/about/">About</a><img src="/img/中文.png"></p>"#;

        let html = prepare(html, &options());

        assert!(html.contains(r#"href="https://example.com/blog/about/""#));
        assert!(html.contains(r#"src="https://example.com/blog/img/%E4%B8%AD%E6%96%87.png""#));
    }

    #[test]
    fn resolves_fragment_links_against_page_url() {
        let html = r##"<a href="#top">Top</a>"##;

        let html = prepare(html, &options());

        assert_eq!(
            html,
            r#"<a href="https://example.com/blog/posts/%E4%B8%AD%E6%96%87/#top">Top</a>"#
        );
    }

    #[test]
    fn rewrites_srcset_urls() {
        let html = r#"<img srcset="/a.png 1x, /b.png 2x">"#;

        let html = prepare(html, &options());

        assert_eq!(
            html,
            r#"<img srcset="https://example.com/blog/a.png 1x, https://example.com/blog/b.png 2x">"#
        );
    }

    #[test]
    fn root_relative_urls_are_site_root_relative() {
        let html = r#"<a href="/blog/about/?q=1#top">About</a>"#;

        let html = prepare(html, &options());

        assert_eq!(
            html,
            r#"<a href="https://example.com/blog/blog/about/?q=1#top">About</a>"#
        );
    }

    #[test]
    fn resolves_relative_urls_against_page_url() {
        let html = r#"<a href="next/">Next</a><img src="../cover.png">"#;

        let html = prepare(html, &options());

        assert_eq!(
            html,
            r#"<a href="https://example.com/blog/posts/%E4%B8%AD%E6%96%87/next/">Next</a><img src="https://example.com/blog/posts/cover.png">"#
        );
    }

    #[test]
    fn no_script_removes_scripts_and_event_handlers() {
        let mut options = options();
        options.no_script = true;

        let html = prepare(
            r#"<button onclick="fold()">Fold</button><script>fold()</script>"#,
            &options,
        );

        assert_eq!(html, "<button>Fold</button>");
    }

    #[test]
    fn no_script_removes_script_urls_and_srcdoc() {
        let mut options = options();
        options.no_script = true;

        let html = prepare(
            r#"<a href="javascript:fold()">Fold</a><iframe srcdoc="<script>fold()</script>"></iframe>"#,
            &options,
        );

        assert_eq!(html, "<a>Fold</a><iframe></iframe>");
    }

    #[test]
    fn leaves_html_unchanged_without_site_url_or_no_script() {
        let html = r#"<p><!-- keep --><a href="/about/">About</a></p>"#;
        let options = HtmlOptions {
            site_url: None,
            page_url: "/posts/post/",
            no_script: false,
        };

        assert_eq!(prepare(html, &options), html);
    }
}
