//! Interactive init prompts.

use anyhow::Result;
use std::io::{self, IsTerminal};

use crate::logger;

use super::{FeedFormat, Settings};

pub fn can_prompt() -> bool {
    io::stdin().is_terminal() && io::stderr().is_terminal()
}

pub fn ask() -> Result<Settings> {
    logger::prompt_line("")?;
    logger::prompt_line("press Enter to accept the default.")?;
    logger::prompt_line("")?;

    section("title", "the name of your site.")?;
    let title = read_text("title", None)?.into_value("");

    section(
        "base url",
        "used for absolute links; include the subpath if needed, e.g. https://example.com/blog.",
    )?;
    let mut base_url = read_text("base url", None)?.into_non_empty();

    section("language", "primary language tag, e.g. en, zh-CN, ja.")?;
    let language = read_text("language", Some("en"))?.into_value("en");

    section("author", "the default author name.")?;
    let author = read_text("author", None)?.into_value("");

    section("email", "optional contact email.")?;
    let email = read_text("email", None)?.into_value("");

    section(
        "Tola Typst library",
        "add tola/lib.typ with useful helpers.",
    )?;
    let tola_lib = confirm("enable?", true)?;

    section(
        "atomic CSS",
        "built-in utility CSS, ready to use. default profile: Tailwind CSS v4.",
    )?;
    let atomic_css = confirm("enable?", false)?;

    section(
        "feed",
        "publish updates for feed readers. requires a base url.",
    )?;
    let enable_feed = confirm("enable?", false)?;
    let feeds = if enable_feed {
        if base_url.is_none() {
            logger::prompt_line("base url is required when feed is enabled.")?;
            base_url = Some(read_required("base url")?);
        }

        section(
            "feed formats",
            "available: rss, atom, json. use comma to select multiple, e.g. rss,json.",
        )?;
        read_feed_formats()?
    } else {
        Vec::new()
    };

    section(
        "sitemap",
        "help search engines discover your pages. requires a base url.",
    )?;
    let sitemap = confirm("enable?", false)?;
    if sitemap && base_url.is_none() {
        logger::prompt_line("base url is required when sitemap is enabled.")?;
        base_url = Some(read_required("base url")?);
    }

    Ok(Settings {
        title,
        base_url,
        language,
        author,
        email,
        tola_lib,
        atomic_css,
        feeds,
        sitemap,
    })
}

pub fn confirm(label: &str, default: bool) -> Result<bool> {
    loop {
        let suffix = if default { " [Y/n]: " } else { " [y/N]: " };
        logger::prompt(format_args!("{label}{suffix}"))?;

        let input = read_line()?.to_ascii_lowercase();
        if input.is_empty() {
            return Ok(default);
        }

        match input.as_str() {
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => logger::prompt_log("error", format_args!("enter y or n"))?,
        }
    }
}

fn section(name: &str, help: &str) -> Result<()> {
    logger::prompt_log("init", format_args!("{name}"))?;
    logger::prompt_line(help)?;
    Ok(())
}

fn read_text(label: &str, default: Option<&str>) -> Result<Answer> {
    let suffix = default.map_or_else(|| ": ".to_string(), |value| format!(" [{value}]: "));
    logger::prompt(format_args!("{label}{suffix}"))?;
    Ok(Answer(read_line()?))
}

fn read_required(label: &str) -> Result<String> {
    loop {
        let value = read_text(label, None)?.into_value("");
        if !value.trim().is_empty() {
            return Ok(value);
        }
        logger::prompt_log("error", format_args!("{label} is required"))?;
    }
}

fn read_feed_formats() -> Result<Vec<FeedFormat>> {
    loop {
        let input = read_text("formats", Some("rss"))?.into_value("rss");
        match parse_feed_formats(&input) {
            Ok(formats) => return Ok(formats),
            Err(message) => logger::prompt_log("error", format_args!("{message}"))?,
        }
    }
}

fn parse_feed_formats(input: &str) -> Result<Vec<FeedFormat>, &'static str> {
    let mut formats = Vec::new();
    for item in input
        .split([',', ' '])
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let format = match item.to_ascii_lowercase().as_str() {
            "rss" => FeedFormat::Rss,
            "atom" => FeedFormat::Atom,
            "json" => FeedFormat::Json,
            _ => return Err("choose one or more of: rss, atom, json"),
        };
        if !formats.contains(&format) {
            formats.push(format);
        }
    }

    if formats.is_empty() {
        Err("choose one or more of: rss, atom, json")
    } else {
        Ok(formats)
    }
}

fn read_line() -> Result<String> {
    let mut input = String::new();
    let bytes = io::stdin().read_line(&mut input)?;
    if bytes == 0 {
        anyhow::bail!("init cancelled");
    }
    Ok(input.trim().to_string())
}

struct Answer(String);

impl Answer {
    fn into_value(self, default: &str) -> String {
        if self.0.is_empty() {
            default.to_string()
        } else {
            self.0
        }
    }

    fn into_non_empty(self) -> Option<String> {
        if self.0.is_empty() {
            None
        } else {
            Some(self.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_feed_formats() {
        assert_eq!(
            parse_feed_formats("rss,json").unwrap(),
            vec![FeedFormat::Rss, FeedFormat::Json]
        );
        assert_eq!(
            parse_feed_formats("rss atom rss").unwrap(),
            vec![FeedFormat::Rss, FeedFormat::Atom]
        );
        assert!(parse_feed_formats("xml").is_err());
    }
}
