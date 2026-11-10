//! Build-side projections of one prepared module contribution.

use anyhow::Result;
use typst::foundations::{Dict, IntoValue, Value};
use typst::syntax::{RootedPath, VirtualRoot};

use crate::asset::{AssetUrls, ModuleAssetInput};
use tola_module::prepared::{PreparedModules, package_path, virtual_path};

/// Record every selected module's manifest root and file as an exact input.
pub(crate) fn observe_manifests(
    prepared: &PreparedModules,
    observation: &mut crate::observation::InputObservation,
) {
    use crate::observation::{InputKind, InputPathScope};
    for module in prepared.modules.values() {
        if let Some(evidence) = &module.manifest_evidence {
            let (root, manifest) = evidence.paths();
            for identity in [root, manifest] {
                observation.path_pair(
                    InputKind::ModuleManifests,
                    InputPathScope::Exact,
                    true,
                    identity.logical_path(),
                    identity.physical_path(),
                );
            }
        }
    }
}

/// The `@tola/module` records this compilation binds.
pub(crate) fn contexts(prepared: &PreparedModules, urls: &AssetUrls) -> Result<Dict> {
    let mut assets = std::collections::BTreeMap::<&str, (Dict, Dict)>::new();
    for (asset, address) in urls.module_assets() {
        let path = match &asset.input {
            ModuleAssetInput::Module(path) => package_path(&asset.alias, path)?,
            ModuleAssetInput::Project(path) => {
                RootedPath::new(VirtualRoot::Project, virtual_path(path)?)
            }
        };
        let (files, addresses) = assets.entry(&asset.alias).or_default();
        files.insert(asset.name.as_str().into(), path.into_value());
        addresses.insert(asset.name.as_str().into(), address.into_value());
    }
    let enabled = prepared
        .assets
        .iter()
        .map(|assets| assets.alias.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut records = Dict::new();
    for (alias, module) in &prepared.modules {
        let options = tola_module::options::to_typst(&module.options);
        let enabled = enabled.contains(alias.as_str());
        let asset_record = if enabled && !urls.is_prepared() {
            Value::None
        } else {
            let (files, addresses) = assets.remove(alias.as_str()).unwrap_or_default();
            let mut asset_record = Dict::new();
            asset_record.insert("enabled".into(), enabled.into_value());
            asset_record.insert("files".into(), files.into_value());
            asset_record.insert("urls".into(), addresses.into_value());
            asset_record.into_value()
        };
        let mut record = Dict::new();
        record.insert("name".into(), alias.as_str().into_value());
        record.insert("options".into(), options);
        record.insert("assets".into(), asset_record);
        records.insert(alias.as_str().into(), record.into_value());
    }
    Ok(records)
}
