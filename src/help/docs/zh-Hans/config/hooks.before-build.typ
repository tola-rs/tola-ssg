#show: config(
  summary: [在内容发现前运行命令，用于生成 Typst 源文件、数据或静态输入。],
  related: ("[hooks]", "@tola-scripts/context", "[assets]"),
)

将生成的文件直接写入站点读取的源路径（例如 `content/` 或 `static/`），并在 `generates` 中声明每个生成的文件或目录。Tola 会在命令结束后检查这些路径，并在 `tola dev` 中自动监听。

同时配置 `dev = "reuse"` 与 `generates` 时，在同一次 `tola dev` 会话中，只要命令、配置、`rerun-on` 输入及生成产物均未变化，Tola 会直接跳过重跑并沿用上次诊断。请将脚本额外读取的文件全部列入 `rerun-on`；若结果依赖时间、网络、环境变量或外部工具状态，请使用 `dev = "run"`。运行 `tola dev --no-reuse` 可强制重跑。

#example(title: [在发现内容前生成 Typst 页面])[
  ```toml
  [[hooks.before-build]]
  name = "release-notes"
  nu = "tools/write-release-notes.nu"
  dev = "reuse"
  rerun-on = { include = ["data/releases.json"] }
  generates = ["content/releases.typ"]
  ```
]

#key("enable")[
  是否启用该 hook。设为 `false` 时直接跳过，不检查 `enabled-by`。
]

#key("name")[
  同一阶段内唯一的非空名称（不含空白），显示在状态输出与诊断中。
]

#key("order")[
  同一站点或模块实例内的非负执行顺序，数值小的先运行，相同数值可并行。
]

#key("argv")[
  在站点根目录执行的外部命令及参数，与 `nu` 二选一。
]

#key("nu")[
  相对于站点根目录的 Nu 脚本路径，`tola dev` 自动监听该文件；与 `argv` 二选一。
]

#key("args")[
  传给 `nu` 脚本的可选参数列表。
]

#key("enabled-by")[
  指向 `[options]` 下布尔值的点分键路径（如 `"images.enable"`）；键名含点号时给该段加引号。
]

#key("dev")[
  `tola dev` 中的行为：`"run"` 每次构建都运行，`"reuse"` 在输入与产物未变化时复用，`"skip"` 跳过。
]

#key("rerun-on")[
  额外监听的输入文件：`{ include = [...], exclude = [...] }`（`exclude` 可省略），变化时触发重建并使 `dev = "reuse"` 失效。
]

#key("generates")[
  该 hook 生成的站点相对路径（文件或目录），必须位于 `build.publish-dir` 和 `.tola` 之外。
]
