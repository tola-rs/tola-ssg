use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let root =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo names the package root"))
            .join("src/demos/sites");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut demos = fs::read_dir(&root)
        .expect("demo sites are readable")
        .map(|demo| demo.expect("demo directory is readable"))
        .collect::<Vec<_>>();
    demos.sort_by_key(|demo| demo.file_name());
    let mut generated = String::new();
    for demo in demos {
        assert!(demo.file_type().expect("demo type is readable").is_dir());
        let id = demo.file_name().into_string().expect("demo ids are UTF-8");
        assert!(
            !id.is_empty()
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        );
        let constant = id.replace('-', "_").to_ascii_uppercase();
        let mut files = Vec::new();
        let mut directories = Vec::new();
        collect_site_paths(&demo.path(), &mut files, &mut directories);
        files.sort();
        directories.sort();
        writeln!(generated, "static {constant}_FILES: &[DemoFile] = &[").unwrap();
        for file in files {
            let path = relative_path(&demo.path(), &file);
            writeln!(
                generated,
                "    DemoFile {{ path: {path:?}, bytes: include_bytes!({file:?}) }},"
            )
            .unwrap();
        }
        generated.push_str("];\n");
        writeln!(generated, "static {constant}_DIRECTORIES: &[&str] = &[").unwrap();
        for directory in directories {
            writeln!(
                generated,
                "    {:?},",
                relative_path(&demo.path(), &directory)
            )
            .unwrap();
        }
        generated.push_str("];\n");
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo names the build output"));
    fs::write(output.join("demo_sites.rs"), generated).expect("embedded demo index is writable");
}

fn collect_site_paths(directory: &Path, files: &mut Vec<PathBuf>, directories: &mut Vec<PathBuf>) {
    for child in fs::read_dir(directory).expect("demo directory is readable") {
        let child = child.expect("demo member is readable");
        let kind = child.file_type().expect("demo member type is readable");
        let path = child.path();
        assert!(
            !kind.is_symlink(),
            "demo sites carry regular files and directories"
        );
        if kind.is_dir() {
            directories.push(path.clone());
            collect_site_paths(&path, files, directories);
        } else {
            assert!(kind.is_file(), "demo sites carry regular files");
            files.push(path);
        }
    }
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .expect("demo member is below its site root")
        .components()
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .expect("demo paths are UTF-8")
        })
        .collect::<Vec<_>>()
        .join("/")
}
