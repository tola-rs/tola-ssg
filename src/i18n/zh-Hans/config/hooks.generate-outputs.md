# `[[hooks.generate-outputs]]`

在站点程序编译完成后产出声明的最终输出文件，并在引用检查之前把它们并入候选输出图。它们对之前的 Typst 编译不可见，但页面可以链接它们已知的 URL，无需 `[assets]` 声明，引用检查也会校验这些链接。

## 配置项

### `enable`

是否运行该 hook。

### `name`

该 hook 的名字，出现在状态行与诊断里。必须是一个不含空白的词，且在本阶段内唯一：为空、含空白、或在本阶段内重名都会报错。

### `argv`

运行的命令，从站点根执行，不经过隐式 shell。命令读 `TOLA_HOOK_INPUT_DIR` 下的上游快照，把声明的输出写到 `TOLA_HOOK_OUTPUT_DIR` 下；写出的路径相对站点最终输出。

### `nu`

嵌入式 Nu 脚本，相对于站点根。

### `args`

传给 Nu 脚本的字面参数。

### `enabled-by`

启用该 hook 的站点顶层布尔选项。

### `dev`

`tola dev` 是否运行该 hook：`"run"`（默认）或 `"skip"`。

### `rerun-on`

额外观察的站点相对模式，使用必填的 `include` 与可选的 `exclude`。所选输入变化触发一次完整的 `tola dev` 构建。

### `outputs`

该 hook 并入候选的最终站点输出路径，写成以 `build.publish-dir`（默认 `public`）为根的相对路径：`{ file = "manifest.json" }` 或 `{ tree = "assets/thumbnails" }`。必填：实际写出的文件必须与这些声明一致，每条路径都要在 `_tola` 之外，而别的生产者已拥有的路径会让构建失败，而不是覆盖那条输出。

## 用法

这个阶段从编译结果生成最终文件。

下面用 Pagefind 举一个完整例子：它读取 HTML 快照，写出的搜索索引与它索引的页面进入同一份 output。

`justfile`：

```just
search:
    deno task search
```

`deno.json`：

```json
{
  "imports": { "pagefind": "npm:pagefind@1.5.2" },
  "tasks": {
    "search": "deno run -A npm:pagefind@1.5.2 --site \"$TOLA_HOOK_INPUT_DIR\" --output-path \"$TOLA_HOOK_OUTPUT_DIR/assets/pagefind-search\""
  }
}
```

`tola.toml`：

```toml
[[hooks.generate-outputs]]
name = "search"
argv = ["just", "search"]
outputs = [{ tree = "assets/pagefind-search" }]
rerun-on = { include = ["deno.json", "deno.lock", "justfile"] }
```

每个生成命令读的是同一份编译结果；几步转换相互依赖时，把它们放进同一个命令里。

与所有 hook 阶段一样，这个阶段只跑单次命令：Tola 不做 watch 这类长驻运行，开发监听由 Tola 负责；命令自己的增量编译与缓存由生产者自己编写，可用 `TOLA_HOOK_CACHE_DIR`。

页面这样用它：模板的 head 里直接链接生成的 CSS 与 JS：

```typst
#import "@tola/address:0.0.0": output-url

#html.link(rel: "stylesheet", href: output-url("assets/pagefind-search/pagefind-ui.css"))
#html.elem("script", attrs: (src: output-url("assets/pagefind-search/pagefind-ui.js")))
```

声明的 output 无需 `[assets]` 条目。`asset-url` 只解析 `[assets]` 声明，并校验该文件确实已发布，否则报错。用 `output-url("assets/pagefind-search/pagefind-ui.css")` 链接已知 output 路径。这些文件在之前的 Typst 编译时尚不存在，因此 Tola 当时无法计算它们的资源 URL。Tola 不会为 hook 的产物做压缩、cache-busting 等，全部由生产者/命令自己解决：例如 Pagefind 会给自己的索引与片段文件起带哈希的名字。

本阶段的命令环境：
- `TOLA_HOOK_INPUT_DIR`：命令的输入——上游已产出的整站的只读快照：页面与 `[assets]` 文件都在、布局与发布后的目录一致、文件不可写；本阶段其它 hook 刚写出的 output 不在里面，它也不是 `build.content-dir` 或 `build.publish-dir`，只在这条命令运行期间存在。例子里的 `--site "$TOLA_HOOK_INPUT_DIR"` 就是读它。
- `TOLA_HOOK_OUTPUT_DIR`：每次运行都是空目录；写进去的相对路径就是 `build.publish-dir` 下的相对路径——写到 `"$TOLA_HOOK_OUTPUT_DIR"/assets/pagefind-search` 会发布成 `assets/pagefind-search`，URL 是 `/assets/pagefind-search/…`。
- `TOLA_HOOK_STAGE`、`TOLA_BUILD_MODE`、`TOLA_HOOK_CACHE_DIR`、`TOLA_HOOK_TEMP_DIR`：与其它阶段相同——阶段名、模式（`prod` 或 `dev`）、跨次缓存目录（例如 `"$TOLA_HOOK_CACHE_DIR"/search/index.json`）、本次临时目录。命中缓存也要重新写出声明的 output。
