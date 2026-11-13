# `@tola/icon`

在文档中使用 `[icons]` 里配置的 SVG 图标。

SVG 的 `fill` 或 `stroke` 用 `currentColor` 时，内联图标跟随文字颜色。SVG 中指定的其他颜色保持原样。

## `icon`

将已配置的图标插入 HTML，生成高 1em 的内联 SVG。

`icon()` 只能在 HTML 文档中使用。其他输出格式可用 `icon-bytes` 或 `icon-url`。

Iconify 的完整配置见 [`[icons]`](tola-help://config/icons)。配置好 Lucide 后，在 HTML 文档中用 `icon("lucide:rocket")` 插入火箭图标。

`icon()` 在调用时检查 id，即使返回的内容没有使用。

示例：插入装饰性图标和带标签的图标：

按 `tola help --lang zh "[icons]"` 配置 `brand:mark`。把下面的代码放到新站点的 Typst 入口文件，运行 `tola build`。首页会包含内联 SVG。

```typst site
#import "@tola/icon:0.0.0": icon
#assert.eq(type(icon("brand:mark")), content)
#document("index.html")[
  #icon("brand:mark")
  #icon("brand:mark", label: "Example site logo", attrs: (class: "brand",))
]
```

示例：同时使用内联 SVG 和图标 URL：

```typst site
#import "@tola/icon:0.0.0": icon, icon-url
#import "@tola/site:0.0.0": site
#assert(icon-url("brand:mark").starts-with(site.base-path))
#document("index.html")[
  #icon("brand:mark")
  #html.img(src: icon-url("brand:mark"), alt: "Example mark")
]
```

示例：把图标发布到指定路径：

```typst site
#import "@tola/icon:0.0.0": icon-bytes
#let mark = icon-bytes("brand:mark")
#assert.eq(type(mark), bytes)
#asset("mark.svg", mark)
```

### `id`

`"collection:name"`，取自你在 `icons.collections` 下声明的集合；名称区分大小写。

### `label`

省略或传 `none` 时，将图标标记为装饰。图标有实际含义时，填写供屏幕阅读器读取的名称，例如 `"站点标志"`。只含空白会报错。

### `attrs`

添加到根 SVG 元素的属性，值须为字符串，例如 `(class: "brand",)`。省略时不加额外属性。`aria-label`、`aria-labelledby`、`aria-hidden`、`role`、`focusable` 由 `label` 掌管，在 `attrs` 里写其中任何一个都会报错；`class` 与 `style` 会追加到 SVG 已有的值上。

## `icon-bytes`

返回 SVG 字节。可传给 Typst 的 `image(bytes)` 显示，或交给 `asset()` 发布到指定路径。需要直接发布并取得 URL 时，用 `icon-url()`。


按 `tola help --lang zh "[icons]"` 配置 `brand:mark`，在 `site.typ` 中使用下面的代码。运行 `tola build` 后，默认会生成 `public/mark.svg`。

```typst site
#import "@tola/icon:0.0.0": icon-bytes
#let mark = icon-bytes("brand:mark")
#assert.eq(type(mark), bytes)
#assert(mark.len() > 0)
#asset("mark.svg", mark)
```

### `id`

配置好的图标的标识，例如 `"brand:mark"`。

## `icon-url`

发布 SVG 图标，返回可以在网页上使用的 URL。

构建只发布文档用到的图标。URL 已包含 `site.base-path`，可直接填在 `<img>` 或链接里。

要让图标跟随文字颜色，用 `icon()` 插入包含 `currentColor` 的 SVG。


按 `tola help --lang zh "[icons]"` 配置 `brand:mark`。把下面的代码加到 `site.typ`，运行 `tola build`。首页会通过图片元素加载已发布的 SVG。

```typst site
#import "@tola/icon:0.0.0": icon-url
#import "@tola/site:0.0.0": site
#let url = icon-url("brand:mark")
#assert(url.starts-with(site.base-path))
#assert(url.ends-with(".svg"))
#document("index.html")[#html.img(src: url, alt: "Example mark")]
```

### `id`

配置好的图标的标识，例如 `"brand:mark"`。
