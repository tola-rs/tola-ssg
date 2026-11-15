//! Apply one configuration fragment and retain its written origins.

use std::{collections::BTreeMap, path::Path};

use tola_config::ValueStep;
use tola_diagnostic::{SourcePositions, push_pointer};

use super::{ConfigFieldOrigins, ConfigFile, ConfigOrigin};

pub(super) fn apply(
    target: &mut toml::Value,
    incoming: &toml::Value,
    positions: &mut SourcePositions,
    source: &ConfigFile,
    source_id: usize,
    origins: &mut BTreeMap<Vec<ValueStep>, ConfigFieldOrigins>,
) {
    let mut merger = Merger {
        positions,
        source_positions: source.positions(),
        source_path: source.path(),
        source_id,
        origins,
    };
    merger.merge(target, incoming, &mut Vec::new(), &mut Vec::new());
}

struct Merger<'a> {
    positions: &'a mut SourcePositions,
    source_positions: &'a SourcePositions,
    source_path: &'a Path,
    source_id: usize,
    origins: &'a mut BTreeMap<Vec<ValueStep>, ConfigFieldOrigins>,
}

impl Merger<'_> {
    fn merge(
        &mut self,
        target: &mut toml::Value,
        incoming: &toml::Value,
        path: &mut Vec<ValueStep>,
        written: &mut Vec<ValueStep>,
    ) {
        if let (toml::Value::Table(target), toml::Value::Table(incoming)) = (&mut *target, incoming)
        {
            if !incoming.is_empty() {
                self.contribute(path, written);
                self.copy_position(path, written);
            }
            for (key, value) in incoming {
                path.push(ValueStep::Key(key.clone()));
                written.push(ValueStep::Key(key.clone()));
                if let Some(previous) = target.get_mut(key) {
                    self.merge(previous, value, path, written);
                } else {
                    target.insert(key.clone(), value.clone());
                    self.record_tree(value, path, written);
                    self.copy_tree(path, written);
                }
                path.pop();
                written.pop();
            }
            return;
        }

        if append_field(path)
            && let (toml::Value::Array(target), toml::Value::Array(incoming)) =
                (&mut *target, incoming)
        {
            if !incoming.is_empty() {
                self.contribute(path, written);
                self.copy_position(path, written);
            }
            let offset = target.len();
            for (index, value) in incoming.iter().enumerate() {
                path.push(ValueStep::Index(offset + index));
                written.push(ValueStep::Index(index));
                self.record_tree(value, path, written);
                self.copy_tree(path, written);
                path.pop();
                written.pop();
            }
            target.extend(incoming.iter().cloned());
            return;
        }

        let mut previous = BTreeMap::new();
        self.origins.retain(|key, value| {
            if key.starts_with(path.as_slice()) {
                previous.insert(key.clone(), value.clone());
                false
            } else {
                true
            }
        });
        self.positions.remove_subtree(&pointer(path));
        *target = incoming.clone();
        self.record_tree(incoming, path, written);
        self.copy_tree(path, written);
        for (key, old) in previous {
            let destination = if self.origins.contains_key(&key) {
                &key
            } else {
                &*path
            };
            let history = self
                .origins
                .get_mut(destination)
                .expect("replacement has an origin");
            history.overridden.extend(old.overridden);
            history.overridden.extend(old.origins);
        }
    }

    fn contribute(&mut self, path: &[ValueStep], written: &[ValueStep]) {
        self.origins
            .entry(path.to_vec())
            .or_default()
            .origins
            .push(ConfigOrigin {
                source_id: self.source_id,
                key_path: written.to_vec(),
            });
    }

    fn record_tree(
        &mut self,
        value: &toml::Value,
        path: &mut Vec<ValueStep>,
        written: &mut Vec<ValueStep>,
    ) {
        self.contribute(path, written);
        match value {
            toml::Value::Table(table) => {
                for (key, value) in table {
                    path.push(ValueStep::Key(key.clone()));
                    written.push(ValueStep::Key(key.clone()));
                    self.record_tree(value, path, written);
                    path.pop();
                    written.pop();
                }
            }
            toml::Value::Array(array) => {
                for (index, value) in array.iter().enumerate() {
                    path.push(ValueStep::Index(index));
                    written.push(ValueStep::Index(index));
                    self.record_tree(value, path, written);
                    path.pop();
                    written.pop();
                }
            }
            _ => {}
        }
    }

    fn copy_position(&mut self, path: &[ValueStep], written: &[ValueStep]) {
        self.positions.copy_position(
            self.source_positions,
            &pointer(written),
            &pointer(path),
            self.source_path,
        );
    }

    fn copy_tree(&mut self, path: &[ValueStep], written: &[ValueStep]) {
        self.positions.copy_subtree(
            self.source_positions,
            &pointer(written),
            &pointer(path),
            self.source_path,
        );
    }
}

fn pointer(path: &[ValueStep]) -> String {
    let mut result = String::new();
    for step in path {
        match step {
            ValueStep::Key(key) => push_pointer(&mut result, key),
            ValueStep::Index(index) => push_pointer(&mut result, &index.to_string()),
        }
    }
    result
}

fn append_field(path: &[ValueStep]) -> bool {
    matches!(path, [ValueStep::Key(section), ValueStep::Key(field)]
        if (section == "hooks" && matches!(field.as_str(), "before-build" | "generate-outputs" | "after-publish"))
        || (section == "assets" && matches!(field.as_str(), "files" | "trees")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn apply_text(
        target: &mut toml::Value,
        positions: &mut SourcePositions,
        origins: &mut BTreeMap<Vec<ValueStep>, ConfigFieldOrigins>,
        id: usize,
        text: &str,
    ) {
        let source: Arc<str> = Arc::from(text);
        let document = toml_edit::Document::parse(Arc::clone(&source)).unwrap();
        let source_positions = SourcePositions::of(&document, source);
        let value: toml::Value = toml::from_str(text).unwrap();
        let path = format!("{id}.toml");
        Merger {
            positions,
            source_positions: &source_positions,
            source_path: Path::new(&path),
            source_id: id,
            origins,
        }
        .merge(target, &value, &mut Vec::new(), &mut Vec::new());
    }

    #[test]
    fn merge_tracks_append_indices_and_ordinary_replacement() {
        let mut target = toml::Value::Table(Default::default());
        let mut positions = SourcePositions::default();
        let mut origins = BTreeMap::new();
        apply_text(
            &mut target,
            &mut positions,
            &mut origins,
            0,
            "[hooks]\nbefore-build = [{name = 'one'}]\n[options.hooks]\nbefore-build = [1, 2]\n'a.b' = 1\n",
        );
        apply_text(
            &mut target,
            &mut positions,
            &mut origins,
            1,
            "[hooks]\nbefore-build = [{name = 'two'}]\n[options.hooks]\nbefore-build = [3]\na.b = 2\n",
        );
        assert_eq!(target["hooks"]["before-build"].as_array().unwrap().len(), 2);
        assert_eq!(
            target["options"]["hooks"]["before-build"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(target["options"]["hooks"]["a.b"].as_integer(), Some(1));
        assert_eq!(target["options"]["hooks"]["a"]["b"].as_integer(), Some(2));
        let key = vec![
            ValueStep::Key("hooks".into()),
            ValueStep::Key("before-build".into()),
            ValueStep::Index(1),
            ValueStep::Key("name".into()),
        ];
        assert_eq!(origins[&key].origins[0].source_id, 1);
        assert_eq!(origins[&key].origins[0].key_path[2], ValueStep::Index(0));
        assert_eq!(
            positions
                .at_pointer("/hooks/before-build/1/name")
                .unwrap()
                .file(),
            Some(Path::new("1.toml"))
        );
        assert!(!origins.contains_key(&vec![
            ValueStep::Key("options".into()),
            ValueStep::Key("hooks".into()),
            ValueStep::Key("before-build".into()),
            ValueStep::Index(1)
        ]));
    }

    #[test]
    fn apply_in_source_order_is_not_associative() {
        let mut target = toml::Value::Table(Default::default());
        let mut positions = SourcePositions::default();
        let mut origins = BTreeMap::new();
        apply_text(
            &mut target,
            &mut positions,
            &mut origins,
            0,
            "[options]\nx = { a = 1 }\n",
        );
        apply_text(
            &mut target,
            &mut positions,
            &mut origins,
            1,
            "[options]\nx = 2\n",
        );
        apply_text(
            &mut target,
            &mut positions,
            &mut origins,
            2,
            "[options]\nx = { b = 3 }\n",
        );
        assert!(target["options"]["x"].get("a").is_none());
        assert_eq!(target["options"]["x"]["b"].as_integer(), Some(3));
        assert!(!origins.contains_key(&vec![
            ValueStep::Key("options".into()),
            ValueStep::Key("x".into()),
            ValueStep::Key("a".into())
        ]));
    }
}
