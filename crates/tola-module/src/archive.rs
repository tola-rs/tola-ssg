//! Read one module archive without executing code or writing outside its staging directory.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context as _, Result, bail, ensure};
use reqwest::blocking::Client;
use tola_input::{Sha256Digest, file_sha256};
use tola_typst::BundleCancellation;

const MAX_DOWNLOAD: u64 = 128 * 1024 * 1024;
const MAX_EXPANDED: u64 = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

/// A complete download; the compressed file disappears after materialization.
pub(super) struct Download {
    pub file: tempfile::NamedTempFile,
    pub digest: Sha256Digest,
}

pub(super) fn from_file(path: &Path, cancellation: &BundleCancellation) -> Result<Download> {
    let mut source = tola_fs::open_regular_file(path, true)
        .context("could not open the module archive as an ordinary file")?;
    ensure!(
        source.metadata()?.len() <= MAX_DOWNLOAD,
        "module archive exceeds 128 MiB"
    );
    copy_download(&mut source, cancellation)
}

pub(super) fn from_url(url: &str, cancellation: &BundleCancellation) -> Result<Download> {
    tola_input::TarballLocation::Url {
        url: url.to_owned(),
    }
    .validate()
    .map_err(anyhow::Error::msg)?;
    static CLIENT: OnceLock<Client> = OnceLock::new();
    let client = match CLIENT.get() {
        Some(client) => client,
        None => {
            let client = Client::builder()
                .https_only(true)
                .no_gzip()
                .no_brotli()
                .no_deflate()
                .no_zstd()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .user_agent(concat!("tola/", env!("CARGO_PKG_VERSION")))
                .build()
                .context("could not prepare module downloads")?;
            CLIENT.get_or_init(|| client)
        }
    };
    download(client, url, cancellation)
}

fn download(client: &Client, url: &str, cancellation: &BundleCancellation) -> Result<Download> {
    cancellation.ensure_active()?;
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send();
    cancellation.ensure_active()?;
    let mut response = response
        .map_err(reqwest::Error::without_url)
        .context("could not download the module archive")?;
    ensure!(
        response.status().is_success(),
        "module download returned HTTP {}",
        response.status().as_u16()
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|size| size <= MAX_DOWNLOAD),
        "module archive exceeds 128 MiB"
    );
    copy_download(&mut response, cancellation)
}

fn copy_download(source: &mut impl Read, cancellation: &BundleCancellation) -> Result<Download> {
    let mut file = tempfile::NamedTempFile::new().context("could not stage the module download")?;
    copy_bounded(source, file.as_file_mut(), MAX_DOWNLOAD, cancellation)?;
    file.flush()?;
    cancellation.ensure_active()?;
    let digest = file_sha256(file.path()).context("could not checksum the module archive")?;
    cancellation.ensure_active()?;
    file.rewind()?;
    Ok(Download { file, digest })
}

pub(super) fn extract(
    file: &mut File,
    root: &Path,
    cancellation: &BundleCancellation,
) -> Result<()> {
    cancellation.ensure_active()?;
    file.rewind()?;
    let mut signature = [0; 4];
    let count = file.read(&mut signature)?;
    file.rewind()?;
    fs::create_dir(root).context("could not prepare the module directory")?;
    tola_fs::set_unix_mode(root, 0o755)?;
    let mut unpacker = Unpacker {
        root,
        wrapper: None,
        paths: BTreeMap::new(),
        entries: 0,
        bytes: 0,
        cancellation,
    };
    let result = if count >= 2 && signature[..2] == [0x1f, 0x8b] {
        unpack_tar(flate2::read::MultiGzDecoder::new(file), &mut unpacker)
    } else if count == 4
        && (signature == [0x28, 0xb5, 0x2f, 0xfd]
            || u32::from_le_bytes(signature) & 0xfffffff0 == 0x184d2a50)
    {
        unpack_tar(ZstdFrames::new(file, cancellation), &mut unpacker)
    } else if count == 4 && signature[..2] == *b"PK" {
        unpack_zip(file, &mut unpacker)
    } else {
        bail!("module archive must be a tar.gz, tar.zst, or ZIP file");
    };
    cancellation.ensure_active()?;
    result?;
    ensure!(
        root.join("tola-module.toml").is_file(),
        "module archive needs `tola-module.toml` directly inside its single outer directory"
    );
    Ok(())
}

fn unpack_tar(reader: impl Read, unpacker: &mut Unpacker<'_>) -> Result<()> {
    let reader = BoundedReader {
        inner: reader,
        remaining: MAX_EXPANDED,
        cancellation: unpacker.cancellation,
    };
    let mut archive = tar::Archive::new(reader);
    for entry in archive
        .entries()
        .context("could not read the tar archive")?
    {
        let mut entry = entry.context("could not read a tar member")?;
        let path = std::str::from_utf8(&entry.path_bytes())
            .context("archive member paths must be UTF-8")?
            .to_owned();
        let kind = entry.header().entry_type();
        ensure!(
            kind.is_file() || kind.is_dir(),
            "archive member `{}` must be an ordinary file or directory",
            path.escape_debug()
        );
        let size = entry.size();
        let mode = entry.header().mode()?;
        unpacker.member(&path, kind.is_dir(), size, mode, &mut entry)?;
    }
    let mut reader = archive.into_inner();
    let mut padding = [0; 8192];
    loop {
        let count = reader
            .read(&mut padding)
            .context("could not finish reading the compressed tar stream")?;
        if count == 0 {
            break;
        }
        ensure!(
            padding[..count].iter().all(|byte| *byte == 0),
            "module archive has data after the tar end marker"
        );
    }
    Ok(())
}

fn unpack_zip(file: &mut File, unpacker: &mut Unpacker<'_>) -> Result<()> {
    let mut archive = zip::ZipArchive::new(file).context("could not read the ZIP archive")?;
    ensure!(
        archive.len() <= MAX_ENTRIES,
        "module archive contains more than 10000 members"
    );
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .context("could not read a ZIP member")?;
        let path = std::str::from_utf8(entry.name_raw())
            .context("archive member paths must be UTF-8")?
            .to_owned();
        let directory = entry.is_dir();
        let mode = entry.unix_mode().unwrap_or(0);
        let kind = mode & 0o170000;
        ensure!(
            kind == 0 || kind == if directory { 0o040000 } else { 0o100000 },
            "archive member `{}` must be an ordinary file or directory",
            path.escape_debug()
        );
        ensure!(
            !entry.encrypted(),
            "encrypted module archives are not supported"
        );
        ensure!(
            matches!(
                entry.compression(),
                zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
            ),
            "ZIP module members must use stored or deflate compression"
        );
        let size = entry.size();
        unpacker.member(&path, directory, size, mode, &mut entry)?;
    }
    Ok(())
}

struct Unpacker<'a> {
    root: &'a Path,
    wrapper: Option<String>,
    paths: BTreeMap<String, PathBuf>,
    entries: usize,
    bytes: u64,
    cancellation: &'a BundleCancellation,
}

impl Unpacker<'_> {
    fn member(
        &mut self,
        name: &str,
        directory: bool,
        size: u64,
        mode: u32,
        reader: &mut impl Read,
    ) -> Result<()> {
        self.cancellation.ensure_active()?;
        self.entries += 1;
        ensure!(
            self.entries <= MAX_ENTRIES,
            "module archive contains more than 10000 members"
        );
        ensure!(
            name.len() <= 4096
                && !name.starts_with('/')
                && !name.contains('\\')
                && !name.chars().any(char::is_control),
            "archive member `{}` needs a portable relative path",
            name.escape_debug()
        );
        let parts = name
            .split('/')
            .filter(|part| !part.is_empty() && *part != ".")
            .collect::<Vec<_>>();
        ensure!(
            !parts.contains(&".."),
            "archive member `{}` must stay inside the module",
            name.escape_debug()
        );
        let Some(wrapper) = parts.first() else {
            ensure!(directory, "module archive needs a single outer directory");
            return Ok(());
        };
        for (index, part) in parts.iter().enumerate() {
            tola_address::validate_portable_segment(part, index == 0).with_context(|| {
                format!("archive member `{}` is not portable", name.escape_debug())
            })?;
        }
        if let Some(previous) = &self.wrapper {
            ensure!(
                previous == wrapper,
                "module archive must contain exactly one outer directory"
            );
        } else {
            self.wrapper = Some((*wrapper).to_owned());
        }
        if parts.len() == 1 {
            ensure!(
                directory,
                "module archive needs a single outer directory containing `tola-module.toml`"
            );
            return Ok(());
        }
        let mut relative = PathBuf::new();
        for (index, part) in parts[1..].iter().enumerate() {
            relative.push(part);
            self.record(&relative)?;
            if index < parts.len() - 2 || directory {
                let target = self.root.join(&relative);
                fs::create_dir_all(&target)?;
                tola_fs::set_unix_mode(&target, 0o755)?;
            }
        }
        let target = self.root.join(&relative);
        if !directory {
            ensure!(
                size <= MAX_EXPANDED - self.bytes,
                "module archive expands beyond 512 MiB"
            );
            let mut output = File::create_new(&target)?;
            self.bytes += copy_bounded(
                reader,
                &mut output,
                MAX_EXPANDED - self.bytes,
                self.cancellation,
            )
            .with_context(|| format!("could not extract `{}`", relative.display()))?;
            tola_fs::set_unix_mode(&target, if mode & 0o111 == 0 { 0o644 } else { 0o755 })?;
        }
        Ok(())
    }

    fn record(&mut self, path: &Path) -> Result<()> {
        let key = path
            .iter()
            .map(|part| {
                tola_address::portable_collision_key(part.to_str().expect("validated UTF-8"))
            })
            .collect::<Vec<_>>()
            .join("/");
        if let Some(previous) = self.paths.get(&key) {
            ensure!(
                previous == path,
                "archive member `{}` conflicts with another path",
                path.display()
            );
        } else {
            self.paths.insert(key, path.to_owned());
        }
        Ok(())
    }
}

fn copy_bounded(
    reader: &mut impl Read,
    writer: &mut impl Write,
    limit: u64,
    cancellation: &BundleCancellation,
) -> Result<u64> {
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        cancellation.ensure_active()?;
        let read = reader.read(&mut buffer);
        cancellation.ensure_active()?;
        let read = read?;
        if read == 0 {
            return Ok(total);
        }
        total += read as u64;
        ensure!(total <= limit, "module archive exceeds its byte limit");
        writer.write_all(&buffer[..read])?;
    }
}

struct BoundedReader<'a, R> {
    inner: R,
    remaining: u64,
    cancellation: &'a BundleCancellation,
}

struct ZstdFrames<'a> {
    file: &'a mut File,
    decoder: ruzstd::decoding::FrameDecoder,
    cancellation: &'a BundleCancellation,
}

impl<'a> ZstdFrames<'a> {
    fn new(file: &'a mut File, cancellation: &'a BundleCancellation) -> Self {
        let mut decoder = ruzstd::decoding::FrameDecoder::new();
        decoder.set_max_window_size(MAX_DOWNLOAD);
        Self {
            file,
            decoder,
            cancellation,
        }
    }
}

impl Read for ZstdFrames<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        use ruzstd::decoding::BlockDecodingStrategy;
        use ruzstd::decoding::errors::{FrameDecoderError, ReadFrameHeaderError};

        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            self.cancellation
                .ensure_active()
                .map_err(io::Error::other)?;
            if self.decoder.can_collect() != 0 {
                return self.decoder.read(buffer);
            }
            if self.decoder.is_finished() {
                if let Some(expected) = self.decoder.get_checksum_from_data()
                    && self.decoder.get_calculated_checksum() != Some(expected)
                {
                    return Err(io::Error::other("Zstandard frame checksum does not match"));
                }
                if self.file.stream_position()? == self.file.metadata()?.len() {
                    return Ok(0);
                }
                match self.decoder.init(&mut *self.file) {
                    Ok(()) => {}
                    Err(FrameDecoderError::ReadFrameHeaderError(
                        ReadFrameHeaderError::SkipFrame { length, .. },
                    )) => {
                        let end = self.file.stream_position()? + u64::from(length);
                        if end > self.file.metadata()?.len() {
                            return Err(io::Error::other("truncated Zstandard metadata frame"));
                        }
                        self.file.seek(SeekFrom::Start(end))?;
                    }
                    Err(error) => return Err(io::Error::other(error)),
                }
            } else {
                self.decoder
                    .decode_blocks(&mut *self.file, BlockDecodingStrategy::UptoBlocks(1))
                    .map_err(io::Error::other)?;
            }
        }
    }
}

impl<R: Read> Read for BoundedReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.cancellation
            .ensure_active()
            .map_err(io::Error::other)?;
        let read = self.inner.read(buffer)?;
        self.remaining = self
            .remaining
            .checked_sub(read as u64)
            .ok_or_else(|| io::Error::other("module archive expands beyond 512 MiB"))?;
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &[u8] = b"[typst]\nentry = 'lib.typ'\n";
    const LIBRARY: &[u8] = b"#let value = 1";

    fn tar_bytes(link: bool) -> Vec<u8> {
        let mut archive = tar::Builder::new(Vec::new());
        for (path, bytes) in [
            ("module/tola-module.toml", MANIFEST),
            ("module/lib.typ", LIBRARY),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            archive.append_data(&mut header, path, bytes).unwrap();
        }
        if link {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_mode(0o777);
            header.set_size(0);
            header.set_link_name("../../outside").unwrap();
            archive
                .append_data(&mut header, "module/link", io::empty())
                .unwrap();
        }
        archive.into_inner().unwrap()
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn zip_bytes(extra: &[&str], link: bool) -> Vec<u8> {
        let mut archive = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (path, bytes) in [
            ("module/tola-module.toml", MANIFEST),
            ("module/lib.typ", LIBRARY),
        ] {
            archive.start_file(path, options).unwrap();
            archive.write_all(bytes).unwrap();
        }
        for path in extra {
            archive.start_file(*path, options).unwrap();
            archive.write_all(b"extra").unwrap();
        }
        if link {
            archive
                .add_symlink("module/link", "../../outside", options)
                .unwrap();
        }
        archive.finish().unwrap().into_inner()
    }

    fn unpack(bytes: &[u8], root: &Path) -> Result<()> {
        let mut source = tempfile::tempfile().unwrap();
        source.write_all(bytes).unwrap();
        extract(&mut source, root, &BundleCancellation::default())
    }

    #[test]
    fn supported_formats_have_the_same_module_contents() {
        let tar = tar_bytes(false);
        let middle = tar.len() / 2;
        let mut zstd = ruzstd::encoding::compress_to_vec(
            &tar[..middle],
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        zstd.extend_from_slice(&[0x50, 0x2a, 0x4d, 0x18, 3, 0, 0, 0, 1, 2, 3]);
        zstd.extend(ruzstd::encoding::compress_to_vec(
            &tar[middle..],
            ruzstd::encoding::CompressionLevel::Fastest,
        ));
        let workspace = tempfile::tempdir().unwrap();
        let mut digests = Vec::new();
        for (index, bytes) in [gzip(&tar), zstd, zip_bytes(&[], false)]
            .into_iter()
            .enumerate()
        {
            let root = workspace.path().join(index.to_string());
            unpack(&bytes, &root).unwrap();
            crate::check::check(&root).unwrap();
            digests.push(tola_input::tree_sha256(&root).unwrap());
        }
        assert!(digests.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn untrusted_members_cannot_escape_or_create_links() {
        let workspace = tempfile::tempdir().unwrap();
        for (index, bytes) in [
            zip_bytes(&["module/../escaped"], false),
            zip_bytes(&[], true),
            gzip(&tar_bytes(true)),
        ]
        .into_iter()
        .enumerate()
        {
            let root = workspace.path().join(index.to_string());
            assert!(unpack(&bytes, &root).is_err());
            assert!(fs::symlink_metadata(root.join("link")).is_err());
            assert!(!workspace.path().join("escaped").exists());
        }
    }
}
