# `[[hooks.before-build]]`

在发现源文件之前运行，先把站点随后要读的文件写好：Typst 的输入、`[assets]` 声明的文件。

## 配置项

### `enable`

是否运行该 hook。

### `name`

该 hook 的名字，出现在状态行与诊断里。必须是一个不含空白的词，且在本阶段内唯一：为空、含空白、或在本阶段内重名都会报错。

### `argv`

运行的命令，从站点根执行，不经过隐式 shell。这个阶段不设置 `TOLA_HOOK_INPUT_DIR`、`TOLA_HOOK_OUTPUT_DIR`（`generate-outputs` 才有）：命令直接生成站点要读的输入，没有构建目录可读。

### `nu`

嵌入式 Nu 脚本，相对于站点根。

### `args`

传给 Nu 脚本的字面参数。

### `enabled-by`

启用该 hook 的站点顶层布尔选项。

### `dev`

`tola dev` 是否运行这一条：`"run"`（默认）或 `"skip"`。

### `rerun-on`

额外观察的站点相对模式，使用必填的 `include` 与可选的 `exclude`。所选输入变化触发一次完整的 `tola dev` 构建。

### `generates`

该 hook 执行命令后生成的文件或目录，相对站点根；不可位于 `build.publish-dir` 与 `.tola` 中。Tola 在 hook 的命令执行后，会检查这些声明的路径是否存在——这是 hook 需要显式传给 Tola 的信息：Tola 不打算推断命令本身生成了哪些内容。声明它并不会发布它：要发布，就用 `[assets]` 或 Bundle 的 `asset(...)`；也可以让 Typst 读它，产出文档或其他 output。

## 用法

这个阶段生成 Typst 与 `[assets]` 随后要读的文件。

下面用 Tailwind CSS 举一个完整例子：`just` 跑 task，Deno 执行 `@tailwindcss/cli` 生成样式表；`before-build` hook 在发现源文件之前运行它，`[assets]` 把 `static/web-assets/tailwind-output` 映射到 `/assets/tailwind-output`。处理前与处理后的资产在路径上分开：输入是 `static/tailwind-sources/site.css`（不发布），产物是 `static/web-assets/tailwind-output/site.css`（随树发布，URL 为 `/assets/tailwind-output/site.css`）。

`justfile`：

```just
css:
    deno task css
```

`deno.json`：

```json
{
  "imports": {
    "@tailwindcss/cli": "npm:@tailwindcss/cli@4.3.3",
    "tailwindcss": "npm:tailwindcss@4.3.3"
  },
  "tasks": {
    "css": "deno run -A npm:@tailwindcss/cli@4.3.3 -i static/tailwind-sources/site.css -o static/web-assets/tailwind-output/site.css --minify"
  }
}
```

`tola.toml`：

```toml
[[hooks.before-build]]
name = "tailwind"
argv = ["just", "css"]
generates = ["static/web-assets/tailwind-output/site.css"]
rerun-on = { include = ["static/tailwind-sources", "deno.json", "deno.lock", "justfile"] }
```

文件要让浏览器读到，得先发布出去：下面的 `[assets]` 把 `static/web-assets` 整棵树映射到 `/assets`，产物因此随树发布在 `/assets/tailwind-output/site.css`。生成的数据也可以不发布，只给 Typst 读：页面 import 它，用来产出文档或其他 output。

`tola.toml`：

```toml
[assets]
trees = [{ source = "static/web-assets", url-prefix = "/assets" }]
```

页面模板的 head 里链接它（`site.typ` 或页面模板）：

```typst
#html.link(rel: "stylesheet", href: asset-url("/assets/tailwind-output/site.css"))
```

Tailwind 的输入 CSS 要写明扫描哪些内容与模板文件。hooks 面向单次运行的命令：Tola 目前没有支持 watch 这类长驻命令的打算——那会给开发、构建流程、hook 机制与分析带来极大的复杂性。开发监听由 Tola 负责，hooks 机制只提供必要的基础设施：命令自己的增量编译、缓存与持久化由生产者自己编写，`TOLA_HOOK_CACHE_DIR`、`TOLA_HOOK_TEMP_DIR` 等环境变量是 Tola 提供的接口。

`generates` 声明命令会写出哪些文件：Tola 在命令结束后检查这些路径是否存在，开发期间生成路径会从触发该生产者的监听输入中排除。站点自身的 hook 可以在站点内本地模块的子路径生成文件；模块根目录、清单与远程或 vendored 输入受保护。

它们是站点的输入，不是输出：等本阶段的 hook 全部跑完，Tola 才开始构建——按 `content-dir` 发现源、编译 Bundle、收集 `[assets]` 声明的文件，然后检查引用、发布。

本阶段的命令环境：
- `TOLA_HOOK_STAGE`：每次调用都是 `before-build`。
- `TOLA_BUILD_MODE`：`prod`（`build`、`check`、`preview`、`vendor`）或 `dev`（`tola dev`）。
- `TOLA_HOOK_CACHE_DIR`：跨次运行保留在 `.tola/hooks/before-build/<条目名>`；内容归命令、Tola 只创建目录——增量产物放这里，例如 `"$TOLA_HOOK_CACHE_DIR"/tailwind`。
- `TOLA_HOOK_TEMP_DIR`：本次调用独有的临时目录，结束即删；`TMPDIR`、`TMP`、`TEMP` 也指向它。

没有 `TOLA_HOOK_INPUT_DIR` 与 `TOLA_HOOK_OUTPUT_DIR`：命令直接写站点根下的输入文件。
