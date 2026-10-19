//! Explicit local package identities, independent of global package discovery.

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use typst::syntax::package::{PackageInfo, PackageManifest, PackageSpec};
use typst::syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};

use crate::world::SourceBoundary;

/// One native package identity backed by a declared local directory.
///
/// The virtual `typst.toml` matches `spec` and names `entry`. It does not read or
/// rewrite a manifest on disk: the host owns the module's actual version and
/// configuration. Other files retain their physical read evidence and native
/// package-root identity, including when this directory is outside the project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalPackage {
    spec: PackageSpec,
    root: PathBuf,
    entry: VirtualPath,
    manifest: Arc<[u8]>,
    boundary: SourceBoundary,
}

impl LocalPackage {
    /// Capture a physical package root and generate its native import manifest.
    ///
    /// File contents are observed by each world's resolver, not at registration.
    /// The root must exist; an absent entry is reported when it is read.
    pub fn new(spec: PackageSpec, root: &Path, entry: VirtualPath) -> io::Result<Self> {
        if entry.get_without_slash().is_empty() || entry.get_without_slash() == "typst.toml" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a local package entry must name a source other than its virtual typst.toml",
            ));
        }
        let root = root.canonicalize()?;
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a local package root must be a directory",
            ));
        }
        let manifest = PackageManifest::new(PackageInfo::new(
            spec.name.clone(),
            spec.version,
            entry.get_without_slash(),
        ));
        let manifest = toml::to_string(&manifest).map_err(io::Error::other)?;
        let boundary = SourceBoundary::new(&root, true);
        Ok(Self {
            spec,
            root,
            entry,
            manifest: Arc::from(manifest.into_bytes()),
            boundary,
        })
    }

    /// The complete native import identity supplied by the host.
    pub fn spec(&self) -> &PackageSpec {
        &self.spec
    }

    /// The physical directory selected at registration, with symlinks resolved.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The public library's complete native package identity.
    pub fn entry_id(&self) -> FileId {
        self.file_id(self.entry.clone())
    }

    /// Address another file, such as a site driver or a model, within this package.
    pub fn file_id(&self, path: VirtualPath) -> FileId {
        FileId::new(RootedPath::new(
            VirtualRoot::Package(self.spec.clone()),
            path,
        ))
    }

    /// The immutable native manifest served for this selected import identity.
    pub fn manifest_bytes(&self) -> &[u8] {
        &self.manifest
    }

    pub(super) fn manifest(&self) -> Arc<[u8]> {
        Arc::clone(&self.manifest)
    }

    pub(super) fn boundary(&self) -> &SourceBoundary {
        &self.boundary
    }
}

/// Immutable bindings installed in one resolver and retained by its worlds.
///
/// Replacing a binding creates another registry. Existing worlds keep their own
/// selection; shared parsed-file caches still revalidate the bytes they observe.
/// Reserved namespaces never fall through to the global package cache, even if
/// no package in that namespace is selected. Namespace policy belongs to the host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalPackages {
    packages: HashMap<PackageSpec, LocalPackage>,
    owned_namespaces: BTreeSet<String>,
}

impl LocalPackages {
    /// Select packages and namespaces that may only resolve from this registry.
    /// Duplicate complete package identities are rejected.
    pub fn new<P, N, S>(packages: P, owned_namespaces: N) -> io::Result<Self>
    where
        P: IntoIterator<Item = LocalPackage>,
        N: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut selected = HashMap::new();
        for package in packages {
            if selected.contains_key(package.spec()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "local package {} is selected more than once",
                        package.spec()
                    ),
                ));
            }
            selected.insert(package.spec.clone(), package);
        }
        let owned_namespaces = owned_namespaces
            .into_iter()
            .map(Into::into)
            .collect::<BTreeSet<_>>();
        for namespace in &owned_namespaces {
            format!("@{namespace}/package:0.0.0")
                .parse::<PackageSpec>()
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
        }
        Ok(Self {
            packages: selected,
            owned_namespaces,
        })
    }

    /// The package selected for this exact name and version.
    pub fn package(&self, spec: &PackageSpec) -> Option<&LocalPackage> {
        self.packages.get(spec)
    }

    /// Selected bindings, in unspecified order.
    pub fn iter(&self) -> impl Iterator<Item = &LocalPackage> {
        self.packages.values()
    }

    /// Whether unresolved identities in this namespace must not use global package lookup.
    pub fn owns_namespace(&self, namespace: &str) -> bool {
        self.owned_namespaces.contains(namespace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::file::{
        FileMap, FileProvider, FileResolver, FileTarget, ReadLocator, ReadOrigin, SharedFileCache,
    };
    use crate::{BundleCancellation, TypstWorld};
    use std::fs;
    use typst::World;
    use typst::foundations::{IntoValue, Value};

    struct Site {
        _directory: tempfile::TempDir,
        project: PathBuf,
        module: PathBuf,
    }

    impl Site {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let project = directory.path().join("project");
            let module = directory.path().join("module");
            fs::create_dir_all(&project).unwrap();
            fs::create_dir_all(&module).unwrap();
            Self {
                _directory: directory,
                project,
                module,
            }
        }

        fn package(&self) -> LocalPackage {
            LocalPackage::new(
                "@tola-modules/forester:0.0.0".parse().unwrap(),
                &self.module,
                VirtualPath::new("lib.typ").unwrap(),
            )
            .unwrap()
        }

        fn files(&self) -> FileResolver {
            FileResolver::new().with_local_packages(Arc::new(
                LocalPackages::new([self.package()], ["tola-modules"]).unwrap(),
            ))
        }

        fn world(&self, files: FileResolver, cache: Arc<SharedFileCache>) -> TypstWorld {
            TypstWorld::builder(&self.project.join("main.typ"), &self.project)
                .with_files(Arc::new(files))
                .with_shared_cache(cache)
                .with_inputs([(
                    "source",
                    RootedPath::new(VirtualRoot::Project, VirtualPath::new("note.txt").unwrap()),
                )])
                .no_fonts()
                .build(&BundleCancellation::default())
                .unwrap()
        }
    }

    fn result(world: &TypstWorld) -> Value {
        let compilation =
            crate::compile_bundle_world(world, &BundleCancellation::default()).unwrap();
        let declarations = compilation.metadata_declarations("result");
        assert_eq!(declarations.len(), 1);
        declarations[0].value().clone()
    }

    #[test]
    fn native_import_keeps_both_roots() {
        let site = Site::new();
        fs::create_dir(site.module.join("lib")).unwrap();
        fs::create_dir(site.module.join("assets")).unwrap();
        fs::write(site.module.join("assets/name.txt"), "module-relative").unwrap();
        fs::write(site.module.join("root.txt"), "module-absolute").unwrap();
        fs::write(site.project.join("root.txt"), "wrong project root").unwrap();
        fs::write(site.project.join("note.txt"), "project source").unwrap();
        fs::write(
            site.module.join("lib/helper.typ"),
            "#let helper() = read(\"../assets/name.txt\")",
        )
        .unwrap();
        fs::write(
            site.module.join("lib.typ"),
            r#"
#import "lib/helper.typ": helper
#let values(file) = (relative: helper(), absolute: read("/root.txt"), project: read(file))
"#,
        )
        .unwrap();
        // An unrelated on-disk manifest must not supply the alias's name/version.
        fs::write(
            site.module.join("typst.toml"),
            "[package]\nname='original'\nversion='7.8.9'\nentrypoint='missing.typ'\n",
        )
        .unwrap();
        fs::write(
            site.project.join("main.typ"),
            r#"
#import "@tola-modules/forester:0.0.0": values
#metadata(values(sys.inputs.source))<result>
"#,
        )
        .unwrap();
        let world = site.world(site.files(), Arc::default());
        let Value::Dict(values) = result(&world) else {
            panic!("the query did not return its dictionary");
        };
        assert_eq!(
            values.get("relative").unwrap(),
            &"module-relative".into_value()
        );
        assert_eq!(
            values.get("absolute").unwrap(),
            &"module-absolute".into_value()
        );
        assert_eq!(
            values.get("project").unwrap(),
            &"project source".into_value()
        );
        assert_eq!(world.root(), site.project.canonicalize().unwrap());

        let selected = site.package();
        assert_eq!(selected.root(), site.module.canonicalize().unwrap());
        let id = selected.file_id(VirtualPath::new("assets/name.txt").unwrap());
        let (bytes, observed) = world.read_file_with_evidence(id);
        assert_eq!(bytes.unwrap().as_slice(), b"module-relative");
        let ReadOrigin::Disk(path) = observed.reads[0].origin() else {
            panic!("the local package asset must retain its disk origin");
        };
        // Registration fixes the physical root; the caller's spelling may be
        // an alias, including /var versus /private/var on macOS.
        assert_eq!(path.as_path(), selected.root().join("assets/name.txt"));
        assert_eq!(observed.disk_reads[0].as_path(), path.as_path());
        assert!(
            matches!(observed.reads[0].evidence().locator(), ReadLocator::ProvidedPackage { package, path }
            if package == selected.spec() && path == Path::new("assets/name.txt"))
        );
        assert!(observed.package_checks.is_empty());
        assert_eq!(
            site.files()
                .read_reported(
                    &site.project,
                    Path::new("@tola-modules/forester:0.0.0/assets/name.txt")
                )
                .unwrap(),
            "module-relative"
        );
        assert_eq!(
            site.files()
                .read_reported(
                    &site.project,
                    Path::new(r"@tola-modules\forester:0.0.0\assets\name.txt"),
                )
                .unwrap(),
            "module-relative"
        );
    }

    #[test]
    fn package_main_keeps_project_root() {
        let site = Site::new();
        fs::write(site.module.join("value.txt"), "package driver").unwrap();
        fs::write(
            site.module.join("driver.typ"),
            "#metadata(read(\"/value.txt\"))<result>",
        )
        .unwrap();
        let main = site
            .package()
            .file_id(VirtualPath::new("driver.typ").unwrap());
        let world = TypstWorld::builder_with_id(main, &site.project)
            .with_files(Arc::new(site.files()))
            .with_local_cache()
            .no_fonts()
            .build(&BundleCancellation::default())
            .unwrap();
        assert_eq!(world.main(), main);
        assert_eq!(world.root(), site.project.canonicalize().unwrap());
        assert_eq!(result(&world), "package driver".into_value());
        assert!(!site.project.join("driver.typ").exists());
    }

    #[test]
    fn reserved_packages_never_fall_back_to_global_roots() {
        let site = Site::new();
        let global = site.project.join("cache");
        for name in ["forester", "unknown"] {
            let directory = global.join(format!("tola-modules/{name}/0.0.0"));
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("missing.typ"), "global package bytes").unwrap();
        }
        let locations = crate::PackageLocations::from_absolute_roots(Some(global), None).unwrap();
        let files =
            FileResolver::from_package_locations(locations, crate::PackageFetchPolicy::LocalOnly)
                .with_local_packages(Arc::new(
                    LocalPackages::new([site.package()], ["tola-modules"]).unwrap(),
                ));
        for name in ["forester", "unknown"] {
            let id = FileId::new(RootedPath::new(
                VirtualRoot::Package(format!("@tola-modules/{name}:0.0.0").parse().unwrap()),
                VirtualPath::new("missing.typ").unwrap(),
            ));
            let observed = files.read_attempt(id, &site.project);
            assert!(observed.result.is_err());
            assert!(observed.package_checks.is_empty());
        }
    }

    #[test]
    fn providers_outrank_local_packages() {
        struct Exclusive;
        impl FileProvider for Exclusive {
            fn target(&self, _: FileId) -> Option<FileTarget> {
                None
            }
            fn owned_namespaces(&self) -> &'static [&'static str] {
                &["tola-modules"]
            }
        }
        let site = Site::new();
        fs::write(site.module.join("lib.typ"), "package source").unwrap();
        let id = site.package().entry_id();
        assert!(
            site.files()
                .with_provider(Exclusive)
                .read(id, &site.project)
                .is_err()
        );
        let mut provided = FileMap::new();
        provided.insert(id, Arc::<[u8]>::from(&b"provider source"[..]));
        assert_eq!(
            site.files()
                .with_provider(provided)
                .read(id, &site.project)
                .unwrap(),
            b"provider source"
        );
        assert!(
            site.files()
                .with_source_boundary(SourceBoundary::new(&site.project, true))
                .read(id, &site.project)
                .is_err()
        );
        assert!(
            site.files()
                .with_source_boundary(SourceBoundary::default().excluding(site.module.clone()))
                .read(id, &site.project)
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn package_symlinks_cannot_escape_even_with_editor_overrides() {
        use std::os::unix::fs::symlink;
        let site = Site::new();
        fs::write(site.module.join("lib.typ"), "inside").unwrap();
        fs::write(site.project.join("outside.typ"), "outside").unwrap();
        symlink("lib.typ", site.module.join("inside.typ")).unwrap();
        symlink(
            site.project.join("outside.typ"),
            site.module.join("outside.typ"),
        )
        .unwrap();
        symlink(
            site.project.join("absent.typ"),
            site.module.join("absent.typ"),
        )
        .unwrap();
        let files = site
            .files()
            .with_disk_overrides([(
                site.project.join("outside.typ"),
                Arc::from(&b"unsaved outside"[..]),
            )])
            .unwrap();
        assert_eq!(
            files
                .read(
                    site.package()
                        .file_id(VirtualPath::new("inside.typ").unwrap()),
                    &site.project
                )
                .unwrap(),
            b"inside"
        );
        for path in ["outside.typ", "absent.typ"] {
            let error = files
                .read(
                    site.package().file_id(VirtualPath::new(path).unwrap()),
                    &site.project,
                )
                .unwrap_err();
            assert!(
                error.to_string().contains("declared package root"),
                "{error}"
            );
        }
    }

    #[test]
    fn shared_cache_revalidates_alias_rebinding_and_local_edits() {
        let site = Site::new();
        fs::write(site.module.join("lib.typ"), "#let value = \"one\"").unwrap();
        fs::write(
            site.project.join("main.typ"),
            "#import \"@tola-modules/forester:0.0.0\": value\n#metadata(value)<result>",
        )
        .unwrap();
        let cache = Arc::new(SharedFileCache::new());
        let first_files = site.files();
        let first = site.world(first_files.clone(), Arc::clone(&cache));
        assert_eq!(result(&first), "one".into_value());
        let second_root = site.module.with_file_name("second-module");
        fs::create_dir(&second_root).unwrap();
        fs::write(second_root.join("lib.typ"), "#let value = \"two\"").unwrap();
        let package = LocalPackage::new(
            site.package().spec().clone(),
            &second_root,
            VirtualPath::new("lib.typ").unwrap(),
        )
        .unwrap();
        let second_files = FileResolver::new().with_local_packages(Arc::new(
            LocalPackages::new([package], ["tola-modules"]).unwrap(),
        ));
        let second = site.world(second_files.clone(), Arc::clone(&cache));
        assert_eq!(result(&second), "two".into_value());
        // The registry selection of an older resolver is immutable.
        assert_eq!(
            result(&site.world(first_files, Arc::clone(&cache))),
            "one".into_value()
        );
        fs::write(second_root.join("lib.typ"), "#let value = \"three\"").unwrap();
        assert_eq!(
            result(&site.world(second_files, cache)),
            "three".into_value()
        );
    }

    #[test]
    fn distinct_aliases_reject_duplicates() {
        let site = Site::new();
        fs::write(site.module.join("lib.typ"), "#let value = 7").unwrap();
        let first = site.package();
        let second = LocalPackage::new(
            "@tola-modules/other:0.0.0".parse().unwrap(),
            &site.module,
            VirtualPath::new("lib.typ").unwrap(),
        )
        .unwrap();
        assert_ne!(first.entry_id(), second.entry_id());
        assert!(LocalPackages::new([first.clone(), first.clone()], ["tola-modules"]).is_err());
        let files = FileResolver::new().with_local_packages(Arc::new(
            LocalPackages::new([first, second], ["tola-modules"]).unwrap(),
        ));
        fs::write(site.project.join("main.typ"), "#import \"@tola-modules/forester:0.0.0\" as a\n#import \"@tola-modules/other:0.0.0\" as b\n#metadata(a.value + b.value)<result>").unwrap();
        assert_eq!(result(&site.world(files, Arc::default())), 14.into_value());
    }
}
