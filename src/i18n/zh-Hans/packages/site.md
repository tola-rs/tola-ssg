# `@tola/site`

在 Typst 中读取站点设置。

## `project-path`

把站点根目录下的文件名转成 Typst `path`。

例如，主题可以把 `project-path("data/catalog.json")` 交给 `json`，读取站点里的数据文件。


在站点根目录创建 `data/catalog.json`，写入以下内容。

```json data/catalog.json
{"title": "Field notes"}
```

用下面的代码替换 `site.typ`，从站点根目录运行 `tola build`。使用默认发布目录时，`public/index.html` 会显示 Field notes。

```typst site
#import "@tola/site:0.0.0": project-path
#let catalog = json(project-path("data/catalog.json"))
#document("index.html")[#catalog.title]
```

### `path`

相对于站点根目录的路径。

## `site`

在 Typst 中读取 `tola.toml` 的 `[site]` 和 `[options]` 设置。

`site` 里可以读取 `origin`、`base-path`、`title`、`authors`、`description`、`language` 和 `languages`，也可以读到拼好的 `url`。用 `site.options` 读 `[options]`，没有配置时返回空字典。

`base-path` 是站点在域名下的路径，例如 `/docs/`，省略时用 `/`。`site.url` 由 `origin` 和 `base-path` 拼成。没设置 `site.origin` 时，`site.url` 为 `none`。

`title` 和 `description` 是你写在 `tola.toml` 里的文本；没有配置该项时它们为空。`authors` 为每位作者保存一条 `(name:, email:, url:)` 记录。`language` 是站点的默认语言（未设置时为 `en`），是一个含 `tag`、`lang`、`script` 和 `region` 的记录：`tag` 是完整的规范 tag，例如 `zh-Hans-CN`，各部分就是 `#set text` 接受的值。声明的语言缺少对应部分时，`script` 是 `auto`，`region` 是 `none`。

`languages` 按配置顺序列出站点支持的语言，默认为空。填了列表就要包含默认语言。多语言页面和切换链接由模板生成。

示例：读取默认配置：

使用默认 `tola.toml` 配置时，在 `site.typ` 中可以读到以下值。

```typst
#import "@tola/site:0.0.0": site
#assert.eq(site.base-path, "/")
#assert.eq(site.url, none)
#assert.eq(site.language.tag, "en")
#assert.eq(site.languages, ())
```

示例：读取配置的标题：

在 `tola.toml` 的 `[site]` 下设置 `title = "Example site"` 后，Typst 可以读到这个标题。

```typst site
#import "@tola/site:0.0.0": site
#assert.eq(site.title, "Example site")
#assert.eq(site.base-path, "/")
```
