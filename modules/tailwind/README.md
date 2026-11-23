# Tailwind

Compile Tailwind CSS v4 from the HTML Tola renders.

```toml
[modules.tailwind]
builtin = "tailwind"
```

Add the stylesheet to your HTML head.

```typst
#import "@tola/address:0.0.0": output-url
#html.link(rel: "stylesheet", href: output-url("_tola-modules/tailwind/tailwind.css"))
```

Use your module alias in the URL. For custom CSS, add `styles/tailwind.css`
starting with `@import "tailwindcss" source(none);`. Use `@source` for extra files
or `@source inline(...)` for classes added in the browser.

## Runner

`runner = "auto"` looks for `node_modules/.bin/tailwindcss` at the site root,
then for `tailwindcss` on `PATH`. Set a command array in `[modules.tailwind.options]`
to choose a launcher.

```toml
[modules.tailwind.options]
runner = ["deno", "task", "tailwind"]
minify = true
```

Other runners include `["bunx", "--no-install", "@tailwindcss/cli"]`,
`["npx", "--no", "--", "@tailwindcss/cli"]`, and `["./tools/tailwind"]`.
Commands run from the site root. A wrapper must forward the Tailwind arguments
and exit after one build. `minify = true` also applies in development.

## Rebuilds

`dev` defaults to `"run"`. Use `"reuse"` to skip the hook when its inputs match,
or `"skip"` to omit the stylesheet in development. Omit the link in that mode.

`watch` covers `styles/`, `tools/`, package and lock files, and the module's
`cache.nu` by default. Add extra imports and scan inputs to `watch.include`.
Your list replaces the defaults. `watch.exclude` also applies to the CSS cache.

The disk cache works across runs. `auto` can cache stock CSS with a standalone
CLI. For a launcher or custom stylesheet, set `toolchain` and update it when the
tool or dependencies outside `watch` change. `cache = false` disables disk
caching. `tola dev --no-reuse` only disables hook reuse.

To edit the module, copy it to your site's `modules/tailwind/` and replace
`builtin` with `path = "modules/tailwind"`.
