#show: package(
  summary: [在 Typst 中读取解析后的 `[site]` 配置、`[options]`、`build-mode` 与站点根路径。],
  related: ("[site]", "[options]", "@tola/head"),
)

#export(
  "site",
  summary: [包含解析后的 `[site]` 配置、`[options]` 与当前构建模式的字典。],
  related: ("project-path", "[site]", "@tola/head#head-metadata"),
)[
  #table(
    columns: 2,
    table.header([字段], [说明]),
    [`origin`], [配置的源地址字符串，未配置时为 `none`。],
    [`base-path`], [以 `/` 开头和结尾的部署前缀（默认 `"/"`）。],
    [`url`], [`origin` 与 `base-path` 拼接的完整地址；未设置 `origin` 时为 `none`。],
    [`title`、`description`], [配置的站点标题与简介（未配置时为空字符串）。],
    [`authors`], [作者 `(name:, email:, url:)` 字典数组。],
    [`language`], [默认语言记录 `(tag:, lang:, script:, region:)`（默认 `"en"`；缺省 `script` 为 `auto`，缺省 `region` 为 `none`）。],
    [`languages`], [按配置顺序列出的语言列表（默认 `()`）。],
    [`options`], [`tola.toml` 中 `[options]` 表的字典（默认 `(:)`）。],
    [`build-mode`], [`tola dev` 下为 `"dev"`，`tola build`/`check`/`preview` 下为 `"prod"`，在编辑器、`tola query` 与 `tola inspect` 中为 `none`。],
  )

  #example(title: [在 `tola dev` 期间裁剪耗时渲染], check: "default")[
  ```typst
  #import "@tola/site:0.0.0": site

  #let is-dev = site.build-mode == "dev"
  #document("index.html")[#site.title]
  ```
  ]
]

#export(
  "project-path",
  summary: [将相对于站点根目录的路径解析为原生 Typst `path`，在模块或包内调用同样指向站点根。],
  params: (
    path: [相对于站点根目录的路径（例如 `"data/catalog.json"`）。],
  ),
  related: ("site", "@tola/module#get-module"),
)[
  可直接把返回的 `path` 传给 `read(...)`、`json(...)` 或 `csv(...)`，便于主题和辅助模块读取调用站点的数据文件。
]
