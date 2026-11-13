# `@tola/module`

读取已配置模块的选项和静态资源。

## `get-module`

读取模块的别名、选项和静态资源。

在 `@tola-modules` 包内，`get-module()` 读取当前实例。在站点文件中，必须传入 `tola.toml` 声明的别名，例如 `get-module("notes")` 对应 `[modules.notes]`。每个别名对应一个实例，即使它们指向同一目录。

`name` 是模块在站点中的别名。`options` 是合并了站点设置后的选项。用 `assets.files` 读文件，用 `assets.urls` 取链接。链接已带部署前缀，开启 `cache-busting` 时也会带文件哈希。

示例：读取模块标题和样式表：

沿用 `tola help --lang zh "[modules]"` 的 `notes` 示例。在 `theme/tola-module.toml` 的 `[assets]` 下加上 `trees = [{ source = "assets", url-prefix = "/" }]`。将 `body { color: rebeccapurple; }` 保存为 `theme/assets/style.css`。在站点的 `tola.toml` 中，把 `[modules.notes.options]` 下的 `title` 设为 `"Lab notes"`。

把示例中的 `content/index.typ` 换成下面的代码。

```typst
#import "@tola/module:0.0.0": get-module
#let notes = get-module("notes")
#html.link(rel: "stylesheet", href: notes.assets.urls.at("style.css"))
= #notes.options.title
```

保留 `tola init` 生成的 `site.typ`。从站点根目录运行 `tola build`。用默认配置时，首页正文会显示 Lab notes，并加载样式表。两个文件分别是 `public/index.html` 和 `public/_tola-modules/notes/style.css`。

### `name`

站点中声明的模块别名。只有在 `@tola-modules` 包内才可省略，此时读取当前实例。
