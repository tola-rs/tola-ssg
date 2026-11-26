use super::*;
use crate::config::loading::{BuildOverrides, resolve_parsed_site_config};

#[test]
fn includes_merge_before_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("config")).unwrap();
    std::fs::write(
        root.join("config/base[1].toml"),
        r#"
[build]
par-hooks = false
hook-jobs = 2
[options]
widths = [480, 960]
[[hooks.before-build]]
name = "styles"
order = 10
argv = ["true"]
"#,
    )
    .unwrap();
    std::fs::write(
        root.join("config/extra.toml"),
        r#"
[site]
title = "Included"
[options]
widths = [1280]
[[hooks.before-build]]
name = "data"
order = 0
argv = ["true"]
"#,
    )
    .unwrap();
    let path = root.join("tola.toml");
    std::fs::write(
        &path,
        "include = ['config/base[1].toml', 'config/extra.toml']\n",
    )
    .unwrap();
    let src = ConfigSource::load(Some(&path), InputScope::Online).unwrap();
    let parsed = src.decode_site().unwrap();
    assert!(!parsed.schema().build.par_hooks);
    assert_eq!(parsed.schema().build.hook_jobs.count(8), 2);
    assert_eq!(
        parsed.schema().options["widths"].as_array().unwrap().len(),
        1
    );
    assert_eq!(parsed.schema().hooks.before_build.len(), 2);
    assert_eq!(parsed.schema().hooks.before_build[0].order, 10);
    assert_eq!(parsed.schema().hooks.before_build[1].order, 0);
    let origin = src
        .origins()
        .get(&vec![
            ValueStep::Key("hooks".into()),
            ValueStep::Key("before-build".into()),
            ValueStep::Index(1),
            ValueStep::Key("name".into()),
        ])
        .unwrap()
        .origins
        .last()
        .unwrap();
    assert!(
        src.applications()[origin.source_id]
            .path
            .ends_with("config/extra.toml")
    );
    assert_eq!(origin.key_path[2], ValueStep::Index(0));
    let resolved = resolve_parsed_site_config(
        parsed,
        tola_typst::PackageLocations::default(),
        &BuildOverrides::default(),
    )
    .unwrap();
    assert_eq!(
        resolved.config().build().entry,
        root.canonicalize().unwrap().join("site.typ")
    );
}

#[test]
fn includes_repeat_branches_reject_cycles() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("common.toml"), "[options]\nvalue = 0\n").unwrap();
    std::fs::write(
        root.join("first.toml"),
        "include = ['common.toml']\n[options]\nvalue = 1\n",
    )
    .unwrap();
    std::fs::write(root.join("second.toml"), "include = ['common.toml']\n").unwrap();
    let path = root.join("tola.toml");
    std::fs::write(&path, "include = ['first.toml', 'second.toml']\n").unwrap();
    let src = ConfigSource::load(Some(&path), InputScope::Online).unwrap();
    assert_eq!(
        src.decode_site().unwrap().schema().options["value"].as_integer(),
        Some(0)
    );
    assert_eq!(src.applications().len(), 5);
    assert_eq!(src.sources().len(), 4);
    std::fs::write(root.join("common.toml"), "include = ['tola.toml']\n").unwrap();
    let err = ConfigSource::load(Some(&path), InputScope::Online).unwrap_err();
    assert!(err.diagnostics().iter().any(|d| !d.related.is_empty()));
    assert!(err.paths().iter().any(|p| p.ends_with("common.toml")));
}

#[test]
fn optional_include_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let path = root.join("tola.toml");
    let local = root.join("local.toml");
    std::fs::write(
        &path,
        "include = [{path='local.toml', optional=true}]\n[build]\npar-hooks = false\n",
    )
    .unwrap();
    let src = ConfigSource::load(Some(&path), InputScope::Online).unwrap();
    assert_eq!(
        src.applications()[1].state,
        ConfigFileState::OptionalMissing
    );
    assert!(src.is_current(None).unwrap());
    std::fs::write(&local, "[build]\npar-hooks = 'wrong type'\n").unwrap();
    let changed = src.read_changed().unwrap().unwrap();
    let err = changed.decode_site().unwrap_err();
    assert_eq!(
        err.diagnostics()[0].location.as_ref().unwrap().path,
        "local.toml"
    );
    std::fs::write(&local, "[options]\nlocal = true\n").unwrap();
    let repaired = src.read_changed().unwrap().unwrap();
    assert!(!repaired.decode_site().unwrap().schema().build.par_hooks);
    std::fs::write(&local, [0xff]).unwrap();
    assert!(src.read_changed().is_err());
}

#[test]
fn overlay_close_restores_disk_freshness() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tola.toml");
    let text = "[site]\ntitle = 'First'\n";
    std::fs::write(&path, text).unwrap();
    let disk = ConfigSource::load(Some(&path), InputScope::Online).unwrap();
    let overlay = disk
        .changed_sources(&[(path.clone(), Arc::from(text))], None)
        .unwrap()
        .expect("opening a buffer changes the input view");
    let closed = overlay
        .changed_sources(&[], None)
        .unwrap()
        .expect("closing a buffer restores the disk input view");
    std::fs::write(&path, "[site]\ntitle = 'Second'\n").unwrap();
    assert!(!closed.is_current(None).unwrap());
    assert!(overlay.is_current(None).unwrap());
}

#[test]
#[cfg(unix)]
fn changed_text_keeps_root() {
    let dir = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let alias = dir.path().join("site");
    std::os::unix::fs::symlink(first.path(), &alias).unwrap();
    let src = ConfigSource::parse(&alias.join("tola.toml"), "", InputScope::Online).unwrap();
    assert!(src.changed_text("").unwrap().is_none());
    std::fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(second.path(), &alias).unwrap();

    let changed = src
        .changed_text("[site]\ntitle = 'Changed'\n")
        .unwrap()
        .unwrap();
    assert_eq!(changed.path(), src.path());
    assert_eq!(changed.root(), src.root());
    assert_ne!(
        changed.root(),
        tola_input::normalize_existing_prefix(&alias)
    );
    assert_eq!(
        changed.decode_site().unwrap().schema().site.title,
        "Changed"
    );
}

#[test]
fn dotted_host_section_preserved() {
    #[derive(Default, serde::Deserialize, tola_config::Fragment)]
    #[serde(default)]
    struct HostConfig {
        site: crate::config::section::SiteSectionConfig,
        build: crate::config::BuildSectionConfig,
        #[serde(rename = "host.example.section")]
        dotted_host: std::collections::BTreeMap<String, toml::Value>,
    }

    let dir = tempfile::tempdir().unwrap();
    let text = "[\"host.example.section\"]\nenable = true\n";
    let src = ConfigSource::parse(&dir.path().join("tola.toml"), text, InputScope::Online).unwrap();
    assert!(src.presence().contains("host.example.section"));
    let direct = src.decode_site().unwrap();
    let host = src.decode::<HostConfig>().unwrap();
    assert_eq!(
        host.schema().dotted_host["enable"],
        toml::Value::Boolean(true)
    );
    let projected = host.map_schema(|host| crate::config::SiteConfigSchema {
        site: host.site,
        build: host.build,
        ..crate::config::SiteConfigSchema::default()
    });

    for parsed in [direct, projected] {
        let core = resolve_parsed_site_config(
            parsed,
            tola_typst::PackageLocations::default(),
            &BuildOverrides::default(),
        )
        .unwrap();
        assert!(core.config().warnings().is_empty());
        assert_eq!(
            core.config().source().unwrap().sources()[0].hash(),
            blake3::hash(text.as_bytes())
        );
    }
}

#[derive(Debug, serde::Deserialize, tola_config::Fragment)]
struct NumericSettings {
    #[serde(rename = "server")]
    _server: NumericFields,
}

#[derive(Debug, serde::Deserialize, tola_config::Fragment)]
struct NumericFields {
    #[serde(rename = "port")]
    _port: u16,
    #[serde(rename = "limit")]
    _limit: usize,
}

#[test]
fn numeric_schema_error_location() {
    for (field, val, max) in [
        ("port", "70000", u16::MAX.to_string()),
        ("port", "-1", u16::MAX.to_string()),
        ("limit", "-1", usize::MAX.to_string()),
    ] {
        let port = if field == "port" { val } else { "5277" };
        let limit = if field == "limit" { val } else { "2" };
        let text = format!("[server]\nport = {port}\nlimit = {limit}\n");
        let dir = tempfile::tempdir().unwrap();
        let src =
            ConfigSource::parse(&dir.path().join("tola.toml"), &text, InputScope::Online).unwrap();
        let err = src.decode::<NumericSettings>().unwrap_err();
        let diag = &err.diagnostics()[0];
        assert_eq!(diag.code, tola_diagnostic::codes::config::TOML);
        assert!(
            diag.message.contains(&format!("`server.{field}`")),
            "{}",
            diag.message
        );
        assert!(
            diag.message.contains(&format!("`{val}`")),
            "{}",
            diag.message
        );
        assert!(diag.message.contains(&max), "{}", diag.message);
        let loc = diag.location.as_ref().unwrap();
        let line = &loc.source_lines[0];
        let (start, end) = line.highlight.unwrap();
        assert_eq!(&line.text[start..end], val);
    }
}

#[test]
fn table_value_error_location() {
    for (text, key, highlighted) in [
        (
            "[server]\nport = { x = 1 }\nlimit = 2\n",
            "server.port",
            "{ x = 1 }",
        ),
        (
            "server = { port = { x = 1 }, limit = 2 }\n",
            "server.port",
            "{ x = 1 }",
        ),
        (
            "[server]\nport = 5277\nlimit = { x = 1 }\n",
            "server.limit",
            "{ x = 1 }",
        ),
        (
            "[server]\nlimit = 2\n[server.port]\nx = 1\n",
            "server.port",
            "[server.port]",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let src =
            ConfigSource::parse(&dir.path().join("tola.toml"), text, InputScope::Online).unwrap();
        let err = src.decode::<NumericSettings>().unwrap_err();
        let diag = &err.diagnostics()[0];
        assert_eq!(diag.code, tola_diagnostic::codes::config::TOML);
        assert!(
            diag.message.contains(&format!("`{key}`")),
            "{}",
            diag.message
        );
        let line = &diag.location.as_ref().unwrap().source_lines[0];
        let (start, end) = line.highlight.unwrap();
        assert_eq!(&line.text[start..end], highlighted);
    }
}

#[test]
#[cfg(unix)]
fn pure_refuses_outside_config() {
    use crate::config::ConfigError;

    let dir = tempfile::tempdir().unwrap();
    let site = dir.path().join("site");
    let outside = tempfile::tempdir().unwrap();
    let text = "[site]\ntitle = \"Site\"\n";
    std::fs::create_dir(&site).unwrap();
    std::fs::write(outside.path().join("tola.toml"), text).unwrap();
    let path = site.join("tola.toml");
    std::fs::write(&path, text).unwrap();
    let src = ConfigSource::load(Some(&path), InputScope::Pure).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(outside.path().join("tola.toml"), &path).unwrap();

    for err in [
        ConfigSource::load(Some(&path), InputScope::Pure).unwrap_err(),
        ConfigSource::parse(&path, text, InputScope::Pure).unwrap_err(),
        src.read_changed().unwrap_err(),
        src.changed_text(text).unwrap_err(),
    ] {
        assert!(anyhow::Error::new(err).chain().any(|cause| matches!(
            cause.downcast_ref::<ConfigError>(),
            Some(ConfigError::Refused {
                reason: ConfigSourceRefusal::OutsideSite,
                ..
            })
        )));
    }

    assert!(ConfigSource::load(Some(&path), InputScope::Online).is_ok());
}

#[test]
#[cfg(unix)]
fn pure_refuses_internal_config() {
    use crate::config::ConfigError;

    let dir = tempfile::tempdir().unwrap();
    let site = dir.path().join("site");
    let internal = site.join(tola_input::INTERNAL_DIR);
    std::fs::create_dir_all(&internal).unwrap();
    std::fs::write(internal.join("tola.toml"), "[site]\ntitle = \"Cached\"\n").unwrap();
    let alias = site.join("tola.toml");
    std::os::unix::fs::symlink(internal.join("tola.toml"), &alias).unwrap();

    let err = ConfigSource::load(Some(&alias), InputScope::Pure).unwrap_err();
    assert!(anyhow::Error::new(err).chain().any(|cause| matches!(
        cause.downcast_ref::<ConfigError>(),
        Some(ConfigError::Refused {
            reason: ConfigSourceRefusal::GeneratedState,
            ..
        })
    )));
}

#[test]
fn composed_decode_rejects_unknown() {
    #[derive(Debug, Default, serde::Deserialize, tola_config::Fragment)]
    #[serde(default)]
    struct EditorConfig {
        mode: String,
    }

    #[derive(Debug, Default, serde::Deserialize, tola_config::Fragment)]
    #[serde(default)]
    struct HostConfig {
        #[serde(rename = "host-editor")]
        editor: EditorConfig,
    }

    let dir = tempfile::tempdir().unwrap();
    let accepted = ConfigSource::parse(
        &dir.path().join("tola.toml"),
        "[host-editor]\nmode = \"custom\"\n",
        InputScope::Online,
    )
    .unwrap();
    let (_, host) = accepted.decode_with_host::<HostConfig>().unwrap();
    assert_eq!(host.schema().editor.mode, "custom");

    for (text, key) in [
        ("[unknown]\nvalue = true\n", "unknown"),
        ("[site]\ntitel = \"Misspelled\"\n", "site.titel"),
        (
            "[host-editor]\nmode = \"custom\"\n\"spe?ed\" = 2\n",
            "host-editor.\"spe?ed\"",
        ),
        ("version = \"0.8.0\"\n", "version"),
    ] {
        let src =
            ConfigSource::parse(&dir.path().join("tola.toml"), text, InputScope::Online).unwrap();
        let err = src.decode_with_host::<HostConfig>().unwrap_err();
        let Some(tola_diagnostic::DiagnosticCause::UnknownConfigurationFields { fields }) =
            &err.diagnostics()[0].cause
        else {
            panic!("missing unknown field diagnostic for {text}");
        };
        assert_eq!(fields[0].name, key);
    }
}
