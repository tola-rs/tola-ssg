//! Help for scripts bundled with the Nu runtime.

use std::fmt::Write as _;

use anyhow::{Result, ensure};
use tola_nu::{BuiltinModule, Runtime, Signature};

use super::{
    data::{Command, Parameter},
    model::{HelpPage, PageId, anchor},
    pages::{code_block, target_error},
};
use crate::i18n::{self, HelpLanguage, Translation};

pub(super) fn builtins() -> impl Iterator<Item = &'static BuiltinModule> {
    tola_nu::BUILTIN_MODULES
        .iter()
        .filter(|script| script.executable)
}

pub(super) fn page(id: PageId, name: &str, language: HelpLanguage) -> Result<HelpPage> {
    let locator = format!("@tola-scripts/{name}");
    let script = builtins()
        .find(|script| script.name == name)
        .ok_or_else(|| {
            target_error(&locator, "Use `tola help scripts` to list built-in scripts")
        })?;
    let runtime = Runtime::new(Vec::new(), Vec::new())?;
    let entry = runtime.resolve_locator(&locator)?;
    let signatures = runtime.script_signatures(&entry.path)?;
    ensure!(
        !signatures.is_empty(),
        "`{locator}` has no Nu command declarations"
    );

    let prose = i18n::script(language, name);
    let intro = prose
        .and_then(|prose| prose.intro())
        .unwrap_or(script.summary);
    let mut markdown = format!("# {locator}\n\n{intro}\n");
    let mut page = HelpPage::new(id, String::new());
    page.reference.title = locator.clone();
    page.reference.blocks = page.prose(intro);
    for signature in &signatures {
        let command = signature
            .name
            .strip_prefix("main ")
            .unwrap_or(&signature.name);
        let invocation = if command == "main" {
            format!("tola nu {locator}")
        } else {
            format!("tola nu {locator} {command}")
        };
        writeln!(markdown, "\n## `{command}`\n").unwrap();
        let usage = usage(&invocation, signature);
        if usage != invocation {
            markdown.push_str(&code_block("sh", &usage));
        }
        let description = prose.and_then(|prose| prose.section(command)).map_or_else(
            || {
                format!(
                    "{}\n\n{}",
                    signature.description, signature.extra_description
                )
            },
            str::to_owned,
        );
        writeln!(markdown, "\n{description}").unwrap();
        let (parameters, options) =
            write_parameters(&mut markdown, &page, signature, prose, command, language);
        page.reference.commands.push(Command {
            name: command.to_owned(),
            anchor: anchor(command).as_str().to_owned(),
            usage,
            blocks: page.prose(&description),
            parameters,
            options,
        });
        let native_help = match language {
            HelpLanguage::English => "Nu help",
            HelpLanguage::SimplifiedChinese => "查看 Nu 帮助",
        };
        writeln!(
            markdown,
            "\n{native_help}{}`{invocation} --help`",
            language.colon()
        )
        .unwrap();
    }
    page.push_markdown(markdown);
    Ok(page)
}

fn usage(invocation: &str, signature: &Signature) -> String {
    let mut usage = invocation.to_owned();
    for arg in &signature.required_positional {
        write!(usage, " <{}>", arg.name).unwrap();
    }
    for arg in &signature.optional_positional {
        write!(usage, " [{}]", arg.name).unwrap();
    }
    if let Some(arg) = &signature.rest_positional {
        write!(usage, " [{}...]", arg.name).unwrap();
    }
    if signature.named.iter().any(|flag| flag.long != "help") {
        usage.push_str(" [OPTIONS]");
    }
    usage
}

fn write_parameters(
    markdown: &mut String,
    page: &HelpPage,
    signature: &Signature,
    prose: Option<&Translation>,
    command: &str,
    language: HelpLanguage,
) -> (Vec<Parameter>, Vec<Parameter>) {
    let mut parameters = Vec::new();
    let mut options_data = Vec::new();
    let (positionals, options) = match language {
        HelpLanguage::English => ("Positional parameters", "Options"),
        HelpLanguage::SimplifiedChinese => ("位置参数", "选项"),
    };
    if !signature.required_positional.is_empty()
        || !signature.optional_positional.is_empty()
        || signature.rest_positional.is_some()
    {
        writeln!(markdown, "\n**{positionals}**\n").unwrap();
    }
    for (argument, is_required, prefix) in signature
        .required_positional
        .iter()
        .map(|arg| (arg, true, ""))
        .chain(
            signature
                .optional_positional
                .iter()
                .map(|arg| (arg, false, "")),
        )
        .chain(
            signature
                .rest_positional
                .iter()
                .map(|arg| (arg, false, "...")),
        )
    {
        let description = prose
            .and_then(|prose| prose.field(command, &argument.name))
            .unwrap_or(&argument.desc);
        writeln!(
            markdown,
            "- `{prefix}{}`{}{description}",
            argument.name,
            language.colon()
        )
        .unwrap();
        parameters.push(Parameter {
            name: argument.name.clone(),
            kind: if prefix.is_empty() {
                "positional"
            } else {
                "rest"
            },
            ty: Some(argument.shape.to_string()),
            required: is_required,
            default: argument
                .default_value
                .as_ref()
                .map(|value| value.to_parsable_string(", ", &Default::default())),
            blocks: page.prose(description),
            short: None,
        });
    }
    let mut flags = signature
        .named
        .iter()
        .filter(|flag| flag.long != "help")
        .peekable();
    if flags.peek().is_some() {
        writeln!(markdown, "\n**{options}**\n").unwrap();
    }
    for flag in flags {
        let heading = match (flag.long_name(), flag.short) {
            (Some(long), Some(short)) => format!("`--{long}`, `-{short}`"),
            (Some(long), None) => format!("`--{long}`"),
            (None, Some(short)) => format!("`-{short}`"),
            (None, None) => continue,
        };
        let name = format!("--{}", flag.long);
        let description = prose
            .and_then(|prose| prose.field(command, &name))
            .unwrap_or(&flag.desc);
        writeln!(markdown, "- {heading}{}{description}", language.colon()).unwrap();
        options_data.push(Parameter {
            name,
            kind: "option",
            ty: flag.arg.as_ref().map(ToString::to_string),
            required: flag.required,
            default: flag
                .default_value
                .as_ref()
                .map(|value| value.to_parsable_string(", ", &Default::default())),
            blocks: page.prose(description),
            short: flag.short,
        });
    }
    (parameters, options_data)
}
