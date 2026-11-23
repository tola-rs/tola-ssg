# `[modules]`

在 `tola.toml` 里用 `[modules.<别名>]` 添加模块。多个别名可以使用同一份模块代码，各自设置选项。模块根目录必须有 `tola-module.toml`。

## 用法

### 选择模块来源和输入范围

用 `path` 指定本地模块。相对路径从站点根目录算起，也可以填绝对路径。用 `--pure` 时，本地模块必须在站点内。模块目录与发布目录要分开。站点的 `before-build` hook 可以为站点内的本地模块生成输入文件。

每个模块从 `path`、`git`、`tarball`、`builtin` 中选一种。Tola 自带 `resize-images` 和 `tailwind`，版本随 Tola 更新。

`tarball` 可以填 HTTPS 下载地址 `url`，也可以填本地归档 `path`，相对路径从站点根目录算起。需要校验归档时，加上 64 位十六进制 `sha256`。

支持 tar.gz、tar.zst 和使用 stored 或 deflate 压缩方式的 ZIP。归档须有一个外层目录，`tola-module.toml` 直接放在里面。下载和解包时 Tola 不会运行模块脚本。要使用私有 Release 附件，先把归档下载到站点里，再填 `tarball.path`。

`git.url` 可用 `https://`、`ssh://` 或 `git@host:path`。`tag`、`branch` 和 `rev` 最多填一项。`rev` 要填完整的提交 ID。三项都省略时用默认分支。`subdir` 从仓库根目录算起，省略时用仓库根目录。Tola 会将提交和模块目录摘要写入 `tola.lock`。把锁文件随站点一起提交。

私有 `https` 仓库使用你自己配置的 Git credential helper；`ssh` 地址（包括 `git@host:path`）使用系统 `ssh`，因此 SSH agent、`~/.ssh/config` 与 `known_hosts` 照常生效。`tola.lock` 不保存任何凭据。

修改来源或想更新版本时，运行 `tola update notes`。如果站点用了 vendor，再运行 `tola vendor --refresh`。构建会按 `tola.lock` 验证依赖，校验失败时退出。

- `--locked`：保留锁文件，允许下载已锁定的依赖。
- `--offline`：禁止联网，可以读取并记录本地依赖。
- `--pure`：用站点内的源码和 vendor 依赖构建，也可解包站点内已锁定的归档。

### 选择站点入口、选项和资源

用 `[build] entry = { module = "notes" }` 选择整站入口。从 `@tola-modules/notes:0.0.0` 导入模块的公开库。

清单里的路径相对于模块根目录。站点可用 `[modules.notes.options]` 覆盖默认值。两边都是表时逐键合并，数组和其他值直接替换。

在 `tola-module.toml` 里写 `dev = { option = "preview" }`，可以用实例的 `preview` 选项决定 hook 的开发行为。引用替换整个字段，取到的值仍按字段规则校验。`tola module check` 使用清单里的 `[options]` 默认值。

引用也适用于构建 hook 的 `order`、`args`、`rerun-on`、`generates`、`outputs`，模块命令的 `args`、`projection`，以及静态资源的 `minify` 和资源树的 `include`、`exclude`。

站点里的模块来源和选项可以用 [`include`](tola-help://config/include) 拆开写。

例如，`notes` 的静态资源发布到 `/_tola-modules/notes/` 下，链接还会带上站点的部署前缀。用 `enabled-by = "images.enable"` 可以按模块的布尔选项决定是否发布。用 `[modules.notes.assets]` 下的 `enable = false` 可关闭该实例的全部静态资源。替换资源时，保留已启用资源的发布名，替换文件从站点根目录找。被目录筛选规则排除的资源不能替换。

模块内用 `get-module()`，站点里用 `get-module("notes")` 读取指定模块。用 `assets.files` 读文件，用 `assets.urls` 取链接。链接已包含部署前缀，开启 `cache-busting` 时也带文件哈希。要从模块里读站点文件，用 `@tola/site` 的 `project-path()`。

### 运行模块 hook

模块清单使用与站点相同的三个构建阶段。`before-build` 先跑站点 hook，后两个阶段先跑模块 hook。各模块按别名排序，`order` 在模块内生效。Nu 脚本用 `get-context | get module.options` 读取站点覆盖后的选项。

`rerun-on` 的字符串从模块根目录匹配。要匹配站点文件，用 `{ project = "..." }`。`generates`、`outputs` 和 `rerun-on` 可用 `{name}`，Tola 会先替换成模块别名，再检查路径。

命令参数还可以写 `{module}`、`{project}` 或 `{tola}`，分别传入模块根目录、站点根目录和 Tola 的可执行文件路径。

运行顺序与并行设置见 [`[hooks]`](tola-help://config/hooks)。

### 给一批照片生成小图

假设原图放在项目的 `photos/`，发布在 `/gallery/` 下。用源文件路径选择要转换的图片。

```toml
[assets]
trees = [{ source = "photos", url-prefix = "/gallery" }]

[modules.resize-images]
builtin = "resize-images"

[modules.resize-images.options]
include = ["photos/**"]
widths = [480, 960]
format = "webp"
jobs = 2

[modules.resize-images.options.overrides."photos/cover.jpg"]
widths = [1200]
```

普通照片生成宽 480 和 960 像素的 WebP，封面只生成宽 1200 像素的一份。默认跳过需要放大的尺寸。用 `exclude` 跳过部分图片，用 `upscale = true` 允许放大。`jobs` 控制这个模块最多同时处理几张原图。

封面宽度足够时，用 `output-url("images/resize-images/photos/cover.jpg/w1200.webp")` 引用小图。构建会为选中的图片生成文件，即使页面还没引用它们。开发时，源图和尺寸设置相同的成品可以直接复用。

### 生成 Tailwind CSS

```toml
[modules.tailwind]
builtin = "tailwind"

[modules.tailwind.options]
minify = true
# runner = ["deno", "task", "tailwind"]
```

模块默认从站点的 `node_modules/.bin` 或 `PATH` 找到已安装的 Tailwind CSS v4。可以用 `runner` 指定项目任务或自己的命令，接收模块传入的参数，编译一次后退出。模块扫描渲染后的 HTML，发布 `_tola-modules/tailwind/tailwind.css`，用 `output-url` 引用。需要自定义样式时，写到 `styles/tailwind.css`，再用 `watch.include` 登记额外的输入文件。

### 声明和运行查询

查询文件可以直接导入模块里的函数，并读取站点内容。模块查询的入口路径相对于模块目录。`label` 必须找到恰好一个 metadata 值。

普通查询可以输出 JSON 支持的值。Typst `path` 会转换成含 `root` 和 `path` 的记录。想输出 Typst 内容或函数里的信息时，先提取所需的数据字段。

要让编辑器识别引用或定义，在查询下写 `references` 或 `definitions`。每一项对应 `[typst].entry` 导出的一个函数及其一个参数。位置参数从 0 数起，具名参数填参数名。`content.local` 表示取公开成员 `content` 下的 `local`。多个函数或参数可以用同一个查询。找不到函数或参数，或者声明冲突时，错误会指出模块和查询名。

供编辑器查引用或定义的查询要返回记录数组。每条记录要有字符串 `id` 和 `name`。`id` 不能为空，也不能与同一模块实例同一查询中的其他结果重复。`description` 和 `url` 可填字符串，`source` 可填 Typst `path`。CLI 和编辑器都会检查。查询的 `label` 用来找到那条 metadata，结果的 `name` 是每条记录的显示名称。普通查询可以自行定义结果字段。

迁移旧清单时，将 `[[semantics]]` 移到对应查询的 `references` 或 `definitions` 数组，删除 `role`、`query` 以及查询的 `returns` 字段。结果字段 `label`、`detail` 分别改为 `name`、`description`，metadata 的 `label` 保留。旧字段会被拒绝。

编辑器中的查询可以读取尚未保存的修改。

运行查询前，先生成查询要读取的文件。查询只求值 Typst 入口，并会指出模块声明里哪些生成文件还没验证。

查询默认打印表格。用 `--json` 输出 JSON，`--pretty` 加缩进，`--output` 写到文件。用 `--interactive` 先看完整记录，再按 `projection` 输出所选记录指定的字段。用 Nu 的 `where` 筛行、`select` 选字段。也可以直接运行 `tola query ./tools/model.typ --label model` 查询单个文件。

### 运行模块命令和导入 Nu 库

模块别名不能和内置命令同名。`--help` 只读声明，不会运行模块查询或工具。清单里拼错或不支持的字段会报错。

Nu 命令的参数、默认值和说明写在脚本的 `main` 签名及注释中。`--help` 读取这些声明。清单里的 `help` 可以覆盖简介，也可以给 `argv` 命令写简介。

查看帮助、运行初始化脚本或执行 `argv` 命令，只需准备这一步会读取的文件。运行查询或构建时，Tola 会再检查各自需要的输入。

`nu` 的脚本路径相对于模块目录，运行时的工作目录是站点根目录。Tola 自带 Nushell。

要运行外部程序，用 `argv` 声明命令。参数里的 `{module}`、`{project}` 和 `{tola}` 分别替换为模块目录、站点目录和 Tola 的可执行文件路径。命令行上追加的参数会原样传入。外部程序可运行 `"$TOLA_EXECUTABLE" nu @tola-scripts/context --json`，查看当前站点、模块选项和本次调用允许读取的输入范围。

Nu 模块命令可以用 `diagnostic submit` 向 Tola 报告问题。每次调用提交一次，允许空列表。提交的诊断有错误时，命令失败。外部程序可把报告写到上下文中的 `reportPath`。完整的 Nu 示例见 [`@tola-scripts/diagnostic`](tola-help://scripts/diagnostic)。

在 Nu 中用 `use tola-modules notes` 导入所选别名，用 `export` 定义可导入的函数。可复用的函数通过参数接收站点选项。模块自己的文件用 `path self` 定位，命令行传入的路径以当前工作目录为准。

`tola nu @tola-modules/notes` 调用公开入口中的 `export def main`。`tola nu @tola-scripts/image` 运行内置图片脚本。在 Nu 脚本里导入图片函数时，写 `use tola image`。运行本地文件时传文件路径。文件名以 `@` 开头时，用 `./@file.nu`。用 `tola nu --help` 查可用的库。

`query` 直接返回 Nu 值，可以接 `where` 筛选，或用 `get` 取字段。要读取单独的 Typst 查询文件，可用 `query ./tools/model.typ --label model`。

在 Nu 中调用其他 Tola 命令时，用 `$env.TOLA_EXECUTABLE` 指定程序，再用 `complete` 读取 `stdout`、`stderr` 和 `exit_code`。需要选择站点配置时传 `--config`。如果子命令支持 `--offline` 或 `--pure`，按 `$env.TOLA_INPUT_SCOPE` 传入相应选项。`tola dev --json` 会持续输出，按行读取 JSON。

选择其他配置可用 `tola notes --config other.toml COMMAND ...`；COMMAND 之后的参数归该命令。声明的工具不会静默回退到全局插件。

图像处理见[内置图像脚本](tola-help://scripts/image)。要让作者在终端搜索和选择结果，运行 `tola skill scripts tui`，查看 `nu-helpers pick` 的完整示例。

### 读取模块指南

`skill` 填模块目录下的 Markdown 文件路径。用模块声明的命令按标题阅读章节。`tola skill` 则可在任意目录打开 Tola 自带的写站指南。

### 初始化站点

把新文件写到 `init.outputDir`。如果生成的配置里要写相对路径，以新站点最终所在的 `init.targetDir` 为准。用 `module.name` 读取新站点中的模块别名。只有 `init.interactive` 为真时才提示用户输入。

`--from` 也支持 Git 地址和归档。Git 可用 `--branch`、`--tag` 或 `--rev` 选版本，用 `--subdir` 选模块目录。已有站点可运行 `tola forest init new-site`，使用该站点已选定的模块。清单中的 `[init]` 与 `[commands.init]` 二选一。

Tola 先校验新配置和模块选项，再写入目标目录。`--dry-run` 照常生成并检查文件，目标目录不会被写入。`--force` 允许使用非空目录，并保留已有文件。

### 打包模块

`before-pack` 用 `hook.inputDir` 取模块根目录，此时还没有站点上下文。

`before-pack` 供模块作者打包时运行。已启用的条目按声明顺序执行。可以填写 `name`、`enable`，再选 `nu`（可带 `args`）或 `argv`。`order` 和 `rerun-on` 用于构建 hook。

`[pack]` 下必须写 `include`，用来选打包脚本运行后要发布的文件。路径从模块根目录算起，选中目录就包含里面的文件。用 `exclude = ["assets/**/*.map"]` 可以排除文件，排除规则优先。Tola 总会附上 `tola-module.toml`。发布版清单仍声明的文件和目录也要一起打包。

`tola module check ./dist/labels-theme` 检查成品清单和声明的路径。打包时已经做过这项检查，也可用这条命令单独检查下载或解包的模块。要验证编译和站点 hooks，在使用成品的站点中运行 `tola check`。

示例：导入模块的 Typst 库：

用 `tola init` 创建站点，在 `tola.toml` 里添加本地模块：

```toml
[modules.notes]
path = "theme"
```

新建 `theme/tola-module.toml`。

```toml
[typst]
entry = "lib.typ"

[options]
title = "Field notes"
```

将下面的代码保存到 `theme/lib.typ`。

```typst
#import "@tola/module:0.0.0": get-module
#let note-title = get-module().options.title
```

在 `content/index.typ` 调用它。

```typst
#import "@tola-modules/notes:0.0.0": note-title
= #note-title
```

从站点根目录运行 `tola build`。`public/index.html` 的正文标题是 Field notes。在 `tola.toml` 的 `[modules.notes.options]` 下写 `title = "Lab notes"`，重新构建后，正文标题会改成 Lab notes。

示例：从归档或 Git 获取模块：

从 GitHub Release 获取模块。

```toml
[modules.notes]
tarball = { url = "https://github.com/example/notebook/releases/download/v1.0.0/theme.tar.zst" }
```

从 Git 仓库获取模块。

```toml
[modules.notes]
git = { url = "https://github.com/example/notebook", tag = "v1.0.0", subdir = "theme" }
```

示例：声明入口、默认选项和资源：

在模块清单中声明公开库、整站入口、资源和默认选项。

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

在站点配置中替换模块的 `style.css`。

```toml
[modules.notes.assets.replace]
"style.css" = "static/custom.css"
```

示例：给模块加一个生成搜索索引的 hook：

写好 `theme/tools/search.nu` 后，在站点的 `[modules.notes.options]` 下设 `search = true`。输出路径里的 `{name}` 会换成模块别名，每个实例可以写自己的索引。

```toml
[[hooks.generate-outputs]]
name = "search"
nu = "tools/search.nu"
enabled-by = "search"
rerun-on = { include = ["tools/data/**/*.json", { project = "content/**/*.typ" }], exclude = [{ project = "content/drafts" }] }
outputs = [{ tree = "_tola-modules/{name}/search" }]
```

示例：添加查询和命令：

在本地 `notes` 模块的 `theme/tola-module.toml` 里加上查询和命令：

```toml
[queries.nodes]
entry = "model.typ"
label = "notebook-nodes"

[commands.list]
help = "List notebook entries."
query = "nodes"
projection = ["id", "name"]
```

将下面一行保存到 `theme/model.typ`。

```typst
#metadata(((id: "welcome", name: "Welcome"),)) <notebook-nodes>
```

从站点根目录运行 `tola query notes.nodes --json` 查看这条记录。运行 `tola notes list` 可查看其中的 `id` 和 `name`。

用 `tola notes list --help` 查看命令选项。

示例：给查询声明引用和定义：

在对应查询下面声明引用和定义。

```toml
[[queries.nodes.references]]
export = "local"
parameter = { positional = 0 }

[[queries.nodes.definitions]]
export = "subtree"
parameter = { named = "address" }
```

示例：用 Nu 编写模块命令：

沿用前面的本地 `notes` 模块。在 `theme/tola-module.toml` 中加入：

```toml
[commands.title]
nu = "tools/title.nu"
```

创建 `theme/tools/`，将下面的代码保存为 `theme/tools/title.nu`：

```nu
use tola get-context

def main [] {
  get-context | get module.options.title
}
```

运行 `tola notes title` 查看模块标题。`[modules.notes.options]` 里覆盖了标题时，会显示站点设置的值。

示例：导出 Nu 库：

在模块清单里声明供其他脚本导入的 Nu 文件：

```toml
[nu]
entry = "tools/mod.nu"
```

把上面的 `[nu]` 加到 `theme/tola-module.toml`，再将下面的代码保存为 `theme/tools/mod.nu`。

```nu
export def note-title [] { "Field notes" }
export def main [] { note-title }
```

从站点根目录运行 `tola nu @tola-modules/notes`，会输出 `Field notes`。

示例：在 Nu 中读取 Typst 查询结果：

配好上面的 `notes.nodes` 查询后，把下面的脚本保存为 `list-titles.nu`，与 `tola.toml` 放在一起：

```nu
use tola query

query notes.nodes | get name | to text
```

从站点根目录运行 `tola nu list-titles.nu`，终端会打印 `Welcome`。

示例：给模块添加指南命令：

在 `theme/tola-module.toml` 中加入：

```toml
[commands.guide]
skill = "GUIDE.md"
help = "Read the theme guide."
```

把下面的内容保存为 `theme/GUIDE.md`：

```md
# Notes theme

## Writing

Put posts in content/posts/.

### Drafts

Set draft to true in a post's metadata to keep it out of the published site.
```

在站点根目录运行 `tola notes guide --list` 查看章节，再用 `tola notes guide writing drafts` 读草稿说明。要把写作章节连同子章节保存到 `writing.md`，用 `tola notes guide writing --all -o writing.md`。

示例：用模块创建站点：

在 `theme/tola-module.toml` 加入初始化脚本声明：

```toml
[init]
nu = "tools/init.nu"
```

将下面的代码保存为 `theme/tools/init.nu`：

```nu
use tola get-context

def main [--title: string = "Field notes"] {
  let out = get-context | get init.outputDir
  {
    site: {title: $title}
  } | to toml | save ($out | path join "tola.toml")
  mkdir ($out | path join "content")
  [
    '#import "@tola/site:0.0.0": site'
    '#document("index.html", format: "html")[#site.title]'
  ] | str join (char newline) | save ($out | path join "site.typ")
}
```

从 `theme/` 所在的目录运行：

```sh
tola init new-site --from ./theme --alias forest --dry-run -- --title "My notes"
tola init new-site --from ./theme --alias forest -- --title "My notes"
cd new-site
tola build
```

打开 `public/index.html`，可以看到 My notes。Tola 会把模块来源写进新 `tola.toml` 的 `[modules.forest]`。单独的 `--` 后面是传给初始化脚本的参数。省略 `--title` 时，脚本用 Field notes。

示例：打包前生成模块要发布的文件：

新建一个独立的模块目录 `labels-theme/`，把下面的配置存为 `labels-theme/tola-module.toml`：

```toml
[pack]
include = ["assets"]

[assets]
files = [{ source = "assets/labels.json", url = "/labels.json" }]

[[hooks.before-pack]]
name = "build-labels"
nu = "prepare-labels.nu"
```

将下面的内容保存为 `labels-theme/labels.csv`：

```csv
id,label
draft,Draft
published,Published
```

把脚本保存为 `labels-theme/prepare-labels.nu`：

```nu
use tola get-context

let source = get-context | get hook.inputDir
let assets = $source | path join "assets"
mkdir $assets
open ($source | path join "labels.csv")
| to json
| save --force ($assets | path join "labels.json")
```

从 `labels-theme/` 所在的目录运行：

```sh
mkdir -p dist
tola module pack ./labels-theme --output ./dist/labels-theme
tola module check ./dist/labels-theme
```

打开 `dist/labels-theme/assets/labels.json` 可以看到两条记录。hook 先把 JSON 写到模块源目录，打包时再按 `include` 把它放进 `dist/labels-theme/assets/`。发布版 `tola-module.toml` 省去 `[pack]` 和 `[[hooks.before-pack]]`，站点可以直接读生成好的 JSON。
