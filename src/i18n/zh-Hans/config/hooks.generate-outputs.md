# `[[hooks.generate-outputs]]`

Typst 编译后运行。适合生成搜索索引、文件清单等要一并发布的文件。要生成编译时读取的文件，用 `before-build`。

## 配置项

### `enable`

默认启用。设为 `false` 跳过这个 hook。

### `name`

状态和错误消息里显示的名称。同一阶段内不能重名，名称不能为空或包含空白。

### `order`

非负整数，省略时为 `0`。同一站点或模块的 `generate-outputs` hook 按此数值从小到大执行。同序号的 hook 可以并行，各自从同一份已编译站点读取文件。

### `argv`

从站点根目录运行的命令及参数。`argv` 和 `nu` 二选一。从 `hook.inputDir` 读取页面和静态文件，写到 `hook.outputDir`。

### `nu`

要运行的 Nu 脚本，路径相对于站点根目录。`nu` 和 `argv` 二选一。

### `args`

传给 `nu` 脚本的参数，可省略。

### `enabled-by`

例如填 `"images.enable"`，只在 `[options.images]` 的 `enable` 为 `true` 时运行。省略时不检查选项。

### `dev`

省略或设为 `"run"` 时，`tola dev` 每次构建都运行。设为 `"reuse"` 时，输入未变就复用上次成功生成的文件。设为 `"skip"` 时，开发构建会跳过。

### `rerun-on`

让额外输入的变化触发重建。指定后须填 `include`，可省略 `exclude`。配置了 `dev = "reuse"` 时，匹配文件的内容也用于判断是否能复用上次输出。

### `outputs`

必填。路径相对于 `build.publish-dir`，例如 `{ file = "build.txt" }` 或 `{ tree = "assets/search" }`。实际生成的文件须与声明一致，路径不能在 `_tola` 下，也不能与其他输出冲突。

## 用法

### 读取页面和写入文件

- `hook.inputDir`：从这个只读目录读取已编译的页面和 `[assets]` 文件。
- `hook.outputDir`：把 `outputs` 声明的文件写到这里。目录每次从空开始，文件按同样的相对路径发布。
- `hook.cacheDir`：保存下次还能用的数据。即使用了缓存，仍要把成品写入 `hook.outputDir`。
- `hook.tempDir`：放本次的临时文件，命令结束后删除。
- `hook.outputDir`：每次都是空目录。把 `outputs` 声明的文件写到这里，Tola 按同样的相对路径发布。
同一阶段的 hook 读取相同的输入，各自生成自己的文件。一个步骤需要读取另一个步骤生成的文件时，把两步写在同一个脚本里。

在 Typst 页面中，从 `@tola/address:0.0.0` 导入 `output-url`，再写 `#link(output-url("build.txt"))[构建信息]`。Tola 会在 hook 运行后检查链接。输出文件如需压缩或 cache-busting，由脚本处理。

### 开发时复用文件

设 `dev = "reuse"` 后，站点内容、脚本及配置、监听的输入和输出声明都没变时，开发构建会用上次成功生成的文件。

把脚本额外读取的文件列进 `rerun-on`。脚本还依赖网络、时间、环境变量或站点外文件时，设 `dev = "run"`。生产构建每次都会运行已启用的 hook。用 `tola dev -v` 查看哪些输出路径变了。


要用内置 Nushell 生成 `public/build.txt`，见 [`@tola-scripts/context`](tola-help://scripts/context) 的完整示例。

示例：发布 HTML 文件清单：

把脚本存为站点根目录下的 `write-page-list.nu`：

```nu
use tola get-context

let hook = get-context | get hook
glob ($hook.inputDir | path join "**/*.html")
| each {|file| $file | path relative-to $hook.inputDir }
| sort
| to json
| save ($hook.outputDir | path join "pages.json")
```

把 hook 加到 `tola.toml`：

```toml
[[hooks.generate-outputs]]
name = "page-list"
nu = "write-page-list.nu"
outputs = [{ file = "pages.json" }]
```

在用 `tola init` 创建的站点中，给 `content/index.typ` 写一个标题，再从站点根目录运行 `tola build`。使用默认发布目录时，打开 `public/pages.json`，可以看到 `index.html` 等相对于发布目录的 HTML 文件名。

示例：用 Pagefind 生成搜索索引：

也可以换成其他搜索工具，读写目录和 `outputs` 的写法相同。

安装 Node.js 和 npm 后，把脚本存为站点根目录下的 `index-search.nu`：

```nu
use tola get-context

let hook = get-context | get hook
let index_dir = $hook.outputDir | path join "pagefind"
^npx -y pagefind --site $hook.inputDir --output-path $index_dir
```

在 `tola.toml` 中加入这个 hook。

```toml
[[hooks.generate-outputs]]
name = "search"
nu = "index-search.nu"
outputs = [{ tree = "pagefind" }]
```

在用 `tola init` 创建的站点中，把下面的代码加到 `content/index.typ`。它会加载 hook 生成的 [Pagefind 搜索组件](https://pagefind.app/docs/)。

```typst
#import "@tola/address:0.0.0": output-url
#import "@tola/site:0.0.0": site
#html.link(rel: "stylesheet", href: output-url("pagefind/pagefind-component-ui.css"))
#html.elem("script", attrs: (
  type: "module",
  src: output-url("pagefind/pagefind-component-ui.js"),
))[]
#html.elem("pagefind-config", attrs: ("base-url": site.base-path))[]
#html.elem("pagefind-modal-trigger", attrs: (placeholder: "搜索"))[]
#html.elem("pagefind-modal")[]
```

从站点根目录运行 `tola build`，默认会把搜索索引和组件写到 `public/pagefind/`。运行 `tola preview` 并打开打印的地址，点击首页的「搜索」，输入站内文章中出现过的词。
