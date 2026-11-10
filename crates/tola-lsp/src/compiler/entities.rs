//! Model queries retained only for their exact editor inputs and module declarations.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tola_build::BuildResources;
use tola_build::cancellation::BuildCancellation;
use tola_build::config::ResolvedSiteConfig;
use tola_build::modules::query::{QueryCompilation, QueryEvaluation, QuerySession};
use tola_input::InputScope;
use tola_module::module_set::LoadedModule;
use tola_typst::typst::syntax::FileId;

struct QueryEntry {
    session: QuerySession,
    compiled: Option<QueryCompilation>,
}

#[derive(Default)]
pub(crate) struct EntityQueries {
    configuration: Option<Arc<ResolvedSiteConfig>>,
    revision: u64,
    scope: Option<InputScope>,
    overlays: Vec<(PathBuf, Arc<str>)>,
    modules: BTreeMap<String, LoadedModule>,
    entries: HashMap<FileId, QueryEntry>,
    queries: BTreeMap<String, Arc<QueryEvaluation>>,
    observed: BTreeSet<PathBuf>,
}

impl EntityQueries {
    pub(crate) fn prepare(
        &mut self,
        configuration: &Arc<ResolvedSiteConfig>,
        revision: u64,
        scope: InputScope,
        overlays: &[(PathBuf, Arc<str>)],
        modules: &BTreeMap<String, LoadedModule>,
    ) {
        let changed = !self
            .configuration
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, configuration))
            || self.scope != Some(scope)
            || &self.modules != modules;
        if changed {
            self.entries.clear();
        }
        if changed || !self.matches(configuration, revision, overlays) {
            self.queries.clear();
            self.observed.clear();
            for entry in self.entries.values_mut() {
                entry.compiled = None;
            }
            self.configuration = Some(Arc::clone(configuration));
            self.revision = revision;
            self.scope = Some(scope);
            self.overlays = overlays.to_vec();
            self.modules = modules.clone();
        }
    }

    pub(crate) fn get(
        &mut self,
        module: &LoadedModule,
        query_name: &str,
        resources: BuildResources,
        cancellation: &BuildCancellation,
    ) -> Result<Arc<QueryEvaluation>> {
        cancellation.ensure_active()?;
        let name = format!("{}.{}", module.name, query_name);
        if let Some(result) = self.queries.get(&name) {
            return Ok(Arc::clone(result));
        }
        let declaration = &module.queries[query_name];
        let entry_id =
            tola_module::prepared::package_path(&module.name, &declaration.entry)?.intern();
        if let std::collections::hash_map::Entry::Vacant(slot) = self.entries.entry(entry_id) {
            let session = QuerySession::new(
                Arc::clone(
                    self.configuration
                        .as_ref()
                        .expect("prepared entity queries"),
                ),
                module,
                declaration,
                &name,
                resources,
                cancellation,
            )?;
            slot.insert(QueryEntry {
                session,
                compiled: None,
            });
        }
        let entry = self
            .entries
            .get_mut(&entry_id)
            .expect("prepared query entry");
        if entry.compiled.is_none() {
            let compiled = entry.session.inspect(self.overlays.clone(), cancellation)?;
            cancellation.ensure_active()?;
            entry.compiled = Some(compiled);
        }
        let result = Arc::new(
            entry
                .compiled
                .as_ref()
                .expect("compiled query entry")
                .read_entities(module, declaration, &name, cancellation)?,
        );
        if !result.result.success {
            entry.compiled = None;
        }
        cancellation.ensure_active()?;
        self.observe(&result);
        // A failed model may recover when a missing input appears without a usable compilation.
        if result.result.success {
            self.queries.insert(name, Arc::clone(&result));
        }
        Ok(result)
    }

    pub(crate) fn observed_paths(
        &self,
        configuration: &Arc<ResolvedSiteConfig>,
        revision: u64,
        overlays: &[(PathBuf, Arc<str>)],
    ) -> Vec<PathBuf> {
        if !self.matches(configuration, revision, overlays) {
            return Vec::new();
        }
        let mut paths = self.observed.iter().cloned().collect::<Vec<_>>();
        for (alias, module) in &self.modules {
            let declared = tola_build::inputs::declaration_root(
                configuration.get_root(),
                configuration.vendor().path.as_deref(),
                alias,
                &configuration.modules()[alias].source,
            )
            .unwrap_or_else(|_| module.root.clone());
            paths.extend([
                declared.join("tola-module.toml"),
                declared,
                module.root.clone(),
                module.manifest.clone(),
            ]);
        }
        paths.sort();
        paths.dedup();
        paths
    }

    fn observe(&mut self, result: &QueryEvaluation) {
        self.observe_accessed(&result.source_accessed);
        if let Some(revision) = &result.revision {
            self.observed.extend(revision.disk_reads().iter().cloned());
            self.observe_packages(revision.package_checks());
        }
    }

    pub(crate) fn observe_accessed(&mut self, accessed: &tola_typst::AccessedDeps) {
        self.observed.extend(
            accessed
                .disk_reads
                .iter()
                .map(|read| read.as_path().to_path_buf()),
        );
        self.observe_packages(&accessed.package_checks);
    }

    fn observe_packages(&mut self, packages: &[tola_typst::PackageCheck]) {
        for check in packages {
            self.observed.insert(check.candidate().to_path_buf());
            if let Some(target) = check.canonical_target() {
                self.observed.insert(target.to_path_buf());
            }
        }
    }

    fn matches(
        &self,
        configuration: &Arc<ResolvedSiteConfig>,
        revision: u64,
        overlays: &[(PathBuf, Arc<str>)],
    ) -> bool {
        self.configuration
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, configuration))
            && self.revision == revision
            && self.overlays == overlays
    }

    pub(crate) fn release(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::tests::site_configuration;
    use tola_build::cancellation::BuildCanceller;

    #[test]
    fn cache_reuse_requires_the_same_inputs() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("content")).unwrap();
        std::fs::create_dir(root.join("theme")).unwrap();
        std::fs::write(root.join("theme/lib.typ"), "#let link(id) = []").unwrap();
        let manifest = root.join("theme/tola-module.toml");
        std::fs::write(
            &manifest,
            "\n[typst]\nentry='lib.typ'\n[queries.nodes]\nentry='model.typ'\nlabel='nodes'\n[[queries.nodes.references]]\nexport='link'\nparameter={positional=0}\n",
        )
        .unwrap();
        let model = root.join("theme/model.typ");
        let first_model = "#metadata(((id: \"ONE\", name: \"One\"),))<nodes>";
        std::fs::write(&model, first_model).unwrap();
        let configuration = site_configuration(root, "[modules.notes]\npath='theme'\n");
        let modules = tola_build::modules::load_modules(
            configuration.get_root(),
            configuration.vendor().path.as_deref(),
            configuration.modules(),
            tola_input::InputMode::new(InputScope::Offline, true),
            &Default::default(),
        )
        .unwrap();
        let resources = BuildResources::new()
            .with_input_scope(InputScope::Offline)
            .without_system_fonts();
        let cancellation = BuildCancellation::default();
        let mut cache = EntityQueries::default();
        cache.prepare(&configuration, 1, InputScope::Offline, &[], &modules);
        let first = cache
            .get(&modules["notes"], "nodes", resources.clone(), &cancellation)
            .unwrap();
        assert!(first.result.success, "{:?}", first.result.diagnostics);
        let cached = cache
            .get(&modules["notes"], "nodes", resources.clone(), &cancellation)
            .unwrap();
        assert!(Arc::ptr_eq(&first, &cached));

        let overlays = vec![(
            model.clone(),
            Arc::<str>::from(first_model.replace("ONE", "UNSAVED")),
        )];
        cache.prepare(&configuration, 1, InputScope::Offline, &overlays, &modules);
        let unsaved = cache
            .get(&modules["notes"], "nodes", resources.clone(), &cancellation)
            .unwrap();
        assert_eq!(unsaved.entities[0].id, "UNSAVED");
        assert!(!Arc::ptr_eq(&first, &unsaved));
        cache.prepare(&configuration, 2, InputScope::Offline, &overlays, &modules);
        let newer = cache
            .get(&modules["notes"], "nodes", resources.clone(), &cancellation)
            .unwrap();
        assert!(!Arc::ptr_eq(&unsaved, &newer));

        let changed =
            std::fs::read_to_string(&manifest).unwrap() + "\n# manifest snapshot changed\n";
        std::fs::write(&manifest, changed).unwrap();
        let updated = tola_build::modules::load_modules(
            configuration.get_root(),
            configuration.vendor().path.as_deref(),
            configuration.modules(),
            tola_input::InputMode::new(InputScope::Offline, true),
            &Default::default(),
        )
        .unwrap();
        cache.prepare(&configuration, 2, InputScope::Offline, &overlays, &updated);
        let manifest_changed = cache
            .get(&updated["notes"], "nodes", resources.clone(), &cancellation)
            .unwrap();
        assert!(!Arc::ptr_eq(&newer, &manifest_changed));

        let pure = resources.clone().with_input_scope(InputScope::Pure);
        cache.prepare(&configuration, 2, InputScope::Pure, &overlays, &updated);
        let restricted = cache
            .get(&updated["notes"], "nodes", pure, &cancellation)
            .unwrap();
        assert!(!Arc::ptr_eq(&manifest_changed, &restricted));
        let reloaded = Arc::new(configuration.as_ref().clone());
        cache.prepare(&reloaded, 2, InputScope::Offline, &overlays, &updated);
        let reconfigured = cache
            .get(&updated["notes"], "nodes", resources.clone(), &cancellation)
            .unwrap();
        assert!(!Arc::ptr_eq(&restricted, &reconfigured));

        let canceller = BuildCanceller::new();
        canceller.cancel();
        assert!(
            cache
                .get(&updated["notes"], "nodes", resources, &canceller.token())
                .is_err(),
            "a warm cache must still respect cancellation"
        );
        cache.release();
        assert!(
            cache
                .observed_paths(&configuration, 2, &overlays)
                .is_empty()
        );
    }

    #[test]
    fn failed_json_read_recovers_when_created() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("content")).unwrap();
        std::fs::create_dir(root.join("theme")).unwrap();
        std::fs::write(root.join("theme/lib.typ"), "#let link(id) = []").unwrap();
        std::fs::write(
            root.join("theme/tola-module.toml"),
            "\n[typst]\nentry='lib.typ'\n[queries.nodes]\nentry='model.typ'\nlabel='nodes'\n[[queries.nodes.references]]\nexport='link'\nparameter={positional=0}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("theme/model.typ"),
            "#metadata(json(\"missing.json\"))<nodes>",
        )
        .unwrap();
        let configuration = site_configuration(root, "[modules.notes]\npath='theme'\n");
        let modules = tola_build::modules::load_modules(
            configuration.get_root(),
            configuration.vendor().path.as_deref(),
            configuration.modules(),
            tola_input::InputMode::new(InputScope::Offline, true),
            &Default::default(),
        )
        .unwrap();
        let resources = BuildResources::new()
            .with_input_scope(InputScope::Offline)
            .without_system_fonts();
        let cancellation = BuildCancellation::default();
        let mut cache = EntityQueries::default();
        cache.prepare(&configuration, 1, InputScope::Offline, &[], &modules);
        let failure = cache
            .get(&modules["notes"], "nodes", resources.clone(), &cancellation)
            .unwrap();
        assert!(!failure.result.success);
        let missing = configuration.get_root().join("theme/missing.json");
        assert!(
            failure
                .revision
                .as_ref()
                .unwrap()
                .disk_reads()
                .contains(&missing)
        );
        assert!(
            cache
                .observed_paths(&configuration, 1, &[])
                .contains(&missing)
        );
        std::fs::write(missing, r#"[{"id":"RECOVERED","name":"Recovered"}]"#).unwrap();
        let recovered = cache
            .get(&modules["notes"], "nodes", resources, &cancellation)
            .unwrap();
        assert!(
            recovered.result.success,
            "{:?}",
            recovered.result.diagnostics
        );
        assert_eq!(recovered.entities[0].id, "RECOVERED");
    }
}
