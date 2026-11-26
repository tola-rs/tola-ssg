# Tailwind

Compile Tailwind CSS v4 from rendered Tola HTML pages.

```toml
[modules.tailwind]
builtin = "tailwind"
```

Include the generated stylesheet in `<head>` (using your module alias):

```typst
#import "@tola/address:0.0.0": output-url
#html.link(rel: "stylesheet", href: output-url("_tola-modules/tailwind/tailwind.css"))
```

For custom CSS, create `styles/tailwind.css` starting with `@import "tailwindcss" source(none);` and add `@source` directives as needed.

## Options

| Option | Default | Meaning |
| --- | --- | --- |
| `runner` | `"auto"` | `"auto"` (`node_modules/.bin/tailwindcss`, then `tailwindcss` on `PATH`) or a command array such as `["bunx", "--no-install", "@tailwindcss/cli"]` |
| `minify` | `false` | Minify CSS (applies in `dev` too) |
| `dev` | `"run"` | `"run"`, `"reuse"`, or `"skip"` in `tola dev` |
| `watch` | `styles/`, `tools/`, lockfiles, `cache.nu` | `watch.include` replaces defaults; `watch.exclude` also applies to disk cache |
| `cache` | `true` | Cross-run disk cache |
| `toolchain` | `""` | Cache key string; set and bump when using a launcher or custom CSS with external dependencies |

Custom runner commands execute from the site root and must forward Tailwind CLI arguments and exit after one build. To customize the module, copy it into your site and switch `builtin` to `path`.
