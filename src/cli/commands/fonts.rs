//! Inspect the compiler fonts available to this invocation.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;
use tola_build::font_failure;
use tola_diagnostic::DiagnosticError;
use tola_typst::prelude::{CompilerFont, CompilerFontInput, FontLoadError, FontOptions, FontStore};
use typst::text::{FontAxis, FontStyle, StandardAxes};

pub(super) fn value(request: &super::SiteRequest<'_>) -> Result<serde_json::Value> {
    let loaded = crate::cli::config::load_optional_config(
        &request.config_file,
        &request.packages,
        request.scope(),
        &crate::config::ConfigOverrides::default(),
        request.output,
        request.cancellation,
    )?;
    let token = request.cancellation.token();
    let config = loaded.as_ref().map(|loaded| loaded.config());
    let prepared = config
        .filter(|config| !config.fonts().modules.is_empty())
        .map(|config| {
            let mode = tola_input::InputMode::new(request.scope(), true);
            tola_build::modules::prepare_site(config, mode, &token)
        })
        .transpose()?;
    let config = prepared.as_ref().map(|site| site.config()).or(config);
    let resources = &request.resources;
    let options = match config {
        Some(config) => config.compiler_font_options(resources),
        None => {
            let directories = resources
                .font_directories()
                .iter()
                .map(PathBuf::as_path)
                .collect::<Vec<_>>();
            FontOptions::new()
                .with_custom_paths(&directories)
                .with_system_fonts(resources.include_system_fonts())
        }
    };
    let fonts = FontStore::with_options(options);
    fonts
        .load(&tola_typst::BundleCancellation::from_flag(
            token.signal_flag(),
        ))
        .map_err(|error| font_load_error(error, config.map(|config| config.get_root())))?;
    Ok(serde_json::to_value(json_records(&fonts.compiler_fonts()))?)
}

/// The failure `tola inspect fonts` reports when the compiler fonts could not be prepared.
///
/// The projection is the build's, so a site author reads the same words here and in a build.
fn font_load_error(error: FontLoadError, root: Option<&Path>) -> anyhow::Error {
    let Some(diagnostic) = root.and_then(|root| font_failure(&error, root)) else {
        return anyhow::Error::new(error);
    };
    anyhow::Error::new(DiagnosticError::attach(
        anyhow::anyhow!("Tola could not prepare the compiler fonts"),
        vec![diagnostic],
    ))
}

#[derive(Serialize)]
struct FontRecord {
    family: String,
    style: &'static str,
    weight: u16,
    stretch: String,
    axes: Vec<AxisRecord>,
    file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    module: Option<String>,
}

#[derive(Serialize)]
struct AxisRecord {
    tag: String,
    min: f32,
    max: f32,
    default: f32,
}

/// Every face as one record, the shape scripts read.
fn json_records(fonts: &[CompilerFont]) -> Vec<FontRecord> {
    fonts
        .iter()
        .map(|font| FontRecord {
            family: font.family.clone(),
            style: style_text(font.variant.style),
            weight: font.variant.weight.to_number(),
            stretch: font.variant.stretch.to_string(),
            axes: axes_records(&font.axes),
            file: face_file(&font.input).map(|file| file.display().to_string()),
            module: match &font.input {
                CompilerFontInput::ConfiguredDirectory { label, .. } => label.clone(),
                _ => None,
            },
        })
        .collect()
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

/// Every variation axis a face declares, in the order Typst lists them.
fn axes_records(axes: &[FontAxis]) -> Vec<AxisRecord> {
    ordered_axes(axes)
        .iter()
        .map(|axis| AxisRecord {
            tag: axis.tag.to_str_lossy().into_owned(),
            min: axis.min.0,
            max: axis.max.0,
            default: axis.default.0,
        })
        .collect()
}

/// The axes a face declares, in the order Typst orders them for display.
fn ordered_axes(axes: &[FontAxis]) -> Vec<FontAxis> {
    let mut ordered = axes.to_vec();
    ordered.sort_by_key(|axis| StandardAxes::order(axis.tag));
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::text::{AxisValue, FontVariant};

    fn configured_face(family: &str, file: &str) -> CompilerFont {
        CompilerFont {
            family: family.into(),
            variant: FontVariant::default(),
            axes: Vec::new(),
            input: CompilerFontInput::ConfiguredDirectory {
                directory: PathBuf::from("/site/fonts"),
                label: None,
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
    fn json_records_name_file_embedded() {
        let records = serde_json::to_value(json_records(&[
            configured_face("Libertinus Serif", "LibertinusSerif-Regular.otf"),
            embedded_face("New Computer Modern"),
        ]))
        .unwrap();

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
    fn variable_axes_read_typst_order() {
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

        let records = serde_json::to_value(json_records(std::slice::from_ref(&face))).unwrap();
        assert_eq!(records[0]["axes"][0]["tag"], "ital");
        assert_eq!(records[0]["axes"][1]["tag"], "wght");
        assert_eq!(records[0]["axes"][1]["min"], 100.0);
        assert_eq!(records[0]["axes"][2]["tag"], "opsz");
    }
}
