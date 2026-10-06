# Code theme sources

The code themes `@tola/code` carries, each with the upstream project, source file, and exact
revision its embedded bytes come from. The catalog is [`src/code_themes.rs`](../src/code_themes.rs);
every row below is one catalog theme, and `license_table_records_every_theme` refuses a table
that stops matching the catalog. `offered_themes_load_from_package_paths` compiles every
embedded file through Typst's `raw(theme:)`, so a file that stops parsing fails the build.

| Theme | Appearance | File | Project | Upstream file | Revision | Licence |
| --- | --- | --- | --- | --- | --- | --- |
| `github` | light | `code-themes/github.tmTheme` | primer/github-textmate-theme | `GitHub Light.tmTheme` | `646400ef02d34cec70696dd03ec6c9c98d19fd6f` | MIT |
| `github-dark` | dark | `code-themes/github-dark.tmTheme` | primer/github-textmate-theme | `GitHub Dark.tmTheme` | `646400ef02d34cec70696dd03ec6c9c98d19fd6f` | MIT |
| `tokyo-night` | dark | `code-themes/tokyo-night.tmTheme` | folke/tokyonight.nvim | `extras/sublime/tokyonight_night.tmTheme` | `054790b8676d0c561b22320d4b5ab3ef175f7445` | Apache-2.0 |
| `tokyo-night-storm` | dark | `code-themes/tokyo-night-storm.tmTheme` | folke/tokyonight.nvim | `extras/sublime/tokyonight_storm.tmTheme` | `054790b8676d0c561b22320d4b5ab3ef175f7445` | Apache-2.0 |
| `tokyo-night-moon` | dark | `code-themes/tokyo-night-moon.tmTheme` | folke/tokyonight.nvim | `extras/sublime/tokyonight_moon.tmTheme` | `054790b8676d0c561b22320d4b5ab3ef175f7445` | Apache-2.0 |
| `tokyo-night-day` | light | `code-themes/tokyo-night-day.tmTheme` | folke/tokyonight.nvim | `extras/sublime/tokyonight_day.tmTheme` | `5598215fa06572048bc857c9c71378a5433ec070` | Apache-2.0 |
| `atom-one` | dark | `code-themes/atom-one.tmTheme` | sonph/onehalf | `sublimetext/OneHalfDark.tmTheme` | `d13f47b0c20923eb13335f7effc5e8ec4431a5cd` | MIT |
| `atom-one-light` | light | `code-themes/atom-one-light.tmTheme` | sonph/onehalf | `sublimetext/OneHalfLight.tmTheme` | `d13f47b0c20923eb13335f7effc5e8ec4431a5cd` | MIT |
| `a11y` | light | `code-themes/a11y.tmTheme` | ericwbailey/a11y-syntax-highlighting | `dist/highlight/a11y-light.css, dist/highlight/a11y-dark.css` | `3200c11458e415933ca33ced7cdc3b4cf428c93b` | MIT |
| `a11y-dark` | dark | `code-themes/a11y-dark.tmTheme` | ericwbailey/a11y-syntax-highlighting | `dist/highlight/a11y-light.css, dist/highlight/a11y-dark.css` | `3200c11458e415933ca33ced7cdc3b4cf428c93b` | MIT |
| `dracula` | dark | `code-themes/dracula.tmTheme` | dracula/sublime | `Dracula.tmTheme` | `811a634420a0cb9023bb7afd1c8968a5aaf6a753` | MIT |
| `catppuccin-frappe` | dark | `code-themes/catppuccin-frappe.tmTheme` | catppuccin/bat | `themes/Catppuccin Frappe.tmTheme` | `6810349b28055dce54076712fc05fc68da4b8ec0` | MIT |
| `catppuccin-latte` | light | `code-themes/catppuccin-latte.tmTheme` | catppuccin/bat | `themes/Catppuccin Latte.tmTheme` | `6810349b28055dce54076712fc05fc68da4b8ec0` | MIT |
| `catppuccin-macchiato` | dark | `code-themes/catppuccin-macchiato.tmTheme` | catppuccin/bat | `themes/Catppuccin Macchiato.tmTheme` | `6810349b28055dce54076712fc05fc68da4b8ec0` | MIT |
| `catppuccin-mocha` | dark | `code-themes/catppuccin-mocha.tmTheme` | catppuccin/bat | `themes/Catppuccin Mocha.tmTheme` | `6810349b28055dce54076712fc05fc68da4b8ec0` | MIT |
| `gruvbox-dark` | dark | `code-themes/gruvbox-dark.tmTheme` | subnut/gruvbox-tmTheme | `gruvbox-dark.tmTheme` | `40503472826e51d87666e548a0634c4f1d74938c` | MIT |
| `gruvbox-light` | light | `code-themes/gruvbox-light.tmTheme` | subnut/gruvbox-tmTheme | `gruvbox-light.tmTheme` | `40503472826e51d87666e548a0634c4f1d74938c` | MIT |
| `nord` | dark | `code-themes/nord.tmTheme` | crabique/Nord-plist | `Nord.tmTheme` | `9d0bcc137cb4a25b0232c1010ce317114dc2d6c2` | MIT |
| `solarized-dark` | dark | `code-themes/solarized-dark.tmTheme` | altercation/solarized | `textmate-colors-solarized/Solarized (dark).tmTheme` | `fc0d4264a4a96fc01349879b2b1d524aa5c7ceea` | MIT |
| `solarized-light` | light | `code-themes/solarized-light.tmTheme` | altercation/solarized | `textmate-colors-solarized/Solarized (light).tmTheme` | `fc0d4264a4a96fc01349879b2b1d524aa5c7ceea` | MIT |
| `monokai` | dark | `code-themes/monokai.tmTheme` | jonschlinkert/sublime-monokai-extended | `Monokai Extended.tmTheme` | `1d472c1765c7c4290bb171abe3f961306c63bd12` | MIT |
| `monokai-bright` | dark | `code-themes/monokai-bright.tmTheme` | jonschlinkert/sublime-monokai-extended | `Monokai Extended Bright.tmTheme` | `1d472c1765c7c4290bb171abe3f961306c63bd12` | MIT |
| `monokai-light` | light | `code-themes/monokai-light.tmTheme` | jonschlinkert/sublime-monokai-extended | `Monokai Extended Light.tmTheme` | `a0f477b0e37951822d44915f9cecfc5b2017dd4d` | MIT |
| `monokai-origin` | dark | `code-themes/monokai-origin.tmTheme` | jonschlinkert/sublime-monokai-extended | `Monokai Extended Origin.tmTheme` | `1d472c1765c7c4290bb171abe3f961306c63bd12` | MIT |
| `leet` | dark | `code-themes/leet.tmTheme` | MarkMichos/1337-Scheme | `1337.tmTheme` | `63d2d1edb1d06dc3b2f456f4205fbbabe62703a7` | MIT |
| `coldark-cold` | light | `code-themes/coldark-cold.tmTheme` | ArmandPhilippot/coldark-bat | `Coldark-Cold.tmTheme` | `93ee1f3fb5e08ecf66baee03dd3900c0abcdc1e9` | MIT |
| `coldark-dark` | dark | `code-themes/coldark-dark.tmTheme` | ArmandPhilippot/coldark-bat | `Coldark-Dark.tmTheme` | `93ee1f3fb5e08ecf66baee03dd3900c0abcdc1e9` | MIT |
| `two-dark` | dark | `code-themes/two-dark.tmTheme` | erremauro/TwoDark | `TwoDark.tmTheme` | `e9e0381be882a4d2bbc06e4dd3b243d5e637754c` | MIT |
| `zenburn` | dark | `code-themes/zenburn.tmTheme` | colinta/zenburn | `zenburn.tmTheme` | `9abc72859dc10618341b33da2e34a7c01071caae` | BSD-2-Clause |
| `snazzy` | dark | `code-themes/snazzy.tmTheme` | greggb/sublime-snazzy | `Sublime Snazzy.tmTheme` | `5c88c8dbbb907184a75557e3c9e30f94916bce66` | MIT |

## Licence snapshots

Verbatim copies of the licence each project distributes, saved at the revision above.

| Project | Licence file | Upstream |
| --- | --- | --- |
| primer/github-textmate-theme | [LICENSE-primer-github-textmate-theme](LICENSE-primer-github-textmate-theme) | <https://github.com/primer/github-textmate-theme/blob/master/LICENSE> |
| folke/tokyonight.nvim | [LICENSE-tokyonight-nvim](LICENSE-tokyonight-nvim) | <https://github.com/folke/tokyonight.nvim/blob/main/LICENSE> |
| sonph/onehalf | [LICENSE-onehalf](LICENSE-onehalf) | <https://github.com/sonph/onehalf/blob/master/LICENSE.txt> |
| ericwbailey/a11y-syntax-highlighting | [LICENSE-a11y-syntax-highlighting](LICENSE-a11y-syntax-highlighting) | <https://github.com/ericwbailey/a11y-syntax-highlighting/blob/main/LICENSE> |
| dracula/sublime | [LICENSE-dracula-sublime](LICENSE-dracula-sublime) | <https://github.com/dracula/sublime/blob/master/LICENSE> |
| catppuccin/bat | [LICENSE-catppuccin-bat](LICENSE-catppuccin-bat) | <https://github.com/catppuccin/bat/blob/main/LICENSE> |
| subnut/gruvbox-tmTheme | [LICENSE-gruvbox-tmtheme](LICENSE-gruvbox-tmtheme) | <https://github.com/subnut/gruvbox-tmTheme/blob/bat-source/LICENSE> |
| crabique/Nord-plist | [LICENSE-nord-plist](LICENSE-nord-plist) | <https://github.com/crabique/Nord-plist/blob/master/LICENSE> |
| altercation/solarized | [LICENSE-solarized](LICENSE-solarized) | <https://github.com/altercation/solarized/blob/master/LICENSE> |
| jonschlinkert/sublime-monokai-extended | [LICENSE-sublime-monokai-extended](LICENSE-sublime-monokai-extended) | <https://github.com/jonschlinkert/sublime-monokai-extended/blob/master/LICENSE> |
| MarkMichos/1337-Scheme | [LICENSE-1337-scheme](LICENSE-1337-scheme) | <https://github.com/MarkMichos/1337-Scheme/blob/master/LICENSE> |
| ArmandPhilippot/coldark-bat | [LICENSE-coldark-bat](LICENSE-coldark-bat) | <https://github.com/ArmandPhilippot/coldark-bat/blob/master/LICENSE> |
| erremauro/TwoDark | [LICENSE-twodark](LICENSE-twodark) | <https://github.com/erremauro/TwoDark/blob/master/LICENSE> |
| colinta/zenburn | [LICENSE-zenburn](LICENSE-zenburn) | <https://github.com/colinta/zenburn/blob/main/LICENSE> |
| greggb/sublime-snazzy | [LICENSE-sublime-snazzy](LICENSE-sublime-snazzy) | <https://github.com/greggb/sublime-snazzy/blob/master/LICENSE> |

## Notes

- `a11y` and `a11y-dark` are Tola's own: the palette values are the custom properties of
  the two highlight CSS files at the pinned revision, converted to sRGB hex, and the scope
  mapping is Tola's. The upstream project publishes no tmTheme.
- `atom-one` carries the upstream `OneHalfDark.tmTheme`, whose internal `<name>` says
  `OneHalfLight`; the catalog name is the one a site writes.
- `tokyo-night` variants are Apache-2.0 with no `NOTICE` file; the attribution is the author
  metadata in the theme files.
- `zenburn` is BSD-2-Clause; its snapshot carries the copyright notice the licence requires.
- `leet` carries upstream `1337.tmTheme`; the catalog name keeps the key reachable as
  `code-themes.leet`.
