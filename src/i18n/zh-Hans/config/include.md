# `include`

把站点配置拆成多份本地 TOML。

## 用法

把 `include` 写在站点配置的第一个表头前。数组项可以是路径字符串，也可以写成 `{ path = "...", optional = true }`。这套写法用于站点的配置文件。

### 加载顺序与路径

- `include` 的相对路径从声明它的文件所在目录算起。配置项中的站点相对路径仍从站点根目录算起，例如 `build.publish-dir` 或本地模块的 `path`。
- 每一项都是本地文件路径。绝对路径能否使用取决于命令的输入限制，例如 `--pure` 要求配置文件在站点内。Tola 不展开通配符或环境变量，也不下载远程 URL。
- 省略 `optional` 时文件必须存在。设为 `true` 时允许文件缺失，存在的文件仍须通过权限、输入范围和配置检查。
- Tola 按数组顺序合并配置。一个文件也写了 `include` 时，先合并它引用的配置，再合并这个文件。`tola.toml` 最后合并，随后给缺失的键填默认值。
- 同一文件引用几次就合并几次。这也会重复添加 hook 和资源，可能导致 hook 重名或发布路径重复。循环引用会报错。

### 合并规则

表逐键合并，标量和普通数组由后写的值替换。`assets.files`、`assets.trees`、`hooks.before-build`、`hooks.generate-outputs` 和 `hooks.after-publish` 按顺序追加。跨文件安排 hook 的例子见 [`[[hooks.before-build]]`](tola-help://config/hooks/before-build)。

| 后面的写法 | 合并结果 |
| --- | --- |
| 省略某个键 | 保留早先的值。 |
| `title = ""` | 设为空字符串，前提是该字段接受空字符串。 |
| `[site]` 中的 `languages = []` | 把早先的语言列表换成空数组。 |
| `[assets]` 中的 `files = []` | 这次追加零项，保留早先的声明。 |
| 空表 | 保留同一张表里已有的键。 |

TOML 没有 `null` 值。

示例：共用一份配置，在本地覆盖个别值：

在用 `tola init` 创建的站点中，把下面这行加到 `tola.toml` 的最前面。若模板里已设置 `build.publish-dir` 或 `formatter.print-width`，删掉对应的键，让拆分后的文件提供这两个值。

```toml
include = ["config/base.toml", { path = "config/local.toml", optional = true }]
```

在站点根目录创建 `config/`，将下面的内容保存为 `config/base.toml`：

```toml
include = ["format.toml"]

[build]
publish-dir = "base-output"
```

将下面的内容保存为 `config/format.toml`：

```toml
[formatter]
print-width = 88
```

先让 `config/local.toml` 保持缺失，从站点根目录运行 `tola inspect config --json --pretty`。`formatter.print_width` 是 88，`output` 指向站点下的 `base-output/`。`config_sources` 中的 `config/local.toml` 标为 `optional-missing`。现在创建 `config/local.toml`，再运行一次同样的命令。

```toml
[build]
publish-dir = "local-output"
```

`output` 改为站点下的 `local-output/`。在 `field_origins` 中找 `key_path` 为 `["build", "publish-dir"]` 的条目，`origins` 指向 `config/local.toml`，`overridden` 指向 `config/base.toml`。按 `config_sources` 里的 `apply_order` 可看到应用顺序：`format.toml`、`base.toml`、`local.toml`、`tola.toml`。

示例：把模块来源和选项分别写进两个文件：

沿用上面的 `include`。在站点根目录下新建 `theme/tola-module.toml`，给模块一个默认标题。

```toml
[options]
title = "Field notes"
```

在 `config/base.toml` 末尾加上模块的来源。

```toml
[modules.notes]
path = "theme"
```

在 `config/local.toml` 末尾写同一别名的选项。

```toml
[modules.notes.options]
title = "Lab notes"
```

运行 `tola inspect config --json --pretty`。`modules.notes.root` 指向站点下的 `theme/`。在 `field_origins` 中找 `["modules", "notes", "options", "title"]`，当前来源是 `config/local.toml`，被覆盖的来源是 `theme/tola-module.toml`。要改用 `git` 或 `tarball`，在 `config/base.toml` 中替换 `path`，让合并后的模块声明只保留一种来源。
