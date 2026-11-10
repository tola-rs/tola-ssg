# `[hooks]`

Tola 在构建前后运行的命令，按阶段分组。每个阶段等自己的命令都退出后才继续。

## 用法

按命令产出什么来选择阶段：`before-build` 在发现源文件之前写站点输入；`generate-outputs` 在 Bundle 编译后添加最终文件；`after-publish` 消费已提交的站点。Bundle、配置资源与生成的 output 在发布前统一检查冲突与引用。

各命令运行的阶段不同。Hook 默认启用，可以设置 `enable = false`，也可以用 `enabled-by` 引用所属 options 表中的布尔键。

| 命令 | 构建模式 | before-build | generate-outputs | after-publish |
| --- | --- | --- | --- | --- |
| `tola build` | `prod` | 运行 | 运行 | 运行 |
| `tola check` | `prod` | 运行 | 运行 | 跳过 |
| `tola preview` | `prod` | 运行 | 运行 | 跳过 |
| `tola dev` | `dev` | 按 `dev` | 按 `dev` | 按 `dev` |
| `tola inspect` | — | 跳过 | 跳过 | 跳过 |
| `tola vendor` | `prod` | 跳过 | 跳过 | 跳过 |

一次 `tola build` 的顺序：`before-build` hook 先跑 → 按 `content-dir` 发现源、编译 Bundle、收集 `[assets]`（这份完整结果就是候选）→ `generate-outputs` hook 把生成文件并入候选 → 检查冲突与引用 → 发布：把整份候选写进 `build.publish-dir` → `after-publish` hook 消费已发布的站点。

构建与发布的区别：构建产出候选并接受检查，发布只做一件事——把检查过的结果整棵写进 `build.publish-dir`（成功构建会替换整棵目录）。前两个阶段都在发布之前，任何一步失败就不会发布，上次输出保持不动；`after-publish` 在发布之后，失败不会撤销发布。

`check` 与 `preview` 用生产设置从源构建，不写 `build.publish-dir`。`dev` 在内存中提供成功的构建。前两个阶段的 `dev` 字段默认 `run`，after-publish 默认 `skip`。

命令从站点根运行。用 `nu` 指定嵌入式 Nu 脚本，可另设 `args`；用 `argv` 指定外部命令及参数，不经过隐式 shell。同一声明者的同阶段条目按配置顺序执行，一条失败就停止剩余条目。相互依赖的步骤与条件放在站点自己的执行器里，例如 `argv = ["just", "css"]`；Tola 不要求使用 `just`。每个 output 生成命令读取同一份上游快照、写入私有目录：后面的生成命令看不到前一个刚写出的 output。

任何阶段的 hook 都可以通过 `TOLA_REPORT_PATH` 提交不带位置的完整诊断批次；Nu 脚本可用 `use tola diagnostic` 和 `diagnostic submit`。warning 不改变成功命令的结果；error 会使其失败。after-publish 的错误不能撤销发布。开发时复用 `generate-outputs` 的结果也会复用对应报告。未提交与提交空批次不同；此报告接口尚不支持独立检查或未保存的编辑器快照。

外部命令接收以下环境变量。嵌入式 Nu 使用 `get-site`、`get-module` 和 `get-hook` 读取本次调用的数据。

| 变量 | 用途 | 提供的阶段 |
| --- | --- | --- |
| `TOLA_REPORT_PATH` | 可选的完整诊断报告 | 全部 |
| `TOLA_PROJECT_ROOT` | 解析后的项目根目录绝对路径 | 全部 |
| `TOLA_CONFIG_PATH` | 解析后的配置文件绝对路径 | 全部 |
| `TOLA_CONTENT_ROOT` | 解析后的内容目录绝对路径 | 全部 |
| `TOLA_EXECUTABLE` | 当前 Tola 可执行文件 | 全部 |
| `TOLA_INPUT_SCOPE` | 调用方输入策略：`online`、`offline` 或 `pure` | 全部 |
| `TOLA_HOOK_STAGE` | 当前阶段：`before-build`、`generate-outputs` 或 `after-publish` | 全部 |
| `TOLA_BUILD_MODE` | 本次构建模式：`prod` 或 `dev` | 全部 |
| `TOLA_HOOK_CACHE_DIR` | `.tola/hooks/<阶段>/<条目名>`，跨次运行保留，例如 `.tola/hooks/generate-outputs/search` | 全部 |
| `TOLA_HOOK_TEMP_DIR` | 本次调用独有的临时目录，结束即删 | 全部 |
| `TOLA_HOOK_INPUT_DIR` | 命令的输入：上游已产出的整站的只读快照，布局与发布后的目录一致 | `generate-outputs`、`after-publish` |
| `TOLA_HOOK_OUTPUT_DIR` | 每次运行都是空目录；写进去的相对路径就是 `build.publish-dir` 下的相对路径 | `generate-outputs` |

缓存目录里的内容归命令所有，Tola 只创建、不清理，也不复用其中的结果；临时目录与输出目录每次运行都新建。`TMPDIR`、`TMP` 与 `TEMP` 指向 `TOLA_HOOK_TEMP_DIR`。阶段没有定义的变量不存在，也不会从调用者继承。

`rerun-on` 用 `{ include = ["data/**/*.json"], exclude = ["data/scratch"] }` 声明额外观察的输入。整个字段可以省略；写了就必须提供 `include`，`exclude` 默认为空。站点 hook 的字符串和 `{ project = "..." }` 模式都相对于站点根。目录匹配覆盖后代，`*` 不跨目录，`**` 可以跨目录，排除优先。glob 可以没有匹配，字面路径也可在启动后创建。`nu` 脚本本身独立观察。所选输入变化会触发一次完整开发构建。可在这里补充工具读取的输入目录、执行器、manifest 或 lockfile。

hook 是以调用者权限运行的受信任脚本。`--offline` 与 `--pure` 限制 Tola 自己读取输入，不限制这些命令。被取代的开发构建会取消自己发布前的命令，但脚本副作用不会回滚。after-publish 的开发调度见该阶段页。
