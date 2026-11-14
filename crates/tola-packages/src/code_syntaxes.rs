use std::sync::OnceLock;

use typst::diag::SourceResult;
use typst::engine::Engine;
use typst::foundations::{
    Arg, Args, Array, Bytes, Content, Dict, NativeElement, NativeRuleMap, Packed, Smart, Str,
    StyleChain, Styles, Synthesize, Target, TargetElem, Value, func,
};
use typst::syntax::{Span, Spanned};
use typst::text::RawElem;

static ORIGINAL_RULES: OnceLock<NativeRuleMap> = OnceLock::new();
static BUNDLED_SYNTAXES: OnceLock<Styles> = OnceLock::new();

pub(super) fn install(rules: &mut NativeRuleMap) {
    ORIGINAL_RULES.get_or_init(|| rules.clone());
    rules.replace(Target::Html, show_raw);
    rules.replace(Target::Paged, show_raw);
}

fn show_raw(
    raw: &Packed<RawElem>,
    engine: &mut Engine,
    styles: StyleChain,
) -> SourceResult<Content> {
    let raw = with_bundled_syntax(raw, engine, styles)?
        .unwrap_or_else(|| raw.clone())
        .pack();
    ORIGINAL_RULES
        .get()
        .expect("the original show rules are installed")
        .get(styles.get(TargetElem::target), &raw)
        .expect("Typst supplies a raw show rule")
        .apply(&raw, engine, styles)
}

/// Add the bundled syntaxes after the author's syntaxes and let Typst highlight again.
pub(super) fn with_bundled_syntax(
    raw: &Packed<RawElem>,
    engine: &mut Engine,
    styles: StyleChain,
) -> SourceResult<Option<Packed<RawElem>>> {
    let Some(language) = raw.lang.get_ref(styles) else {
        return Ok(None);
    };
    if matches!(raw.theme.get_ref(styles), Smart::Custom(None))
        || !tola_highlight::has_bundled_syntax(&language.to_lowercase())
    {
        return Ok(None);
    }

    let bundled = BUNDLED_SYNTAXES.get_or_init(|| {
        let span = Span::detached();
        let syntaxes: Array = syntax_bytes()
            .map(|(_, bytes)| Value::Bytes(bytes))
            .collect();
        let mut args = Args::new::<Value>(span, []);
        args.items.push(Arg {
            span,
            name: Some("syntaxes".into()),
            value: Spanned::new(Value::Array(syntaxes), span),
        });
        RawElem::ELEM
            .set(engine, args)
            .expect("bundled raw syntaxes load")
    });
    let mut syntaxes = StyleChain::new(bundled).get_cloned(RawElem::syntaxes);
    if let Some(author) = raw.syntaxes.as_option() {
        // Typst folds this element field after the author's set rules.
        syntaxes
            .source
            .0
            .splice(..0, author.source.0.iter().cloned());
        syntaxes.derived.splice(..0, author.derived.iter().cloned());
    }
    let mut raw = raw.clone();
    raw.syntaxes.set(syntaxes);
    raw.synthesize(engine, styles)?;
    Ok(Some(raw))
}

fn syntax_bytes() -> impl Iterator<Item = (&'static str, Bytes)> {
    tola_highlight::CODE_SYNTAXES
        .iter()
        .map(|&(language, source)| (language, Bytes::from_string(source)))
}

/// Extra syntax files for Typst's `raw.syntaxes`.
pub(super) fn files() -> Dict {
    let mut files = Dict::new();
    for (language, bytes) in syntax_bytes() {
        files.insert(language.into(), Value::Bytes(bytes));
    }
    files
}

/// The built-in syntax name for a source-code label.
#[func]
pub(super) fn tola_code_language(
    /// The source-code label to look up.
    language: Str,
) -> Str {
    tola_highlight::code_language(language.as_str()).into()
}
