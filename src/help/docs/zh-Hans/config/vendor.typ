#show: config(
  summary: [保存模块、Typst 包与远程图标集的本地副本，用于离线与 `--pure` 构建。],
  related: ("[modules]", "[icons]", "[typst.fonts]"),
)

运行 `tola vendor` 准备依赖副本，再用 `tola build --pure` 验证可脱离网络与全局缓存完成构建。请将 vendor 目录与 `tola.lock` 随站点一并提交。

替换现有目录前，`tola vendor` 会先校验以下三个子目录：
- `modules/`：按别名保存锁定的远程模块。
- `typst-packages/`：按命名空间、包名与版本保存 Typst 包。
- `icons/`：为每个远程图标集保存一份 JSON 文件。

`tola vendor` 不复制编译字体，请将项目字体放在 #ref("[typst.fonts]") 指定的仓库目录内。可用 `tola vendor --dry-run` 仅校验不写入，或用 `tola vendor --refresh` 从声明来源重新拉取。

- `--offline` 禁止 Tola 联网。
- `--pure` 在此基础上还禁止读取全局包缓存、系统字体以及站点外的任何路径或符号链接目标（内置包与内置字体仍可用）。hook 启动的外部工具保持其自身环境权限。

#example(title: [将依赖副本存放于 `vendor/`])[
  ```toml
  [vendor]
  path = "vendor"
  ```
]

#key("path")[
  相对于站点根目录的 vendor 目录路径，必须位于站点内且在 `.tola` 之外。
]
