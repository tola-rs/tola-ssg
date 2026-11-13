# `[site]`

设置站点地址、标题和语言。例如，要部署到 `https://example.com/docs/`，在 `tola.toml` 中写：

```toml
[site]
origin = "https://example.com"
base-path = "/docs/"
title = "Typst 笔记"
language = "zh-Hant-TW"
```

在用 `tola init` 创建的站点中，把 `= Guide` 写进 `content/guide/index.typ`。从站点根目录运行 `tola build`，会生成 `public/guide/index.html`。把 `public/` 部署到 `/docs/` 后，访问 `https://example.com/docs/guide/`。用 `tola dev` 在默认端口预览时，打开 `http://127.0.0.1:5277/docs/guide/`。

## 配置项

### `origin`

URL 的协议、主机和可选端口，例如 `https://example.com`。路径写在 `base-path` 里。省略时 `site.url` 为 `none`。生成 feed 或 sitemap 时须填写 `origin`。

### `base-path`

站点在域名下的部署路径，例如 `/docs/`。开头和结尾都要有 `/`，默认是 `/`。

### `title`

站点名称，可用作模板和 feed 的标题。默认是空字符串。

### `authors`

站点作者，例如 `[{ name = "Alice", email = "alice@example.com" }]`。每位作者都要写 `name`，`email` 和 `url` 可以省略。省略 `authors` 时用空列表。

### `description`

站点简介，可用在 feed 和页面元数据中。默认是空字符串。

### `languages`

站点提供的语言，例如 `["en", "zh-Hans"]`，写法和 `language` 相同。默认是空数组。填了列表就要包含 `language` 的值。不同语言的页面和切换链接由模板生成。

### `language`

站点的默认语言，省略时为 `"en"`。可以写 BCP 47 语言标签，如 `"zh-Hant-TW"`，也可以拆开写成 `{ lang = "zh", script = "Hant", region = "TW" }`。

## 用法

### 在 Typst 模板中读取设置

用 `@tola/site` 读取这些设置。`tola init` 创建的 `site.typ` 已经这样设置了文本语言。

```typst
#import "@tola/site:0.0.0": site
#set text(lang: site.language.lang, script: site.language.script, region: site.language.region)
```

手写 HTML 的 `lang` 属性时，用 `site.language.tag` 取完整语言标签，例如 `zh-Hant-TW`。需要把汉字转写成 ASCII slug 时，可把 `site.language.lang` 传给 `slugify` 的 `language` 参数。`ja` 按日语转写，其他值按中文转写。

上面的配置对应的 `site.url` 是 `https://example.com/docs/`。省略 `origin` 时，`site.url` 为 `none`。想临时换个地址或部署路径，可以加 `--origin` 或 `--base-path`，只对本次运行生效。
