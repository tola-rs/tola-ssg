//! Negotiated wire shapes for one client; source answers remain client-independent.

use lsp_types::{
    ClientCapabilities, CodeActionOrCommand, CompletionItem, CompletionResponse,
    CompletionTextEdit, Diagnostic, DiagnosticTag, DocumentChangeOperation, DocumentChanges,
    Documentation, HoverContents, InsertTextFormat, MarkedString, MarkupContent, MarkupKind, OneOf,
    PublishDiagnosticsParams, ResourceOp, ResourceOperationKind, SignatureHelp, TextDocumentEdit,
    TextEdit, WorkspaceEdit,
};

use crate::markdown;
use crate::protocol::SourceReply;

#[derive(Clone, Default)]
pub(super) struct ClientFeatures {
    snippets: bool,
    /// Whether the client reads a completion's insert-and-replace edit, which has the range it
    /// replaces and the range it inserts at.
    insert_replace: bool,
    completion_markup: Option<MarkupKind>,
    hover_markup: Option<MarkupKind>,
    signature_markup: Option<MarkupKind>,
    signature_active_parameter: bool,
    pub(super) hierarchical_symbols: bool,
    pub(super) will_rename_files: bool,
    document_changes: bool,
    resource_operations: Vec<ResourceOperationKind>,
    annotations: bool,
    action_annotations: bool,
    action_literals: bool,
    diagnostic_related: bool,
    diagnostic_version: bool,
    diagnostic_tags: Vec<DiagnosticTag>,
    diagnostic_data: bool,
    /// Whether the client pulls diagnostics with `textDocument/diagnostic`.
    pub(super) pull_diagnostics: bool,
    /// Whether the client registers file watchers dynamically, so a file it never opened still
    /// starts a check.
    pub(super) watched_files_dynamic: bool,
    /// Whether the client accepts `workspace/diagnostic/refresh`.
    pub(super) diagnostic_refresh: bool,
    /// Whether the client receives `window/workDoneProgress` and the `$/progress` reports behind
    /// it.
    pub(super) work_done_progress: bool,
    /// Whether the client answers `workspace/configuration`.
    pub(super) configuration: bool,
    /// Whether the client opens a document this server names, through `window/showDocument`.
    pub(super) show_document: bool,
}

impl ClientFeatures {
    pub(super) fn new(capabilities: &ClientCapabilities) -> Self {
        let document = capabilities.text_document.as_ref();
        let completion = document
            .and_then(|document| document.completion.as_ref())
            .and_then(|completion| completion.completion_item.as_ref());
        let signature = document
            .and_then(|document| document.signature_help.as_ref())
            .and_then(|signature| signature.signature_information.as_ref());
        let action = document.and_then(|document| document.code_action.as_ref());
        let diagnostic = document.and_then(|document| document.publish_diagnostics.as_ref());
        let workspace = capabilities.workspace.as_ref();
        let edits = workspace.and_then(|workspace| workspace.workspace_edit.as_ref());
        Self {
            snippets: completion
                .and_then(|completion| completion.snippet_support)
                .unwrap_or(false),
            insert_replace: completion
                .and_then(|completion| completion.insert_replace_support)
                .unwrap_or(false),
            completion_markup: preferred_markup(
                completion.and_then(|completion| completion.documentation_format.as_deref()),
            ),
            hover_markup: preferred_markup(
                document
                    .and_then(|document| document.hover.as_ref())
                    .and_then(|hover| hover.content_format.as_deref()),
            ),
            signature_markup: preferred_markup(
                signature.and_then(|signature| signature.documentation_format.as_deref()),
            ),
            signature_active_parameter: signature
                .and_then(|signature| signature.active_parameter_support)
                .unwrap_or(false),
            hierarchical_symbols: document
                .and_then(|document| document.document_symbol.as_ref())
                .and_then(|symbols| symbols.hierarchical_document_symbol_support)
                .unwrap_or(false),
            will_rename_files: workspace
                .and_then(|workspace| workspace.file_operations.as_ref())
                .and_then(|operations| operations.will_rename)
                .unwrap_or(false),
            document_changes: edits
                .and_then(|edits| edits.document_changes)
                .unwrap_or(false),
            resource_operations: edits
                .and_then(|edits| edits.resource_operations.clone())
                .unwrap_or_default(),
            annotations: edits.is_some_and(|edits| edits.change_annotation_support.is_some()),
            action_annotations: action
                .and_then(|action| action.honors_change_annotations)
                .unwrap_or(false),
            action_literals: action
                .is_some_and(|action| action.code_action_literal_support.is_some()),
            diagnostic_related: diagnostic
                .and_then(|diagnostic| diagnostic.related_information)
                .unwrap_or(false),
            diagnostic_version: diagnostic
                .and_then(|diagnostic| diagnostic.version_support)
                .unwrap_or(false),
            diagnostic_tags: diagnostic
                .and_then(|diagnostic| diagnostic.tag_support.as_ref())
                .map(|tags| tags.value_set.clone())
                .unwrap_or_default(),
            diagnostic_data: diagnostic
                .and_then(|diagnostic| diagnostic.data_support)
                .unwrap_or(false),
            pull_diagnostics: document
                .and_then(|document| document.diagnostic.as_ref())
                .is_some(),
            watched_files_dynamic: workspace
                .and_then(|workspace| workspace.did_change_watched_files.as_ref())
                .and_then(|watched| watched.dynamic_registration)
                .unwrap_or(false),
            diagnostic_refresh: workspace
                .and_then(|workspace| workspace.diagnostic.as_ref())
                .and_then(|diagnostics| diagnostics.refresh_support)
                .unwrap_or(false),
            work_done_progress: capabilities
                .window
                .as_ref()
                .and_then(|window| window.work_done_progress)
                .unwrap_or(false),
            configuration: workspace
                .and_then(|workspace| workspace.configuration)
                .unwrap_or(false),
            show_document: capabilities
                .window
                .as_ref()
                .and_then(|window| window.show_document.as_ref())
                .map(|document| document.support)
                .unwrap_or(false),
        }
    }

    pub(super) fn reply(&self, reply: &mut SourceReply) {
        match reply {
            SourceReply::Completion(response) => {
                let completions = match response {
                    CompletionResponse::Array(completions) => completions,
                    CompletionResponse::List(list) => &mut list.items,
                };
                for completion in completions {
                    self.completion(completion);
                }
            }
            SourceReply::Hover(Some(hover)) => {
                let kind = self.hover_markup.clone().unwrap_or(MarkupKind::Markdown);
                let contents =
                    std::mem::replace(&mut hover.contents, HoverContents::Array(Vec::new()));
                let value = match contents {
                    HoverContents::Markup(markup) => markup_value(markup, &kind),
                    HoverContents::Scalar(marked) => marked_value(marked, &kind),
                    HoverContents::Array(marked) => marked
                        .into_iter()
                        .map(|marked| marked_value(marked, &kind))
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                };
                hover.contents = if self.hover_markup.is_some() {
                    HoverContents::Markup(MarkupContent { kind, value })
                } else {
                    HoverContents::Scalar(MarkedString::String(value))
                };
            }
            SourceReply::SignatureHelp(Some(help)) => self.signature(help),
            SourceReply::Rename(edit) => {
                if let Some(edit) = edit
                    && !self.workspace_edit(edit, self.annotations)
                {
                    *reply = SourceReply::Rename(None);
                }
            }
            SourceReply::CodeActions(Some(actions)) => {
                actions.retain_mut(|action| {
                    let CodeActionOrCommand::CodeAction(action) = action else {
                        return true;
                    };
                    if !self.action_literals {
                        return false;
                    }
                    if let Some(diagnostics) = &mut action.diagnostics {
                        for diagnostic in diagnostics {
                            self.diagnostic(diagnostic);
                        }
                    }
                    action.edit.as_mut().is_none_or(|edit| {
                        self.workspace_edit(edit, self.annotations && self.action_annotations)
                    })
                });
            }
            _ => {}
        }
    }

    fn completion(&self, completion: &mut CompletionItem) {
        if !self.snippets && completion.insert_text_format == Some(InsertTextFormat::SNIPPET) {
            if let Some(text) = &mut completion.insert_text {
                *text = crate::completion::plain_text(text);
            }
            if let Some(edit) = &mut completion.text_edit {
                let text = match edit {
                    CompletionTextEdit::Edit(edit) => &mut edit.new_text,
                    CompletionTextEdit::InsertAndReplace(edit) => &mut edit.new_text,
                };
                *text = crate::completion::plain_text(text);
            }
            completion.insert_text_format = Some(InsertTextFormat::PLAIN_TEXT);
        }
        if !self.insert_replace {
            completion.text_edit = completion.text_edit.take().map(|edit| match edit {
                CompletionTextEdit::InsertAndReplace(insert) => {
                    CompletionTextEdit::Edit(TextEdit {
                        range: insert.replace,
                        new_text: insert.new_text,
                    })
                }
                edit => edit,
            });
        }
        project_documentation(&mut completion.documentation, &self.completion_markup);
    }

    fn signature(&self, help: &mut SignatureHelp) {
        for signature in &mut help.signatures {
            project_documentation(&mut signature.documentation, &self.signature_markup);
            if !self.signature_active_parameter {
                signature.active_parameter = None;
            }
            if let Some(parameters) = &mut signature.parameters {
                for parameter in parameters {
                    project_documentation(&mut parameter.documentation, &self.signature_markup);
                }
            }
        }
    }

    /// Whether this client reads a diagnostic's related information.
    pub(super) fn reads_related_information(&self) -> bool {
        self.diagnostic_related
    }

    /// Whether this client keeps a diagnostic's `data`, so it can echo the cause back with a
    /// request that asks for a fix.
    pub(super) fn reads_diagnostic_data(&self) -> bool {
        self.diagnostic_data
    }

    pub(super) fn diagnostics(&self, parameters: &mut PublishDiagnosticsParams) {
        if !self.diagnostic_version {
            parameters.version = None;
        }
        for diagnostic in &mut parameters.diagnostics {
            self.diagnostic(diagnostic);
        }
    }

    fn diagnostic(&self, diagnostic: &mut Diagnostic) {
        if !self.reads_related_information() {
            diagnostic.related_information = None;
        }
        if !self.reads_diagnostic_data() {
            diagnostic.data = None;
        }
        if let Some(tags) = &mut diagnostic.tags {
            tags.retain(|tag| self.diagnostic_tags.contains(tag));
            if tags.is_empty() {
                diagnostic.tags = None;
            }
        }
    }

    fn workspace_edit(&self, edit: &mut WorkspaceEdit, annotations: bool) -> bool {
        let annotations = annotations && self.document_changes;
        if !annotations
            && edit.change_annotations.as_ref().is_some_and(|annotations| {
                annotations
                    .values()
                    .any(|annotation| annotation.needs_confirmation == Some(true))
            })
        {
            return false;
        }
        if let Some(DocumentChanges::Operations(operations)) = &edit.document_changes {
            for operation in operations {
                if let DocumentChangeOperation::Op(operation) = operation {
                    let kind = match operation {
                        ResourceOp::Create(_) => ResourceOperationKind::Create,
                        ResourceOp::Rename(_) => ResourceOperationKind::Rename,
                        ResourceOp::Delete(_) => ResourceOperationKind::Delete,
                    };
                    if !self.document_changes || !self.resource_operations.contains(&kind) {
                        return false;
                    }
                }
            }
        }
        if !annotations {
            edit.change_annotations = None;
            match &mut edit.document_changes {
                Some(DocumentChanges::Edits(edits)) => {
                    for edit in edits {
                        strip_annotations(edit);
                    }
                }
                Some(DocumentChanges::Operations(operations)) => {
                    for operation in operations {
                        match operation {
                            DocumentChangeOperation::Edit(edit) => strip_annotations(edit),
                            DocumentChangeOperation::Op(ResourceOp::Create(create)) => {
                                create.annotation_id = None
                            }
                            DocumentChangeOperation::Op(ResourceOp::Rename(rename)) => {
                                rename.annotation_id = None
                            }
                            DocumentChangeOperation::Op(ResourceOp::Delete(delete)) => {
                                if let Some(options) = &mut delete.options {
                                    options.annotation_id = None;
                                }
                            }
                        }
                    }
                }
                None => {}
            }
        }
        if !self.document_changes
            && let Some(changes) = edit.document_changes.take()
        {
            let changes = match changes {
                DocumentChanges::Edits(edits) => edits,
                DocumentChanges::Operations(operations) => operations
                    .into_iter()
                    .filter_map(|operation| match operation {
                        DocumentChangeOperation::Edit(edit) => Some(edit),
                        DocumentChangeOperation::Op(_) => None,
                    })
                    .collect(),
            };
            #[expect(clippy::mutable_key_type, reason = "the protocol keys edits by URI")]
            let documents = edit.changes.get_or_insert_default();
            for document in changes {
                documents
                    .entry(document.text_document.uri)
                    .or_default()
                    .extend(document.edits.into_iter().map(|edit| match edit {
                        OneOf::Left(edit) => edit,
                        OneOf::Right(edit) => edit.text_edit,
                    }));
            }
        }
        true
    }
}

/// The first format a client declared, which is the one it prefers; both `MarkupKind`s are formats
/// this server writes.
fn preferred_markup(formats: Option<&[MarkupKind]>) -> Option<MarkupKind> {
    formats.and_then(|formats| formats.first().cloned())
}

fn project_documentation(documentation: &mut Option<Documentation>, kind: &Option<MarkupKind>) {
    let Some(Documentation::MarkupContent(markup)) = documentation else {
        return;
    };
    match kind {
        Some(kind) if markup.kind != *kind => {
            let old = std::mem::replace(
                markup,
                MarkupContent {
                    kind: kind.clone(),
                    value: String::new(),
                },
            );
            markup.value = markup_value(old, kind);
        }
        None => {
            let value = if markup.kind == MarkupKind::Markdown {
                markdown::plain(&markup.value)
            } else {
                std::mem::take(&mut markup.value)
            };
            *documentation = Some(Documentation::String(value));
        }
        _ => {}
    }
}

fn markup_value(markup: MarkupContent, kind: &MarkupKind) -> String {
    if markup.kind == *kind {
        return markup.value;
    }
    if *kind == MarkupKind::PlainText {
        return markdown::plain(&markup.value);
    }
    let mut markdown = String::with_capacity(markup.value.len());
    for ch in markup.value.chars() {
        if ch.is_ascii_punctuation() {
            markdown.push('\\');
        }
        markdown.push(ch);
    }
    markdown
}

fn marked_value(marked: MarkedString, kind: &MarkupKind) -> String {
    match marked {
        MarkedString::String(text) if *kind == MarkupKind::PlainText => markdown::plain(&text),
        MarkedString::String(text) => text,
        MarkedString::LanguageString(code) if *kind == MarkupKind::Markdown => {
            format!("```{}\n{}\n```", code.language, code.value)
        }
        MarkedString::LanguageString(code) => code.value,
    }
}

fn strip_annotations(edit: &mut TextDocumentEdit) {
    for change in &mut edit.edits {
        if let OneOf::Right(annotated) = change {
            *change = OneOf::Left(TextEdit {
                range: annotated.text_edit.range,
                new_text: std::mem::take(&mut annotated.text_edit.new_text),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn features(capabilities: serde_json::Value) -> ClientFeatures {
        ClientFeatures::new(&serde_json::from_value(capabilities).unwrap())
    }

    #[test]
    fn snippet_completion_becomes_plain_text() {
        let mut completion = CompletionItem {
            label: "call".into(),
            insert_text: Some("call(${1:value})$0".into()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        };
        ClientFeatures::default().completion(&mut completion);
        assert_eq!(completion.insert_text.as_deref(), Some("call(value)"));
        assert_eq!(
            completion.insert_text_format,
            Some(InsertTextFormat::PLAIN_TEXT)
        );
    }

    /// The insert-and-replace shape follows the declaration: a client that reads it keeps the edit,
    /// and one that does not receives the replace range as a plain edit.
    #[test]
    fn insert_replace_follows_the_declaration() {
        let insert_replace = json!({
            "newText": "\"brand/logo.svg\"",
            "insert": {"start":{"line":0,"character":11},"end":{"line":0,"character":11}},
            "replace": {"start":{"line":0,"character":11},"end":{"line":0,"character":18}}
        });
        let item = || -> CompletionItem {
            serde_json::from_value(json!({"label": "logo", "textEdit": insert_replace.clone()}))
                .unwrap()
        };
        let mut kept = item();
        features(json!({"textDocument":{"completion":{"completionItem":{
            "insertReplaceSupport":true
        }}}}))
        .completion(&mut kept);
        assert_eq!(
            serde_json::to_value(&kept).unwrap()["textEdit"],
            insert_replace
        );
        let mut replaced = item();
        ClientFeatures::default().completion(&mut replaced);
        assert_eq!(
            serde_json::to_value(&replaced).unwrap()["textEdit"],
            json!({
                "range": {"start":{"line":0,"character":11},"end":{"line":0,"character":18}},
                "newText": "\"brand/logo.svg\""
            })
        );
    }

    #[test]
    fn hover_uses_preferred_markup() {
        for (formats, kind, text) in [
            (
                json!(["plaintext", "markdown"]),
                MarkupKind::PlainText,
                "let snake_name = 1\n\nA name.",
            ),
            (
                json!(["markdown", "plaintext"]),
                MarkupKind::Markdown,
                "```typst\nlet snake_name = 1\n```\n\nA **name**.",
            ),
        ] {
            let client = features(json!({"textDocument":{"hover":{"contentFormat":formats}}}));
            let mut reply = SourceReply::Hover(Some(lsp_types::Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: "```typst\nlet snake_name = 1\n```\n\nA **name**.".into(),
                }),
                range: None,
            }));
            client.reply(&mut reply);
            let SourceReply::Hover(Some(lsp_types::Hover {
                contents: HoverContents::Markup(markup),
                ..
            })) = reply
            else {
                panic!("markup hover");
            };
            assert_eq!(markup.kind, kind);
            assert_eq!(markup.value, text);
        }
    }

    #[test]
    fn document_changes_fold_into_changes() {
        let mut edit: WorkspaceEdit = serde_json::from_value(json!({"documentChanges":[
            {"textDocument":{"uri":"file:///site/a.typ","version":2},"edits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"newText":"renamed","annotationId":"rename"}]}
        ],"changeAnnotations":{"rename":{"label":"rename"}}})).unwrap();
        assert!(ClientFeatures::default().workspace_edit(&mut edit, false));
        assert!(edit.document_changes.is_none());
        assert!(edit.change_annotations.is_none());
        assert_eq!(
            edit.changes.unwrap()[&"file:///site/a.typ".parse::<lsp_types::Uri>().unwrap()][0]
                .new_text,
            "renamed"
        );
    }

    #[test]
    fn unsupported_creation_keeps_no_half_edit() {
        let path = std::env::temp_dir().join("missing.typ");
        let CodeActionOrCommand::CodeAction(action) = crate::protocol::create_file(&path).unwrap()
        else {
            panic!("create action");
        };
        let edit = action.edit.unwrap();
        for capabilities in [
            json!({}),
            json!({"workspace":{"workspaceEdit":{"documentChanges":true,"resourceOperations":["rename"]}}}),
        ] {
            assert!(!features(capabilities).workspace_edit(&mut edit.clone(), true));
        }
        let client = features(
            json!({"workspace":{"workspaceEdit":{"documentChanges":true,"resourceOperations":["create"],"changeAnnotationSupport":{}}}}),
        );
        assert!(client.workspace_edit(&mut edit.clone(), true));
    }

    #[test]
    fn diagnostics_omit_unnegotiated_fields() {
        let mut parameters: PublishDiagnosticsParams = serde_json::from_value(json!({
            "uri":"file:///site/a.typ","version":3,"diagnostics":[{
                "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},
                "message":"bad name","tags":[1],"data":{},
                "relatedInformation":[{"location":{"uri":"file:///site/b.typ","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}}},"message":"declared here"}]
            }]
        })).unwrap();
        ClientFeatures::default().diagnostics(&mut parameters);
        let value = serde_json::to_value(parameters).unwrap();
        assert!(value.get("version").is_none());
        for field in ["relatedInformation", "tags", "data"] {
            assert!(value["diagnostics"][0].get(field).is_none());
        }
        assert_eq!(value["diagnostics"][0]["message"], "bad name");
    }

    #[test]
    fn diagnostics_keep_negotiated_fields() {
        let mut parameters: PublishDiagnosticsParams = serde_json::from_value(json!({
            "uri":"file:///site/a.typ","diagnostics":[{
                "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},
                "message":"unknown variable: x",
                "data":{"kind":"unknown-variable","name":"x"},
                "relatedInformation":[{"location":{"uri":"file:///site/b.typ","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}}},"message":"declared here"}]
            }]
        })).unwrap();
        let client = features(json!({"textDocument":{"publishDiagnostics":{
            "dataSupport":true,"relatedInformation":true
        }}}));
        client.diagnostics(&mut parameters);
        let value = serde_json::to_value(parameters).unwrap();
        assert_eq!(value["diagnostics"][0]["data"]["name"], "x");
        assert_eq!(
            value["diagnostics"][0]["relatedInformation"][0]["message"],
            "declared here"
        );
    }

    #[test]
    fn signature_uses_preferred_markup() {
        let client = features(json!({"textDocument":{"signatureHelp":{
            "signatureInformation":{"documentationFormat":["plaintext"]}
        }}}));
        let mut help: SignatureHelp = serde_json::from_value(json!({
            "signatures":[{"label":"paint(tone)","documentation":{"kind":"markdown","value":"Paint a **tone**."},
                "parameters":[{"label":"tone","documentation":{"kind":"markdown","value":"A `color`."}}]
            }]
        })).unwrap();
        client.signature(&mut help);
        let signature = &help.signatures[0];
        assert_eq!(
            signature.documentation,
            Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::PlainText,
                value: "Paint a tone.".into(),
            }))
        );
        assert_eq!(
            signature.parameters.as_ref().unwrap()[0].documentation,
            Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::PlainText,
                value: "A color.".into(),
            }))
        );
    }
}
