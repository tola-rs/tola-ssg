# `@tola-scripts/context`

在 hook、模块命令或初始化脚本中查看站点路径、模块选项和临时目录。

## `main`

从 Tola 启动的 hook、模块命令或初始化脚本里运行。普通终端没有这些信息，直接运行会报错。

#### 上下文字段

临时目录和报告路径只在本次调用期间有效。

| 字段 | 用途 |
| --- | --- |
| `executable` | 当前 Tola 可执行文件的路径。 |
| `inputScope` | 构建 hook、模块命令和初始化脚本会收到 `online`、`offline` 或 `pure`。 |
| `site.root`、`site.configPath`、`site.contentRoot` | 构建 hook 和模块命令可读取站点根目录、配置文件和内容目录。`before-build` hook 可把待编译的内容写到 `site.contentRoot` 下。 |
| `module.name`、`module.root`、`module.manifest` | 模块的构建 hook、模块命令和初始化脚本可读取模块别名、根目录和清单路径。 |
| `module.options` | 模块 hook 和命令可读取站点实际使用的模块选项。 |
| `hook.name`、`hook.stage`、`hook.tempDir` | 构建 hook 和 `before-pack` 可读取配置里的名称、执行阶段和本次的临时目录。 |
| `hook.mode`、`hook.cacheDir` | 构建 hook 可读取构建模式（`prod` 或 `dev`）和跨次运行保留的缓存目录。 |
| `hook.inputDir` | `generate-outputs` 从这里读取编译好的页面和静态文件，`after-publish` 读取已发布的站点，`before-pack` 读取模块源目录。 |
| `hook.outputDir` | `generate-outputs` 把文件写到这里，使用 `outputs` 声明的相对路径。 |
| `init.outputDir`、`init.targetDir`、`init.interactive` | 初始化脚本把文件写到 `outputDir`。配置里的相对路径以最终站点目录 `targetDir` 为准。`interactive` 为真时才提示用户输入。 |
| `reportPath` | 外部构建 hook、`before-pack` hook 和模块命令可以在退出前把 JSON 诊断报告写到这个路径。 |

示例：在 Nu 脚本里读取 hook 名称：

用 `get-context` 读取记录，再用 `get` 取出字段：

```nu
use tola get-context
print (get-context | get hook.name)
```

要用 JSON 文本时，可写 `get-context --json`。比如把结果交给接收 JSON 的外部程序。

示例：从外部命令读取：

在 Shell 脚本里可用 `$TOLA_EXECUTABLE` 调用 Tola：

```sh
"$TOLA_EXECUTABLE" nu @tola-scripts/context --json
```

想直接在终端看结果，可以省略 `--json`。

示例：用 hook 发布文件：

脚本在 `hook.outputDir` 写入 `build.txt`，Tola 会按 `outputs` 里的路径发布。

将下面的脚本保存为 `write-build-info.nu`，与 `tola.toml` 放在一起：

```nu
use tola get-context
let output_dir = get-context | get hook.outputDir
"Built with Tola\n" | save ($output_dir | path join "build.txt")
```

在 `tola.toml` 中使用这段配置：

```toml
[[hooks.generate-outputs]]
name = "build-info"
nu = "write-build-info.nu"
outputs = [{ file = "build.txt" }]
```

从站点根目录运行 `tola build`。使用默认发布目录时，`public/build.txt` 的内容是 `Built with Tola`。

示例：读取可能不存在的字段：

只有 `generate-outputs` 提供 `hook.outputDir`。如果同一个脚本也在其他阶段运行，用 `get -o` 读取，字段不存在时返回 `null`：

```nu
use tola get-context
let out = get-context | get -o hook.outputDir
if $out != null {
  print $out
}
```

### 选项

- `--json`：输出 JSON。省略时在终端显示记录，在 Nu 脚本里返回可用 `get` 读取的记录。
