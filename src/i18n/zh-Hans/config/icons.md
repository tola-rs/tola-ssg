# `[icons]`

从 `@tola/icon:0.0.0` 引入 `icon`；`icon("namespace:name")` 会在 HTML 中插入一个 SVG。

## 配置项

### `collections`

每个 namespace 说明它的图标从哪来：本地 SVG 目录或 IconifyJSON 文件，或按 Tola 已索引的 preset、或按 URL 与摘要指名的远程 collection。

## 用法

`[icons.collections]` 下的每一条就是一个 namespace，而 namespace 是你在 Typst 里写的每个
`"namespace:name"` id 的前半段：`brand:mark` 指的是 namespace `brand` 里的 `mark`。id 区分大小写。`icon()` 在调用时检查 id，即使返回的内容没有使用；名称不存在时，可能会提示该已配置集合中拼写接近的名称。

一个是自己的 SVG 目录，一个是 Tola 已索引的 collection：

```toml
# SVG files you own: icons/brand/mark.svg becomes brand:mark.
[icons.collections.brand]
source-type = "local-svg-dir"     # or "local-json" for one IconifyJSON file
path = "icons/brand"

[icons.collections.lucide]
source-type = "remote-json"
preset = "lucide"                  # newest release Tola indexes
```

preset 指明一个 Iconify collection，比如 `lucide`、`mdi`、`tabler`、`simple-icons`。Tola 通过 npm
registry 解析版本与完整性，并写入 `tola.lock`；在 preset 旁边加上 `version = "0.468.0"` 可固定 release。

若 `preset` 里没有你想要的 collection 或版本，就自己写 `url`：首次解析按 TOFU 把 `sha256` 与 `size`
写入 `tola.lock`，之后按锁校验：

```toml
[icons.collections.brand-remote]
source-type = "remote-json"
url = "https://cdn.example.com/brand/1.0.0/icons.json"
```

配置里不再声明 `sha256`：完整性只存在于 `tola.lock`。要刷新，运行 `tola update brand-remote`，再运行 `tola vendor` 更新冻结副本。

在页面里画一个已配置的图标，或把它作为独立文件发布：

```typst
#import "@tola/icon:0.0.0": icon, icon-url
#document("index.html")[
  #icon("lucide:rocket")
  #html.img(src: icon-url("brand:mark"), alt: "Brand mark")
]
```

自己的图标请导出为自包含 SVG：把文字转为路径，把外部 symbol 放进这个文件，并让引用指向同一文件内的内容。根元素使用宽高为正的 `viewBox`，或绝对单位下为正的 `width` 与 `height`。显式尺寸即使搭配 `viewBox` 也必须使用绝对单位，百分比与 `em` 会被拒绝。例如 `icons/brand/mark.svg` 可以写成：

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <path fill="currentColor" d="M12 2 22 22H2Z"/>
</svg>
```

图标保留它自己 SVG 画的颜色。用 `currentColor` 画的图标跟随所在页面的 `color`，主题或 CSS 类就能改变它的颜色；fills 与 strokes 写死颜色的图标不受页面影响。`lucide` 的每个图标都是用 `currentColor` 画的，所以这个 collection 适合会换主题的站点。像 `icon-url("brand:mark")` 这样发布出去的文件不继承任何东西，想让图标跟着主题走就用 `icon()`。

看看站点能用什么：

```sh
tola inspect icons                       # every namespace, its source, and its icon count
tola inspect icons lucide                # one namespace/name row per icon or alias
tola inspect icons lucide --interactive  # browse those names in a table
```

不指定 namespace 时，JSON 数组每个 collection 一行，包含 namespace、解析后的来源、collection 自己的图标前缀和图标数量。指定 namespace 后，图标与别名按词法顺序逐行返回 `namespace`/`name`；交互选择返回所选行。

远程 collection 下载与缓存的时机和构建完全一样。`tola vendor` 会把站点的远程 collection 冻结进它自己的 `icons/<namespace>.json`，另一台机器就能离线构建。
