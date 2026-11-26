//! Embedded code highlighting themes carried by `@tola/code`.
//!
//! `licenses/README.md` beside this crate's manifest mirrors the upstream attribution rows.

use std::sync::LazyLock;

use typst::foundations::{Dict, IntoValue};
use typst::syntax::{RootedPath, VirtualPath, VirtualRoot};

use crate::builtin::TolaPackage;

/// Directory under `@tola/code` holding one file per theme.
pub const THEME_DIRECTORY: &str = "code-themes";

include!(concat!(env!("OUT_DIR"), "/theme_files.rs"));

static THEME_BYTES: LazyLock<Vec<u8>> = LazyLock::new(|| {
    let mut bytes = miniz_oxide::inflate::decompress_to_vec_zlib(include_bytes!(concat!(
        env!("OUT_DIR"),
        "/code-themes.z"
    )))
    .expect("bundled theme archive is valid zlib");
    bytes.shrink_to_fit();
    bytes
});

pub(crate) fn file_contents(file: &str) -> &'static str {
    let (_, offset, len) = THEME_FILES
        .iter()
        .find(|(path, _, _)| *path == file)
        .expect("catalog theme references a bundled file");
    std::str::from_utf8(&THEME_BYTES[*offset..*offset + *len]).expect("theme file is UTF-8")
}

const A11Y_SOURCE: ThemeSource = ThemeSource {
    project: "ericwbailey/a11y-syntax-highlighting",
    revision: "3200c11458e415933ca33ced7cdc3b4cf428c93b",
    file: "dist/highlight/a11y-light.css, dist/highlight/a11y-dark.css",
    license: "MIT",
};

pub fn themes() -> &'static [CodeTheme] {
    CODE_THEMES
}

/// Names of all offered themes in catalog order.
pub fn theme_names() -> impl Iterator<Item = &'static str> {
    themes().iter().map(|t| t.name)
}

/// Package-relative path of the theme `name`, if offered.
pub fn theme_file(name: &str) -> Option<&'static str> {
    themes().iter().find(|t| t.name == name).map(|t| t.file)
}

/// `code-themes` host dictionary mapping each theme name to its package-anchored path.
pub fn themes_dict() -> Dict {
    let pkg = TolaPackage::Code.spec();
    crate::dict(themes().iter().map(|t| {
        let path = RootedPath::new(
            VirtualRoot::Package(pkg.clone()),
            VirtualPath::new(t.file).expect("theme path is virtualizable"),
        );
        (t.name, path.into_value())
    }))
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
        source: &A11Y_SOURCE,
    },
    CodeTheme {
        name: "a11y-dark",
        appearance: ThemeAppearance::Dark,
        file: "code-themes/a11y-dark.tmTheme",
        source: &A11Y_SOURCE,
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
    use std::sync::Arc;

    use tola_typst::compile_world;
    use typst::foundations::{Module, Scope, Value};

    use super::*;
    use crate::protocol::HOST_MODULE;
    use crate::test_support::{file_map, test_world};

    #[test]
    fn themes_load_in_world() {
        let mut src = String::from("#import \"@tola/host:0.0.0\": code-themes\n");
        for t in themes() {
            src.push_str(&format!(
                "#raw(\"let x = 1\", lang: \"rust\", block: true, theme: code-themes.{})\n",
                t.name
            ));
        }

        let mut scope = Scope::new();
        scope.define("code-themes", themes_dict());
        let inputs = crate::dict([(HOST_MODULE, Value::Module(Module::new(HOST_MODULE, scope)))]);
        let world = test_world(
            "tola-code-themes",
            file_map(&[("main.typ", &src)]),
            Arc::new(tola_typst::create_library_with_inputs(inputs)),
        );

        let html = compile_world(&world)
            .expect("all offered themes load")
            .html()
            .expect("theme test exports to HTML");
        assert!(String::from_utf8_lossy(&html).contains("style=\"color:"));
    }
}
