//! Immutable image-production requests carried by tracked Typst file reads.
//!
//! Observation IDs retain source root and path for dependency tracking; derivative keys hash only
//! source bytes, the resolved recipe, and the image pipeline revision.

use serde::{Deserialize, Serialize};
use tola_address::OutputPath;
use tola_image::{IMAGE_PIPELINE_REVISION, ImageRecipe};
use tola_typst::{ContentDigest, PackageSpec, ReadLocator};
use typst::foundations::{Bytes, PathOrStr};
use typst::loading::DataSource;
use typst::syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};

use crate::TolaPackage;
use crate::protocol::{IMAGE_REQUEST_OBSERVATION_DIRECTORY, IMAGE_REQUEST_OBSERVATION_EXTENSION};

const KEY_DOMAIN: &[u8] = b"tola:image-variant\0";

#[derive(Debug, thiserror::Error)]
pub enum DescriptorError {
    #[error("image request descriptor must end in .json")]
    MissingExtension,
    #[error("image request descriptor must use lowercase hexadecimal encoding")]
    NotLowerHex,
    #[error("invalid image request encoding")]
    Encoding(#[source] hex::FromHexError),
    #[error("invalid image request descriptor")]
    Malformed(#[source] serde_json::Error),
    #[error("unsupported image request processing revision")]
    UnsupportedRevision,
    #[error("noncanonical image request descriptor")]
    Noncanonical,
    #[error(transparent)]
    Serialization(serde_json::Error),
    #[error("image source digest must use lowercase hexadecimal encoding")]
    DigestNotLowerHex,
    #[error("invalid image source digest")]
    DigestEncoding(#[source] hex::FromHexError),
    #[error("invalid image source package")]
    SourcePackage(#[source] PackageSpecMessage),
    #[error("noncanonical image source package")]
    NoncanonicalSourcePackage,
    #[error("invalid image source path")]
    SourcePath,
    #[error("image source path must be a canonical rooted file path")]
    NoncanonicalSourcePath,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct PackageSpecMessage(String);

#[derive(Clone, Debug)]
pub struct ImageRequest {
    source: DataSource,
    /// Resolved file for a path source, or call site for a bytes source.
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
        Self::variant_key(self.source_digest, &self.recipe)
    }

    /// Encoded image identity shared across sources with matching bytes and recipe.
    pub fn variant_key(source_digest: ContentDigest, recipe: &ImageRecipe) -> ContentDigest {
        let mut hash = blake3::Hasher::new();
        hash.update(KEY_DOMAIN);
        hash.update(&IMAGE_PIPELINE_REVISION.to_le_bytes());
        hash.update(source_digest.as_bytes());
        serde_json::to_writer(&mut hash, recipe)
            .expect("image recipe serialization into a digest is infallible");
        ContentDigest::from_bytes(*hash.finalize().as_bytes())
    }

    pub fn output_path(&self) -> OutputPath {
        OutputPath::parse(&format!(
            "_tola/images/{}.{}",
            self.key().to_hex(),
            self.recipe.format().extension(),
        ))
        .expect("image output paths contain only fixed segments and hex")
    }

    pub fn observation_id(&self) -> FileId {
        let bytes = serde_json::to_vec(&self.descriptor())
            .expect("image request descriptors contain only serializable values");
        let path = VirtualPath::new(format!(
            "{IMAGE_REQUEST_OBSERVATION_DIRECTORY}{}{IMAGE_REQUEST_OBSERVATION_EXTENSION}",
            hex::encode(bytes),
        ))
        .expect("image observation paths contain only fixed segments and hex");
        FileId::new(RootedPath::new(
            VirtualRoot::Package(TolaPackage::Image.spec()),
            path,
        ))
    }

    /// Recovers a request from read evidence; malformed descriptors in this namespace error.
    pub fn from_read(locator: &ReadLocator) -> Result<Option<Self>, DescriptorError> {
        let (ReadLocator::Package { package, path }
        | ReadLocator::ProvidedPackage { package, path }) = locator
        else {
            return Ok(None);
        };
        if !owns_pkg(package) {
            return Ok(None);
        }
        let Some(path) = path.to_str() else {
            return Ok(None);
        };
        Ok(decode_observation(path)?.map(|(req, _)| req))
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

/// Pure path-to-bytes mapping for the virtual request namespace.
pub fn observation_bytes(pkg: &PackageSpec, path: &str) -> Option<Vec<u8>> {
    if !owns_pkg(pkg) {
        return None;
    }
    decode_observation(path).ok().flatten().map(|(_, b)| b)
}

pub fn owns_observation(pkg: &PackageSpec, path: &str) -> bool {
    let path = path.strip_prefix('/').unwrap_or(path);
    owns_pkg(pkg)
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
    Bytes {
        origin: Box<SourceDescriptor>,
        bytes: Vec<u8>,
    },
}

impl SourceDescriptor {
    fn from_source(src: &DataSource, origin: &RootedPath) -> Self {
        match src {
            DataSource::Path(_) => Self::from_path(origin),
            DataSource::Bytes(bytes) => Self::Bytes {
                origin: Box::new(Self::from_path(origin)),
                bytes: bytes.to_vec(),
            },
        }
    }

    fn from_path(src: &RootedPath) -> Self {
        let path = src.vpath().get_with_slash().to_owned();
        match src.root() {
            VirtualRoot::Project => Self::Site { path },
            VirtualRoot::Package(pkg) => Self::Package {
                package: pkg.to_string(),
                path,
            },
        }
    }

    fn into_source(self) -> Result<(DataSource, RootedPath), DescriptorError> {
        if let Self::Bytes { origin, bytes } = self {
            let (_, origin) = origin.into_source()?;
            return Ok((DataSource::Bytes(Bytes::new(bytes)), origin));
        }
        let (root, path) = match self {
            Self::Site { path } => (VirtualRoot::Project, path),
            Self::Package { package, path } => {
                let spec = package.parse::<PackageSpec>().map_err(|e| {
                    DescriptorError::SourcePackage(PackageSpecMessage(e.to_string()))
                })?;
                if spec.to_string() != package {
                    return Err(DescriptorError::NoncanonicalSourcePackage);
                }
                (VirtualRoot::Package(spec), path)
            }
            Self::Bytes { .. } => unreachable!(),
        };
        let vpath = VirtualPath::new(&path).map_err(|_| DescriptorError::SourcePath)?;
        if vpath.is_root() || vpath.get_with_slash() != path {
            return Err(DescriptorError::NoncanonicalSourcePath);
        }
        let origin = RootedPath::new(root, vpath);
        Ok((DataSource::Path(PathOrStr::Str(path.into())), origin))
    }
}

fn decode_observation(path: &str) -> Result<Option<(ImageRequest, Vec<u8>)>, DescriptorError> {
    let path = path.strip_prefix('/').unwrap_or(path);
    let Some(encoded) = path.strip_prefix(IMAGE_REQUEST_OBSERVATION_DIRECTORY) else {
        return Ok(None);
    };
    let encoded = encoded
        .strip_suffix(IMAGE_REQUEST_OBSERVATION_EXTENSION)
        .ok_or(DescriptorError::MissingExtension)?;
    if encoded.is_empty() || !is_lower_hex(encoded) {
        return Err(DescriptorError::NotLowerHex);
    }
    let bytes = hex::decode(encoded).map_err(DescriptorError::Encoding)?;
    let desc: RequestDescriptor<ImageRecipe> =
        serde_json::from_slice(&bytes).map_err(DescriptorError::Malformed)?;
    if desc.revision != IMAGE_PIPELINE_REVISION {
        return Err(DescriptorError::UnsupportedRevision);
    }
    if serde_json::to_vec(&desc).map_err(DescriptorError::Serialization)? != bytes {
        return Err(DescriptorError::Noncanonical);
    }
    if !is_lower_hex(&desc.source_digest) {
        return Err(DescriptorError::DigestNotLowerHex);
    }
    let mut digest = [0; 32];
    hex::decode_to_slice(&desc.source_digest, &mut digest)
        .map_err(DescriptorError::DigestEncoding)?;
    let (src, origin) = desc.source.into_source()?;
    Ok(Some((
        ImageRequest::new(src, origin, ContentDigest::from_bytes(digest), desc.recipe),
        bytes,
    )))
}

fn is_lower_hex(val: &str) -> bool {
    val.bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
}

fn owns_pkg(pkg: &PackageSpec) -> bool {
    TolaPackage::from_spec(pkg) == Some(TolaPackage::Image)
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

    fn site_src(path: &str) -> RootedPath {
        RootedPath::new(VirtualRoot::Project, VirtualPath::new(path).unwrap())
    }

    fn path_src(path: &str) -> DataSource {
        DataSource::Path(PathOrStr::Str(path.into()))
    }

    fn locator(path: impl Into<PathBuf>) -> ReadLocator {
        ReadLocator::ProvidedPackage {
            package: TolaPackage::Image.spec(),
            path: path.into(),
        }
    }

    #[test]
    fn observation_id_roundtrip() {
        let pkg_spec = TolaPackage::Image.spec();
        let pkg_src = RootedPath::new(
            VirtualRoot::Package("@local/artwork:1.2.3".parse().unwrap()),
            VirtualPath::new("/assets/illustration.png").unwrap(),
        );
        for src in [site_src("/images/hero.png"), pkg_src] {
            let req = ImageRequest::new(
                path_src(src.vpath().get_with_slash()),
                src.clone(),
                ContentDigest::of(b"pixels"),
                recipe(120),
            );
            let id = req.observation_id();
            let recovered = ImageRequest::from_read(&locator(id.vpath().get_without_slash()))
                .unwrap()
                .unwrap();
            let bytes = observation_bytes(&pkg_spec, id.vpath().get_with_slash()).unwrap();

            assert_eq!(recovered.origin(), &src);
            assert_eq!(recovered.source(), &path_src(src.vpath().get_with_slash()));
            assert_eq!(recovered.source_digest(), req.source_digest());
            assert_eq!(recovered.recipe(), req.recipe());
            assert_eq!(recovered.output_path(), req.output_path());
            assert_eq!(recovered.observation_id(), id);
            assert_eq!(
                observation_bytes(
                    &pkg_spec,
                    recovered.observation_id().vpath().get_with_slash()
                ),
                Some(bytes),
            );
            assert_eq!(
                ImageRequest::from_read(&ReadLocator::Package {
                    package: pkg_spec.clone(),
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
    fn variant_key_shared_across_sources() {
        let bytes = b"same pixels".to_vec();
        let digest = ContentDigest::of(&bytes);
        let by_path = ImageRequest::new(
            path_src("/assets/photo.png"),
            site_src("/assets/photo.png"),
            digest,
            recipe(120),
        );
        let by_bytes = ImageRequest::new(
            DataSource::Bytes(Bytes::new(bytes)),
            site_src("/content/figure.typ"),
            digest,
            recipe(120),
        );

        assert_eq!(by_path.key(), by_bytes.key());
        assert_eq!(by_path.output_path(), by_bytes.output_path());
        assert_ne!(by_path.observation_id(), by_bytes.observation_id());
    }

    #[test]
    fn from_read_filters_package() {
        let req = ImageRequest::new(
            path_src("/hero.png"),
            site_src("/hero.png"),
            ContentDigest::of(b"pixels"),
            recipe(120),
        );
        let path = PathBuf::from(req.observation_id().vpath().get_without_slash());
        for spec in [
            "@preview/images:0.0.0",
            "@tola/icon:0.0.0",
            "@tola/image:0.0.1",
        ] {
            let pkg = spec.parse::<PackageSpec>().unwrap();
            assert!(
                ImageRequest::from_read(&ReadLocator::ProvidedPackage {
                    package: pkg.clone(),
                    path: path.clone(),
                })
                .unwrap()
                .is_none()
            );
            assert!(observation_bytes(&pkg, path.to_str().unwrap()).is_none());
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
