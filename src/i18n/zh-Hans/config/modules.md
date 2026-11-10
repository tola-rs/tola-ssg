# `[modules]`

按命名空间选择的模块。

## 用法

模块命名空间选择一个包含 `tola-module.toml` 的目录：

```toml
[modules.notes]
path = "theme"
```

路径相对于站点根目录，也可以写绝对路径。使用模块时，Tola 按本次调用的输入范围读取清单；`--pure` 要求模块的物理目录位于站点内。模块目录是构建输入，发布目录不能覆盖它。站点自己的 before-build hook 可以写入站点内本地模块的指定子路径。只加载项目明确声明的模块，不自动合并多份站点配置。

准备好的模块可以从 HTTPS 附件或本地归档文件获取：

```toml
[modules.notes]
tarball = { url = "https://github.com/example/notebook/releases/download/v1.0.0/theme.tar.zst" }
```

`path`、`git`、`tarball` 三选一。`tarball` 内的 `url` 与文件 `path` 二选一，可用 `sha256` 提供 64 位十六进制校验值。本地文件路径相对于站点配置。支持 tar.gz、tar.zst、ZIP（stored/deflate）；归档只有一层外包目录，其中直接包含 `tola-module.toml`，Tola 去掉这一层后加载模块。不提供归档 `subdir`，不搜索清单，也不执行作者的 before-pack hook。HTTPS 地址不得嵌入凭据；尚不提供私有 Release 鉴权。

使用 `https` 或 `ssh` Git 仓库时，将 `path` 换成远程来源：

```toml
[modules.notes]
git = { url = "https://github.com/example/notebook", tag = "v1.0.0", subdir = "theme" }
```

`git.url` 接受 `https://`、`ssh://` 或 `git@host:path`。`git.tag`、`git.branch`、`git.rev` 最多选择一个；`git.rev` 是完整提交 ID，全部省略时选择默认分支。`git.subdir` 相对于仓库根，省略时使用仓库根。解析会写入 `tola.lock`，记录解析出的提交与模块目录摘要；随站点一起提交。

私有 `https` 仓库使用你自己配置的 Git credential helper；`ssh` 地址（包括 `git@host:path`）使用系统 `ssh`，因此 SSH agent、`~/.ssh/config` 与 `known_hosts` 照常生效。`tola.lock` 不保存任何凭据。

已有锁记录始终校验内容；普通构建不会因校验失败而自动改锁。`--locked` 禁止改锁，但允许获取已锁定的缺失内容；`--offline` 禁止联网，仍可读取并记录本地输入。`--pure` 只使用项目源码与 vendor，不使用网络或宿主缓存，也可以重新展开项目内已锁定的本地归档。修改来源或需要刷新时运行 `tola update notes`，再用 `tola vendor --refresh` 更新冻结副本。归档锁记录文件摘要和展开后的目录摘要。远程初始化尚未实现，初始化仍接收本地模块目录。

用 `[build] entry = { module = "notes" }` 选择整站入口。从 `@tola-modules/notes:0.0.0` 导入模块的公开库。模块清单示例：

```toml
[typst]
entry = "lib.typ"
[build]
entry = "site.typ"
[assets]
trees = [{ source = "assets", url-prefix = "/" }]
[options]
search = false
```

清单中的路径相对于模块根。项目的 `[modules.notes.options]` 覆盖默认值；两侧都是表时递归合并，数组和其他值整体替换。

模块静态资源发布到 `/_tola-modules/notes/` 下，再应用站点部署前缀。可在资源声明中用 `enabled-by` 引用实例的顶层布尔选项。`[modules.notes.assets]` 下的 `enable = false` 关闭该实例的静态资源。替换项保留已启用资源的发布名，来源相对于项目根。树过滤掉的成员不属于可替换的资源。

```toml
[modules.notes.assets.replace]
"style.css" = "static/custom.css"
```

在模块源码中调用 `@tola/module` 的 `get-module()` 读取该实例的别名、选项和资源。也可传入明确的别名，例如 `get-module("notes")`。`assets.files` 将所选名称映射到 Typst 原生输入路径，项目替换项保持项目根。`assets.urls` 使用同一组键，值已包含部署前缀与适用的缓存标识。这两组记录静态资源。可用 `@tola/site` 的 `project-path()` 构造项目输入路径。

模块清单将构建 hooks 放在 `hooks.before-build`、`hooks.generate-outputs` 或 `hooks.after-publish` 中。站点的 before-build 先运行，其余阶段先运行模块 hooks；模块按别名及声明顺序执行。外部模块 hook 可以从 `TOLA_MODULE_OPTIONS` 指定的 JSON 文件读取完整的有效选项；嵌入式 Nu 可以读取 `(get-module).options`。

```toml
[[hooks.generate-outputs]]
name = "search"
nu = "tools/search.nu"
enabled-by = "search"
rerun-on = { include = ["tools/data/**/*.json", { project = "content/**/*.typ" }], exclude = [{ project = "content/drafts" }] }
outputs = [{ tree = "_tola-modules/{name}/search" }]
```

查询不会执行 hook。查询结果会标明模块声明的生成输入仍待验证。

命令参数中的 `{name}`、`{module}`、`{project}`、`{tola}` 分别逐字替换为别名、模块根目录、项目根目录和当前可执行文件。`generates`、`outputs` 和 `rerun-on` 模式支持 `{name}`，替换后仍执行校验。`rerun-on` 的两个列表都接受模块相对字符串和 `{ project = "..." }` 形式的站点相对模式。命令参数里的 `{module}`、`{project}` 替换与这两种模式的根目录无关。

清单可声明命名查询和命令。

```toml
[queries.nodes]
entry = "model.typ"
label = "notebook-nodes"

[commands.list]
help = "List notebook entries."
query = "nodes"
projection = ["id", "name"]

[commands.edit]
help = "Open a notebook entry."
argv = ["{module}/tools/edit"]
```

运行 `tola notes --help`、`tola notes list --help` 或 `tola query notes.nodes`。内置命令名不能用作模块名；多个别名可以选择同一个模块。帮助只读取声明，不求值查询或运行工具。清单中未知字段会报错。

帮助、初始化入口和 argv 工具不要求无关的查询入口或资产文件存在；执行查询和构建站点时仍校验相应输入。对于以 `nu` 声明的命令，在脚本的 `main` 签名和注释中定义参数、类型、默认值与说明。根帮助、模块帮助和命令帮助读取这份签名，不执行脚本；运行时由 Nu 绑定参数。清单中的 `help` 可省略，也可用来覆盖说明；其他 argv 工具可用它描述用法。

查询入口相对于模块目录，支持声明在站点根之外的模块。查询沿用站点的源文件发现与包解析，只选择自己的 Typst 入口。`label` 必须选中恰好一个 metadata 值。普通查询接受能转换为 JSON 的标量、数组和字典；原生 Typst 路径转换为包含 `root`、`path` 的记录。内容和函数需要先投影成数据字段。查询可以直接导入模块内部的 helper。

在对应查询下面声明引用和定义。

```toml
[[queries.nodes.references]]
export = "local"
parameter = { positional = 0 }

[[queries.nodes.definitions]]
export = "subtree"
parameter = { named = "address" }
```

每条记录指定 `[typst].entry` 导出的函数及其一个参数。位置参数从零计数，具名参数按名称选择。`content.local` 这样的点分路径按公开成员查找。数组允许多个函数或参数使用同一个查询。找不到导出、参数不存在或声明冲突时，诊断会指出模块与查询条目。

带有引用或定义的查询自动要求返回记录数组，每项必须有非空且在该查询内唯一的字符串 `id`、字符串 `name`。`description`、`url` 是可选字符串，`source` 是可选原生 Typst 路径。CLI 和 LSP 使用同一套校验。ID 的范围由模块别名和查询名确定。配置中的 `label` 选择整份 metadata，结果中的 `name` 用于显示。普通查询自行定义字段和类型，消费它的工具检查自己的字段要求。

迁移旧清单时，将 `[[semantics]]` 移到对应查询的 `references` 或 `definitions` 数组，删除 `role`、`query` 以及查询的 `returns` 字段。结果字段 `label`、`detail` 分别改为 `name`、`description`，metadata 的 `label` 保留。旧字段会被拒绝。

LSP 为每个模块实例的查询入口保留会话，同一修订中多个标签共用入口编译结果。编辑后复用有效的源分析，回答仍与当前未保存文本一致。配置、模块声明和输入范围改变时更新会话。同一入口中的多个查询共享计算和失败范围。

查询不运行 hook、不导出页面，也不发布站点；缺失的生成输入仍会报错，需要先显式准备。默认输出人类可读的表格；`--json` 返回 compact JSON，`--json --pretty` 缩进。

查询命令支持 `--json`、`--pretty`、`--output` 和 `--interactive`。声明的 `projection` 是固定字段投影，在交互选择后应用。用 Nu 的 `where` 和 `select` 过滤或选择字段。`tola query ./tools/model.typ --label model` 可直接求值显式 Typst 查询文件，不要求先声明模块。

argv 命令仅在调用时执行，以站点根为工作目录并继承标准流。参数中的 `{module}`、`{project}` 和 `{tola}` 做字面替换，用户追加的参数原样传递，不经过 shell 插值。工具收到 `TOLA_PROJECT_ROOT`、`TOLA_MODULE_ROOT`、`TOLA_MODULE_NAME`、`TOLA_CONTENT_ROOT`、`TOLA_CONFIG_PATH`、`TOLA_EXECUTABLE` 和 `TOLA_INPUT_SCOPE`；后两者让工具以调用方的输入策略调用同一个 Tola，但不会限制工具自身的权限。内容目录已经由 Tola 解析，工具不必猜测默认值。Tola 传递退出状态和取消操作，交互命令结束后恢复控制终端；此接口不管理 Ctrl-Z 引起的 shell 作业挂起。

使用 `nu = "tools/new.nu"` 运行模块脚本，无须另外安装 Nu。脚本入口相对模块目录，工作目录保持站点根；普通 `tola nu file.nu` 相对调用者目录。`tola nu --lsp` 服务Nu文件，`tola lsp` 服务Typst。

模块 argv 命令和 hook 可以通过 `TOLA_REPORT_PATH` 提交不带位置的诊断批次，例如在 Nu 中用 `use tola diagnostic` 和 `diagnostic submit`。warning 不改变成功结果；error 使其失败。after-publish 的错误不会撤销发布。开发时复用的 `generate-outputs` 结果保留对应报告；未提交不同于提交空批次。此报告接口尚不支持独立检查或未保存的编辑器快照。

模块可同时公开可复用的Nu库：

```toml
[nu]
entry = "tools/mod.nu"
```

在Nu里通过 `use tola-modules notes` 导入所选别名，使用标准 `export` 定义公开函数。导入不会选择当前模块配置；站点选项由调用者显式传入。模块资源使用 `path self`，用户路径按调用工作目录解释。

`tola nu @tola-modules/notes` 直接运行公共入口，要求 `export def main`；`tola nu @tola-scripts/image` 运行内置图片入口。这些定位符不是Nu文件路径或下载请求，未知名称不会回退到磁盘。普通以 `@` 开头的文件用 `./@file.nu`。Nu导入采用原生根模块，例如 `use tola image`；`tola nu --help` 静态列出内置和配置的Nu库。

根命令 `query` 调用已有站点查询：`use tola` 后使用 `tola query`；`use tola query` 则直接导入 `query`。`use tola cli` 提供用于有限命令的 `cli capture`，返回 `stdout`、`stderr` 和 `exit_code`；需要时在参数列表中显式传入 `--config`。持续开发事件使用原生外部命令运行 `tola dev --json`，逐行读取 JSON，而不捕获整个进程。

`use tola image` 提供字节和文件操作，例如：

```nu
use tola image
let source = (image metadata photo.png)
image resize photo.png --output preview.webp --width 640 --op fit-width --format webp
```

`image resize` 可接收文件路径或管道二进制。指定 `--output` 时写入文件并返回元数据，目标文件必须不存在且所在目录必须已存在。省略 `--output` 时返回编码后的二进制。

模块还可声明站点初始化入口：

```toml
[init]
argv = ["{tola}", "nu", "tools/init.nu"]
```

站点创建前运行 `tola init notes --from path/to/module -- --title "My notes"`。`--alias forest` 选择别名，省略时使用清单名称；已有站点可运行 `tola forest init notes`。`[init]` 与 `[commands.init]` 不能同时声明。

初始化器在空的候选目录运行，`TOLA_INIT_OUTPUT_DIR` 是写入目录，`TOLA_INIT_TARGET_DIR` 是最终目录，`TOLA_INIT_INTERACTIVE` 表示是否请求交互。模块和可执行文件上下文可用，项目、内容和配置环境变量暂缺。生成配置的模块路径须从最终目录解析，并以 `TOLA_MODULE_NAME` 注册当前模块。

Tola先校验候选配置和模块选项，再创建目标文件。`--dry-run` 执行生成和校验但不写目标目录；`--force` 允许非空目录但不会覆盖已有文件。初始化器是可信外部程序，应自行把输出写在 `TOLA_INIT_OUTPUT_DIR`，该接口不是沙箱。

选择其他配置可用 `tola notes --config other.toml COMMAND ...`；COMMAND 之后的参数归该命令。声明的工具不会静默回退到全局插件。
