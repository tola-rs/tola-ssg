# `[hooks]`

生成构建需要的文件、添加发布文件，或在发布后运行命令。

## 用法

### 选择阶段

| 阶段 | 用途 |
| --- | --- |
| `before-build` | 在发现源文件之前生成 Typst 页面、模板、数据或静态文件。 |
| `generate-outputs` | 编译后添加输出文件，例如搜索索引。 |
| `after-publish` | 读取已发布的文件，例如上传站点。 |

### 选择执行方式

用 `nu` 指定由内置 Nushell 运行的脚本，参数写在 `args` 中。用 `argv` 指定外部程序及其参数。站点 hook 从站点根目录运行。

### 控制执行顺序

在同一阶段、同一站点或模块内，`order` 小的先运行。省略时为 `0`，同序号的 hook 可以并行。用 [`include`](tola-help://config/include) 拆到多个文件里的站点 hook 也一起排序。

模块按别名排序。`before-build` 先跑站点 hook，再跑模块 hook。后两个阶段先跑模块 hook。某组失败后，后面的组会跳过。所有 `generate-outputs` hook 都读取同一份编译结果。要让后一步读取前一步生成的文件，把两步放进同一个脚本。

在 `[build]` 中用 `hook-jobs` 限制并行数，或用 `par-hooks = false` 逐个运行。

### 查看哪些 hook 会运行

运行 `tola inspect config --mode dev --json --pretty`。`hooks.stages` 下的 `declarations` 列出每个 hook 来自哪里，以及跳过的原因。`batches` 列出哪些 hook 会一起运行。用 `--mode prod` 查看生产构建的顺序。这条命令只读配置。

### 哪些命令会运行 hook

hook 默认开启。设 `enable = false` 可直接关闭。用 `enabled-by = "images.enable"` 时，对应选项为 `true` 才运行。键名本身含点号时，写成 `enabled-by = '"images.enable"'`。缺键、途中遇到非表值或最终值不是布尔值都会报错。`enable = false` 时不查该选项。

| 命令 | 构建模式 | before-build | generate-outputs | after-publish |
| --- | --- | --- | --- | --- |
| `tola build` | `prod` | 运行 | 运行 | 运行 |
| `tola check` | `prod` | 运行 | 运行 | 跳过 |
| `tola preview` | `prod` | 运行 | 运行 | 跳过 |
| `tola dev` | `dev` | 按 `dev` | 按 `dev` | 按 `dev` |
| `tola inspect` | — | 跳过 | 跳过 | 跳过 |
| `tola vendor` | `prod` | 跳过 | 跳过 | 跳过 |

`tola check --no-hooks` 跳过两个构建阶段的 hook。`tola dev` 默认运行前两个阶段，跳过 `after-publish`。见 `[[hooks.generate-outputs]]` 的 `dev = "reuse"`。

发布前失败时保留上次发布的文件。`after-publish` 报错时，新文件已经写入发布目录。

### 报告问题和查路径

在 Nu 中导入 `use tola diagnostic`，再用 `$issues | diagnostic submit` 一次提交所有问题，没有问题可传 `[]`。普通 hook 的报告省略 `location`。警告会显示在终端，错误会使 hook 失败。外部程序可把报告写到 `reportPath`。示例见 [`@tola-scripts/diagnostic`](tola-help://scripts/diagnostic)。

`tola dev` 复用 `generate-outputs` 写出的文件时，也会复用上次的诊断。

在 Nu 脚本中用 `use tola get-context` 导入，再调用 `get-context | get hook.stage` 读取当前阶段。外部命令可以运行 `"$TOLA_EXECUTABLE" nu @tola-scripts/context --json`。字段和示例见 [`@tola-scripts/context`](tola-help://scripts/context)。

脚本自行管理缓存文件。每次运行 hook 都有新的临时目录。`TMPDIR`、`TMP` 与 `TEMP` 指向 `hook.tempDir`。

### 监听脚本读取的文件

`nu` 指定的入口会自动监听。脚本还读 JSON、模板或配置时，用 `rerun-on` 加入这些文件。

写了 `rerun-on` 就要提供 `include`，`exclude` 可省略。站点 hook 的模式从站点根目录匹配。模块 hook 中的字符串从模块根目录匹配，要监听使用该模块的站点文件时，写 `{ project = "content/**/*.typ" }`。

匹配到目录时，包含其下所有文件。`*` 只匹配一层，`**` 可以跨目录。排除规则优先。匹配不到文件也可以启动，文件创建后照样会触发构建。

hook 以你的权限运行。`--offline` 和 `--pure` 限制 Tola 自己读哪些文件和下载哪些依赖，脚本仍可访问文件和网络。取消开发构建时，发布前的脚本也会停止。脚本已经写出的文件或做出的其他修改会保留。

各阶段的例子：

- [跨配置文件按顺序执行 hook](tola-help://config/hooks/before-build)
- [为站点添加文件清单或搜索索引](tola-help://config/hooks/generate-outputs)
- [上传站点或检查首页](tola-help://config/hooks/after-publish)
