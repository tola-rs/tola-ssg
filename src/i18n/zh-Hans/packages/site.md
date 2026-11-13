# `@tola/site`

本站的配置值，解析自 `tola.toml`

## `project-path`

返回相对于站点根目录的 Typst `path`。在主题包中调用时，仍以站点根目录为准。

用于读取主题已知位置的用户数据或生成输入。这里只创建路径，不读取或生成文件；实际读取仍遵守本次调用的输入边界。

```typst site
#import "@tola/site:0.0.0": project-path
#let data = project-path("generated/catalog.json")
```

### `path`

相对于站点根目录的路径。

## `site`

在 Typst 中读取 `tola.toml` 的 `[site]` 和 `[options]` 设置。

`site` 为每项 `[site]` 设置提供一个条目，另外还有派生出来的 `url`：`origin`、`base-path`、`url`、`title`、`authors`、`description`、`language` 和 `languages`。顶层 `[options]` 表通过 `site.options` 读取，默认为空字典 `(:)`。

`base-path` 是主机根之下的部署路径，默认是 `/`。`url` 把 `origin` 和 `base-path` 拼成站点的规范地址。没有声明 `site.origin` 的站点只发布相对地址，所以它的 `url` 是 `none`。

`title` 和 `description` 是你写在 `tola.toml` 里的文本；没有配置该项时它们为空。`authors` 为每位作者保存一条 `(name:, email:, url:)` 记录。`language` 是站点的默认语言（未设置时为 `en`），是一个含 `tag`、`lang`、`script` 和 `region` 的记录：`tag` 是完整的规范 tag，例如 `zh-Hans-CN`，各部分就是 `#set text` 接受的值。声明的语言缺少对应部分时，`script` 是 `auto`，`region` 是 `none`。

`languages` 是配置中的所有语言，保持配置顺序，默认为 `()`；非空数组必须包含 `language`。`language` 和 `languages` 都是模板的输入：语言的路由、配对与切换由站点本身决定。

使用默认 `tola.toml` 配置时，在 `site.typ` 中可以读到以下值。

```typst
#import "@tola/site:0.0.0": site
#assert.eq(site.base-path, "/")
#assert.eq(site.url, none)
#assert.eq(site.language.tag, "en")
#assert.eq(site.languages, ())
```

在 `tola.toml` 的 `[site]` 下设置 `title = "Example site"` 后，Typst 可以读到这个标题。

```typst site
#import "@tola/site:0.0.0": site
#assert.eq(site.title, "Example site")
#assert.eq(site.base-path, "/")
```
