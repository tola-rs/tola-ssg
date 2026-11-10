# `[build.minify]`

在发布前，对由 Typst 编译而来的 HTML、`[assets]` 中的 CSS 与 JavaScript 文件进行压缩。

## 配置项

### `html`

是否压缩每个已发布页面的 HTML。

### `css`

是否压缩 `[assets]` 下声明的 CSS 源，以及页面 HTML 里内嵌的 `<style>`。

### `javascript`

是否压缩 `[assets]` 下声明的 `.js` 与 `.mjs` 源，以及页面 HTML 里内嵌的 `<script>`（含 module）。

## 用法

压缩发生在构建 output 时，`check`、`dev`、`preview` 都会根据 `[build.minify]` 选择是否压缩。因为进行压缩的时间很快，因此默认全部压缩。

`--minify=false` 为单次运行关闭三项。`[assets]` 下的 `cache-busting` 设置为 `true` 时，`asset-url`（位于 `@tola/address`）返回的 URL 上会被追加 `?h=`，标识压缩之后的字节——也就是浏览器实际收到的内容；压缩结果一变，URL 就变。如果 CSS 或 JavaScript 无法被成功压缩，那么会产生警告，保留原内容。
