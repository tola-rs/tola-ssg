//! Built-in Nu sources registered in the official parser's virtual filesystem.

use nu_protocol::engine::{StateWorkingSet, VirtualPath};
use std::path::Path;

pub(super) fn register(working: &mut StateWorkingSet, root: &Path, sources: &[(&str, &[u8])]) {
    for &(name, source) in sources {
        let name = root.join(name).to_string_lossy().into_owned();
        let file = working.add_file(&name, source);
        let _ = working.add_virtual_path(name, VirtualPath::File(file));
    }
}
