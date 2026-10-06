//! Immutable image-production requests carried by ordinary Typst file reads.
//!
//! The observation identity retains the source root and path for dependency tracking; the
//! derivative identity contains only source bytes, the resolved recipe, and the image pipeline
//! revision.
//!
//! A descriptor is a file in the reserved namespace whose path is its own content, so a read
//! recovers the request without a lookup table.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use tola_typst::{ContentDigest, PackageSpec, ReadLocator};
use typst::syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};

use crate::TolaPackage;
use tola_address::OutputPath;

use crate::protocol::{IMAGE_REQUEST_OBSERVATION_DIRECTORY, IMAGE_REQUEST_OBSERVATION_EXTENSION};
use tola_image::{IMAGE_PIPELINE_REVISION, ImageRecipe};

/// Keeps the derivative digest distinct from any other hash over the same bytes.
const KEY_DOMAIN: &[u8] = b"tola:image-variant\0";

#[derive(Clone, Debug)]
pub struct ImageRequest {
    source: RootedPath,
    source_digest: ContentDigest,
    recipe: ImageRecipe,
}

impl ImageRequest {
    pub fn new(source: RootedPath, source_digest: ContentDigest, recipe: ImageRecipe) -> Self {
        Self {
            source,
            source_digest,
            recipe,
        }
    }

    pub fn source(&self) -> &RootedPath {
        &self.source
    }

    pub fn source_digest(&self) -> ContentDigest {
        self.source_digest
    }

    pub fn recipe(&self) -> &ImageRecipe {
        &self.recipe
    }

    pub fn key(&self) -> ContentDigest {
        let mut hash = blake3::Hasher::new();
        hash.update(KEY_DOMAIN);
        hash.update(&IMAGE_PIPELINE_REVISION.to_le_bytes());
        hash.update(self.source_digest.as_bytes());
        // Recipes carry only validated scalar fields; streaming their canonical serialization
        // avoids allocating a second descriptor.
        serde_json::to_writer(&mut hash, &self.recipe)
            .expect("image recipe serialization into a digest is infallible");
        ContentDigest::from_bytes(*hash.finalize().as_bytes())
    }

    pub fn output_path(&self) -> OutputPath {
        OutputPath::parse(&format!(
            "_tola/images/{}.{}",
            self.key().to_hex(),
            self.recipe.format().extension(),
        ))
        .expect("image output paths contain only fixed segments and hexadecimal keys")
    }

    pub fn observation_id(&self) -> FileId {
        let bytes = serde_json::to_vec(&self.descriptor())
            .expect("image request descriptors contain only serializable scalar values");
        let path = VirtualPath::new(format!(
            "{IMAGE_REQUEST_OBSERVATION_DIRECTORY}{}{IMAGE_REQUEST_OBSERVATION_EXTENSION}",
            hex::encode(bytes),
        ))
        .expect("image observation paths contain only fixed segments and hexadecimal bytes");
        FileId::new(RootedPath::new(VirtualRoot::Package(package_spec()), path))
    }

    /// Recover only this protocol's requests from successful read evidence. A file in the protocol
    /// namespace with a malformed descriptor is an error, not a dependency to skip silently.
    pub fn from_read(locator: &ReadLocator) -> Result<Option<Self>> {
        let (ReadLocator::Package { package, path }
        | ReadLocator::ProvidedPackage { package, path }) = locator
        else {
            return Ok(None);
        };
        if !owns_package(package) {
            return Ok(None);
        }
        let Some(path) = path.to_str() else {
            return Ok(None);
        };
        Ok(decode_observation(path)?.map(|(request, _)| request))
    }

    fn descriptor(&self) -> RequestDescriptor<&ImageRecipe> {
        RequestDescriptor {
            revision: IMAGE_PIPELINE_REVISION,
            source: SourceDescriptor::from_path(&self.source),
            source_digest: self.source_digest.to_hex(),
            recipe: &self.recipe,
        }
    }
}

/// Provider dispatch is a pure path-to-bytes mapping: it does not load the
/// source, enqueue work, encode an image, or touch the filesystem.
pub fn observation_bytes(package: &PackageSpec, path: &str) -> Option<Vec<u8>> {
    if !owns_package(package) {
        return None;
    }
    decode_observation(path)
        .ok()
        .flatten()
        .map(|(_, bytes)| bytes)
}

/// This reserved virtual namespace never falls through to package-directory files.
pub fn owns_observation(package: &PackageSpec, path: &str) -> bool {
    let path = path.strip_prefix('/').unwrap_or(path);
    owns_package(package)
        && (path == IMAGE_REQUEST_OBSERVATION_DIRECTORY.trim_end_matches('/')
            || path.starts_with(IMAGE_REQUEST_OBSERVATION_DIRECTORY))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestDescriptor<R> {
    revision: u32,
    source: SourceDescriptor,
    source_digest: String,
    recipe: R,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "root", rename_all = "lowercase", deny_unknown_fields)]
enum SourceDescriptor {
    Site { path: String },
    Package { package: String, path: String },
}

impl SourceDescriptor {
    fn from_path(source: &RootedPath) -> Self {
        let path = source.vpath().get_with_slash().to_owned();
        match source.root() {
            VirtualRoot::Project => Self::Site { path },
            VirtualRoot::Package(package) => Self::Package {
                package: package.to_string(),
                path,
            },
        }
    }

    fn into_path(self) -> Result<RootedPath> {
        let (root, path) = match self {
            Self::Site { path } => (VirtualRoot::Project, path),
            Self::Package { package, path } => {
                let spec = package
                    .parse::<PackageSpec>()
                    .map_err(anyhow::Error::msg)
                    .context("invalid image source package")?;
                ensure!(
                    spec.to_string() == package,
                    "noncanonical image source package"
                );
                (VirtualRoot::Package(spec), path)
            }
        };
        let virtual_path = VirtualPath::new(&path).context("invalid image source path")?;
        ensure!(
            !virtual_path.is_root() && virtual_path.get_with_slash() == path,
            "image source path must be a canonical rooted file path",
        );
        Ok(RootedPath::new(root, virtual_path))
    }
}

fn decode_observation(path: &str) -> Result<Option<(ImageRequest, Vec<u8>)>> {
    let path = path.strip_prefix('/').unwrap_or(path);
    let Some(encoded) = path.strip_prefix(IMAGE_REQUEST_OBSERVATION_DIRECTORY) else {
        return Ok(None);
    };
    let encoded = encoded
        .strip_suffix(IMAGE_REQUEST_OBSERVATION_EXTENSION)
        .context("image request descriptor must end in .json")?;
    ensure!(
        !encoded.is_empty() && is_lower_hex(encoded),
        "image request descriptor must use lowercase hexadecimal encoding",
    );
    let bytes = hex::decode(encoded).context("invalid image request encoding")?;
    // ImageRecipe's deserializer validates structural bounds and encoding options; the producer
    // additionally validates it against actual metadata.
    let descriptor: RequestDescriptor<ImageRecipe> =
        serde_json::from_slice(&bytes).context("invalid image request descriptor")?;
    ensure!(
        descriptor.revision == IMAGE_PIPELINE_REVISION,
        "unsupported image request processing revision",
    );
    ensure!(
        serde_json::to_vec(&descriptor)? == bytes,
        "noncanonical image request descriptor",
    );
    ensure!(
        is_lower_hex(&descriptor.source_digest),
        "image source digest must use lowercase hexadecimal encoding",
    );
    let mut digest = [0; 32];
    hex::decode_to_slice(&descriptor.source_digest, &mut digest)
        .context("invalid image source digest")?;
    let source = descriptor.source.into_path()?;
    Ok(Some((
        ImageRequest::new(source, ContentDigest::from_bytes(digest), descriptor.recipe),
        bytes,
    )))
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn owns_package(package: &PackageSpec) -> bool {
    TolaPackage::from_spec(package) == Some(TolaPackage::Image)
}

fn package_spec() -> PackageSpec {
    TolaPackage::Image.spec()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use tola_image::{
        ImageFormat, ImageMetadata, OutputFormat, ResizeFilter, ResizeOperation, ResizeOptions,
    };

    fn recipe(width: u32) -> ImageRecipe {
        ImageRecipe::resolve(
            &ImageMetadata {
                width: 800,
                height: 600,
                format: ImageFormat::Png,
                has_alpha: false,
                is_lossy: false,
            },
            ResizeOptions {
                width: Some(width),
                height: Some(80),
                operation: ResizeOperation::Fill,
                format: OutputFormat::Png,
                quality: None,
                filter: ResizeFilter::Lanczos3,
                background: None,
            },
        )
        .unwrap()
    }

    fn site_source(path: &str) -> RootedPath {
        RootedPath::new(VirtualRoot::Project, VirtualPath::new(path).unwrap())
    }

    fn locator(path: impl Into<PathBuf>) -> ReadLocator {
        ReadLocator::ProvidedPackage {
            package: package_spec(),
            path: path.into(),
        }
    }

    fn descriptor_path(bytes: &[u8]) -> String {
        format!(
            "{IMAGE_REQUEST_OBSERVATION_DIRECTORY}{}{IMAGE_REQUEST_OBSERVATION_EXTENSION}",
            hex::encode(bytes),
        )
    }

    #[test]
    fn derivative_identity_ignores_location() {
        let source = site_source("/images/hero.png");
        let digest = ContentDigest::of(b"original image bytes");
        let first = ImageRequest::new(source.clone(), digest, recipe(120));
        let relocated = ImageRequest::new(site_source("/other/copy.png"), digest, recipe(120));
        let changed =
            ImageRequest::new(source.clone(), ContentDigest::of(b"new bytes"), recipe(120));
        let resized = ImageRequest::new(source, digest, recipe(121));

        assert_eq!(first.key(), relocated.key());
        assert_eq!(first.output_path(), relocated.output_path());
        assert_ne!(first.observation_id(), relocated.observation_id());
        assert_ne!(first.key(), changed.key());
        assert_ne!(first.key(), resized.key());
    }

    #[test]
    fn observation_ids_round_trip_per_root() {
        let package_source = RootedPath::new(
            VirtualRoot::Package("@local/artwork:1.2.3".parse().unwrap()),
            VirtualPath::new("/assets/illustration.png").unwrap(),
        );
        for source in [site_source("/images/hero.png"), package_source] {
            let request =
                ImageRequest::new(source.clone(), ContentDigest::of(b"pixels"), recipe(120));
            let id = request.observation_id();
            let read = locator(id.vpath().get_without_slash());
            let recovered = ImageRequest::from_read(&read).unwrap().unwrap();
            let bytes = observation_bytes(&package_spec(), id.vpath().get_with_slash()).unwrap();

            assert_eq!(recovered.source(), &source);
            assert_eq!(recovered.source_digest(), request.source_digest());
            assert_eq!(recovered.recipe(), request.recipe());
            assert_eq!(recovered.output_path(), request.output_path());
            assert_eq!(recovered.observation_id(), id);
            assert_eq!(
                observation_bytes(
                    &package_spec(),
                    recovered.observation_id().vpath().get_with_slash()
                ),
                Some(bytes),
            );
            assert_eq!(
                ImageRequest::from_read(&ReadLocator::Package {
                    package: package_spec(),
                    path: PathBuf::from(id.vpath().get_without_slash()),
                })
                .unwrap()
                .unwrap()
                .observation_id(),
                id,
            );
        }
    }

    #[test]
    fn malformed_descriptors_are_rejected() {
        let request = ImageRequest::new(
            site_source("/hero.png"),
            ContentDigest::of(b"pixels"),
            recipe(120),
        );
        let descriptor = serde_json::to_value(request.descriptor()).unwrap();
        let reject = |value: &serde_json::Value| {
            let path = descriptor_path(&serde_json::to_vec(value).unwrap());
            assert!(ImageRequest::from_read(&locator(&path)).is_err());
            assert!(observation_bytes(&package_spec(), &path).is_none());
        };

        let mut escaping = descriptor.clone();
        escaping["source"]["path"] = "../../outside.png".into();
        reject(&escaping);
        let mut alias = descriptor.clone();
        alias["source"]["path"] = "/images/../hero.png".into();
        reject(&alias);
        let mut invalid_package = descriptor.clone();
        invalid_package["source"] = serde_json::json!({
            "root": "package", "package": "@local/../secret:1.0.0", "path": "/hero.png",
        });
        reject(&invalid_package);
        let mut invalid_digest = descriptor.clone();
        invalid_digest["source_digest"] = "00".into();
        reject(&invalid_digest);
        let mut invalid_recipe = descriptor.clone();
        invalid_recipe["recipe"] = serde_json::Value::Null;
        reject(&invalid_recipe);
        let mut stale_revision = descriptor.clone();
        stale_revision["revision"] = 999u32.into();
        reject(&stale_revision);
        let mut unknown_field = descriptor.clone();
        unknown_field["source"]["machine_root"] = "/private/site".into();
        reject(&unknown_field);

        let pretty = descriptor_path(&serde_json::to_vec_pretty(&descriptor).unwrap());
        assert!(ImageRequest::from_read(&locator(pretty)).is_err());
        assert!(ImageRequest::from_read(&locator(".tola-image-request/not-hex.json")).is_err());
    }

    #[test]
    fn only_image_package_reads_requests() {
        let request = ImageRequest::new(
            site_source("/hero.png"),
            ContentDigest::of(b"pixels"),
            recipe(120),
        );
        let path = PathBuf::from(request.observation_id().vpath().get_without_slash());
        for spec in [
            "@preview/images:0.0.0",
            "@tola/icon:0.0.0",
            "@tola/image:0.0.1",
        ] {
            let package = spec.parse::<PackageSpec>().unwrap();
            assert!(
                ImageRequest::from_read(&ReadLocator::ProvidedPackage {
                    package: package.clone(),
                    path: path.clone(),
                })
                .unwrap()
                .is_none()
            );
            assert!(observation_bytes(&package, path.to_str().unwrap()).is_none());
        }
        assert!(
            ImageRequest::from_read(&ReadLocator::ProvidedRoot(path))
                .unwrap()
                .is_none()
        );
        assert!(
            ImageRequest::from_read(&locator("lib.typ"))
                .unwrap()
                .is_none()
        );
    }
}
