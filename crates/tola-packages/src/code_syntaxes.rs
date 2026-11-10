use typst::foundations::{Bytes, Dict, Str, Value, func};

/// Extra syntax files for Typst's `raw.syntaxes`.
pub(super) fn files() -> Dict {
    let mut files = Dict::new();
    for &(language, source) in tola_highlight::CODE_SYNTAXES {
        files.insert(language.into(), Value::Bytes(Bytes::from_string(source)));
    }
    files
}

/// The built-in syntax name for a source-code label.
#[func]
pub(super) fn tola_code_language(language: Str) -> Str {
    tola_highlight::code_language(language.as_str()).into()
}
