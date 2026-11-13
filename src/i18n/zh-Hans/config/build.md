# `[build]`

下面是 `tola.toml` 中的默认构建设置。

```toml
[build]
entry = "site.typ"
content-dir = "content"
publish-dir = "public"
```

用 `tola init` 创建站点后，在 `content/index.typ` 写一行 `= Hello`。从站点根目录运行 `tola build`，会生成 `public/index.html`。

## 配置项

### `par-hooks`

默认允许同一站点或同一模块实例中，阶段和 `order` 都相同的 hook 并行。设为 `false` 后，仍按 `order` 逐个运行。

### `hook-jobs`

同时运行的 hook 数量上限，各模块和构建阶段共用。默认 `"auto"`，Tola 启动时按本机可用的并行度决定。也可以填正整数。设了 `par-hooks = false` 时，每次只运行一个。

### `entry`

Typst 入口文件，默认是站点根目录下的 `site.typ`。也可以用 `{ module = "notes" }` 读 `notes` 模块声明的入口。

### `content-dir`

从这个目录查找内容文件。路径相对于站点根目录，默认是 `content/`。

### `publish-dir`

把站点发布到这个目录。路径相对于站点根目录，默认是 `public/`。请选择空目录或本站上次发布的目录，构建成功后会替换整个目录。

## 用法

入口文件构建一个 Typst Bundle。它决定发布哪些源文件，并为每个 `#document` 指定输出路径。

例如，`all-sources()` 读到 `content/guide/install.typ` 时，返回的 `path` 是 `guide/install.typ`。移动文件也会改变它的 `path` 和默认的 `route-segments`。Tola 会跳过入口文件。共用函数和模板请放在内容目录之外。

`tola init` 生成的 `site.typ` 用下面的循环发布页面。新建 `content/guide/index.typ` 后运行 `tola build`，会生成 `public/guide/index.html`。

```typst
#import "@tola/address:0.0.0": join-route, output-of
#import "@tola/source:0.0.0": all-sources
#for source in all-sources() {
  document(output-of(join-route(source.route-segments)), format: "html")[
    #include source.file
  ]
}
```

要跳过草稿，可先用 `parse-sources` 校验 metadata，再筛掉草稿。要让页面移动后保留原 URL，可用 `@tola/schema` 的 `url-path` 解析 `permalink`，再把得到的 route 传给 `output-of`。

静态文件用 `[assets]` 发布，脚本生成的文件用 `[[hooks.generate-outputs]]` 声明。Tola 在发布前检查所有输出路径和引用。发布前出错或取消会保留上次的文件。

`[build.minify]` 控制文件压缩。`[build.references]` 设置无效引用的诊断级别。
