#show: package(
  summary: [在 HTML 中内联渲染已配置的 SVG 图标，或将其发布为独立 SVG 资源。],
  related: ("[icons]",),
)

SVG 的 `fill` 或 `stroke` 使用 `currentColor` 时，内联图标会自动跟随所在位置的文字颜色；显式颜色保持不变。

#export(
  "icon",
  summary: [将已配置图标以 1em 高的内联 SVG 插入 HTML 文档。],
  params: (
    id: [区分大小写的 `"namespace:name"` 图标标识，来自 `[icons.collections]`。],
    module: [拥有该图标集合的可选模块别名；省略时使用站点集合。],
    label: [供屏幕阅读器读取的非空名称；省略或传 `none` 时标记为装饰性图标（`aria-hidden="true"`）。],
    attrs: [添加到根 `<svg>` 的字符串属性字典（例如 `(class: "brand",)`）。`class` 与 `style` 会追加；无障碍属性（`aria-label`、`aria-labelledby`、`aria-hidden`、`role`、`focusable`）由 `label` 统一控制，不可写在 `attrs` 中。],
  ),
  related: ("icon-url", "icon-bytes", "[icons]"),
)[
  调用时立即校验 `id`。仅适用于 HTML 文档；PDF 等格式请用 `icon-bytes` 或 `icon-url`。

  #example(title: [插入装饰性图标与带无障碍标签的图标], check: "site")[
  ```typst
  #import "@tola/icon:0.0.0": icon

  #document("index.html")[
    #icon("brand:mark")
    #icon("brand:mark", label: "站点标志", attrs: (class: "brand",))
  ]
  ```
  ]
]

#export(
  "icon-bytes",
  summary: [返回图标的原始 SVG 字节，供 Typst `image(...)` 或自定义 `asset(...)` 发布。],
  params: (
    id: [区分大小写的 `"namespace:name"` 图标标识。],
    module: [拥有该图标集合的可选模块别名；省略时使用站点集合。],
  ),
  related: ("icon", "icon-url"),
)

#export(
  "icon-url",
  summary: [将图标发布为独立 SVG 文件并返回带 `site.base-path` 的浏览器 URL。],
  params: (
    id: [区分大小写的 `"namespace:name"` 图标标识。],
    module: [拥有该图标集合的可选模块别名；省略时使用站点集合。],
  ),
  related: ("icon", "icon-bytes"),
)[
  仅发布求值过程中实际引用到的图标。

  #example(title: [通过 `<img>` 加载已发布的 SVG 图标], check: "site")[
  ```typst
  #import "@tola/icon:0.0.0": icon-url

  #document("index.html")[
    #html.img(src: icon-url("brand:mark"), alt: "站点标志")
  ]
  ```
  ]
]
