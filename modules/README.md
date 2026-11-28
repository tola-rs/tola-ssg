# Built-in modules

Modules embedded in the `tola` binary. Site modules are enabled in `tola.toml` with `builtin = "<name>"`; Nu libraries are always available via `use tola <name>`.

| Module | Usage | Summary |
| --- | --- | --- |
| [`resize-images`](resize-images/README.md) | `builtin = "resize-images"` | Generate resized raster variants for assets |
| [`tailwind`](tailwind/README.md) | `builtin = "tailwind"` | Compile Tailwind CSS v4 from rendered HTML |
| [`terminal`](terminal/README.md) | `use tola terminal` | Interactive terminal table picker |
