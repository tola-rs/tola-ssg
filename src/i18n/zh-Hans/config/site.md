# `[site]`

站点的配置值：Tola 会解析并校验它们的格式，例如 `language` 必须符合 BCP 47 语言标签格式；模板通过 `@tola/site` 读取它们。

## 配置项

### `origin`

站点的公开地址，例如 `https://example.com`，不带路径。未设置时只发布相对地址。

### `base-path`

站点挂载在哪个部署路径之下，例如 `/docs/`。必须以 `/` 开头和结尾。

### `title`

站点自称的名字，出现在浏览器标签页与 feed 里。

### `authors`

署名的作者，模板里读作 `site.authors`。每项形如 `{ name = "…", email = "…", url = "…" }`。

### `description`

feed 与搜索引擎展示的站点摘要。

### `languages`

多语言站点发布的语言，写成 `["en", "zh-Hans"]`，或按标签各部分写成 `[{ lang = "en" }, { lang = "zh", script = "Hans" }]`。每项的写法与校验都与 `site.language` 相同；`site.language` 是默认语言（未填写时为 `"en"`）；列表非空时，必须包含它。Tola 只把它当数据读取，不做任何处理：站点如何在语言之间路由、配对与切换，由站点自己决定。

### `language`

语言标签，例如 `"zh-Hant-TW"`，或写成 `{ lang = "zh", script = "Hant", region = "TW" }`。模板读 `site.language.lang`（主语言）、`.script`（书写系统）、`.region` 与 `.tag`（组合形式）。Typst 文本使用各部分，HTML 的 `lang` 使用 `.tag`。ASCII 模式下把 `.lang` 传给 `slugify` 决定汉字读音：`ja` 取日文，其余语言取中文。书写系统与地区不参与 slug 化。

## 用法

模板把这些读作 `site.<key>`（`tola help "@tola/site"` 说明这个对象）：

```toml
[site]
origin = "https://example.com"    # The public address, with no path
base-path = "/docs/"              # The path the site is served below
title = "Example"
description = "Notes on Typst, published with Tola"
language = "zh-Hant-TW"           # Or { lang = "zh", script = "Hant", region = "TW" }
authors = [{ name = "Alice", email = "alice@example.com" }]
```

`origin` 与 `base-path` 一起决定每一条发布地址：上面的站点把 `guide/index.html` 发布在
`https://example.com/docs/guide/`。没有 `origin` 时，站点只发布相对地址——这正是 feed 与
sitemap 需要它的原因：它们列出的每条地址都是绝对地址。`tola dev` 在本地按同一个挂载路径提供服务，例如
`http://127.0.0.1:5277/docs/`；`--origin` 与 `--base-path` 可以为单次运行覆盖这一对值。

这两个键还会派生出 `site.url`：站点的规范地址，没设 `origin` 时为 `none`。`site.language` 用
`tag` 给出完整标签，用 `lang`、`script`、`region` 给出各部分——上面那种语言就是 `zh`、`Hant`、
`TW`——而各部分正是 `#set text` 接受的东西：

```typst
#import "@tola/site:0.0.0": site
#let url = site.url                     // none when `origin` is unset
#let tag = site.language.tag            // "zh-Hant-TW"
#let script = site.language.script      // "Hant", or `auto` when none
#let names = site.authors.map(author => author.name)
```
