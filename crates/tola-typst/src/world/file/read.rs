//! Basic file identifiers and disk reads.

use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use typst::diag::{FileError, FileResult};
use typst::syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};

/// Virtual `FileId` for stdin input.
pub static STDIN_ID: LazyLock<FileId> = LazyLock::new(|| virtual_file_id("<stdin>"));

/// Virtual `FileId` for empty/no input.
pub static EMPTY_ID: LazyLock<FileId> = LazyLock::new(|| virtual_file_id("<empty>"));

fn root_file_id(vpath: VirtualPath) -> FileId {
    FileId::new(RootedPath::new(VirtualRoot::Project, vpath))
}

/// Create a `FileId` for a file below the compilation root.
pub fn file_id(path: impl AsRef<Path>) -> FileId {
    let path = path.as_ref().to_string_lossy();
    root_file_id(VirtualPath::new(&path).expect("root path must be virtualizable"))
}

/// Create a `FileId` from an absolute path within the compilation root.
pub fn file_id_from_path(file_path: &Path, root: &Path) -> Option<FileId> {
    VirtualPath::virtualize(root, file_path)
        .ok()
        .map(root_file_id)
}

/// Create a unique `FileId` for dynamically generated content.
pub fn virtual_file_id(name: &str) -> FileId {
    let vpath = VirtualPath::new(name).expect("virtual file name must be valid");
    FileId::unique(RootedPath::new(VirtualRoot::Project, vpath))
}

pub(super) fn non_persistent_label(id: FileId) -> Option<String> {
    // Unique IDs differ from interned IDs even when their rooted paths match.
    (FileId::new(id.get().clone()) != id).then(|| id.vpath().get_without_slash().to_owned())
}

/// Decode bytes as UTF-8, stripping BOM if present.
pub fn decode_utf8(buf: &[u8]) -> FileResult<&str> {
    let buf = buf.strip_prefix(b"\xef\xbb\xbf").unwrap_or(buf);
    std::str::from_utf8(buf).map_err(|_| FileError::InvalidUtf8)
}

/// Read file from disk.
///
/// A failed read is named by `reported`, the path the caller wrote, so no
/// message this error reaches can contain a realized host path.
pub(crate) fn read_disk(path: &Path, reported: &Path) -> FileResult<Vec<u8>> {
    let map_err = |e| FileError::from_io(e, reported);
    fs::metadata(path).map_err(map_err).and_then(|m| {
        if m.is_dir() {
            Err(FileError::IsDirectory)
        } else {
            fs::read(path).map_err(map_err)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn decode_utf8_strips_bom_or_rejects() {
        for (name, bytes, expected) in [
            (
                "plain text",
                &b"Hello, \xe4\xb8\x96\xe7\x95\x8c!"[..],
                Some("Hello, 世界!"),
            ),
            ("utf-8 bom", &b"\xef\xbb\xbfHello"[..], Some("Hello")),
            ("invalid utf-8", &[0xff, 0xfe][..], None),
        ] {
            let decoded = decode_utf8(bytes);
            match expected {
                Some(text) => assert_eq!(decoded.unwrap(), text, "{name}"),
                None => assert_eq!(decoded.unwrap_err(), FileError::InvalidUtf8, "{name}"),
            }
        }
    }

    #[test]
    fn read_disk_names_authored_path() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "test content").unwrap();

        assert_eq!(read_disk(&file, &file).unwrap(), b"test content");
        assert_eq!(
            read_disk(dir.path(), dir.path()).unwrap_err(),
            FileError::IsDirectory
        );

        let reported = Path::new("content/missing.typ");
        assert_eq!(
            read_disk(&dir.path().join("missing.typ"), reported).unwrap_err(),
            FileError::NotFound(reported.to_path_buf())
        );
    }

    #[test]
    fn unique_ids_are_non_persistent() {
        let unique = virtual_file_id("generated.typ");
        let normal = file_id("generated.typ");

        assert_eq!(
            non_persistent_label(unique).as_deref(),
            Some("generated.typ")
        );
        assert_eq!(non_persistent_label(normal), None);
    }
}
