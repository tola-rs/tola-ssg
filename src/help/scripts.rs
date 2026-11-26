//! Help pages for scripts bundled with the Nu runtime.

use std::fmt::Write as _;

use anyhow::{Result, ensure};
use tola_nu::{BuiltinModule, Runtime, Signature};

use super::docs;
use super::language::HelpLanguage;
use super::markup::Markup;
use super::model::{Parameter, ScriptCommand, ScriptPage, anchor};
use super::pages::target_error;

pub(super) fn builtins() -> impl Iterator<Item = &'static BuiltinModule> {
    tola_nu::BUILTIN_MODULES
        .iter()
        .filter(|script| script.executable)
}

pub(super) fn page(
    name: &str,
    command_filter: Option<&str>,
    language: HelpLanguage,
) -> Result<ScriptPage> {
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

    let doc =
        docs::script(language, name).unwrap_or_else(|| panic!("missing script docs for `{name}`"));
    let mut commands = Vec::new();
    for signature in &signatures {
        let command_name = signature
            .name
            .strip_prefix("main ")
            .unwrap_or(&signature.name);
        if let Some(wanted) = command_filter
            && anchor(command_name).as_str() != wanted
        {
            continue;
        }
        let invocation = if command_name == "main" {
            format!("tola nu {locator}")
        } else {
            format!("tola nu {locator} {command_name}")
        };
        let command_doc = doc.commands.get(command_name);
        let summary = command_doc
            .map(|item| item.summary.clone())
            .filter(|markup| !markup.is_empty())
            .unwrap_or_else(|| Markup::parse(&signature.description).unwrap_or_default());
        let body = command_doc
            .map(|item| item.body.clone())
            .unwrap_or_default();
        let examples = command_doc
            .map(|item| item.examples.clone())
            .unwrap_or_default();
        let (parameters, options) = collect_parameters(signature, command_doc);
        commands.push(ScriptCommand {
            name: command_name.to_owned(),
            usage: usage(&invocation, signature),
            summary,
            body,
            parameters,
            options,
            examples,
        });
    }
    if let Some(wanted) = command_filter {
        ensure!(
            !commands.is_empty(),
            "`{locator}` has no command matching `{wanted}`"
        );
    }
    let summary = if command_filter.is_some() {
        Markup::default()
    } else if doc.summary.is_empty() {
        Markup::parse(script.summary).unwrap_or_default()
    } else {
        doc.summary.clone()
    };
    let body = if command_filter.is_some() {
        Markup::default()
    } else {
        doc.body.clone()
    };
    Ok(ScriptPage {
        name: name.to_owned(),
        locator,
        language: language.tag(),
        summary,
        body,
        commands,
    })
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

fn collect_parameters(
    signature: &Signature,
    doc: Option<&super::markup::CommandDoc>,
) -> (Vec<Parameter>, Vec<Parameter>) {
    let mut parameters = Vec::new();
    for (argument, is_required, kind) in signature
        .required_positional
        .iter()
        .map(|arg| (arg, true, "positional"))
        .chain(
            signature
                .optional_positional
                .iter()
                .map(|arg| (arg, false, "positional")),
        )
        .chain(
            signature
                .rest_positional
                .iter()
                .map(|arg| (arg, false, "rest")),
        )
    {
        let description = doc
            .and_then(|item| item.params.get(&argument.name))
            .cloned()
            .unwrap_or_else(|| Markup::parse(&argument.desc).unwrap_or_default());
        parameters.push(Parameter {
            name: argument.name.clone(),
            kind,
            ty: Some(argument.shape.to_string()),
            required: is_required,
            default: argument
                .default_value
                .as_ref()
                .map(|value| value.to_parsable_string(", ", &Default::default())),
            short: None,
            description,
        });
    }

    let mut options = Vec::new();
    for flag in signature.named.iter().filter(|flag| flag.long != "help") {
        let name = format!("--{}", flag.long);
        let description = doc
            .and_then(|item| item.params.get(&name))
            .cloned()
            .unwrap_or_else(|| Markup::parse(&flag.desc).unwrap_or_default());
        options.push(Parameter {
            name,
            kind: "option",
            ty: flag.arg.as_ref().map(ToString::to_string),
            required: flag.required,
            default: flag
                .default_value
                .as_ref()
                .map(|value| value.to_parsable_string(", ", &Default::default())),
            short: flag.short,
            description,
        });
    }
    (parameters, options)
}
