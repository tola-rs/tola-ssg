#show: package(
  summary: [读取已配置 Tola 模块的解析选项与静态资源。],
  related: ("[modules]", "[options]"),
)

#export(
  "get-module",
  summary: [返回指定模块实例的运行时信息（`name`、`options`、`assets`）。],
  params: (
    name: [`tola.toml` 中 `[modules.<alias>]` 声明的模块别名。仅在 `@tola-modules/*` 包内部调用时可省略，此时返回当前模块实例。],
  ),
)[
  即使多个别名指向同一份模块源码，每个别名也对应独立实例。

  - `name`：模块别名字符串。
  - `options`：合并 `[modules.<alias>.options]` 站点覆盖后的选项字典。
  - `assets.files`：用于读取模块资源文件的 Typst `path` 字典。
  - `assets.urls`：模块资源的发布 URL 字典（已带 `site.base-path`，开启 cache-busting 时带 `?h=<hash>`）。

  #example(title: [读取模块的样式表链接与标题选项])[
  ```typst
  #import "@tola/module:0.0.0": get-module

  #let theme = get-module("theme")
  #html.link(rel: "stylesheet", href: theme.assets.urls.at("style.css"))
  = #theme.options.title
  ```
  ]
]
