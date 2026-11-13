//! Help for scripts bundled with the Nu runtime.

use std::fmt::Write as _;

use anyhow::{Result, ensure};
use tola_nu::{BuiltinModule, Runtime, Signature};

use super::{
    model::{HelpPage, PageId},
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
        if let Some(example) = prose.and_then(|prose| prose.section(command)) {
            writeln!(markdown, "\n{example}").unwrap();
        } else {
            writeln!(markdown, "\n{}", signature.description).unwrap();
            if !signature.extra_description.is_empty() {
                writeln!(markdown, "\n{}", signature.extra_description).unwrap();
            }
        }
        write_parameters(&mut markdown, signature, prose, command, language);
        let native_help = match language {
            HelpLanguage::English => "Nu help",
            HelpLanguage::SimplifiedChinese => "查看 Nu 帮助",
        };
        writeln!(markdown, "\n{native_help} · `{invocation} --help`").unwrap();
    }
    Ok(HelpPage::new(id, markdown))
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
    signature: &Signature,
    prose: Option<&Translation>,
    command: &str,
    language: HelpLanguage,
) {
    let (positionals, options, required, default) = match language {
        HelpLanguage::English => ("Positional parameters", "Options", "required", "default"),
        HelpLanguage::SimplifiedChinese => ("位置参数", "选项", "必填", "默认"),
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
        write!(
            markdown,
            "- `{prefix}{}: {}`",
            argument.name, argument.shape
        )
        .unwrap();
        if is_required {
            write!(markdown, " · {required}").unwrap();
        }
        if let Some(value) = &argument.default_value {
            let value = value.to_parsable_string(", ", &Default::default());
            write!(markdown, " · {default} `{value}`").unwrap();
        }
        let description = prose
            .and_then(|prose| prose.field(command, &argument.name))
            .unwrap_or(&argument.desc);
        writeln!(markdown, "{}{description}", language.colon()).unwrap();
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
        write!(markdown, "- {heading}").unwrap();
        if let Some(shape) = &flag.arg {
            write!(markdown, " `<{shape}>`").unwrap();
        }
        if flag.required {
            write!(markdown, " · {required}").unwrap();
        }
        if let Some(value) = &flag.default_value {
            let value = value.to_parsable_string(", ", &Default::default());
            write!(markdown, " · {default} `{value}`").unwrap();
        }
        let name = format!("--{}", flag.long);
        let description = prose
            .and_then(|prose| prose.field(command, &name))
            .unwrap_or(&flag.desc);
        writeln!(markdown, "{}{description}", language.colon()).unwrap();
    }
}
