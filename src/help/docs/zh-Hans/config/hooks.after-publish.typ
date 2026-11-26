#show: config(
  summary: [在站点发布完成后执行命令，例如部署上传或产物验收检查。],
  related: ("[hooks]", "@tola-scripts/context", "@tola-scripts/diagnostic"),
)

从只读目录 `hook.inputDir` 读取本次发布的站点文件。由于发布已经完成，此阶段报错会返回构建失败，但本地已发布的文件保持不变。若要为站点新增发布文件，请使用 #ref("[[hooks.generate-outputs]]")。

`tola dev` 默认跳过该阶段。设为 `dev = "run"` 时，每次开发构建成功后会在后台运行；若执行期间发生多次重建，结束后仅处理最新一次结果。

#example(title: [使用 rsync 上传已发布站点])[
  ```toml
  [[hooks.after-publish]]
  name = "upload"
  nu = "tools/upload.nu"
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
  相对于站点根目录的 Nu 脚本路径，与 `argv` 二选一。
]

#key("args")[
  传给 `nu` 脚本的可选参数列表。
]

#key("enabled-by")[
  指向 `[options]` 下布尔值的键路径，决定该 hook 是否运行。
]

#key("dev")[
  `tola dev` 中的行为：`"skip"`（默认）跳过，`"run"` 在每次开发构建成功后于后台运行。
]

#key("rerun-on")[
  额外监听的输入文件（`{ include = [...], exclude = [...] }`），变化时在 `tola dev` 中触发重建并重新执行已启用的发布后 hook。
]
