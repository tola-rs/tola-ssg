# `[[hooks.before-build]]`

在 Tola 开始查找内容文件前运行。可以生成 Typst 页面、模板、数据或静态文件。

## 配置项

### `enable`

默认启用。设为 `false` 跳过这个 hook。

### `name`

状态和错误消息里显示的名称。同一阶段内不能重名，名称不能为空或包含空白。

### `order`

非负整数，省略时为 `0`。同一站点或模块的 `before-build` hook 按此数值从小到大执行。同序号的 hook 可以并行。

### `argv`

从站点根目录运行的命令及参数。`argv` 和 `nu` 二选一。需要 shell 时，在 `argv` 里指定。将生成的文件写到站点会读取的位置。

### `nu`

要运行的 Nu 脚本，路径相对于站点根目录。`nu` 和 `argv` 二选一。

### `args`

传给 `nu` 脚本的参数，可省略。

### `enabled-by`

例如填 `"images.enable"`，只在 `[options.images]` 的 `enable` 为 `true` 时运行。省略时不检查选项。

### `dev`

省略或设为 `"run"` 时，`tola dev` 每次构建都运行。设为 `"reuse"` 时，可以沿用 `generates` 中未变化的文件。设为 `"skip"` 时，开发构建会跳过。

### `rerun-on`

让额外输入的变化触发重建，例如 `{ include = ["data/**/*.json"] }`。指定后须填 `include`，可省略 `exclude`。Tola 已自动监听 `nu` 指定的入口脚本。

### `generates`

生成的文件或目录，路径相对于站点根。Tola 在命令结束后检查这些路径。路径必须位于 `build.publish-dir` 和 `.tola` 之外。

## 用法

### 生成其他输入

把模板、数据或静态文件写到它们的源路径。例如，生成 `static/site.css` 后用 `[assets]` 发布。用 `generates` 列出生成的文件或目录。Tola 在命令结束后检查这些路径，开发时也会监听它们。

把下次还能用的文件放进 `hook.cacheDir`，只用一次的文件放进 `hook.tempDir`。需要参与构建的文件直接写到站点读取它们的位置。

### 在开发时复用生成的输入

在站点或模块的 hook 中设 `dev = "reuse"`，并在 `generates` 中列出每个生成的文件或目录。在同一次 `tola dev` 会话中，如果命令、配置、监听的输入和生成的文件都与某次成功运行一致，Tola 可以跳过该命令并沿用上次的诊断。声明的输入出现、消失或发生变化，或生成的文件缺失或发生变化时，Tola 会重跑命令。

把命令额外读取的文件都列入 `rerun-on`，包括导入的脚本、工具文件及其配置、较早执行的 hook 生成的文件。Tola 会自动追踪 Nu 入口脚本。如果命令不读其他文件，可以省略 `rerun-on`。如果结果依赖时间、网络、环境变量、系统工具等无法追踪的状态，或命令每次构建都需要产生副作用，使用 `dev = "run"`。

用 `tola dev --no-reuse` 可以在开发时强制运行这些命令。生产构建每次都会运行已启用的 hook。

示例：生成 `content/releases.typ`：

在用 `tola init` 创建的站点根目录，把下面的代码存为 `write-release-notes.nu`。脚本从上下文读取站点配置的内容目录。

```nu
use tola get-context

let content = get-context | get site.contentRoot
"= Release notes\n" | save --force ($content | path join "releases.typ")
```

在 `tola.toml` 中加入以下配置。

```toml
[[hooks.before-build]]
name = "release-notes"
nu = "write-release-notes.nu"
dev = "reuse"
generates = ["content/releases.typ"]
```

从站点根目录运行 `tola build`。hook 先写入 `content/releases.typ`，默认的 `site.typ` 再把它发布到 `public/releases/index.html`。如果改过 `build.content-dir`，也要把 `generates` 中的路径改成对应位置。

示例：安排不同配置文件里的 hook 先后运行：

在用 `tola init` 创建的站点根目录下建好 `config/`、`data/` 和 `tools/`。把下面的内容存为 `data/release.json`：

```json
{"title": "Atlas 2.0"}
```

把数据脚本存为 `tools/read-release.nu`：

```nu
mkdir generated
open data/release.json | get title | save --force generated/release.txt
```

把生成页面的脚本存为 `tools/write-upcoming.nu`：

```nu
let title = open --raw generated/release.txt
$"= ($title)\n" | save --force content/upcoming.typ
```

把这一行加到 `tola.toml` 的最前面：

```toml
include = ["config/release.toml"]
```

将页面 hook 存为 `config/release.toml`：

```toml
[[hooks.before-build]]
name = "upcoming-page"
order = 10
nu = "tools/write-upcoming.nu"
generates = ["content/upcoming.typ"]
```

把数据 hook 加到 `tola.toml`：

```toml
[[hooks.before-build]]
name = "release-data"
order = 0
nu = "tools/read-release.nu"
rerun-on = { include = ["data/release.json"] }
generates = ["generated/release.txt"]
```

运行 `tola build`。`release-data` 的 `order` 更小，所以它先运行，即使 Tola 先读到 `config/release.toml`。按默认设置，打开 `public/upcoming/index.html` 可以看到 Atlas 2.0。用 `tola dev` 时，修改 `data/release.json` 也会重建这页。`tools/write-upcoming.nu` 仍从站点根目录找。
