#show: script(
  summary: [在构建 hook、模块命令或初始化脚本中读取当前运行的路径、阶段与选项上下文。],
)

#command(
  "main",
  summary: [返回由 Tola 注入的当前执行上下文。],
  params: (
    "--json": [以 JSON 文本输出，便于传递给外部程序；省略时返回或展示 Nu 记录。],
  ),
)[
  仅在 Tola 启动的构建 hook、模块命令或初始化脚本中可用；在终端中直接调用会报错。临时目录和 `reportPath` 仅在本次调用期间有效。

  #table(
    columns: 2,
    table.header([字段], [适用范围与含义]),
    [`executable`], [当前运行的 `tola` 可执行文件路径。],
    [`inputScope`], [当前输入策略：`online`、`offline` 或 `pure`。],
    [`site.root`、`site.configPath`、`site.contentRoot`], [站点根目录、`tola.toml` 路径与内容目录（构建 hook 与模块命令）。`before-build` hook 可在 `site.contentRoot` 下写入供 Typst 读取的内容。],
    [`module.name`、`module.root`、`module.manifest`], [模块别名、根目录与 `tola-module.toml` 路径（模块 hook、命令与初始化器）。],
    [`module.options`], [站点为该模块解析后的选项表（模块 hook 与命令）。],
    [`hook.name`、`hook.stage`、`hook.tempDir`], [hook 名称、执行阶段及本次运行的临时目录（构建 hook 与 `before-pack`）。],
    [`hook.mode`、`hook.cacheDir`], [构建模式（`prod` 或 `dev`）及跨运行保留的缓存目录（构建 hook）。],
    [`hook.inputDir`], [`generate-outputs` 中指向已编译页面与资源目录，`after-publish` 中指向已发布目录，`before-pack` 中指向模块源码目录。],
    [`hook.outputDir`], [`generate-outputs` 阶段写入 `outputs` 声明文件的暂存目录。],
    [`init.outputDir`、`init.targetDir`、`init.interactive`], [初始化器的文件输出目录、用于计算相对路径的最终站点目录，以及是否允许终端交互提示。],
    [`reportPath`], [外部 hook 或模块命令退出前可写入 JSON 诊断报告的文件路径。],
  )

  #example(title: [在 Nu hook 中读取上下文])[
    读取仅在特定阶段存在的字段（如 `hook.outputDir`）时，用 `get -o` 安全判空。
    ```nu
    use tola get-context

    let ctx = get-context
    let out = $ctx | get -o hook.outputDir
    if $out != null {
      "Built with Tola\n" | save ($out | path join "build.txt")
    }
    ```
  ]

  #example(title: [在外部 shell hook 中获取 JSON 上下文])[
    ```sh
    "$TOLA_EXECUTABLE" nu @tola-scripts/context --json
    ```
  ]
]
