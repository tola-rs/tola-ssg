//! Reload runtime injection into development HTML responses.

use bytes::Bytes;

/// Injection bytes do not depend on insertion offsets, so HEAD never scans the page.
pub(in crate::dev) struct HtmlInjection {
    script: Bytes,
}

#[derive(serde::Serialize)]
struct ReloadBootstrap<'a> {
    port: u16,
    session: &'a str,
    revision: Option<&'a str>,
    output: Option<&'a str>,
    page_availability: Option<tola_build::output::PageAvailability>,
    mount: &'a str,
    generation: &'a str,
}

impl HtmlInjection {
    pub(in crate::dev) fn new(
        url_mount: &tola_address::SiteUrlMount,
        reload_endpoint: &crate::dev::reload::transport::ReloadEndpoint,
        revision: Option<&tola_build::output::manifest::RevisionId>,
        page_availability: Option<tola_build::output::PageAvailability>,
        output: Option<&tola_address::OutputPath>,
    ) -> Self {
        let bootstrap = serde_json::to_string(&ReloadBootstrap {
            port: reload_endpoint.port(),
            session: reload_endpoint.session_token(),
            revision: revision.map(|revision| revision.as_str()),
            output: output.map(|output| output.as_str()),
            page_availability,
            mount: url_mount.as_str(),
            generation: reload_endpoint.generation(),
        })
        .expect("reload bootstrap metadata is serializable");
        Self {
            script: Bytes::from(crate::embed::dev::hotreload_script_tag(
                url_mount, &bootstrap,
            )),
        }
    }

    pub(in crate::dev) fn byte_len(&self, content_len: usize) -> usize {
        content_len + self.script.len()
    }

    pub(in crate::dev) fn slices(&self, content: Bytes) -> [Bytes; 3] {
        let insertion = bootstrap_insertion(&content);
        [
            content.slice(..insertion),
            self.script.clone(),
            content.slice(insertion..),
        ]
    }
}

fn bootstrap_insertion(content: &[u8]) -> usize {
    fn whitespace(byte: u8) -> bool {
        matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
    }

    fn skip_whitespace(content: &[u8], mut cursor: usize) -> usize {
        while content.get(cursor).is_some_and(|byte| whitespace(*byte)) {
            cursor += 1;
        }
        cursor
    }

    fn named(content: &[u8], cursor: usize, name: &[u8]) -> bool {
        content
            .get(cursor..cursor + name.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(name))
            && content
                .get(cursor + name.len())
                .is_some_and(|byte| whitespace(*byte) || matches!(*byte, b'/' | b'>'))
    }

    fn quoted_end(content: &[u8], cursor: usize) -> Option<usize> {
        let quote = *content.get(cursor)?;
        if !matches!(quote, b'\'' | b'"') {
            return None;
        }
        content[cursor + 1..]
            .iter()
            .position(|byte| *byte == quote)
            .map(|end| cursor + end + 2)
    }

    fn tag_end(content: &[u8], mut cursor: usize) -> Option<usize> {
        loop {
            cursor = skip_whitespace(content, cursor);
            match *content.get(cursor)? {
                b'>' => return Some(cursor + 1),
                b'/' if content.get(cursor + 1) == Some(&b'>') => return Some(cursor + 2),
                b'/' => return None,
                _ => {}
            }
            let start = cursor;
            while let Some(&byte) = content.get(cursor) {
                if whitespace(byte) || matches!(byte, b'=' | b'/' | b'>') {
                    break;
                }
                if matches!(byte, b'\0' | b'\'' | b'"' | b'<' | b'`') {
                    return None;
                }
                cursor += 1;
            }
            if cursor == start {
                return None;
            }
            cursor = skip_whitespace(content, cursor);
            if content.get(cursor) != Some(&b'=') {
                continue;
            }
            cursor = skip_whitespace(content, cursor + 1);
            match *content.get(cursor)? {
                b'\'' | b'"' => cursor = quoted_end(content, cursor)?,
                _ => {
                    let start = cursor;
                    while let Some(&byte) = content.get(cursor) {
                        if whitespace(byte) || byte == b'>' {
                            break;
                        }
                        if matches!(byte, b'\0' | b'\'' | b'"' | b'=' | b'<' | b'`') {
                            return None;
                        }
                        cursor += 1;
                    }
                    if cursor == start {
                        return None;
                    }
                }
            }
        }
    }

    fn doctype_end(content: &[u8], cursor: usize) -> Option<usize> {
        let mut cursor = skip_whitespace(content, cursor);
        if !content
            .get(cursor..cursor + 4)?
            .eq_ignore_ascii_case(b"html")
        {
            return None;
        }
        cursor += 4;
        if content
            .get(cursor)
            .is_none_or(|byte| !whitespace(*byte) && *byte != b'>')
        {
            return None;
        }
        cursor = skip_whitespace(content, cursor);
        if content.get(cursor) == Some(&b'>') {
            return Some(cursor + 1);
        }
        let public = named(content, cursor, b"public");
        if !public && !named(content, cursor, b"system") {
            return None;
        }
        cursor += 6;
        cursor = quoted_end(content, skip_whitespace(content, cursor))?;
        cursor = skip_whitespace(content, cursor);
        if public && matches!(content.get(cursor), Some(b'\'' | b'"')) {
            cursor = quoted_end(content, cursor)?;
            cursor = skip_whitespace(content, cursor);
        }
        (content.get(cursor) == Some(&b'>')).then_some(cursor + 1)
    }

    let mut cursor = usize::from(content.starts_with(b"\xef\xbb\xbf")) * 3;
    let mut insertion = cursor;
    // Only leading markup is inspected; raw-text and other content remain opaque.
    loop {
        cursor = skip_whitespace(content, cursor);
        if content
            .get(cursor..)
            .is_some_and(|tail| tail.starts_with(b"<!--"))
        {
            let start = cursor + 4;
            let tail = &content[start..];
            let end = if tail.starts_with(b">") {
                Some(start + 1)
            } else if tail.starts_with(b"->") {
                Some(start + 2)
            } else {
                (0..tail.len()).find_map(|offset| {
                    if tail[offset..].starts_with(b"-->") {
                        Some(start + offset + 3)
                    } else if tail[offset..].starts_with(b"--!>") {
                        Some(start + offset + 4)
                    } else {
                        None
                    }
                })
            };
            let Some(end) = end else {
                return insertion;
            };
            cursor = end;
            insertion = end;
        } else if named(content, cursor, b"<!doctype") {
            let Some(end) = doctype_end(content, cursor + b"<!doctype".len()) else {
                return insertion;
            };
            cursor = end;
            insertion = end;
        } else if named(content, cursor, b"<html") {
            let Some(end) = tag_end(content, cursor + b"<html".len()) else {
                return insertion;
            };
            cursor = end;
            insertion = end;
        } else if named(content, cursor, b"<head") {
            return tag_end(content, cursor + b"<head".len()).unwrap_or(insertion);
        } else {
            return insertion;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_precedes_user_scripts() {
        let endpoint = crate::dev::reload::transport::ReloadEndpoint::new(
            35729,
            "test-session",
            "test-generation",
        );
        for content in [
            "<html><head><script>application()</script></head><body>Page</body></html>",
            "<!doctype html><html><head lang=\"en\"><script>application()</script></head></html>",
            "<main><script>application(\"<head>\")</script></main>",
        ] {
            let slices = HtmlInjection::new(
                &tola_address::SiteUrlMount::root(),
                &endpoint,
                None,
                None,
                None,
            )
            .slices(Bytes::copy_from_slice(content.as_bytes()));
            let body = String::from_utf8(slices.into_iter().flatten().collect()).unwrap();
            assert!(body.find("hotreload.js").unwrap() < body.find("application(").unwrap());
        }
    }
}
