# `[assets]`

把声明的文件与目录发布到它们自己写明的站点根 URL 上。

## 配置项

### `cache-busting`

让内容变过的已发布字节在 `asset-url` 返回的 URL 里获得新的 `?h=...`。已发布的路径本身不改名。默认关闭。

### `trees`

发布在某个 URL prefix 之下的源目录。`include` 与 `exclude` 从站点根匹配，只选择 `source` 内的成员。省略 `include` 选择整棵源树。

### `files`

按自己写明的 URL 发布的单个源文件，每项写成 `{ source = "static/robots.txt", url = "/robots.txt" }`。落在已声明树里的文件在这里发布，而不是在树里。

## 用法

声明的源无论有没有文档引用或读取它，都会被发布。目录把成员发布到一个 URL prefix 之下；精确的文件声明则给某个源自己的 URL：

```toml
[assets]
cache-busting = true
trees = [{ source = "static/web-assets", url-prefix = "/assets" }]
files = [{ source = "static/web-assets/tailwind-output/site.css", url = "/assets/css/site.css" }]
```

这里 `files` 从树中接管生成的样式表：它发布在 `/assets/css/site.css`，不再发布在 `/assets/tailwind-output/site.css`。树仍然发布其余成员。`source` 是相对站点根的路径；`url` 与 `url-prefix` 是尚未加上 `site.base-path` 的站点根 URL 路径。`before-build` hook 可以写这个源；`generate-outputs` hook 直接添加自己的 output，无需 `[assets]` 条目。

每棵树可以用 `include` 和 `exclude` 选择成员，模式相对于声明者根。站点树以站点根为基准，模块树以模块根为基准。`source` 限定可发布的子树，文件在其中的相对路径附加到 `url-prefix`。例如 `{ source = "static", url-prefix = "/assets", include = ["static/**/*.css"], exclude = ["static/drafts"] }` 选择 `static` 内的 CSS 并排除 `static/drafts`。省略 `include` 等价于 `["."]`，覆盖整棵源树；`include = []` 不选任何成员，`exclude` 默认为空。目录和文件声明均可用 `minify = false` 保留源字节，或用 `enabled-by` 引用所属 options 表中的顶层布尔键来控制发布。精确文件声明在判断启用条件之前接管对应源，树的过滤只影响自己的成员。启用的文件和树在 before-build 完成后读取。

用 `asset-url` 链接声明发布的文件，传入声明的 URL，而非源路径。`cache-busting = true` 时，结果带有标识发布字节的 `?h=...`；字节变化就得到新 URL，output 文件名保持不变。结果已包含 `site.base-path`，直接使用即可：

```typst
#import "@tola/address:0.0.0": asset-url

#let style = asset-url("/assets/css/site.css")   // /assets/css/site.css (or /assets/css/site.css?h=... with cache-busting)
#let script = asset-url("/assets/app.js")        // /assets/app.js (or /assets/app.js?h=... with cache-busting)

#html.link(rel: "stylesheet", href: style)       // link it in the head
#html.script(src: script)                        // add the script in the head
```

每个 output 路径只能有一个生产者——它要么来自 Bundle 的编译结果，要么来自 `[assets]` 声明，要么来自 hook。声明之间有一条硬规则：**同一条 output 路径不能被两个声明认领**。

两张 `trees` 的源路径不能互相嵌套。像下面这样第二棵树套在第一棵里面，构建会报错：

```toml
[assets]
trees = [
  { source = "static/assets", url-prefix = "/assets" },
  { source = "static/assets/css", url-prefix = "/css" },
]
```

两个声明认领同一条 output 路径也一样会报错——比如两个 `files` 写到同一个 `url`：

```toml
files = [
  { source = "static/a.css", url = "/app.css" },
  { source = "static/b.css", url = "/app.css" },
]
```

例外只有一种：`files` 可以从树里接管某个成员（就是上面那张样式表）——被接管的成员改由 `files` 声明发布，树发布其余成员。发布之前，Tola 把 `[assets]`、Bundle 与 hook 的 output 放在一起检查这些规则。
