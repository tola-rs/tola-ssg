//! Program arguments supplied through a personal editor or pager preference.

use std::ffi::{OsStr, OsString};

use anyhow::{Result, bail};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommandLine {
    pub program: OsString,
    pub arguments: Vec<OsString>,
}

impl CommandLine {
    /// Quotes group words; backslashes escape a quote or unquoted whitespace. Other
    /// backslashes remain literal, including those in Windows paths. No shell expansion runs.
    pub(crate) fn parse(source: &OsStr) -> Result<Option<Self>> {
        let Some(source) = source.to_str() else {
            bail!("the command is not valid UTF-8");
        };
        let mut characters = source.chars().peekable();
        let mut quote = None;
        let mut word = String::new();
        let mut started = false;
        let mut words = Vec::new();
        while let Some(character) = characters.next() {
            if character == '\\' && quote != Some('\'') {
                let escapes = characters.peek().is_some_and(|next| {
                    Some(*next) == quote
                        || (quote.is_none() && (next.is_whitespace() || matches!(next, '\'' | '"')))
                });
                if escapes {
                    word.push(characters.next().expect("the escaped character exists"));
                } else {
                    word.push(character);
                }
                started = true;
                continue;
            }
            match quote {
                Some(delimiter) if character == delimiter => quote = None,
                Some(_) => word.push(character),
                None if matches!(character, '\'' | '"') => quote = Some(character),
                None if character.is_whitespace() => {
                    if started {
                        words.push(std::mem::take(&mut word));
                        started = false;
                    }
                    continue;
                }
                None => word.push(character),
            }
            started = true;
        }
        if quote.is_some() {
            bail!("the command has an unclosed quote");
        }
        if started {
            words.push(word);
        }
        let mut words = words.into_iter();
        let Some(program) = words.next() else {
            return Ok(None);
        };
        if program.is_empty() {
            bail!("the command's program name is empty");
        }
        Ok(Some(Self {
            program: program.into(),
            arguments: words.map(OsString::from).collect(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_commands_keep_argument_boundaries() {
        for (source, program, arguments) in [
            ("code --wait", "code", vec!["--wait"]),
            (
                "'/tools/my editor' --wait",
                "/tools/my editor",
                vec!["--wait"],
            ),
            (
                r#""C:\Program Files\Editor\edit.exe" --wait"#,
                r"C:\Program Files\Editor\edit.exe",
                vec!["--wait"],
            ),
            (
                r"\\server\share\editor.exe",
                r"\\server\share\editor.exe",
                vec![],
            ),
            (r"my\ editor --wait", "my editor", vec!["--wait"]),
            (
                r#"editor "" '$HOME' ';' '$(pwd)'"#,
                "editor",
                vec!["", "$HOME", ";", "$(pwd)"],
            ),
        ] {
            let command = CommandLine::parse(OsStr::new(source)).unwrap().unwrap();
            assert_eq!(command.program, OsStr::new(program));
            assert_eq!(
                command.arguments,
                arguments
                    .iter()
                    .map(|argument| OsString::from(*argument))
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn unfinished_commands_are_rejected() {
        for source in ["'editor", "editor \"argument", "'' --wait"] {
            assert!(CommandLine::parse(OsStr::new(source)).is_err(), "{source}");
        }
        assert_eq!(CommandLine::parse(OsStr::new(" \t ")).unwrap(), None);
    }
}
