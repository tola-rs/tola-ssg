use std::ffi::OsStr;
use std::path::Path;

#[rustfmt::skip]
const EXCLUDED_DIRS: &[&str] = &[
    ".git", ".hg", ".jj", ".next", ".parcel-cache", ".pnpm-store", ".svelte-kit", ".svn",
    ".turbo", ".venv", ".vercel", ".yarn", "__pycache__", "node_modules", "venv",
];

#[rustfmt::skip]
const EXCLUDED_FILES: &[&str] = &[
    ".gitignore", ".env", "package-lock.json", "pnpm-lock.yaml", "bun.lockb",
];

const EXCLUDED_FILE_PREFIXES: &[&str] = &[".env."];

#[rustfmt::skip]
const IGNORED_EXTENSIONS: &[&str] = &["less", "lock", "sass", "scss", "styl", "log"];

#[rustfmt::skip]
const CSS_EXTENSIONS: &[&str] = &["css"];

#[rustfmt::skip]
const BINARY_EXTENSIONS: &[&str] = &[
    "3dm", "3ds", "3g2", "3gp", "7z", "a", "aac", "adp", "ai", "aif", "aiff", "alz", "ape",
    "apk", "appimage", "ar", "arj", "asf", "au", "avi", "avif", "bak", "baml", "bh", "bin",
    "bk", "bmp", "btif", "bz2", "bzip2", "cab", "caf", "cgm", "class", "cmx", "cpio", "cr2",
    "cur", "dat", "db", "dcm", "deb", "dex", "djvu", "dll", "dmg", "dng", "doc", "docm", "docx",
    "dot", "dotm", "dra", "DS_Store", "dsk", "dts", "dtshd", "dvb", "dwg", "dxf", "ecelp4800",
    "ecelp7470", "ecelp9600", "egg", "eol", "eot", "epub", "exe", "exr", "f4v", "fbs", "fh",
    "fla", "flac", "flatpak", "fli", "flv", "fpx", "fst", "fvt", "g3", "geojson", "gh", "gif",
    "graffle", "gz", "gzip", "h261", "h263", "h264", "hdr", "icns", "ico", "ief", "img", "ipa",
    "iso", "jar", "jpeg", "jpg", "jpgv", "jpm", "jxr", "key", "ktx", "lha", "lib", "lockb",
    "lvp", "lz", "lzh", "lzma", "lzo", "m3u", "m4a", "m4v", "mar", "mdi", "mht", "mid", "midi",
    "mj2", "mka", "mkv", "mmr", "mng", "mobi", "mov", "movie", "mp3", "mp4", "mp4a", "mpeg",
    "mpg", "mpga", "mxu", "nef", "node", "npx", "numbers", "nupkg", "o", "oa", "odp", "ods",
    "odt", "oga", "ogg", "ogv", "otf", "ott", "pages", "pbm", "pcx", "pdb", "pdf", "pea",
    "pgm", "pic", "plist", "png", "pnm", "pot", "potm", "potx", "ppa", "ppam", "ppm", "pps",
    "ppsm", "ppsx", "ppt", "pptm", "pptx", "psd", "pya", "pyc", "pyo", "pyv", "qt", "rar",
    "ras", "raw", "resources", "rgb", "rip", "rlc", "rmf", "rmvb", "rpm", "rtf", "rz", "s3m",
    "s7z", "scpt", "sgi", "shar", "sil", "sketch", "slk", "smv", "snap", "snk", "so", "sqlite",
    "sqlite3", "stl", "storedata", "sub", "suo", "swf", "symbolsarchive", "tar", "tbz", "tbz2",
    "tga", "tgz", "thmx", "tif", "tiff", "tlz", "ttc", "ttf", "txz", "udf", "uvh", "uvi", "uvm",
    "uvp", "uvs", "uvu", "viv", "vob", "war", "wasm", "wav", "wax", "wbmp", "wdp", "weba",
    "webm", "webp", "whl", "wim", "wm", "wma", "wmv", "wmx", "woff", "woff2", "wrm", "wvx",
    "xbm", "xif", "xla", "xlam", "xls", "xlsb", "xlsm", "xlsx", "xlt", "xltm", "xltx", "xm",
    "xmind", "xpi", "xpm", "xwd", "xz", "z", "zip", "zipx",
];

pub(super) fn excluded_dir(path: &Path) -> bool {
    path.components().any(|component| {
        let value = component.as_os_str().to_string_lossy();
        EXCLUDED_DIRS.iter().any(|dir| value.as_ref() == *dir)
    })
}

pub(super) fn ignored_file(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| {
            EXCLUDED_FILES.iter().any(|excluded| name == *excluded)
                || EXCLUDED_FILE_PREFIXES
                    .iter()
                    .any(|excluded| name.starts_with(excluded))
        })
}

pub(super) fn ignored_extension(path: &Path) -> bool {
    extension_matches(path, IGNORED_EXTENSIONS)
}

pub(super) fn css_extension(path: &Path) -> bool {
    extension_matches(path, CSS_EXTENSIONS)
}

pub(super) fn binary_extension(path: &Path) -> bool {
    extension_matches(path, BINARY_EXTENSIONS)
}

fn extension_matches(path: &Path, values: &[&str]) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|ext| values.iter().any(|value| ext.eq_ignore_ascii_case(value)))
}
