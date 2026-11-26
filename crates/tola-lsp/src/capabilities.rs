//! Negotiated wire shapes for one client; source answers remain client-independent.

use lsp_types::{
    ClientCapabilities, CodeActionOrCommand, CompletionItem, CompletionResponse,
    CompletionTextEdit, Diagnostic, DiagnosticTag, DocumentChangeOperation, DocumentChanges,
    Documentation, Hover, HoverContents, InsertTextFormat, MarkedString, MarkupContent, MarkupKind,
    OneOf, PublishDiagnosticsParams, ResourceOp, ResourceOperationKind, SignatureHelp,
    TextDocumentEdit, TextEdit, WorkspaceEdit,
};

use crate::markdown;
use crate::protocol::SourceReply;

#[derive(Clone, Default)]
pub(super) struct ClientFeatures {
    snippets: bool,
    insert_replace: bool,
    completion_label_details: bool,
    completion_markup: Option<MarkupKind>,
    hover_markup: Option<MarkupKind>,
    signature_markup: Option<MarkupKind>,
    signature_active_parameter: bool,
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
    pub(super) pull_diagnostics: bool,
    pub(super) watched_files_dynamic: bool,
    pub(super) watched_files_relative: bool,
    pub(super) semantic_tokens_refresh: bool,
    pub(super) diagnostic_refresh: bool,
    pub(super) work_done_progress: bool,
    pub(super) configuration: bool,
    pub(super) show_document: bool,
}

impl ClientFeatures {
    pub(super) fn new(caps: &ClientCapabilities) -> Self {
        let doc = caps.text_document.as_ref();
        let comp = doc
            .and_then(|d| d.completion.as_ref())
            .and_then(|c| c.completion_item.as_ref());
        let sig = doc
            .and_then(|d| d.signature_help.as_ref())
            .and_then(|s| s.signature_information.as_ref());
        let action = doc.and_then(|d| d.code_action.as_ref());
        let publish_diag = doc.and_then(|d| d.publish_diagnostics.as_ref());
        let ws = caps.workspace.as_ref();
        let edits = ws.and_then(|w| w.workspace_edit.as_ref());
        let watched = ws.and_then(|w| w.did_change_watched_files.as_ref());
        let window = caps.window.as_ref();
        Self {
            snippets: comp.and_then(|c| c.snippet_support).unwrap_or(false),
            insert_replace: comp.and_then(|c| c.insert_replace_support).unwrap_or(false),
            completion_label_details: comp.and_then(|c| c.label_details_support).unwrap_or(false),
            completion_markup: preferred_markup(
                comp.and_then(|c| c.documentation_format.as_deref()),
            ),
            hover_markup: preferred_markup(
                doc.and_then(|d| d.hover.as_ref())
                    .and_then(|h| h.content_format.as_deref()),
            ),
            signature_markup: preferred_markup(sig.and_then(|s| s.documentation_format.as_deref())),
            signature_active_parameter: sig
                .and_then(|s| s.active_parameter_support)
                .unwrap_or(false),
            will_rename_files: ws
                .and_then(|w| w.file_operations.as_ref())
                .and_then(|o| o.will_rename)
                .unwrap_or(false),
            document_changes: edits.and_then(|e| e.document_changes).unwrap_or(false),
            resource_operations: edits
                .and_then(|e| e.resource_operations.clone())
                .unwrap_or_default(),
            annotations: edits.is_some_and(|e| e.change_annotation_support.is_some()),
            action_annotations: action
                .and_then(|a| a.honors_change_annotations)
                .unwrap_or(false),
            action_literals: action.is_some_and(|a| a.code_action_literal_support.is_some()),
            diagnostic_related: publish_diag
                .and_then(|d| d.related_information)
                .unwrap_or(false),
            diagnostic_version: publish_diag
                .and_then(|d| d.version_support)
                .unwrap_or(false),
            diagnostic_tags: publish_diag
                .and_then(|d| d.tag_support.as_ref())
                .map(|t| t.value_set.clone())
                .unwrap_or_default(),
            diagnostic_data: publish_diag.and_then(|d| d.data_support).unwrap_or(false),
            pull_diagnostics: doc.and_then(|d| d.diagnostic.as_ref()).is_some(),
            watched_files_dynamic: watched
                .and_then(|w| w.dynamic_registration)
                .unwrap_or(false),
            watched_files_relative: watched
                .and_then(|w| w.relative_pattern_support)
                .unwrap_or(false),
            semantic_tokens_refresh: ws
                .and_then(|w| w.semantic_tokens.as_ref())
                .and_then(|t| t.refresh_support)
                .unwrap_or(false),
            diagnostic_refresh: ws
                .and_then(|w| w.diagnostic.as_ref())
                .and_then(|d| d.refresh_support)
                .unwrap_or(false),
            work_done_progress: window.and_then(|w| w.work_done_progress).unwrap_or(false),
            configuration: ws.and_then(|w| w.configuration).unwrap_or(false),
            show_document: window
                .and_then(|w| w.show_document.as_ref())
                .map(|d| d.support)
                .unwrap_or(false),
        }
    }

    pub(super) fn reply(&self, reply: &mut SourceReply) {
        match reply {
            SourceReply::Completion(resp) => {
                let items = match resp {
                    CompletionResponse::Array(items) => items,
                    CompletionResponse::List(list) => &mut list.items,
                };
                for item in items {
                    self.completion(item);
                }
            }
            SourceReply::Hover(Some(hover)) => self.hover(hover),
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
                    for diag in action.diagnostics.iter_mut().flatten() {
                        self.diagnostic(diag);
                    }
                    action.edit.as_mut().is_none_or(|edit| {
                        self.workspace_edit(edit, self.annotations && self.action_annotations)
                    })
                });
            }
            _ => {}
        }
    }

    fn hover(&self, hover: &mut Hover) {
        let kind = self.hover_markup.clone().unwrap_or(MarkupKind::Markdown);
        let contents = std::mem::replace(&mut hover.contents, HoverContents::Array(Vec::new()));
        let value = match contents {
            HoverContents::Markup(markup) => markup_value(markup, &kind),
            HoverContents::Scalar(marked) => marked_value(marked, &kind),
            HoverContents::Array(marked) => marked
                .into_iter()
                .map(|m| marked_value(m, &kind))
                .collect::<Vec<_>>()
                .join("\n\n"),
        };
        hover.contents = if self.hover_markup.is_some() {
            HoverContents::Markup(MarkupContent { kind, value })
        } else {
            HoverContents::Scalar(MarkedString::String(value))
        };
    }

    fn completion(&self, item: &mut CompletionItem) {
        if !self.completion_label_details
            && let Some(labels) = item.label_details.take()
        {
            item.detail = [item.detail.take(), labels.detail, labels.description]
                .into_iter()
                .flatten()
                .reduce(|detail, label| format!("{detail}\n{label}"));
        }
        if !self.snippets && item.insert_text_format == Some(InsertTextFormat::SNIPPET) {
            if let Some(text) = &mut item.insert_text {
                *text = crate::completion::plain_text(text);
            }
            if let Some(edit) = &mut item.text_edit {
                let text = match edit {
                    CompletionTextEdit::Edit(edit) => &mut edit.new_text,
                    CompletionTextEdit::InsertAndReplace(edit) => &mut edit.new_text,
                };
                *text = crate::completion::plain_text(text);
            }
            item.insert_text_format = Some(InsertTextFormat::PLAIN_TEXT);
        }
        if !self.insert_replace {
            item.text_edit = item.text_edit.take().map(|edit| match edit {
                CompletionTextEdit::InsertAndReplace(insert) => {
                    CompletionTextEdit::Edit(TextEdit {
                        range: insert.replace,
                        new_text: insert.new_text,
                    })
                }
                edit => edit,
            });
        }
        project_documentation(&mut item.documentation, &self.completion_markup);
    }

    fn signature(&self, help: &mut SignatureHelp) {
        if !self.signature_active_parameter {
            help.active_parameter = help
                .signatures
                .get(help.active_signature.unwrap_or(0) as usize)
                .or_else(|| help.signatures.first())
                .and_then(|sig| sig.active_parameter)
                .or(help.active_parameter);
        }
        for sig in &mut help.signatures {
            project_documentation(&mut sig.documentation, &self.signature_markup);
            if !self.signature_active_parameter {
                sig.active_parameter = None;
            }
            if let Some(params) = &mut sig.parameters {
                for param in params {
                    project_documentation(&mut param.documentation, &self.signature_markup);
                }
            }
        }
    }

    pub(super) fn reads_related_information(&self) -> bool {
        self.diagnostic_related
    }

    /// Whether this client echoes `diagnostic.data` back in code-action requests.
    pub(super) fn reads_diagnostic_data(&self) -> bool {
        self.diagnostic_data
    }

    pub(super) fn diagnostics(&self, params: &mut PublishDiagnosticsParams) {
        if !self.diagnostic_version {
            params.version = None;
        }
        for diag in &mut params.diagnostics {
            self.diagnostic(diag);
        }
    }

    fn diagnostic(&self, diag: &mut Diagnostic) {
        if !self.reads_related_information() {
            diag.related_information = None;
        }
        if !self.reads_diagnostic_data() {
            diag.data = None;
        }
        if let Some(tags) = &mut diag.tags {
            tags.retain(|tag| self.diagnostic_tags.contains(tag));
            if tags.is_empty() {
                diag.tags = None;
            }
        }
    }

    fn workspace_edit(&self, edit: &mut WorkspaceEdit, keep_annotations: bool) -> bool {
        let keep_annotations = keep_annotations && self.document_changes;
        if !keep_annotations
            && edit.change_annotations.as_ref().is_some_and(|anns| {
                anns.values()
                    .any(|ann| ann.needs_confirmation == Some(true))
            })
        {
            return false;
        }
        if let Some(DocumentChanges::Operations(ops)) = &edit.document_changes {
            for op in ops {
                if let DocumentChangeOperation::Op(op) = op {
                    let kind = match op {
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
        if !keep_annotations {
            strip_edit_annotations(edit);
        }
        if !self.document_changes
            && let Some(changes) = edit.document_changes.take()
        {
            fold_document_changes(edit, changes);
        }
        true
    }
}

fn strip_edit_annotations(edit: &mut WorkspaceEdit) {
    edit.change_annotations = None;
    match &mut edit.document_changes {
        Some(DocumentChanges::Edits(edits)) => edits.iter_mut().for_each(strip_doc_annotations),
        Some(DocumentChanges::Operations(ops)) => {
            for op in ops {
                match op {
                    DocumentChangeOperation::Edit(edit) => strip_doc_annotations(edit),
                    DocumentChangeOperation::Op(ResourceOp::Create(c)) => c.annotation_id = None,
                    DocumentChangeOperation::Op(ResourceOp::Rename(r)) => r.annotation_id = None,
                    DocumentChangeOperation::Op(ResourceOp::Delete(d)) => {
                        if let Some(opts) = &mut d.options {
                            opts.annotation_id = None;
                        }
                    }
                }
            }
        }
        None => {}
    }
}

fn fold_document_changes(edit: &mut WorkspaceEdit, changes: DocumentChanges) {
    let changes = match changes {
        DocumentChanges::Edits(edits) => edits,
        DocumentChanges::Operations(ops) => ops
            .into_iter()
            .filter_map(|op| match op {
                DocumentChangeOperation::Edit(edit) => Some(edit),
                DocumentChangeOperation::Op(_) => None,
            })
            .collect(),
    };
    let docs = edit.changes.get_or_insert_default();
    for doc in changes {
        docs.entry(doc.text_document.uri)
            .or_default()
            .extend(doc.edits.into_iter().map(|edit| match edit {
                OneOf::Left(edit) => edit,
                OneOf::Right(edit) => edit.text_edit,
            }));
    }
}

fn preferred_markup(formats: Option<&[MarkupKind]>) -> Option<MarkupKind> {
    formats.and_then(|formats| formats.first().cloned())
}

fn project_documentation(doc: &mut Option<Documentation>, kind: &Option<MarkupKind>) {
    let Some(Documentation::MarkupContent(markup)) = doc else {
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
            *doc = Some(Documentation::String(value));
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
    let mut md = String::with_capacity(markup.value.len());
    for ch in markup.value.chars() {
        if ch.is_ascii_punctuation() {
            md.push('\\');
        }
        md.push(ch);
    }
    md
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

fn strip_doc_annotations(edit: &mut TextDocumentEdit) {
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

    fn features(caps: serde_json::Value) -> ClientFeatures {
        ClientFeatures::new(&serde_json::from_value(caps).unwrap())
    }

    #[test]
    fn completion_details_projection() {
        for supported in [false, true] {
            let client = features(json!({"textDocument":{"completion":{"completionItem":{
                "labelDetailsSupport":supported
            }}}}));
            for detail in [None, Some("bibliography citation")] {
                let mut item: CompletionItem = serde_json::from_value(json!({
                    "label":"draw", "detail":detail,
                    "labelDetails":{"detail":"(value)","description":"draw(value: int)"}
                }))
                .unwrap();
                let orig = item.clone();
                let mut reply = SourceReply::Completion(CompletionResponse::Array(vec![item]));
                client.reply(&mut reply);
                let SourceReply::Completion(CompletionResponse::Array(mut items)) = reply else {
                    panic!("completion array");
                };
                item = items.remove(0);
                if supported {
                    assert_eq!(item, orig);
                } else {
                    assert!(item.label_details.is_none());
                    let expected = match detail {
                        Some(detail) => format!("{detail}\n(value)\ndraw(value: int)"),
                        None => "(value)\ndraw(value: int)".to_owned(),
                    };
                    assert_eq!(item.detail, Some(expected));
                    assert_eq!(item.label, orig.label);
                }
            }
        }
    }

    #[test]
    fn document_changes_fold() {
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
    fn unsupported_creation_rejected() {
        let path = std::env::temp_dir().join("missing.typ");
        let CodeActionOrCommand::CodeAction(action) = crate::protocol::create_file(&path).unwrap()
        else {
            panic!("create action");
        };
        let edit = action.edit.unwrap();
        for caps in [
            json!({}),
            json!({"workspace":{"workspaceEdit":{"documentChanges":true,"resourceOperations":["rename"]}}}),
        ] {
            assert!(!features(caps).workspace_edit(&mut edit.clone(), true));
        }
        let client = features(
            json!({"workspace":{"workspaceEdit":{"documentChanges":true,"resourceOperations":["create"],"changeAnnotationSupport":{}}}}),
        );
        assert!(client.workspace_edit(&mut edit.clone(), true));
    }

    #[test]
    fn diagnostics_strip_unsupported() {
        let mut params: PublishDiagnosticsParams = serde_json::from_value(json!({
            "uri":"file:///site/a.typ","version":3,"diagnostics":[{
                "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},
                "message":"bad name","tags":[1],"data":{},
                "relatedInformation":[{"location":{"uri":"file:///site/b.typ","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}}},"message":"declared here"}]
            }]
        })).unwrap();
        ClientFeatures::default().diagnostics(&mut params);
        let val = serde_json::to_value(params).unwrap();
        assert!(val.get("version").is_none());
        for field in ["relatedInformation", "tags", "data"] {
            assert!(val["diagnostics"][0].get(field).is_none());
        }
        assert_eq!(val["diagnostics"][0]["message"], "bad name");
    }
}
