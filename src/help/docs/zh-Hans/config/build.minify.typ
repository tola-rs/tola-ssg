#show: config(
  summary: [控制编译输出的 HTML 以及站点提供的 CSS、JavaScript 是否压缩。],
)

对 `build`、`check`、`dev` 与 `preview` 均生效；可通过命令行 `--minify=false` 在单次运行中同时关闭三项压缩。

压缩 CSS 与 JavaScript 时会保留 `/*!` 开头的许可证注释。若某文件或内联 `<style>`/`<script>` 压缩失败，Tola 会保留原始内容并给出警告。启用 `assets.cache-busting` 时，哈希基于最终发布字节计算。#ref("[[hooks.generate-outputs]]") 产出的文件不受此配置影响。

#example(title: [保留可读 HTML，继续压缩 CSS 与 JavaScript])[
  ```toml
  [build.minify]
  html = false
  ```
]

#key("html")[
  压缩 Typst 导出的 HTML 文档。
]

#key("css")[
  压缩 Typst 或 `[assets]` 提供的 CSS 文件，以及页面内联的 `<style>` 标签。
]

#key("javascript")[
  压缩 Typst 或 `[assets]` 提供的 `.js`、`.mjs` 文件，以及页面内联的 `<script>` 标签（含 ES module）。
]
