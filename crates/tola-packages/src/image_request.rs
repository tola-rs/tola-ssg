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
use typst::foundations::{Bytes, PathOrStr};
use typst::loading::DataSource;
use typst::syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};

use crate::TolaPackage;
use tola_address::OutputPath;

use crate::protocol::{IMAGE_REQUEST_OBSERVATION_DIRECTORY, IMAGE_REQUEST_OBSERVATION_EXTENSION};
use tola_image::{IMAGE_PIPELINE_REVISION, ImageRecipe};

/// Keeps the derivative digest distinct from any other hash over the same bytes.
const KEY_DOMAIN: &[u8] = b"tola:image-variant\0";

#[derive(Clone, Debug)]
pub struct ImageRequest {
    source: DataSource,
    /// The resolved file for a path source; the call site for a bytes source.
    origin: RootedPath,
    source_digest: ContentDigest,
    recipe: ImageRecipe,
}

impl ImageRequest {
    pub fn new(
        source: DataSource,
        origin: RootedPath,
        source_digest: ContentDigest,
        recipe: ImageRecipe,
    ) -> Self {
        Self {
            source,
            origin,
            source_digest,
            recipe,
        }
    }

    pub fn source(&self) -> &DataSource {
        &self.source
    }

    pub fn origin(&self) -> &RootedPath {
        &self.origin
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
            source: SourceDescriptor::from_source(&self.source, &self.origin),
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
    Site {
        path: String,
    },
    Package {
        package: String,
        path: String,
    },
    /// An in-memory source: the call site that supplied it, plus the bytes.
    Bytes {
        origin: Box<SourceDescriptor>,
        bytes: Vec<u8>,
    },
}

impl SourceDescriptor {
    fn from_source(source: &DataSource, origin: &RootedPath) -> Self {
        match source {
            DataSource::Path(_) => Self::from_path(origin),
            DataSource::Bytes(bytes) => Self::Bytes {
                origin: Box::new(Self::from_path(origin)),
                bytes: bytes.to_vec(),
            },
        }
    }

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

    fn into_source(self) -> Result<(DataSource, RootedPath)> {
        if let Self::Bytes { origin, bytes } = self {
            let (_, origin) = origin.into_source()?;
            return Ok((DataSource::Bytes(Bytes::new(bytes)), origin));
        }
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
            Self::Bytes { .. } => unreachable!(),
        };
        let virtual_path = VirtualPath::new(&path).context("invalid image source path")?;
        ensure!(
            !virtual_path.is_root() && virtual_path.get_with_slash() == path,
            "image source path must be a canonical rooted file path",
        );
        let origin = RootedPath::new(root, virtual_path);
        Ok((DataSource::Path(PathOrStr::Str(path.into())), origin))
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
    let (source, origin) = descriptor.source.into_source()?;
    Ok(Some((
        ImageRequest::new(
            source,
            origin,
            ContentDigest::from_bytes(digest),
            descriptor.recipe,
        ),
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
    use tola_image::{ImageFormat, ImageMetadata, OutputFormat, ResizeOperation, ResizeOptions};

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
                ..ResizeOptions::default()
            },
        )
        .unwrap()
    }

    fn site_source(path: &str) -> RootedPath {
        RootedPath::new(VirtualRoot::Project, VirtualPath::new(path).unwrap())
    }

    fn path_source(path: &str) -> DataSource {
        DataSource::Path(PathOrStr::Str(path.into()))
    }

    fn locator(path: impl Into<PathBuf>) -> ReadLocator {
        ReadLocator::ProvidedPackage {
            package: package_spec(),
            path: path.into(),
        }
    }
    #[test]
    fn observation_ids_round_trip_per_root() {
        let package_source = RootedPath::new(
            VirtualRoot::Package("@local/artwork:1.2.3".parse().unwrap()),
            VirtualPath::new("/assets/illustration.png").unwrap(),
        );
        for source in [site_source("/images/hero.png"), package_source] {
            let request = ImageRequest::new(
                path_source(source.vpath().get_with_slash()),
                source.clone(),
                ContentDigest::of(b"pixels"),
                recipe(120),
            );
            let id = request.observation_id();
            let read = locator(id.vpath().get_without_slash());
            let recovered = ImageRequest::from_read(&read).unwrap().unwrap();
            let bytes = observation_bytes(&package_spec(), id.vpath().get_with_slash()).unwrap();

            assert_eq!(recovered.origin(), &source);
            assert_eq!(
                recovered.source(),
                &path_source(source.vpath().get_with_slash())
            );
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
    fn bytes_and_path_sources_share_derivative_identity() {
        let bytes = b"same pixels".to_vec();
        let digest = ContentDigest::of(&bytes);
        let by_path = ImageRequest::new(
            path_source("/assets/photo.png"),
            site_source("/assets/photo.png"),
            digest,
            recipe(120),
        );
        let by_bytes = ImageRequest::new(
            DataSource::Bytes(Bytes::new(bytes)),
            site_source("/content/figure.typ"),
            digest,
            recipe(120),
        );

        assert_eq!(by_path.key(), by_bytes.key());
        assert_eq!(by_path.output_path(), by_bytes.output_path());
        assert_ne!(by_path.observation_id(), by_bytes.observation_id());
    }
    #[test]
    fn only_image_package_reads_requests() {
        let request = ImageRequest::new(
            path_source("/hero.png"),
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
