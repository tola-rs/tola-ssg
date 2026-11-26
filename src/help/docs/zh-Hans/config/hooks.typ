#show: config(
  summary: [在内容发现前、编译后或发布后运行 Nu 脚本或外部命令。],
  related: ("@tola-scripts/context", "@tola-scripts/diagnostic", "[build]"),
)

= 三个阶段

#table(
  columns: 2,
  table.header([阶段], [用途]),
  [#ref("[[hooks.before-build]]")], [在内容发现前生成 Typst 页面、模板、数据或静态文件。],
  [#ref("[[hooks.generate-outputs]]")], [基于已编译站点生成额外发布文件（例如搜索索引）。],
  [#ref("[[hooks.after-publish]]")], [在站点发布完成后读取已发布目录（例如上传或验收检查）。],
)

= 执行顺序与并发

每个 hook 通过 `nu`（内置 Nushell 脚本，配可选 `args`）或 `argv`（外部可执行程序及参数）执行，工作目录为站点根目录。

- 同一阶段、同一站点或模块实例内，按 `order` 从小到大执行（默认 `0`）。同序号的 hook 可并行运行；并发策略由 #ref("[build]") 中的 `par-hooks` 与 `hook-jobs` 控制。
- 跨实例时，模块按别名字典序排列。`before-build` 先跑站点 hook 再跑模块 hook；后两个阶段先跑模块 hook。任一批次失败会跳过后续批次。
- 所有 `generate-outputs` hook 读取同一份编译快照；若后一步依赖前一步产物，请合并到同一个脚本中。

= 各命令的执行矩阵

#table(
  columns: 5,
  table.header([命令], [模式], [`before-build`], [`generate-outputs`], [`after-publish`]),
  [`tola build`], [`prod`], [运行], [运行], [运行],
  [`tola check`], [`prod`], [运行], [运行], [跳过],
  [`tola preview`], [`prod`], [运行], [运行], [跳过],
  [`tola dev`], [`dev`], [按 `dev`], [按 `dev`], [按 `dev`],
  [`tola inspect` / `vendor`], [—], [跳过], [跳过], [跳过],
)

`tola check --no-hooks` 会跳过构建阶段 hook 并直接复用已有生成文件。运行 `tola inspect config --mode prod --json --pretty`（或 `--mode dev`）可查看解析后的执行批次与跳过原因。

= 上下文、诊断与文件监听

- 在 Nu 中用 `use tola get-context` 或在外部命令中调用 #ref("@tola-scripts/context") 读取路径与阶段信息。每次运行都会分配新的临时目录 `hook.tempDir`（同时设置 `TMPDIR`/`TMP`/`TEMP`）；跨次保留的缓存写入 `hook.cacheDir`。
- 在 Nu 中用 `use tola diagnostic`（`diagnostic submit`）提交错误或警告，外部命令可将 JSON 报告写入 `reportPath`（见 #ref("@tola-scripts/diagnostic")）。
- Tola 自动监听 `nu` 入口脚本；其他输入文件通过 `rerun-on = { include = [...], exclude = [...] }` 声明。Nu hook 可用 `use tola get-hook; get-hook --rerun-on-digest` 读取监听输入的内容摘要作为缓存键。
