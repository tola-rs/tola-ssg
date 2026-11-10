查看命令选项，请用 `tola <command> --help`。

`tola help` 提供 Tola 内置的离线文档：`tola.toml` 配置（`tola help "[build]"`）和 `@tola/xxx` 内置包（`tola help "@tola/address" slugify`）。加上 `-i` 或 `--interactive` 可交互式浏览：筛选章节和链接列表，按 Enter 打开选项，选择 Back 返回，按 Esc 结束浏览。文档默认使用英文；加上 `--lang zh` 可阅读简体中文。

`tola skill` 提供按任务组织的写作指南入口。使用 `tola skill writing math` 或 `tola skill modules create` 只读取当前任务需要的内容。`--list` 显示标题树，`--all` 包含所选主题的所有子章节。
