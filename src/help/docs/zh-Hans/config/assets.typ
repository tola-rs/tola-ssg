#show: config(
  summary: [发布图片、样式表、网页字体和下载文件等静态资源。],
  related: ("@tola/address#asset-url", "[build.minify]", "include"),
)

已启用的静态资源即使未被页面引用也会一并发布。在 Typst 中通过 #ref("@tola/address#asset-url") 获取资源链接，会自动附带 `site.base-path` 及可选的缓存指纹。

= 匹配与优先级

- `trees` 将整个目录发布到 `url-prefix` 下并保留相对路径。可通过区分大小写的 `include` / `exclude` 过滤（站点中相对于站点根目录，模块中相对于模块根目录）；`include = []` 不选中任何文件。
- 若同一源文件同时被 `files` 与 `trees` 匹配，以 `files` 为准（即使该单文件条目被禁用）。`trees` 的源目录不可嵌套，URL 前缀不可重复或互相包含。
- 每条声明均支持 `enable = false`、`enabled-by = "option.path"`，以及 `minify = false`（保留原始字节）。
- 使用 #ref("include") 拆分配置时，`files` 与 `trees` 会按加载顺序汇总；每个发布路径必须唯一。

= 文件符号链接

`trees` 默认拒绝符号链接。在对应目录条目中设置 `follow-file-symlinks = true` 后，指向可读普通文件的符号链接会按链接自身文件名发布其目标内容（`include`/`exclude` 按链接路径匹配）。目标文件可位于 `source` 目录之外，但在 `--pure` 模式下须位于站点内、在模块中须位于模块内，且始终不得位于 `build.content-dir` 或 `build.publish-dir` 内。若需发布目录符号链接的内容，请为目标目录单独声明 `trees`。

#example(title: [发布单文件与静态目录])[
  ```toml
  [assets]
  cache-busting = true
  files = [{ source = "static/logo.png", url = "/assets/logo.png" }]
  trees = [
    { source = "static/icons", url-prefix = "/icons", include = ["static/icons/**/*.svg"] },
  ]
  ```
]

#key("cache-busting")[
  开启后，`asset-url` 会在 URL 后附加 `?h=<hash>`，文件内容变化时哈希同步更新，发布文件名不变。
]

#key("trees")[
  目录映射列表（`{ source = "...", url-prefix = "/...", include = [...], exclude = [...], follow-file-symlinks = false }`）。
]

#key("files")[
  单文件映射列表（`{ source = "...", url = "/..." }`），对同一源文件优先级高于 `trees`。
]
