#show: config(
  summary: [在编译完成后读取页面与资源，生成额外的发布文件（如搜索索引或清单）。],
  related: ("[hooks]", "@tola-scripts/context", "@tola/address#output-url"),
)

从只读目录 `hook.inputDir` 读取已编译页面与静态资源，将 `outputs` 中声明的文件写入 `hook.outputDir`（每次运行均为空目录）。在 Typst 页面中通过 #ref("@tola/address#output-url") 链接这些输出，Tola 会在 hook 结束后统一校验链接。

配置 `dev = "reuse"` 后，若站点内容、hook 配置与 `rerun-on` 输入未变化，`tola dev` 会复用上次成功生成的文件。若脚本依赖时间、网络或外部环境，请使用 `dev = "run"`。

#example(title: [生成 Pagefind 搜索索引])[
  ```toml
  [[hooks.generate-outputs]]
  name = "search"
  nu = "tools/index-search.nu"
  dev = "reuse"
  outputs = [{ tree = "pagefind" }]
  ```
]

#key("enable")[
  是否启用该 hook。设为 `false` 时直接跳过，不检查 `enabled-by`。
]

#key("name")[
  同一阶段内唯一的非空名称（不含空白）。
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
  指向 `[options]` 下布尔值的键路径，决定该 hook 是否运行。
]

#key("dev")[
  `tola dev` 中的行为：`"run"` 每次构建都运行，`"reuse"` 在输入未变化时复用上次产物，`"skip"` 跳过。
]

#key("rerun-on")[
  额外监听的输入文件（`{ include = [...], exclude = [...] }`），变化时触发重建并使 `dev = "reuse"` 失效。
]

#key("outputs")[
  必填。相对于 `build.publish-dir` 的输出声明列表，每项为 `{ file = "路径" }` 或 `{ tree = "目录" }`。实际产出须与声明一致，不得位于 `_tola` 下，且不得与其他输出冲突。
]
