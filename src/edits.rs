//! Text changes with explicit preconditions, shared by command-line callers.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::writes::FileWrites;

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum Before {
    Missing,
    Digest(String),
}

impl Before {
    fn check(&self, path: &Path, bytes: Option<&[u8]>) -> Result<()> {
        let matches = match self {
            Self::Missing => bytes.is_none(),
            Self::Digest(value) => {
                let encoded = value.strip_prefix("blake3:").context(
                    "expected a blake3: digest from `tola edit read` or a query input read",
                )?;
                let expected = blake3::Hash::from_hex(encoded).context("invalid input digest")?;
                bytes.is_some_and(|bytes| blake3::hash(bytes) == expected)
            }
        };
        ensure!(
            matches,
            "{} changed since the edit was prepared",
            path.display()
        );
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileChange {
    pub path: PathBuf,
    pub before: Before,
    pub text: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileCheck {
    pub path: PathBuf,
    pub before: Before,
}

/// One batch of text edits and the preconditions that must still hold when it is applied.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct EditBatch {
    pub schema_version: u32,
    pub changes: Vec<FileChange>,
    #[serde(default)]
    pub checks: Vec<FileCheck>,
}

#[derive(Serialize)]
pub(crate) struct FileText {
    pub path: PathBuf,
    pub digest: Option<String>,
    pub text: Option<String>,
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

pub(crate) fn read(root: &Path, paths: &[PathBuf]) -> Result<Vec<FileText>> {
    let files = FileWrites::new(root)?;
    paths
        .iter()
        .map(|path| {
            let bytes = files.read_file(path)?;
            let digest = bytes.as_deref().map(digest);
            let text = bytes
                .map(String::from_utf8)
                .transpose()
                .with_context(|| format!("{} is not UTF-8 text", path.display()))?;
            Ok(FileText {
                path: path.clone(),
                digest,
                text,
            })
        })
        .collect()
}

pub(crate) struct PreparedEdits {
    writes: FileWrites,
    checks: Vec<(PathBuf, Option<Vec<u8>>)>,
}

impl EditBatch {
    pub(crate) fn paths(&self) -> impl Iterator<Item = &Path> {
        self.changes
            .iter()
            .map(|change| change.path.as_path())
            .chain(self.checks.iter().map(|check| check.path.as_path()))
    }

    pub(crate) fn prepare(&self, root: &Path) -> Result<PreparedEdits> {
        ensure!(
            self.schema_version == 1,
            "unsupported edit schema version {}",
            self.schema_version
        );
        let mut writes = FileWrites::new(root)?;
        let mut checks = Vec::new();
        for check in &self.checks {
            let bytes = writes.read_file(&check.path)?;
            check.before.check(&check.path, bytes.as_deref())?;
            checks.push((check.path.clone(), bytes));
        }
        let mut seen = BTreeSet::new();
        for change in &self.changes {
            let path = tola_input::lexical_path_identity(&change.path);
            ensure!(
                seen.insert(path),
                "duplicate edit target {}",
                change.path.display()
            );
            let bytes = writes.read_file(&change.path)?;
            change.before.check(&change.path, bytes.as_deref())?;
            if let Some(bytes) = &bytes {
                std::str::from_utf8(bytes)
                    .with_context(|| format!("{} is not UTF-8 text", change.path.display()))?;
            }
            writes.replace_file_if_unchanged(&change.path, bytes, change.text.as_bytes())?;
        }
        writes.check()?;
        Ok(PreparedEdits { writes, checks })
    }
}

impl PreparedEdits {
    pub(crate) fn apply(
        &self,
        cancellation: &tola_build::cancellation::BuildCancellation,
    ) -> Result<()> {
        cancellation.ensure_active()?;
        for (path, expected) in &self.checks {
            ensure!(
                self.writes.read_file(path)? == *expected,
                "{} changed since the edit was prepared",
                path.display()
            );
        }
        self.writes.apply(cancellation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(changes: serde_json::Value) -> EditBatch {
        serde_json::from_value(serde_json::json!({"schemaVersion":1,"changes":changes})).unwrap()
    }

    #[test]
    fn changes_require_observed_bytes() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("one.typ"), "Old λ\r\n").unwrap();
        let files = read(directory.path(), &["one.typ".into(), "new.typ".into()]).unwrap();
        let edits = batch(serde_json::json!([
            {"path":"one.typ","before":files[0].digest,"text":"New λ\r\n"},
            {"path":"new.typ","before":null,"text":"New file"}
        ]));
        let prepared = edits.prepare(directory.path()).unwrap();
        assert!(!directory.path().join("new.typ").exists());
        prepared.apply(&Default::default()).unwrap();
        assert_eq!(
            std::fs::read_to_string(directory.path().join("one.typ")).unwrap(),
            "New λ\r\n"
        );
        assert!(edits.prepare(directory.path()).is_err());
    }

    #[test]
    fn later_change_prevents_every_write() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("a.typ"), "a").unwrap();
        std::fs::write(directory.path().join("model.typ"), "before").unwrap();
        let mut edits = batch(serde_json::json!([
            {"path":"a.typ","before":digest(b"a"),"text":"changed"},
            {"path":"b.typ","before":null,"text":"new"}
        ]));
        edits.checks.push(FileCheck {
            path: "model.typ".into(),
            before: Before::Digest(digest(b"before")),
        });
        let prepared = edits.prepare(directory.path()).unwrap();
        std::fs::write(directory.path().join("model.typ"), "after").unwrap();
        assert!(prepared.apply(&Default::default()).is_err());
        assert_eq!(
            std::fs::read_to_string(directory.path().join("a.typ")).unwrap(),
            "a"
        );
        assert!(!directory.path().join("b.typ").exists());
    }

    #[test]
    fn batch_rejects_bad_targets() {
        assert!(
            serde_json::from_value::<EditBatch>(serde_json::json!({
                "schemaVersion":1,"changes":[{"path":"a.typ","text":"new"}]
            }))
            .is_err()
        );
        let directory = tempfile::tempdir().unwrap();
        for changes in [
            serde_json::json!([{"path":"../a.typ","before":null,"text":"new"}]),
            serde_json::json!([
                {"path":"a.typ","before":null,"text":"one"},
                {"path":"./a.typ","before":null,"text":"two"}
            ]),
        ] {
            assert!(batch(changes).prepare(directory.path()).is_err());
        }
    }

    #[test]
    fn binary_guard_rejects_text_edit() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("asset.bin"), [0xff]).unwrap();
        let change = batch(serde_json::json!([
            {"path":"asset.bin","before":digest(&[0xff]),"text":"replacement"}
        ]));
        assert!(change.prepare(directory.path()).is_err());
        let mut guard = batch(serde_json::json!([]));
        guard.checks.push(FileCheck {
            path: "asset.bin".into(),
            before: Before::Digest(digest(&[0xff])),
        });
        guard
            .prepare(directory.path())
            .unwrap()
            .apply(&Default::default())
            .unwrap();
        assert_eq!(
            std::fs::read(directory.path().join("asset.bin")).unwrap(),
            [0xff]
        );
    }
}
