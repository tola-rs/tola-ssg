//! Built-in Nu sources registered in the official parser's virtual filesystem.

use nu_protocol::engine::{StateWorkingSet, VirtualPath};

pub(super) const FILES: &[(&str, &[u8])] = &[
    ("tola/tola/tola.nu", include_bytes!("../nu/tola.nu")),
    ("tola/cli/cli.nu", include_bytes!("../nu/cli.nu")),
    ("tola/image/image.nu", include_bytes!("../nu/image.nu")),
    (
        "tola/image/image_core.nu",
        include_bytes!("../nu/image_core.nu"),
    ),
    (
        "tola/diagnostic/diagnostic.nu",
        include_bytes!("../nu/diagnostic.nu"),
    ),
    ("tola/ui/ui.nu", include_bytes!("../nu/ui.nu")),
];

pub(super) fn register(working: &mut StateWorkingSet) {
    for &(name, source) in FILES {
        let file = working.add_file(name, source);
        let _ = working.add_virtual_path(name.into(), VirtualPath::File(file));
    }
}
