# `[build]`

站点如何构建：Typst 入口程序、源文件目录，以及发布目录。

## 配置项

### `entry`

站点相对的 Typst 文件路径，或指定模块别名的表，例如 `{ module = "notes" }`。

### `content-dir`

站点源文件所在的目录，相对站点根。Tola 读取其下的 `.typ` 文件。

### `publish-dir`

构建结果发布到的目录，相对站点根。该目录整棵归 Tola：Tola 没写过的非空目录会让构建失败；Tola 写过的目录，每次成功构建都会整棵替换。不要往里放其他东西。

## 用法

站点是一个 Typst Bundle，`entry` 是它的入口程序。它 import 的模板、helper 与页面程序都进入同一次编译。`tola init` 默认生成的脚手架将 `entry` 设置为站点根目录下的 `site.typ`。可指定其他站点文件，或设置 `entry = { module = "notes" }`，由已声明的模块实例提供 `[build].entry`。

`content-dir` 决定源的身份：`content/guide/install.typ` 在 `all-sources()` 对应的 source 里，其 `path` 字段是 `guide/install.typ`。文件层级不影响 `permalink`；route segments 是由文件层级映射而来的“默认路由的分段”，方便你自定义路由。改名或移动源文件，身份与这些 segments 都会随之改变。`entry` 本身永远不是源；共享 helper 与模板请放在这个目录之外，这样就不会被当成内容。

发现过程不会发布页面：哪些源成为文档、各自使用哪个 output 路径，都由 `entry` 决定。例如，下面把已发现的 segments 转成默认 output 路径：

```typst
#import "@tola/address:0.0.0": join-route, output-of
#import "@tola/source:0.0.0": all-sources
#let outputs = all-sources().map(source => output-of(join-route(source.route-segments)))
```

站点如果在入口里校验 metadata，也可以在这里筛掉草稿、把每个 source 的 `route-segments` slugify；这些策略由你自己定义。schema 用 `url-path` 解析 `permalink`，因此选中的 metadata 已经持有解码后的 route。它会覆盖文件布局给出的默认 route，使源移动后 URL 保持不变。

```typst
#let source-route(source) = {
  if source.meta.permalink == none {
    join-route(source.route-segments.map(segment => slugify(segment, language: site.language.lang)))
  } else {
    source.meta.permalink
  }
}

#let select-pages(sources) = {
  // Every source is validated here, drafts included, before choosing which to publish.
  parse-sources(sources, page-schema)
    .filter(source => not source.meta.draft)
    .map(source => (
      source: source,
      output: output-of(source-route(source)),
    ))
}
```

`publish-dir` 是 `tola build` 整体接管的目录：每次成功构建都替换整棵目录，旧 output 一并清掉。它要么是空目录，要么是 Tola 为这个站点写过的目录；已经放着别的内容、又不是 Tola 写出的目录，会让构建失败。手工维护的文件请通过 `[assets]` 或 `[[hooks.generate-outputs]]` 声明的 output 发布。发布前失败或取消会保留上次输出；`check`、`dev` 与 `preview` 不替换这个目录。

两张子表调整构建：`[build.minify]` 压缩支持的 output 字节；`[build.references]` 为无效引用选择 `error` 或 `warn`。顶层 `[hooks]` 在编译前、output 生成时或发布后运行站点自己的命令。所有 output 汇入同一份完整结果，发布前统一检查冲突与引用，因此一个生产者不能悄悄替换另一个的文件。
