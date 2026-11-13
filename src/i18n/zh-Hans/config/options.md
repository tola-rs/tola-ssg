# `[options]`

把模板和 Nu 脚本共用的设置写在这里，也可以用它们控制 hook 是否运行。

## 用法

键名由模板和脚本自己约定。默认值可以写在站点的 `tola.toml` 中，也可以在模板或脚本读不到值时补上。

### 用一个开关控制页面和 hook

在用 `tola init` 创建的站点中，将下面的配置加到 `tola.toml`：

```toml
[options.banner]
enabled = true
text = "Field notes"
```

将下面的代码写入 `content/index.typ`：

```typst
#import "@tola/site:0.0.0": site
#if site.options.banner.enabled [#site.options.banner.text]
```

要把同一段文字发布成文件，把下面的脚本保存为 `write-banner.nu`，与 `tola.toml` 放在一起：

```nu
use tola [get-context get-site]

let out = get-context | get hook.outputDir
get-site | get options.banner.text | save ($out | path join "banner.txt")
```

把 hook 加到 `tola.toml`：

```toml
[[hooks.generate-outputs]]
name = "banner"
nu = "write-banner.nu"
enabled-by = "banner.enabled"
outputs = [{ file = "banner.txt" }]
```

运行 `tola build`，默认会在 `public/index.html` 和 `public/banner.txt` 看到 Field notes。把 `[options.banner]` 里的 `enabled` 改成 `false` 后重新构建，首页会隐藏这行文字，`public/banner.txt` 也会消失。

### 从哪里读和改选项

Nu 脚本用 `get-site` 读站点选项，用 `get-context` 读本次 hook 的目录和阶段。站点 hook 的 `enabled-by` 填 `[options]` 下的键路径，例如 `"banner.enabled"`，对应的值必须是布尔值。

用 `[modules.<alias>.options]` 覆盖指定模块的默认值。模块 hook 的 `enabled-by` 读取自己的模块选项。嵌套表逐键合并，站点提供的数组替换整个默认数组。TOML 日期和时间转成字符串，数字必须是有限值。
