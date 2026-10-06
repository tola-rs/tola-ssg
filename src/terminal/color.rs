//! Color decisions shared with Clap's stream renderer.

use clap::ColorChoice;

#[derive(Clone, Copy)]
pub(crate) enum Stream {
    Stdout,
    Stderr,
}

pub(crate) fn enabled(choice: ColorChoice, stream: Stream) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => {
            let choice = match stream {
                Stream::Stdout => anstream::AutoStream::choice(&std::io::stdout()),
                Stream::Stderr => anstream::AutoStream::choice(&std::io::stderr()),
            };
            matches!(
                choice,
                anstream::ColorChoice::Always | anstream::ColorChoice::AlwaysAnsi
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_choice_overrides_detection() {
        for stream in [Stream::Stdout, Stream::Stderr] {
            assert!(enabled(ColorChoice::Always, stream));
            assert!(!enabled(ColorChoice::Never, stream));
        }
    }
}
