#show: config(
  summary: [按自定义别名引入本地目录、Git 仓库或压缩包中的模块。],
  related: ("@tola/module", "[hooks]", "[options]", "[vendor]"),
)

使用 `[modules.<alias>]` 声明模块实例。多个别名可指向同一份模块源码并配置各自独立的选项。每个模块根目录都必须包含 `tola-module.toml`。

= 模块来源

每个别名从以下三种来源中任选其一：

- `path`：本地目录（相对站点根目录，或在输入范围允许时使用绝对路径；`--pure` 要求位于站点内，且始终不能与 `build.publish-dir` 重叠）。
- `git`：`{ url = "...", tag | branch | rev = "...", subdir = "..." }`。支持 `https://`、`ssh://` 与 `git@host:path`，沿用本机 Git 凭据助手或 SSH 配置，不在 `tola.lock` 中保存凭据。
- `tarball`：`{ url = "https://..." }` 或 `{ path = "..." }`，可选 64 位十六进制 `sha256`。支持 `.tar.gz`、`.tar.zst` 与 ZIP，归档内须包含单个外层目录且 `tola-module.toml` 位于其中。

修改来源或更新锁定版本时运行 `tola update <alias>`；使用 vendor 时再运行 `tola vendor --refresh`。输入标志行为：`--locked` 保持 `tola.lock` 不变但允许下载已锁定依赖；`--offline` 禁止联网；`--pure` 仅允许使用站点内源码与 vendor 副本。

= 入口、库、选项、资源与 hook

- 设置 `[build] entry = { module = "<alias>" }` 可将模块作为整站入口。通过 `@tola-modules/<alias>:0.0.0` 导入模块公开库，通过 #ref("@tola/module#get-module") 读取实例信息。
- 在 `[modules.<alias>.options]` 下覆盖模块默认选项：嵌套表逐键合并，数组与标量直接替换。
- 模块静态资源默认发布到 `/_tola-modules/<alias>/`（自动附带 `site.base-path`）。可通过 `[modules.<alias>.assets] enable = false` 关闭该实例全部资源，或从站点根目录覆盖单个已启用资源。
- 模块 hook 参与相同的三个构建阶段（见 #ref("[hooks]")）。清单参数与路径支持 `{name}`、`{module}`、`{project}`、`{tola}` 展开；`rerun-on` 中可用 `{ project = "..." }` 监听站点文件。

= 命令、指南与初始化器

- 模块可在 `tola-module.toml` 中声明作者命令（`[commands.<name>]`，以 `tola <alias> <command>` 调用）、文档指南（`skill = "GUIDE.md"`）和站点初始化器（`[init]`，以 `tola init <dir> --from <source> --alias <alias>` 调用）。
- 发布模块前可用 `tola module pack` 与 `tola module check` 打包和校验。

#example(title: [配置本地主题与官方 Tailwind 模块])[
  ```toml
  [modules.theme]
  path = "theme"

  [modules.theme.options]
  title = "Typst 笔记"

  [modules.tailwind]
  git = { url = "https://github.com/tola-rs/modules.git", subdir = "tailwind" }
  ```
]
