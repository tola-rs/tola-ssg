//! `tola fonts`: the compiler fonts this invocation can lay out with.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use tola_build::InputScope;
use tola_typst::prelude::{CompilerFont, CompilerFontInput, FontOptions, FontStore};
use typst::text::{
    AxisValue, FontAxis, FontStretch, FontStyle, FontVariant, FontWeight, StandardAxes,
};
use unicode_width::UnicodeWidthStr;

use crate::cancellation::Cancellation;
use crate::cli::FontsArgs;
use crate::cli::output::CommandOutput;

pub(in crate::cli) fn run(
    args: &FontsArgs,
    scope: InputScope,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let resources = crate::cli::config::build_resources_with_fonts(scope, &args.fonts)?;
    let loaded = crate::cli::config::load_optional_config(
        &args.config,
        &args.packages,
        scope,
        &crate::config::ConfigOverrides::default(),
        output,
        cancellation,
    )?;

    // Without a site there is nothing to configure, and the host's own fonts are the answer.
    let site = loaded.as_ref().map(|loaded| loaded.config());
    let directories = match site {
        Some(config) => config.compiler_font_directories(&resources),
        None => resources.font_directories().to_vec(),
    };
    let system_fonts = match site {
        Some(config) => config.system_fonts_allowed(&resources),
        None => resources.include_system_fonts(),
    };
    let root = site.map(|config| config.get_root());

    let paths: Vec<&Path> = directories.iter().map(PathBuf::as_path).collect();
    let mut options = FontOptions::new()
        .with_custom_paths(&paths)
        .with_system_fonts(system_fonts)
        .with_embedded_fonts(resources.include_embedded_fonts());
    if let Some(config) = site {
        options = options.with_source_boundary(resources.source_boundary(config));
    }
    let fonts = FontStore::with_options(options).preload()?.compiler_fonts();

    if args.json {
        output.write_stdout_line(serde_json::to_vec_pretty(&json_records(&fonts))?)?;
        return Ok(());
    }
    for line in listing_lines(&fonts, root, args.variants) {
        output.write_stdout_line(line)?;
    }
    Ok(())
}

/// Every face as one record, the shape scripts read.
fn json_records(fonts: &[CompilerFont]) -> Vec<serde_json::Value> {
    fonts
        .iter()
        .map(|font| {
            serde_json::json!({
                "family": font.family,
                "style": style_text(font.variant.style),
                "weight": font.variant.weight.to_number(),
                "stretch": font.variant.stretch.to_string(),
                "axes": axes_records(&font.axes),
                "file": face_file(&font.input).map(|file| file.display().to_string()),
            })
        })
        .collect()
}

/// One line per family, or one line per face when `variants` is set.
fn listing_lines(fonts: &[CompilerFont], root: Option<&Path>, variants: bool) -> Vec<String> {
    if variants {
        let mut faces: Vec<&CompilerFont> = fonts.iter().collect();
        faces.sort_by(|left, right| {
            left.family
                .cmp(&right.family)
                .then(left.variant.cmp(&right.variant))
        });
        let width = faces
            .iter()
            .map(|font| font.family.width())
            .max()
            .unwrap_or(0);
        return faces
            .into_iter()
            .map(|font| face_line(font, width, root))
            .collect();
    }

    // A family's variants share one line, so the line carries every input the family comes from.
    let mut families: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for font in fonts {
        families
            .entry(font.family.as_str())
            .or_default()
            .insert(source_text(&font.input, root));
    }
    let width = families
        .keys()
        .map(|family| family.width())
        .max()
        .unwrap_or(0);
    families
        .into_iter()
        .map(|(family, sources)| {
            format!(
                "{}{}  {}",
                family,
                " ".repeat(width - family.width()),
                sources.into_iter().collect::<Vec<_>>().join(", "),
            )
        })
        .collect()
}

/// One face as `family  variant  file`, with the variation axes a variable face declares.
fn face_line(font: &CompilerFont, width: usize, root: Option<&Path>) -> String {
    let mut line = format!(
        "{}{}  {}  {}",
        font.family,
        " ".repeat(width - font.family.width()),
        variant_text(&font.variant),
        file_text(&font.input, root),
    );
    if let Some(axes) = axes_text(&font.axes) {
        line.push_str("  ");
        line.push_str(&axes);
    }
    line
}

/// The style, weight, and stretch `text(font:)` selects this face by.
fn variant_text(variant: &FontVariant) -> String {
    format!(
        "{} {} {}",
        style_text(variant.style),
        variant.weight.to_number(),
        variant.stretch,
    )
}

fn style_text(style: FontStyle) -> &'static str {
    match style {
        FontStyle::Normal => "normal",
        FontStyle::Italic => "italic",
        FontStyle::Oblique => "oblique",
    }
}

/// The file one face is read from, absent for a face the binary carries.
fn face_file(input: &CompilerFontInput) -> Option<&Path> {
    match input {
        CompilerFontInput::ConfiguredDirectory { file, .. }
        | CompilerFontInput::SystemFile { file } => Some(file),
        CompilerFontInput::Embedded => None,
    }
}

/// How one face's input reads: the file it is read from, or `embedded`.
fn file_text(input: &CompilerFontInput, root: Option<&Path>) -> String {
    let Some(file) = face_file(input) else {
        return "embedded".into();
    };
    match root {
        Some(root) => crate::terminal::display_path_within(file, root),
        None => file.display().to_string(),
    }
}

/// How one family's input reads: the directory it is read from, `system`, or `embedded`.
fn source_text(input: &CompilerFontInput, root: Option<&Path>) -> String {
    match input {
        CompilerFontInput::ConfiguredDirectory { directory, .. } => match root {
            Some(root) => crate::terminal::display_path_within(directory, root),
            None => directory.display().to_string(),
        },
        CompilerFontInput::SystemFile { .. } => "system".into(),
        CompilerFontInput::Embedded => "embedded".into(),
    }
}

/// Every variation axis a face declares, in the order Typst lists them.
fn axes_records(axes: &[FontAxis]) -> Vec<serde_json::Value> {
    ordered_axes(axes)
        .iter()
        .map(|axis| {
            serde_json::json!({
                "tag": axis.tag.to_str_lossy(),
                "min": axis.min.0,
                "max": axis.max.0,
                "default": axis.default.0,
            })
        })
        .collect()
}

/// Every variation axis as `name min-max (default)`, in the order Typst lists them.
fn axes_text(axes: &[FontAxis]) -> Option<String> {
    if axes.is_empty() {
        return None;
    }
    Some(
        ordered_axes(axes)
            .iter()
            .map(|axis| {
                format!(
                    "{} {}-{} ({})",
                    axis_name(axis),
                    axis_value(axis, axis.min),
                    axis_value(axis, axis.max),
                    axis_value(axis, axis.default),
                )
            })
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// The axes a face declares, in the order Typst orders them for display.
fn ordered_axes(axes: &[FontAxis]) -> Vec<FontAxis> {
    let mut ordered = axes.to_vec();
    ordered.sort_by_key(|axis| StandardAxes::order(axis.tag));
    ordered
}

/// The name Typst gives one variation axis.
fn axis_name(axis: &FontAxis) -> String {
    match axis.tag {
        StandardAxes::ITAL => "Italic".into(),
        StandardAxes::SLNT => "Slant".into(),
        StandardAxes::WGHT => "Weight".into(),
        StandardAxes::WDTH => "Stretch".into(),
        StandardAxes::OPSZ => "Optical Size".into(),
        tag => tag.to_str_lossy().into_owned(),
    }
}

/// One value of one axis, spelled as that axis means it.
fn axis_value(axis: &FontAxis, value: AxisValue) -> String {
    match axis.tag {
        StandardAxes::WGHT => FontWeight::from_wght(value).to_string(),
        StandardAxes::WDTH => FontStretch::from_wdth(value).to_string(),
        StandardAxes::OPSZ => format!("{value}pt"),
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::text::FontVariant;

    fn configured_face(family: &str, file: &str) -> CompilerFont {
        CompilerFont {
            family: family.into(),
            variant: FontVariant::default(),
            axes: Vec::new(),
            input: CompilerFontInput::ConfiguredDirectory {
                directory: PathBuf::from("/site/fonts"),
                file: PathBuf::from(format!("/site/fonts/{file}")),
            },
        }
    }

    fn embedded_face(family: &str) -> CompilerFont {
        CompilerFont {
            family: family.into(),
            variant: FontVariant::default(),
            axes: Vec::new(),
            input: CompilerFontInput::Embedded,
        }
    }

    #[test]
    fn json_records_name_the_file_or_embedded() {
        let records = json_records(&[
            configured_face("Libertinus Serif", "LibertinusSerif-Regular.otf"),
            embedded_face("New Computer Modern"),
        ]);

        assert_eq!(records[0]["family"], "Libertinus Serif");
        assert_eq!(records[0]["style"], "normal");
        assert_eq!(records[0]["weight"], 400);
        assert_eq!(records[0]["stretch"], "100%");
        assert_eq!(
            records[0]["file"],
            "/site/fonts/LibertinusSerif-Regular.otf"
        );
        assert!(records[1]["file"].is_null());
        assert_eq!(records[0]["axes"], serde_json::json!([]));
    }

    #[test]
    fn families_share_one_line_and_variants_get_their_own() {
        let fonts = [
            configured_face("Libertinus Serif", "LibertinusSerif-Regular.otf"),
            configured_face("Libertinus Serif", "LibertinusSerif-Bold.otf"),
            embedded_face("New Computer Modern"),
        ];
        let root = Path::new("/site");

        let families = listing_lines(&fonts, Some(root), false);
        assert_eq!(families.len(), 2);
        assert!(families[0].contains("Libertinus Serif"));
        assert!(families[0].contains("fonts"));
        assert!(families[1].contains("embedded"));

        let variants = listing_lines(&fonts, Some(root), true);
        assert_eq!(variants.len(), 3);
        assert!(
            variants
                .iter()
                .any(|line| line.contains("LibertinusSerif-Bold.otf"))
        );
        assert!(variants.iter().any(|line| line.contains("embedded")));
    }

    #[test]
    fn variable_face_axes_read_in_typst_order() {
        let mut face = configured_face("Inter", "Inter.ttf");
        face.axes = vec![
            FontAxis {
                tag: StandardAxes::OPSZ,
                min: AxisValue(8.0),
                max: AxisValue(144.0),
                default: AxisValue(14.0),
            },
            FontAxis {
                tag: StandardAxes::WGHT,
                min: AxisValue(100.0),
                max: AxisValue(900.0),
                default: AxisValue(400.0),
            },
            FontAxis {
                tag: StandardAxes::ITAL,
                min: AxisValue(0.0),
                max: AxisValue(1.0),
                default: AxisValue(0.0),
            },
        ];

        let records = json_records(std::slice::from_ref(&face));
        assert_eq!(records[0]["axes"][0]["tag"], "ital");
        assert_eq!(records[0]["axes"][1]["tag"], "wght");
        assert_eq!(records[0]["axes"][1]["min"], 100.0);
        assert_eq!(records[0]["axes"][2]["tag"], "opsz");

        let lines = listing_lines(std::slice::from_ref(&face), None, true);
        assert!(lines[0].contains("Italic 0-1 (0)"));
        assert!(lines[0].contains("Weight 100-900 (400)"));
        assert!(lines[0].contains("Optical Size 8pt-144pt (14pt)"));
    }
}
