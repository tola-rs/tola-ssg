//! Atomic CSS source discovery.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use crate::config::SiteConfig;
use crate::utils::git::ignore::IgnoreMatcher;
use crate::utils::path::normalize_path;

mod rules;

#[derive(Debug, Clone, Copy)]
enum ScanMode {
    Auto,
    ExplicitDir,
    ExplicitFile,
}

struct Gitignore {
    root: PathBuf,
    matcher: IgnoreMatcher,
}

impl Gitignore {
    fn load(root: &Path) -> Option<Self> {
        let root = normalize_path(root);
        let bytes = fs::read(root.join(".gitignore")).ok()?;
        Some(Self {
            root,
            matcher: IgnoreMatcher::new(&bytes),
        })
    }

    fn matches(&self, path: &Path, is_dir: bool) -> bool {
        let path = normalize_path(path);
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return false;
        };
        let relative = path_to_slash(relative);
        if relative.is_empty() {
            return false;
        }
        if self.matcher.matches(&relative, is_dir) {
            return true;
        }

        let mut current = relative.as_str();
        while let Some((parent, _)) = current.rsplit_once('/') {
            if self.matcher.matches(parent, true) {
                return true;
            }
            current = parent;
        }
        false
    }
}

fn ignored_by(ignores: &[Gitignore], path: &Path, is_dir: bool) -> bool {
    ignores.iter().any(|ignore| ignore.matches(path, is_dir))
}

pub fn texts(config: &SiteConfig) -> Result<Vec<String>> {
    files(config)?
        .into_iter()
        .filter_map(|path| read_source_text(&path).transpose())
        .collect()
}

pub fn files(config: &SiteConfig) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if let Some(entries) = &config.build.atomic_css.source {
        for entry in entries {
            let path = source_path(config, entry);
            let mode = if path.is_file() {
                ScanMode::ExplicitFile
            } else {
                ScanMode::ExplicitDir
            };
            collect_path(&path, config, mode, &mut Vec::new(), &mut files)?;
        }
    } else {
        let mut ignores = Vec::new();
        collect_path(
            config.get_root(),
            config,
            ScanMode::Auto,
            &mut ignores,
            &mut files,
        )?;
    }
    files.sort();
    files.dedup();
    Ok(files)
}

pub fn roots(config: &SiteConfig) -> Vec<PathBuf> {
    if !config.build.atomic_css.enable {
        return Vec::new();
    }
    if let Some(entries) = &config.build.atomic_css.source {
        return entries
            .iter()
            .map(|entry| source_path(config, entry))
            .filter(|entry| entry.exists())
            .collect();
    }

    let root = normalize_path(config.get_root());
    root.exists().then_some(root).into_iter().collect()
}

pub fn is_input(path: &Path, config: &SiteConfig) -> bool {
    if !config.build.atomic_css.enable {
        return false;
    }

    let path = normalize_path(path);
    if config
        .build
        .atomic_css
        .config
        .as_ref()
        .is_some_and(|config_path| path == normalize_path(config_path))
    {
        return true;
    }

    if let Some(entries) = &config.build.atomic_css.source {
        return entries.iter().any(|entry| {
            let source = source_path(config, entry);
            if path == source {
                return !is_excluded_tree(&path, config, ScanMode::ExplicitFile);
            }
            path.starts_with(&source) && is_candidate_path(&path, config, ScanMode::ExplicitDir)
        });
    }

    if !is_candidate_path(&path, config, ScanMode::Auto) || is_ignored_by_gitignore(&path, config) {
        return false;
    }
    path == normalize_path(config.get_root()) || path.starts_with(normalize_path(config.get_root()))
}

fn collect_path(
    path: &Path,
    config: &SiteConfig,
    mode: ScanMode,
    ignores: &mut Vec<Gitignore>,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    let path = normalize_path(path);
    if is_excluded_tree(&path, config, mode) || ignored_by(ignores, &path, path.is_dir()) {
        return Ok(());
    }
    if path.is_file() {
        if matches!(mode, ScanMode::ExplicitFile) && !rules::css_extension(&path)
            || is_candidate_path(&path, config, mode)
        {
            files.push(path);
        }
        return Ok(());
    }
    if !path.is_dir() {
        return Err(anyhow!(
            "Atomic CSS source '{}' is not a file or directory",
            path.display()
        ));
    }

    let ignore_count = ignores.len();
    if matches!(mode, ScanMode::Auto)
        && let Some(ignore) = Gitignore::load(&path)
    {
        ignores.push(ignore);
    }

    for entry in fs::read_dir(&path)
        .with_context(|| format!("failed to read Atomic CSS source dir '{}'", path.display()))?
    {
        let entry = entry?;
        collect_path(&entry.path(), config, mode, ignores, files)?;
    }
    ignores.truncate(ignore_count);
    Ok(())
}

fn source_path(config: &SiteConfig, source: &Path) -> PathBuf {
    normalize_path(&config.get_root().join(source))
}

fn is_ignored_by_gitignore(path: &Path, config: &SiteConfig) -> bool {
    let path = normalize_path(path);
    let root = normalize_path(config.get_root());
    if path.strip_prefix(&root).is_err() {
        return false;
    }

    let mut ignores = Vec::new();
    for dir in gitignore_dirs(&root, &path) {
        if let Some(ignore) = Gitignore::load(&dir) {
            ignores.push(ignore);
        }
    }
    ignored_by(&ignores, &path, path.is_dir())
}

fn gitignore_dirs(root: &Path, path: &Path) -> Vec<PathBuf> {
    let stop = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(root)
    };
    let Ok(relative) = stop.strip_prefix(root) else {
        return vec![root.to_path_buf()];
    };

    let mut dirs = vec![root.to_path_buf()];
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        dirs.push(current.clone());
    }
    dirs
}

fn is_candidate_path(path: &Path, config: &SiteConfig, mode: ScanMode) -> bool {
    !is_excluded_tree(path, config, mode)
        && !rules::ignored_file(path)
        && !rules::css_extension(path)
        && !rules::ignored_extension(path)
        && !rules::binary_extension(path)
}

fn is_excluded_tree(path: &Path, config: &SiteConfig, mode: ScanMode) -> bool {
    let path = normalize_path(path);
    let output = normalize_path(&config.paths().output_dir());
    path == output
        || path.starts_with(&output)
        || matches!(mode, ScanMode::Auto) && rules::excluded_dir(&path)
}

fn read_source_text(path: &Path) -> Result<Option<String>> {
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read Atomic CSS source '{}'", path.display()))?;
    if bytes.contains(&0) {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

fn path_to_slash(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn config(root: &Path) -> SiteConfig {
        let mut config = SiteConfig::default();
        config.set_root(root);
        config.build.output = root.join("public");
        config.build.atomic_css.enable = true;
        config
    }

    #[test]
    fn auto_scan_respects_gitignore() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let content = root.join("content");
        let ignored = root.join("ignored");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&ignored).unwrap();
        fs::write(content.join("index.html"), r#"<div class="flex"></div>"#).unwrap();
        fs::write(
            ignored.join("button.html"),
            r#"<button class="grid"></button>"#,
        )
        .unwrap();
        fs::write(root.join(".gitignore"), "ignored/\n").unwrap();

        let texts = texts(&config(root)).unwrap().join("\n");

        assert!(texts.contains("flex"));
        assert!(!texts.contains("grid"));
    }

    #[test]
    fn auto_scan_respects_nested_gitignore() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let components = root.join("components");
        fs::create_dir_all(&components).unwrap();
        fs::write(
            components.join("button.html"),
            r#"<button class="flex"></button>"#,
        )
        .unwrap();
        fs::write(
            components.join("debug.html"),
            r#"<button class="grid"></button>"#,
        )
        .unwrap();
        fs::write(components.join(".gitignore"), "debug.html\n").unwrap();

        let texts = texts(&config(root)).unwrap().join("\n");

        assert!(texts.contains("flex"));
        assert!(!texts.contains("grid"));
    }

    #[test]
    fn explicit_source_can_scan_gitignored_dir() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let components = root.join("components");
        fs::create_dir_all(&components).unwrap();
        fs::write(
            components.join("button.html"),
            r#"<button class="grid"></button>"#,
        )
        .unwrap();
        fs::write(root.join(".gitignore"), "components/\n").unwrap();

        let mut config = config(root);
        config.build.atomic_css.source = Some(vec![PathBuf::from("components")]);

        let texts = texts(&config).unwrap().join("\n");

        assert!(texts.contains("grid"));
    }

    #[test]
    fn auto_scan_ignores_common_non_template_extensions() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        fs::write(root.join("page.html"), r#"<div class="flex"></div>"#).unwrap();
        fs::write(root.join("style.css"), r#".grid {}"#).unwrap();
        fs::write(root.join("debug.log"), r#"<div class="grid"></div>"#).unwrap();
        fs::write(root.join("theme.scss"), r#"<div class="block"></div>"#).unwrap();

        let texts = texts(&config(root)).unwrap().join("\n");

        assert!(texts.contains("flex"));
        assert!(!texts.contains("grid"));
        assert!(!texts.contains("block"));
    }

    #[test]
    fn explicit_source_file_does_not_scan_css_as_candidate_text() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        fs::write(root.join("style.css"), r#".grid {}"#).unwrap();

        let mut config = config(root);
        config.build.atomic_css.source = Some(vec![PathBuf::from("style.css")]);

        let texts = texts(&config).unwrap().join("\n");

        assert!(!texts.contains("grid"));
    }

    #[test]
    fn explicit_source_file_can_scan_default_ignored_extension() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        fs::write(root.join("tokens.bin"), r#"<div class="grid"></div>"#).unwrap();

        let mut config = config(root);
        config.build.atomic_css.source = Some(vec![PathBuf::from("tokens.bin")]);

        let texts = texts(&config).unwrap().join("\n");

        assert!(texts.contains("grid"));
    }

    #[test]
    fn explicit_source_dir_still_filters_binary_extensions() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let components = root.join("components");
        fs::create_dir_all(&components).unwrap();
        fs::write(
            components.join("button.html"),
            r#"<button class="flex"></button>"#,
        )
        .unwrap();
        fs::write(components.join("tokens.bin"), r#"<div class="grid"></div>"#).unwrap();

        let mut config = config(root);
        config.build.atomic_css.source = Some(vec![PathBuf::from("components")]);

        let texts = texts(&config).unwrap().join("\n");

        assert!(texts.contains("flex"));
        assert!(!texts.contains("grid"));
    }
}
