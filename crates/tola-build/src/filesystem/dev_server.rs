//! The address a running development server serves a site at.

use std::io;
use std::path::{Path, PathBuf};

use super::DEV_SERVER_ADDRESS_VERSION;
use tola_input::layout;

/// The address one `tola dev` wrote for a site, withdrawn when that server stops.
///
/// An editor's language server runs in a process of its own, so the address a page answers at is
/// stated once here rather than configured in the editor as well. Dropping withdraws this server's
/// address; one a later server wrote in its place is left standing.
#[derive(Debug)]
pub struct DevServerAddress {
    path: PathBuf,
    address: String,
}

impl DevServerAddress {
    /// Write `address` as the address this site is served at.
    pub fn write(root: &Path, address: &str) -> io::Result<Self> {
        let path = layout::dev_server_address(root);
        let state = serde_json::json!({
            "version": DEV_SERVER_ADDRESS_VERSION,
            "origin": address,
        });
        super::atomic_write(&path, state.to_string().as_bytes())?;
        Ok(Self {
            path,
            address: address.to_owned(),
        })
    }

    /// The address a running development server wrote, or `None` when none is serving.
    pub fn read(root: &Path) -> Option<String> {
        Self::recorded(&layout::dev_server_address(root))
    }

    fn recorded(path: &Path) -> Option<String> {
        let state: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        if state.get("version")?.as_u64()? != u64::from(DEV_SERVER_ADDRESS_VERSION) {
            return None;
        }
        state.get("origin")?.as_str().map(str::to_owned)
    }
}

impl Drop for DevServerAddress {
    fn drop(&mut self) {
        if Self::recorded(&self.path).as_deref() == Some(self.address.as_str()) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_writer_survives_earlier_drop() {
        let directory = tempfile::tempdir().unwrap();
        let earlier = DevServerAddress::write(directory.path(), "http://127.0.0.1:5277").unwrap();
        let later = DevServerAddress::write(directory.path(), "http://127.0.0.1:5278").unwrap();
        drop(earlier);
        assert_eq!(
            DevServerAddress::read(directory.path()).as_deref(),
            Some("http://127.0.0.1:5278")
        );
        drop(later);
        assert_eq!(DevServerAddress::read(directory.path()), None);
    }

    /// A record another version wrote, or one this version cannot read, names no server.
    #[test]
    fn no_address_from_foreign_record() {
        for contents in [
            r#"{"version":0,"origin":"http://127.0.0.1:5277"}"#,
            "not json",
        ] {
            let directory = tempfile::tempdir().unwrap();
            write_recorded(directory.path(), contents);
            assert_eq!(DevServerAddress::read(directory.path()), None, "{contents}");
        }
    }

    /// Leave the file a writer other than this one produced, which is how another version reads.
    fn write_recorded(root: &Path, text: &str) {
        let path = layout::dev_server_address(root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}
