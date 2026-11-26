#show: config(
  summary: [Control HTML, CSS, and JavaScript minification for compiled pages and static assets.],
)

Applies to `build`, `check`, `dev`, and `preview`. Pass `--minify=false` on the command line to disable all three settings for a single run.

License comments starting with `/*!` are preserved in CSS and JavaScript. If minifying a file or inline `<style>`/`<script>` block fails, Tola keeps the original bytes and emits a warning. When `assets.cache-busting` is enabled, hashes are computed over the final published bytes. Files produced by #ref("[[hooks.generate-outputs]]") are not modified by these settings.

#example(title: [Keep readable HTML while minifying CSS and JavaScript])[
  ```toml
  [build.minify]
  html = false
  ```
]

#key("html")[
  Minify HTML documents exported by Typst.
]

#key("css")[
  Minify CSS files emitted by Typst or `[assets]` as well as inline `<style>` blocks.
]

#key("javascript")[
  Minify `.js` and `.mjs` files emitted by Typst or `[assets]` as well as inline `<script>` blocks (including ES modules).
]
