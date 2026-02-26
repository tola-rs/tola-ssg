use std::path::{Path, PathBuf};

use crate::reload::compile::CompileOutcome;

use super::utils::is_reloadable_output_asset;

#[test]
fn test_compile_outcome_variants() {
    let _ = CompileOutcome::Reload {
        reason: "test".into(),
    };
    let _ = CompileOutcome::Skipped;
    let _ = CompileOutcome::Error {
        path: PathBuf::from("/test.typ"),
        url_path: None,
        error: "test".into(),
    };
}

#[test]
fn test_is_reloadable_output_asset() {
    assert!(is_reloadable_output_asset(Path::new(
        "/public/assets/app.css"
    )));
    assert!(is_reloadable_output_asset(Path::new(
        "/public/assets/app.js"
    )));
    assert!(!is_reloadable_output_asset(Path::new(
        "/public/page/index.html"
    )));
    assert!(!is_reloadable_output_asset(Path::new(
        "/public/page/index.htm"
    )));
}
