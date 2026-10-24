//! Inspect the compiler fonts available to this invocation.

use std::path::{Path, PathBuf};

use anyhow::Result;
use tola_build::InputScope;
use tola_build::font_failure;
use tola_diagnostic::DiagnosticError;
use tola_typst::prelude::{CompilerFont, CompilerFontInput, FontLoadError, FontOptions, FontStore};
use typst::text::{FontAxis, FontStyle, StandardAxes};

use crate::cancellation::Cancellation;
use crate::cli::output::CommandOutput;
use crate::cli::{ConfigFileArgs, TypstPackageArgs};

pub(super) fn value(
    config_file: &ConfigFileArgs,
    packages: &TypstPackageArgs,
    scope: InputScope,
    resources: &tola_build::BuildResources,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<serde_json::Value> {
    let loaded = crate::cli::config::load_optional_config(
        config_file,
        packages,
        scope,
        &crate::config::ConfigOverrides::default(),
        output,
        cancellation,
    )?;

    // Without a site there is nothing to configure, and the host's own fonts are the answer.
    let site = loaded.as_ref().map(|loaded| loaded.config());
    let directories = match site {
        Some(config) => config.compiler_font_directories(resources),
        None => resources.font_directories().to_vec(),
    };
    let system_fonts = match site {
        Some(config) => config.system_fonts_allowed(resources),
        None => resources.include_system_fonts(),
    };
    let root = site.map(|config| config.get_root());

    let paths: Vec<&Path> = directories.iter().map(PathBuf::as_path).collect();
    let mut options = FontOptions::new()
        .with_custom_paths(&paths)
        .with_system_fonts(system_fonts);
    if let Some(config) = site {
        options = options.with_source_boundary(resources.source_boundary(config));
    }
    let fonts = match FontStore::with_options(options).preload() {
        Ok(store) => store.compiler_fonts(),
        Err(error) => return Err(font_load_error(error, root)),
    };

    Ok(serde_json::Value::Array(json_records(&fonts)))
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

        let records = json_records(std::slice::from_ref(&face));
        assert_eq!(records[0]["axes"][0]["tag"], "ital");
        assert_eq!(records[0]["axes"][1]["tag"], "wght");
        assert_eq!(records[0]["axes"][1]["min"], 100.0);
        assert_eq!(records[0]["axes"][2]["tag"], "opsz");
    }
}
