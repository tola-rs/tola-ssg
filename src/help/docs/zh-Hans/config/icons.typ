#show: config(
  summary: [声明供 `@tola/icon` 使用的本地 SVG 或 Iconify 图标集合。],
  related: ("@tola/icon", "[vendor]"),
)

`[icons.collections.<namespace>]` 下的表名即 `icon("namespace:name")` 中的命名空间前缀（图标名称区分大小写）。

= 集合来源类型

#table(
  columns: 2,
  table.header([`source-type`], [配置说明]),
  [`"remote-json"`], [Iconify 预设包（`preset = "lucide"`，可选 `version` 与 `mirrors`）或直接指定 `url`。运行 `tola update <namespace>` 下载并写入 `tola.lock`；`tola vendor` 会将锁定副本复制到 `vendor/icons/`。],
  [`"local-svg-dir"`], [相对于站点根目录的独立 SVG 文件目录（`path = "icons/brand"`）。SVG 须有正的 `viewBox` 或绝对单位宽高，且文字已转为路径。],
  [`"local-json"`], [相对于站点根目录的本地 IconifyJSON 文件（`path = "icons/custom.json"`）。],
)

运行 `tola inspect icons <namespace>`（加 `--interactive` 可交互浏览）查看图标名。模块可在 `tola-module.toml` 中声明自己的 `[icons.collections]`；站点可通过 `[modules.<alias>.icons.collections.<namespace>]` 替换模块集合，并通过 `tola update modules/<alias>/<namespace>` 更新远程模块图标集。

#example(title: [注册远程 Iconify 预设与本地 SVG 目录])[
  ```toml
  [icons.collections.lucide]
  source-type = "remote-json"
  preset = "lucide"

  [icons.collections.brand]
  source-type = "local-svg-dir"
  path = "icons/brand"
  ```
]

#key("collections")[
  以 `<namespace>` 为键的图标集合声明映射，支持 `remote-json`、`local-svg-dir` 与 `local-json`。
]
