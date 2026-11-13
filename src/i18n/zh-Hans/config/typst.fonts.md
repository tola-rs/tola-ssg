# `[typst.fonts]`

在这里设置 Typst 编译用的字体。给浏览器加载的字体写在 `[assets]` 下。

## 配置项

### `paths`

要搜索的字体目录，默认是 `static/typst-fonts/`。路径相对于站点根目录，子目录也会搜索。

### `modules`

用这些模块里附带的字体，默认为空列表。模块要在 `tola-module.toml` 的
`[typst]` 下声明字体目录，例如 `fonts = ["typst-fonts"]`，路径相对于模块根目录。
Tola 先搜 `paths`，再按 `modules` 的顺序搜模块字体。Typst 按字体匹配规则选择，匹配条件相同时优先用先搜到的字体。

要同时用两个同名字体，可以给其中一个模块提供的字体换个名字：

```toml
[typst.fonts]
modules = [
  "theme",
  { module = "cjk", rename = { "IBM Plex Sans" = "CJK Plex Sans" } },
]
```

原字体族名称按不区分大小写的方式匹配。该模块项中此字体族的所有字重和样式
都登记在新名称下。在 Typst 中用 `CJK Plex Sans` 选用它，可通过
`tola inspect fonts --json` 查看名称和来源模块。

### `system`

设为 `true` 时也搜索系统字体。省略时跳过系统字体。

## 用法

默认搜索 `static/typst-fonts/`。要改用其他目录，把字体放到新目录，再在 `tola.toml` 里设置 `paths`。例如，改用 `static/print-fonts/`：

```toml
[typst.fonts]
paths = ["static/print-fonts"]
system = false
```

从站点根目录运行 `tola inspect fonts` 查看可用的字体族。在 Typst 中按字体族名称选择，例如 `#set text(font: "IBM Plex Sans")`。

站点的 Nu hook 缩放带文字的 SVG 时也用这套字体。在 SVG 的 `font-family` 里填 `tola inspect fonts` 列出的字体族名称。单独运行图片脚本时，把 `wordmark.svg` 和 `tola.toml` 放在一起，再运行：

```sh
tola nu --config tola.toml @tola-scripts/image resize wordmark.svg --width 240 --format png --output wordmark.png
```

命令会生成宽 240 像素的 `wordmark.png`。再次运行时换一个输出文件名。

`--pure` 下仍可用 Tola 内置字体。`system = true` 与 `--pure` 一起使用会报错。`tola vendor` 不复制编译器字体。

`--font-path DIR` 为本次调用添加优先搜索的目录，`--pure` 下不可用。`--ignore-system-fonts` 为本次调用排除系统字体。

### 给网页加载字体

把网页字体放到 `static/web-fonts/notes.woff2`，在同一目录下创建 `fonts.css`。

```css
@font-face {
  font-family: Notes;
  src: url("./notes.woff2") format("woff2");
  font-display: swap;
}
body { font-family: Notes, sans-serif; }
```

在 `tola.toml` 里用 `[assets]` 发布这两个文件。

```toml
[assets]
trees = [{ source = "static/web-fonts", url-prefix = "/fonts" }]
```

在用 `tola init` 创建的站点里，把这段放进 `content/index.typ`。

```typst
#import "@tola/address:0.0.0": asset-url
#html.link(rel: "stylesheet", href: asset-url("/fonts/fonts.css"))
= Font preview
```

从站点根目录运行 `tola build`，默认会在 `public/fonts/` 下写出字体和样式表。运行 `tola dev` 并打开打印的地址，首页会使用该字体。`asset-url` 会加上站点的部署前缀，CSS 中的字体路径相对于样式表。
