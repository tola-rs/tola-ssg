//! Human-facing progress, status, and output-count wording.

use tola_build::output::summary::OutputCounts;

use super::Palette;

pub(crate) fn format_duration(duration: std::time::Duration) -> String {
    if duration.as_secs() > 0 {
        format!("{:.2} s", duration.as_secs_f64())
    } else {
        format!("{} ms", duration.as_millis())
    }
}

pub(crate) fn text(text: &str) -> String {
    super::text::multiline(text).trim_matches('\n').to_owned()
}

pub(crate) fn status(text: &str) -> String {
    let text = super::text::multiline(text);
    super::text::indent_continuation_lines(text.trim_matches('\n'))
}

pub(crate) fn summary(text: &str, palette: Palette) -> String {
    let text = status(text);
    if text.is_empty() {
        return text;
    }
    palette.summary(&text)
}

/// The status line of a round that published nothing.
pub(crate) fn failure(text: &str, palette: Palette) -> String {
    let text = status(text);
    palette.failure(&text)
}

/// A notice naming what the author has to change.
pub(crate) fn notice(text: &str, palette: Palette) -> String {
    let text = status(text);
    palette.notice(&text)
}

/// The label every line of a running hook's output has.
pub(crate) fn stream_label(label: &str, palette: Palette) -> String {
    let label = format!("[{}] ", super::text::single_line(label));
    palette.stream_label(&label)
}

/// A transient or secondary status line.
pub(crate) fn secondary(text: &str, palette: Palette) -> String {
    let text = status(text);
    palette.secondary(&text)
}

/// The line announcing the address the development server serves.
///
/// The address is underlined and coloured like the link it is; the label stays plain, so
/// only the part the reader opens has emphasis.
pub(crate) fn serving_line(url: &str, palette: Palette) -> String {
    format!("Serving {}", palette.serving(url))
}

pub(crate) fn plural_count(count: usize, noun: &str) -> String {
    let suffix = if count == 1 { "" } else { "s" };
    format!("{count} {noun}{suffix}")
}

pub(crate) fn describe_outputs(counts: OutputCounts) -> String {
    let mut parts = Vec::new();
    if counts.pages > 0 {
        parts.push(plural_count(counts.pages, "page"));
    }
    if counts.assets > 0 {
        parts.push(plural_count(counts.assets, "asset"));
    }
    if counts.documents > 0 {
        parts.push(plural_count(counts.documents, "document"));
    }
    parts.join(", ")
}

/// What a round changed, stating `nothing changed` rather than a zero count.
pub(crate) fn describe_change(counts: OutputCounts) -> String {
    match describe_outputs(counts) {
        outputs if outputs.is_empty() => "nothing changed".to_owned(),
        outputs => format!("{outputs} changed"),
    }
}

/// One sentence for a command that worked on the site: the verb, the elapsed time, and an
/// optional detail clause.
pub(crate) fn site_summary(verb: &str, elapsed: std::time::Duration, detail: &str) -> String {
    let duration = format_duration(elapsed);
    if detail.is_empty() {
        format!("{verb} site in {duration}")
    } else {
        format!("{verb} site in {duration}; {detail}")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        describe_change, describe_outputs, notice, secondary, serving_line, site_summary, status,
        stream_label, summary, text,
    };
    use crate::terminal::Palette;
    use tola_build::output::summary::OutputCounts;

    #[test]
    fn colorless_output_stays_unstyled() {
        assert_eq!(
            serving_line("http://127.0.0.1:5277", Palette::new(false)),
            "Serving http://127.0.0.1:5277"
        );
        assert_eq!(
            summary("Built site in 615 ms", Palette::new(false)),
            "Built site in 615 ms"
        );
        assert_eq!(
            notice("first\nsecond", Palette::new(false)),
            "first\n  second"
        );
        assert_eq!(
            secondary("Watching for changes", Palette::new(false)),
            "Watching for changes"
        );
        assert_eq!(stream_label("assets", Palette::new(false)), "[assets] ");
    }

    #[test]
    fn colored_output_stays_styled() {
        assert_eq!(
            summary("Built site in 615 ms", Palette::new(true)),
            "\u{1b}[1m\u{1b}[32mBuilt\u{1b}[39m\u{1b}[0m site in 615 ms"
        );
        assert_eq!(
            notice("first\nsecond", Palette::new(true)),
            "\u{1b}[33mfirst\n  second\u{1b}[39m"
        );
        assert_eq!(
            secondary("Watching for changes", Palette::new(true)),
            "\u{1b}[2mWatching for changes\u{1b}[0m"
        );
        assert_eq!(
            stream_label("assets", Palette::new(true)),
            "\u{1b}[2m[assets] \u{1b}[0m"
        );
    }

    #[test]
    fn status_lines_cannot_escape_their_block() {
        assert_eq!(status("first\nsecond\n"), "first\n  second");
        assert_eq!(text("\nblock\n"), "block");
        assert_eq!(
            status("first\rreplaced\n\u{1b}[2Jsecond"),
            "first\\rreplaced\n  second"
        );
    }

    #[test]
    fn descriptions_list_nonempty_roles() {
        assert_eq!(describe_outputs(OutputCounts::default()), "");
        assert_eq!(
            describe_outputs(OutputCounts {
                pages: 1,
                assets: 0,
                documents: 0
            }),
            "1 page"
        );
        assert_eq!(
            describe_outputs(OutputCounts {
                pages: 2,
                assets: 1,
                documents: 3
            }),
            "2 pages, 1 asset, 3 documents"
        );
    }

    #[test]
    fn summaries_state_the_elapsed_time() {
        let elapsed = std::time::Duration::from_millis(615);

        assert_eq!(
            site_summary("Rebuilt", elapsed, ""),
            "Rebuilt site in 615 ms"
        );
        assert_eq!(
            site_summary(
                "Rebuilt",
                elapsed,
                &describe_change(OutputCounts::default())
            ),
            "Rebuilt site in 615 ms; nothing changed"
        );
        assert_eq!(
            site_summary(
                "Rebuilt",
                elapsed,
                &describe_change(OutputCounts {
                    pages: 1,
                    assets: 0,
                    documents: 0
                })
            ),
            "Rebuilt site in 615 ms; 1 page changed"
        );
        assert_eq!(
            site_summary(
                "Built",
                elapsed,
                &describe_outputs(OutputCounts {
                    pages: 2,
                    assets: 1,
                    documents: 0
                })
            ),
            "Built site in 615 ms; 2 pages, 1 asset"
        );
    }
}
