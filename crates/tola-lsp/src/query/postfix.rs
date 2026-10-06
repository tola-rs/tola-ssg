//! The wraps a value accepts, as the completions a client shows.

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, Documentation, InsertTextFormat,
    TextEdit,
};
use tola_typst_syntax::typst_syntax::Source;

use crate::position;

/// The postfix completions the value under the cursor accepts.
pub(super) fn completions(source: &Source, cursor: usize) -> Vec<CompletionItem> {
    tola_typst_syntax::wraps::wraps(source, cursor)
        .into_iter()
        .filter_map(|wrap| {
            let range = position::utf16_range(source.lines(), wrap.edit.range)?;
            let mut text = String::with_capacity(wrap.edit.text.len() + 2);
            if let Some(insertion) = wrap.edit.insertion {
                crate::completion::escape(&wrap.edit.text[..insertion], &mut text);
                text.push_str("$0");
                crate::completion::escape(&wrap.edit.text[insertion..], &mut text);
            } else {
                crate::completion::escape(&wrap.edit.text, &mut text);
            }
            Some(CompletionItem {
                label: wrap.name.to_owned(),
                kind: Some(CompletionItemKind::SNIPPET),
                detail: Some(format!(".{}", wrap.name)),
                documentation: Some(Documentation::String(wrap.documentation.to_owned())),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: text,
                })),
                ..CompletionItem::default()
            })
        })
        .collect()
}
