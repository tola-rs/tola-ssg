//! Compresses the theme files the package carries.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// The directory holding one file per carried theme.
const THEMES: &str = "src/embed/code-themes";

/// The package directory the carried files are served at.
const DIRECTORY: &str = "code-themes";

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo gives the script an OUT_DIR"));
    println!("cargo:rerun-if-changed={THEMES}");
    let mut names: Vec<String> = fs::read_dir(THEMES)
        .expect("the carried theme files are readable")
        .map(|entry| {
            entry
                .expect("a theme directory entry is readable")
                .file_name()
                .into_string()
                .expect("a theme file name is UTF-8")
        })
        .filter(|name| name.ends_with(".tmTheme"))
        .collect();
    names.sort();

    let mut installed = Vec::new();
    let mut index = String::from(
        "/// (package path, offset, length) for every carried theme file, ordered by path.\n\
         pub(crate) const THEME_FILES: &[(&str, usize, usize)] = &[\n",
    );
    for name in &names {
        let bytes =
            fs::read(Path::new(THEMES).join(name)).expect("a carried theme file is readable");
        let offset = installed.len();
        installed.extend_from_slice(&bytes);
        writeln!(
            index,
            "    (\"{DIRECTORY}/{name}\", {offset}, {}),",
            bytes.len()
        )
        .expect("writing to a string cannot fail");
    }
    index.push_str("];\n");

    // Level 9: the build pays for the bytes once, and every binary carries the result.
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&installed, 9);
    fs::write(out.join("code-themes.z"), compressed).expect("the compressed themes are writable");
    fs::write(out.join("theme_files.rs"), index).expect("the theme index is writable");
}
