# `[icons]`

配置 `@tola/icon` 可用的图标集合。

## 配置项

### `collections`

配置表名是图标 ID 的前缀。图标可以来自本地 SVG 目录、IconifyJSON 文件、预设图标集或 JSON 下载地址。

## 用法

### 使用 Iconify 图标集

在 `tola.toml` 中添加 Lucide。

```toml
[icons.collections.lucide]
source-type = "remote-json"
preset = "lucide"
```

从站点根目录运行 `tola update lucide` 下载并锁定图标集，用 `tola inspect icons lucide --interactive` 浏览图标名。`lucide:rocket` 的前缀来自配置表名。

在用 `tola init` 创建的站点里，把下面的代码加到 `content/index.typ`。

```typst
#import "@tola/icon:0.0.0": icon
#icon("lucide:rocket") Launch notes
```

运行 `tola dev`，打开打印的地址。首页会在 Launch notes 旁显示火箭图标。要用独立的 SVG 文件，查 `tola help --lang zh "@tola/icon" icon-url`。

要用 `ui` 作前缀，把表名改成 `[icons.collections.ui]`，保留 `preset = "lucide"`。运行 `tola update ui`，把页面里的调用改成 `icon("ui:rocket")`。

`preset` 对应一个 `@iconify-json` 包。省略 `version` 时，首次下载会选最新版本。要固定版本，写明 `version`。Tola 按包仓库提供的完整性信息检查归档。下载失败时会依次尝试 `mirrors`，镜像必须提供相同的文件。镜像 URL 可含 `{name}` 和 `{version}`。

要添加包仓库以外的图标集，直接填写 JSON 的 `url`，省略 `preset` 和 `version`。镜像地址也要填完整。Tola 会把首次下载的信息写到 `tola.lock`，以后按锁文件校验。把 `tola.lock` 随站点提交。

运行 `tola vendor` 后，远程集合会复制到 vendor 目录的 `icons/<namespace>.json` 中，供 `--pure` 构建读取。

### 使用本地 SVG

在站点根目录下，将下面的 SVG 保存到 `icons/brand/mark.svg`。

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <path fill="currentColor" d="M12 2 22 22H2Z"/>
</svg>
```

在 `tola.toml` 中添加该目录。

```toml
[icons.collections.brand]
source-type = "local-svg-dir"
path = "icons/brand"
```

在用 `tola init` 创建的站点里，把下面的代码放到 `content/index.typ`。

```typst
#import "@tola/icon:0.0.0": icon
#icon("brand:mark")
```

运行 `tola dev`，打开打印的地址。首页会显示内联 SVG，图标的 `currentColor` 跟随所在位置的文字颜色。用 `icon-url` 可以把 SVG 单独发布为文件。

图标 ID 写成 `namespace:name`，名称区分大小写。运行 `tola inspect icons brand` 查看 `brand` 中的图标。要读取本地 IconifyJSON 文件，用 `source-type = "local-json"`。本地 SVG 必须是独立文件。根元素的 `viewBox` 宽高须为正，也可提供为正的绝对宽高。显式尺寸必须使用绝对单位。把图标里的文字转换为路径。

模块可以在 `tola-module.toml` 中声明自己的 `[icons.collections]`，本地路径从模块根目录算起。`icon`、`icon-bytes` 和 `icon-url` 都可以用 `module:` 选择模块实例。在模块内部可用 `icon.with(module: get-module().name)` 绑定当前实例。

示例：显示本地模块提供的图标：

把上面的 `mark.svg` 复制到 `theme/icons/brand/mark.svg`。将下面的配置存为 `theme/tola-module.toml`：

```toml
[icons.collections.brand]
source-type = "local-svg-dir"
path = "icons/brand"
```

在站点的 `tola.toml` 中启用这个模块：

```toml
[modules.notes]
path = "theme"
```

从站点根目录运行 `tola inspect icons brand --module notes`，会列出 `mark`。把下面的代码放进 `content/index.typ`，运行 `tola build`，首页就会显示这个图标：

```typst
#import "@tola/icon:0.0.0": icon
#icon("brand:mark", module: "notes")
```

要替换这个图标集，在站点配置的 `[modules.notes.icons.collections.brand]` 中写完整的来源。替换项的本地路径从站点根目录算起。更新远程图标集时，用 `tola update modules/notes/brand`。
