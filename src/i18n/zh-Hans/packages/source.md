# `@tola/source`

列出 Typst 内容文件，读取源文件信息，或声明和校验文章元数据。

站点入口决定每个源文件用在哪些文档里。站点可以自行约定 `title`、`draft`、`permalink` 等元数据字段。

## `all-sources`

列出 `build.content-dir` 下发现的内容文件，按相对路径排序。

只列出 `build.content-dir` 下以小写 `.typ` 结尾的文件。隐藏文件、符号链接和站点入口会跳过。目录里的 `index.typ` 和其他文件一起按路径排序。需要按文章日期等顺序排列时，自行排序。

每条记录包含：

- `id` 与 `path` 是同一个路径字符串，相对于 `build.content-dir`，包含扩展名，例如 `posts/deep.typ` 或 `about/index.typ`。源文件改名或在内容目录内移动时，两个值都会改变。
- `file`：可直接传给 `include` 或 `read` 的 Typst `path`，以站点根目录为准。
- `filename`：`path` 的最后一段，例如 `deep.typ`。
- `route-segments`：从文件路径取出的默认 route 各段。首页为 `()`，`about.typ` 和 `about/index.typ` 都是 `("about",)`，`posts/deep.typ` 是 `("posts", "deep")`。
- `meta`：通过 `tola-meta` 声明的元数据。需要读取多篇文章时，在 `site.typ` 中读取。

为 feed 条目保存固定的 `id`，页面移动后继续使用它。这样订阅阅读器仍能识别同一条内容。

`tola inspect sources` 列出发现的文件路径，不求值 metadata。
用 `tola query` 可以将选定的 metadata 输出为 JSON。

要按源文件路径安排输出，先用 `@tola/address` 的 `join-route` 拼接 `route-segments`，再用 `output-of` 得到输出路径，传给 `#document`。

示例：列出空站点的源：

```typst
#import "@tola/source:0.0.0": all-sources
#let sources = all-sources()
#assert.eq(sources.len(), 0)
```

示例：按路径选出一篇文章并发布：

在 `content/guide/index.typ` 写入 `= Guide`。在新站点中将这段代码保存为 `site.typ`，再运行 `tola build`。默认输出为 `public/guide/index.html`。

```typst site
#import "@tola/source:0.0.0": all-sources
#let guide = all-sources().find(source => source.path == "guide/index.typ")
#document("guide/index.html", format: "html", title: "Guide")[
  #include guide.file
]
```

## `current-source`

读取调用所在的 Typst 源文件的信息。

返回的 `id`、`file`、`path`、`filename` 和 `route-segments` 和 `all-sources()` 中含义相同。可以直接调用，无需 `context`。

在内容文件里调用，再把结果传给模板。在模板里直接调用，读到的是模板文件的位置，如果模板不在内容目录中，就会报错。同一个内容文件用在不同文档里时，这个函数始终返回源文件的信息。

## `tola-meta`

声明当前内容文件的元数据。

每个内容文件可调用一次。通常把它写在正文前，也可以调用从其他文件导入的封装函数。放在 `context` 或带 selector 的 show rule 里时，Tola 不会把它当作源文件的 metadata。全局 `#show:` 在正文求值之后运行，因此正文出错可能使其中的声明无法执行。即使站点会筛掉某个草稿，该文件的错误仍会使构建失败。

返回值是一个不可见的 `<tola-meta>` 标记，里面是传入的字典。把它放进文档后可用 `query(<tola-meta>)` 查询。Tola 在调用时就记录 metadata，因此把返回值赋给变量也能声明。导入的封装函数为调用它的源文件声明 metadata。其他文件的顶层调用仍归各自文件。

示例：声明 metadata 并读取它的标记：

```typst site
#import "@tola/source:0.0.0": tola-meta
#let marker = tola-meta((title: "Hello"))
#assert.eq(marker.value, (title: "Hello"))
#assert.eq(marker.label, <tola-meta>)
```

示例：封装具名 metadata 字段：

```typst
#import "@tola/source:0.0.0": tola-meta
#let note(..fields) = tola-meta(fields.named())
```

将这段定义保存到 `site/metadata.typ`。在 `content/post.typ` 中用 `#import "../site/metadata.typ": note` 导入，再用 `#note(title: "Hello")` 声明。

### `payload`

要为这篇内容声明的元数据字典。

## `parse-sources`

校验源文件的 metadata，并返回带有解析后 `meta` 的记录。

在 `site.typ` 中传入要校验的源文件记录和一个 `@tola/schema` 规则。不同文章需要不同字段时，可以传一个函数，按源文件返回对应的 schema。没声明元数据的文件按空字典校验。

返回记录的顺序和路径照旧，筛选和模板可以直接用其中的 `meta`。校验失败时，Tola 会指出相关文件及 metadata 的位置。文件没有声明时，诊断指向文件开头。

示例：发布文章，跳过草稿：

在用 `tola init` 创建的站点里，新建 `content/guide.typ` 和 `content/draft.typ`。两个文件都先写 `#import "@tola/source:0.0.0": tola-meta`。

- `guide.typ`：接着写 `#tola-meta((title: "Guide"))`，下一行写正文 `= Guide`。
- `draft.typ`：接着写 `#tola-meta((title: "Draft", draft: true))`，下一行写正文 `= Draft`。

把下面的代码保存为 `site.typ`，在站点根目录运行 `tola build`。使用默认配置时会生成 `public/guide/index.html`。草稿参与校验，随后被筛掉。若 `content/` 下还有其他文章，也要给它们声明非空的 `title`。

```typst site
#import "@tola/address:0.0.0": join-route, output-of, slugify
#import "@tola/schema:0.0.0": non-empty, optional, schema, trim
#import "@tola/source:0.0.0": all-sources, parse-sources

#let page-schema = schema((
  title: non-empty(trim(str)),
  draft: optional(bool, default: false),
), unknown: "keep")
#let pages = {
  parse-sources(all-sources(), page-schema)
    .filter(source => not source.meta.draft)
}
#for page in pages {
  let output = output-of(join-route(page.route-segments.map(slugify)))
  document(output, format: "html", title: page.meta.title)[#include page.file]
}
```

### `sources`

要解析的记录，来自 `all-sources()` 或它的子集。

### `schema`

校验元数据的 schema，也可以传入一个函数，根据每条源文件记录返回对应的 schema。
