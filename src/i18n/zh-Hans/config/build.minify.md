# `[build.minify]`

压缩 Typst 生成的 HTML，以及 Typst 和 `[assets]` 提供的 CSS 和 JavaScript。

## 配置项

### `html`

压缩 Typst 生成的 HTML。省略时开启。

### `css`

压缩 Typst `asset` 和 `[assets]` 提供的 CSS 文件，以及页面里的 `<style>`。省略时开启。

### `javascript`

压缩 Typst `asset` 和 `[assets]` 提供的 `.js`、`.mjs` 文件，以及页面里的 `<script>`，包括 module。省略时开启。

## 用法

要保留易读的 HTML，可以单独关闭 HTML 压缩：

```toml
[build.minify]
html = false
```

在用 `tola init` 创建的站点中，把 `= Hello` 写进 `content/index.typ`。从站点根目录运行 `tola build`，用文本编辑器打开 `public/index.html`，可以看到带缩进的 HTML。CSS 和 JavaScript 仍按各自的设置处理。用 `tola build --minify=false` 可一次关闭三项压缩。`check`、`dev` 和 `preview` 也读取这些设置。

压缩 CSS 和 JavaScript 时会保留以 `/*!` 开头的许可证注释。压缩失败时，Tola 保留原内容，并对 `[assets]` 中的资源和内联样式或脚本发出警告。启用 `cache-busting` 后，资源 URL 的哈希按实际发布的内容计算。`generate-outputs` 的文件由 hook 自行压缩。
