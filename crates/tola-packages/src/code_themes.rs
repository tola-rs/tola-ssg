//! The code themes `@tola/code` carries: the catalog, the embedded files, and the values a site
//! names.
//!
//! Every embedded file is one catalog row: the name a site writes (the `code-themes` key and the
//! file stem), the appearance its palette is designed for, and the upstream project, revision,
//! source file, and licence the data comes from. The licence audit record mirrored from these
//! rows is `licenses/README.md` beside this crate's manifest.

use std::sync::LazyLock;

use typst::foundations::{Dict, IntoValue};
use typst::syntax::{RootedPath, VirtualPath, VirtualRoot};

use crate::builtin::TolaPackage;

/// The `@tola/code` directory holding one file per theme.
pub const THEME_DIRECTORY: &str = "code-themes";

// The packed theme files: their package paths, offsets, and lengths.
include!(concat!(env!("OUT_DIR"), "/theme_files.rs"));

/// The carried theme files, compressed by the build and decoded by the first lookup.
static THEME_BYTES: LazyLock<Vec<u8>> = LazyLock::new(|| {
    let mut bytes = miniz_oxide::inflate::decompress_to_vec_zlib(include_bytes!(concat!(
        env!("OUT_DIR"),
        "/code-themes.z"
    )))
    .expect("the carried theme files are the stream the build wrote");
    // The bytes back every theme file for the process, so their growth slack is not kept.
    bytes.shrink_to_fit();
    bytes
});

/// The embedded bytes of the theme file at `file`.
pub(crate) fn file_contents(file: &str) -> &'static str {
    let (_, offset, length) = THEME_FILES
        .iter()
        .find(|(path, _, _)| *path == file)
        .expect("a catalog theme names a theme file the build carries");
    std::str::from_utf8(&THEME_BYTES[*offset..*offset + *length]).expect("a theme file is UTF-8")
}

/// Every code theme the package carries, in catalog order.
pub fn themes() -> &'static [CodeTheme] {
    CODE_THEMES
}

/// The name of every theme as a site writes it, in catalog order.
pub fn theme_names() -> impl Iterator<Item = &'static str> {
    themes().iter().map(|theme| theme.name)
}

/// The package-relative path of the theme named `name`, or `None` for a name the package does
/// not offer.
pub fn theme_file(name: &str) -> Option<&'static str> {
    themes()
        .iter()
        .find(|theme| theme.name == name)
        .map(|theme| theme.file)
}

/// The `code-themes` host value: every theme name mapped to the package-anchored path of the
/// file that colors it.
pub fn themes_dict() -> Dict {
    let mut dict = Dict::new();
    for theme in themes() {
        let path = RootedPath::new(
            VirtualRoot::Package(TolaPackage::Code.spec()),
            VirtualPath::new(theme.file).expect("a theme file path is virtualizable"),
        );
        dict.insert(theme.name.into(), path.into_value());
    }
    dict
}

/// One code theme `@tola/code` carries.
pub struct CodeTheme {
    /// The name a site writes: the `code-themes` key and the file stem.
    pub name: &'static str,
    /// The appearance the palette is designed for.
    pub appearance: ThemeAppearance,
    /// The package-relative path of the embedded file.
    pub file: &'static str,
    /// Where the embedded data comes from.
    pub source: &'static ThemeSource,
}

/// The appearance a theme's palette is designed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeAppearance {
    Light,
    Dark,
}

/// Where one embedded theme's data comes from, and under which licence it is redistributed.
pub struct ThemeSource {
    /// The upstream project as `owner/name`.
    pub project: &'static str,
    /// The full revision of the upstream file.
    pub revision: &'static str,
    /// The upstream file the data comes from; for a derived theme, the palettes it was converted
    /// from.
    pub file: &'static str,
    /// The SPDX identifier of the licence the data is redistributed under.
    pub license: &'static str,
}

/// Every carried theme, in catalog order.
static CODE_THEMES: &[CodeTheme] = &[
    CodeTheme {
        name: "github",
        appearance: ThemeAppearance::Light,
        file: "code-themes/github.tmTheme",
        source: &ThemeSource {
            project: "primer/github-textmate-theme",
            revision: "646400ef02d34cec70696dd03ec6c9c98d19fd6f",
            file: "GitHub Light.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "github-dark",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/github-dark.tmTheme",
        source: &ThemeSource {
            project: "primer/github-textmate-theme",
            revision: "646400ef02d34cec70696dd03ec6c9c98d19fd6f",
            file: "GitHub Dark.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "tokyo-night",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/tokyo-night.tmTheme",
        source: &ThemeSource {
            project: "folke/tokyonight.nvim",
            revision: "054790b8676d0c561b22320d4b5ab3ef175f7445",
            file: "extras/sublime/tokyonight_night.tmTheme",
            license: "Apache-2.0",
        },
    },
    CodeTheme {
        name: "tokyo-night-storm",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/tokyo-night-storm.tmTheme",
        source: &ThemeSource {
            project: "folke/tokyonight.nvim",
            revision: "054790b8676d0c561b22320d4b5ab3ef175f7445",
            file: "extras/sublime/tokyonight_storm.tmTheme",
            license: "Apache-2.0",
        },
    },
    CodeTheme {
        name: "tokyo-night-moon",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/tokyo-night-moon.tmTheme",
        source: &ThemeSource {
            project: "folke/tokyonight.nvim",
            revision: "054790b8676d0c561b22320d4b5ab3ef175f7445",
            file: "extras/sublime/tokyonight_moon.tmTheme",
            license: "Apache-2.0",
        },
    },
    CodeTheme {
        name: "tokyo-night-day",
        appearance: ThemeAppearance::Light,
        file: "code-themes/tokyo-night-day.tmTheme",
        source: &ThemeSource {
            project: "folke/tokyonight.nvim",
            revision: "5598215fa06572048bc857c9c71378a5433ec070",
            file: "extras/sublime/tokyonight_day.tmTheme",
            license: "Apache-2.0",
        },
    },
    CodeTheme {
        name: "atom-one",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/atom-one.tmTheme",
        source: &ThemeSource {
            project: "sonph/onehalf",
            revision: "d13f47b0c20923eb13335f7effc5e8ec4431a5cd",
            file: "sublimetext/OneHalfDark.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "atom-one-light",
        appearance: ThemeAppearance::Light,
        file: "code-themes/atom-one-light.tmTheme",
        source: &ThemeSource {
            project: "sonph/onehalf",
            revision: "d13f47b0c20923eb13335f7effc5e8ec4431a5cd",
            file: "sublimetext/OneHalfLight.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "a11y",
        appearance: ThemeAppearance::Light,
        file: "code-themes/a11y.tmTheme",
        source: &ThemeSource {
            project: "ericwbailey/a11y-syntax-highlighting",
            revision: "3200c11458e415933ca33ced7cdc3b4cf428c93b",
            file: "dist/highlight/a11y-light.css, dist/highlight/a11y-dark.css",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "a11y-dark",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/a11y-dark.tmTheme",
        source: &ThemeSource {
            project: "ericwbailey/a11y-syntax-highlighting",
            revision: "3200c11458e415933ca33ced7cdc3b4cf428c93b",
            file: "dist/highlight/a11y-light.css, dist/highlight/a11y-dark.css",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "dracula",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/dracula.tmTheme",
        source: &ThemeSource {
            project: "dracula/sublime",
            revision: "811a634420a0cb9023bb7afd1c8968a5aaf6a753",
            file: "Dracula.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "catppuccin-frappe",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/catppuccin-frappe.tmTheme",
        source: &ThemeSource {
            project: "catppuccin/bat",
            revision: "6810349b28055dce54076712fc05fc68da4b8ec0",
            file: "themes/Catppuccin Frappe.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "catppuccin-latte",
        appearance: ThemeAppearance::Light,
        file: "code-themes/catppuccin-latte.tmTheme",
        source: &ThemeSource {
            project: "catppuccin/bat",
            revision: "6810349b28055dce54076712fc05fc68da4b8ec0",
            file: "themes/Catppuccin Latte.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "catppuccin-macchiato",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/catppuccin-macchiato.tmTheme",
        source: &ThemeSource {
            project: "catppuccin/bat",
            revision: "6810349b28055dce54076712fc05fc68da4b8ec0",
            file: "themes/Catppuccin Macchiato.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "catppuccin-mocha",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/catppuccin-mocha.tmTheme",
        source: &ThemeSource {
            project: "catppuccin/bat",
            revision: "6810349b28055dce54076712fc05fc68da4b8ec0",
            file: "themes/Catppuccin Mocha.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "gruvbox-dark",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/gruvbox-dark.tmTheme",
        source: &ThemeSource {
            project: "subnut/gruvbox-tmTheme",
            revision: "40503472826e51d87666e548a0634c4f1d74938c",
            file: "gruvbox-dark.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "gruvbox-light",
        appearance: ThemeAppearance::Light,
        file: "code-themes/gruvbox-light.tmTheme",
        source: &ThemeSource {
            project: "subnut/gruvbox-tmTheme",
            revision: "40503472826e51d87666e548a0634c4f1d74938c",
            file: "gruvbox-light.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "nord",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/nord.tmTheme",
        source: &ThemeSource {
            project: "crabique/Nord-plist",
            revision: "9d0bcc137cb4a25b0232c1010ce317114dc2d6c2",
            file: "Nord.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "solarized-dark",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/solarized-dark.tmTheme",
        source: &ThemeSource {
            project: "altercation/solarized",
            revision: "fc0d4264a4a96fc01349879b2b1d524aa5c7ceea",
            file: "textmate-colors-solarized/Solarized (dark).tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "solarized-light",
        appearance: ThemeAppearance::Light,
        file: "code-themes/solarized-light.tmTheme",
        source: &ThemeSource {
            project: "altercation/solarized",
            revision: "fc0d4264a4a96fc01349879b2b1d524aa5c7ceea",
            file: "textmate-colors-solarized/Solarized (light).tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "monokai",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/monokai.tmTheme",
        source: &ThemeSource {
            project: "jonschlinkert/sublime-monokai-extended",
            revision: "1d472c1765c7c4290bb171abe3f961306c63bd12",
            file: "Monokai Extended.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "monokai-bright",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/monokai-bright.tmTheme",
        source: &ThemeSource {
            project: "jonschlinkert/sublime-monokai-extended",
            revision: "1d472c1765c7c4290bb171abe3f961306c63bd12",
            file: "Monokai Extended Bright.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "monokai-light",
        appearance: ThemeAppearance::Light,
        file: "code-themes/monokai-light.tmTheme",
        source: &ThemeSource {
            project: "jonschlinkert/sublime-monokai-extended",
            revision: "a0f477b0e37951822d44915f9cecfc5b2017dd4d",
            file: "Monokai Extended Light.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "monokai-origin",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/monokai-origin.tmTheme",
        source: &ThemeSource {
            project: "jonschlinkert/sublime-monokai-extended",
            revision: "1d472c1765c7c4290bb171abe3f961306c63bd12",
            file: "Monokai Extended Origin.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "leet",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/leet.tmTheme",
        source: &ThemeSource {
            project: "MarkMichos/1337-Scheme",
            revision: "63d2d1edb1d06dc3b2f456f4205fbbabe62703a7",
            file: "1337.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "coldark-cold",
        appearance: ThemeAppearance::Light,
        file: "code-themes/coldark-cold.tmTheme",
        source: &ThemeSource {
            project: "ArmandPhilippot/coldark-bat",
            revision: "93ee1f3fb5e08ecf66baee03dd3900c0abcdc1e9",
            file: "Coldark-Cold.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "coldark-dark",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/coldark-dark.tmTheme",
        source: &ThemeSource {
            project: "ArmandPhilippot/coldark-bat",
            revision: "93ee1f3fb5e08ecf66baee03dd3900c0abcdc1e9",
            file: "Coldark-Dark.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "two-dark",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/two-dark.tmTheme",
        source: &ThemeSource {
            project: "erremauro/TwoDark",
            revision: "e9e0381be882a4d2bbc06e4dd3b243d5e637754c",
            file: "TwoDark.tmTheme",
            license: "MIT",
        },
    },
    CodeTheme {
        name: "zenburn",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/zenburn.tmTheme",
        source: &ThemeSource {
            project: "colinta/zenburn",
            revision: "9abc72859dc10618341b33da2e34a7c01071caae",
            file: "zenburn.tmTheme",
            license: "BSD-2-Clause",
        },
    },
    CodeTheme {
        name: "snazzy",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/snazzy.tmTheme",
        source: &ThemeSource {
            project: "greggb/sublime-snazzy",
            revision: "5c88c8dbbb907184a75557e3c9e30f94916bce66",
            file: "Sublime Snazzy.tmTheme",
            license: "MIT",
        },
    },
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;
    use std::sync::Arc;

    use tola_typst::{
        BundleCancellation, FileMap, FileProvider, FileResolver, FileTarget, TypstWorld,
        compile_world, file_id,
    };
    use typst::foundations::{Module, Scope, Value};
    use typst::syntax::FileId;

    use super::*;
    use crate::builtin::TOLA_NAMESPACE;
    use crate::protocol::HOST_MODULE;

    /// Whether `name` is lowercase words joined by single hyphens: the spelling a `code-themes`
    /// key needs to stay reachable as a field.
    fn hyphenated(name: &str) -> bool {
        let mut words = name.split('-');
        let Some(first) = words.next() else {
            return false;
        };
        let word = |word: &str| {
            !word.is_empty()
                && word
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        };
        word(first) && words.all(word)
    }

    fn appearance_name(appearance: ThemeAppearance) -> &'static str {
        match appearance {
            ThemeAppearance::Light => "light",
            ThemeAppearance::Dark => "dark",
        }
    }

    #[test]
    fn theme_file_paths_derive_from_names() {
        let mut files = BTreeSet::new();
        for theme in themes() {
            assert!(hyphenated(theme.name), "`{}` is not hyphenated", theme.name);
            assert_eq!(
                theme.file,
                format!("{THEME_DIRECTORY}/{}.tmTheme", theme.name)
            );
            assert!(files.insert(theme.file), "{}", theme.file);
        }
        assert!(!themes().is_empty(), "the package offers a code theme");
    }

    #[test]
    fn package_carries_every_theme_file() {
        for theme in themes() {
            let carried = TolaPackage::Code
                .file(Path::new(theme.file))
                .unwrap_or_else(|| panic!("@tola/code carries `{}`", theme.file));
            assert_eq!(
                carried.as_ref(),
                file_contents(theme.file),
                "{}",
                theme.file
            );
        }
    }

    #[test]
    fn license_table_records_every_theme() {
        let table = include_str!("../licenses/README.md");
        for theme in themes() {
            let row = format!(
                "| `{}` | {} | `{}` | {} | `{}` | `{}` | {} |",
                theme.name,
                appearance_name(theme.appearance),
                theme.file,
                theme.source.project,
                theme.source.file,
                theme.source.revision,
                theme.source.license,
            );
            assert!(table.contains(&row), "the licence table lacks:\n{row}");
        }
    }

    /// One site's sources, then the builtin packages serving their own files.
    struct SiteFiles(FileMap);

    impl FileProvider for SiteFiles {
        fn target(&self, id: FileId) -> Option<FileTarget> {
            if let Some(target) = self.0.target(id) {
                return Some(target);
            }
            let VirtualRoot::Package(spec) = id.root() else {
                return None;
            };
            let path = Path::new(id.vpath().get_with_slash().trim_start_matches('/'));
            TolaPackage::from_spec(spec)
                .and_then(|package| package.file(path))
                .map(|bytes| FileTarget::Bytes(Arc::from(bytes.into_owned().into_bytes())))
        }

        fn owned_namespaces(&self) -> &'static [&'static str] {
            &[TOLA_NAMESPACE]
        }
    }

    #[test]
    fn offered_themes_load_from_package_paths() {
        let mut source = String::from("#import \"@tola/host:0.0.0\": code-themes\n");
        for theme in themes() {
            source.push_str(&format!(
                "#raw(\"let x = 1\", lang: \"rust\", block: true, theme: code-themes.{})\n",
                theme.name
            ));
        }

        let mut sources = FileMap::new();
        sources.insert(file_id("main.typ"), source.into_bytes());

        let mut scope = Scope::new();
        scope.define("code-themes", themes_dict());
        let mut inputs = Dict::new();
        inputs.insert(
            HOST_MODULE.into(),
            Value::Module(Module::new(HOST_MODULE, scope)),
        );

        let root = std::env::temp_dir().join("tola-code-themes");
        let main = root.join("main.typ");
        let world = TypstWorld::builder(&main, &root)
            .with_files(Arc::new(
                FileResolver::new().with_provider(SiteFiles(sources)),
            ))
            .with_local_cache()
            .no_fonts()
            .with_shared_library(Arc::new(tola_typst::create_library_with_inputs(inputs)))
            .build(&BundleCancellation::default())
            .expect("a valid theme check world");

        let html = compile_world(&world)
            .expect("every offered theme loads from its package path")
            .html()
            .expect("the check document exports to HTML");
        assert!(
            String::from_utf8_lossy(&html).contains("style=\"color:"),
            "the loaded themes color the rendered code"
        );
    }
}
