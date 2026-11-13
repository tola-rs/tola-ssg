# `[assets]`

发布图片、样式表和其他静态文件。

## 配置项

### `cache-busting`

开启后，`asset-url` 会在 URL 末尾加上 `?h=...`。文件内容变了，链接里的哈希就会变，发布文件名照旧。默认关闭。

### `trees`

把整个目录发布到指定的 URL 前缀下。用 `include` 和 `exclude` 选文件。要发布指向普通文件的符号链接，在对应的目录声明中设 `follow-file-symlinks = true`。

### `files`

给单个文件指定 URL。`source` 相对于站点根目录，`url` 以 `/` 开头。同一个文件也被 `trees` 匹配时，以 `files` 为准。

## 用法

### 发布一张图片

将图片放在 `static/logo.png`，在 `tola.toml` 中加入以下配置。

```toml
[assets]
files = [{ source = "static/logo.png", url = "/assets/logo.png" }]
```

在用 `tola init` 创建的站点里，把下面的代码加到 `content/index.typ`。

```typst
#import "@tola/address:0.0.0": asset-url
#html.img(src: asset-url("/assets/logo.png"), alt: "站点标志")
```

运行 `tola build`。默认会生成 `public/assets/logo.png`，页面引用 `/assets/logo.png`。如果 `site.base-path = "/docs/"`，页面会引用 `/docs/assets/logo.png`。

### 发布一个目录

用 [`include`](tola-help://config/include) 拆分配置时，各文件的资源声明会汇总。每个文件都要有独立的输出路径。

用 `trees = [{ source = "static/icons", url-prefix = "/icons" }]` 把 `static/icons/` 下的文件发布到 `/icons/`，保留文件在源目录中的相对路径。

默认选中目录中的所有文件。用 `include = ["static/icons/**/*.svg"]` 选 SVG，用 `exclude = ["static/icons/drafts"]` 排除草稿目录。模式区分大小写，站点里的模式从站点根目录算起，模块里的从模块根目录算起。`include = []` 不选任何文件。

同一个源文件同时被 `files` 和 `trees` 选中时，以 `files` 为准，即使那条单文件声明已禁用。`trees` 的源目录不能嵌套，URL 前缀也不能重复或互相包含。页面、静态文件和 hook 产物各用独立的输出路径。

用 `enable = false` 禁用声明，或用 `enabled-by = "images.enable"` 读取布尔选项。键名含点号时，要给该段加引号。用 `minify = false` 保留源文件字节。

启用的静态文件即使没有被页面引用也会发布。`before-build` hook 可以生成这些源文件。`generate-outputs` 的文件直接声明在该 hook 的 `outputs` 中。

### 发布符号链接指向的文件

在用 `tola init` 创建的站点根目录运行以下命令，在 `static/downloads/` 中创建一个指向共享文件的符号链接。

```sh
mkdir -p shared static/downloads
printf '%s\n' 'Project notes' > shared/notes.txt
ln -s ../../shared/notes.txt static/downloads/guide.txt
```

在站点的 `tola.toml` 中加入这条声明。

```toml
[[assets.trees]]
source = "static/downloads"
url-prefix = "/downloads"
follow-file-symlinks = true
```

从站点根目录运行 `tola build`。使用默认发布目录时，`public/downloads/guide.txt` 是一个普通文件，内容是 `Project notes`。发布路径用链接的名字，文件内容取自 `shared/notes.txt`。

要在首页链接到这个文件，把下面两行放进 `content/index.typ`，再运行 `tola build`。使用默认 `site.base-path` 时，首页链接指向 `/downloads/guide.txt`。

```typst
#import "@tola/address:0.0.0": asset-url
#link(asset-url("/downloads/guide.txt"))[下载笔记]
```

`follow-file-symlinks` 默认关闭，只影响这一条目录声明。`include` 和 `exclude` 按链接自身的路径匹配。默认选中符号链接就会报错。开启后，链接必须指向可读的普通文件。遇到断链或目录链接，可以用 `exclude` 跳过。要发布目录链接里的文件，直接为目标目录写一条 `trees` 声明。

目标可以在 `source` 目录之外，例如上面的 `shared/notes.txt`。模块里的链接必须指向该模块内的文件。用 `--pure` 构建时，目标还必须位于站点内。和其他资源一样，目标应放在 `build.content-dir` 与 `build.publish-dir` 之外。
