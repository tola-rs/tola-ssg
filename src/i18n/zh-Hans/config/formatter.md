# `[formatter]`

`tola fmt` 和编辑器格式化 Typst 文件时，都会读取这些设置。

## 配置项

### `indent`

每层缩进的空格数，默认 2。

### `print-width`

折行宽度，默认 120 列。小于 20 时按 20 列处理。

### `prose-wrap`

默认 `none`，保留正文原有的换行。`fill` 按列宽换行，`sentence` 每个句子单独一行。

## 用法

例如，把折行宽度改为 88 列：

```toml
[formatter]
print-width = 88
```

要试一下，可在用 `tola init` 创建的站点里，把这一行写入 `content/index.typ`：

```typst
#let sizes=(small:8,large:16)
```

从站点根目录运行 `tola fmt --check`，它会列出需要格式化的文件。运行 `tola fmt` 写回文件后，打开 `content/index.typ`，可以看到赋值符号两边、字段名和字段值之间的空格。

找不到站点配置时，`tola fmt` 使用默认值格式化当前目录下的 `.typ` 文件。
