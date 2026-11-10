# `[formatter]`

Tola 书写站点自己的 `.typ` 文件所用的样式：`tola fmt` 与语言服务都按这些值格式化。

## 配置项

### `indent`

一级嵌套缩进多少个空格。

### `print-width`

格式化结果折行的列宽。格式化器最窄只折到 20 列：更小的值会被提升到 20，而不是被拒绝。

### `prose-wrap`

标记文本中的散文怎么换行：`none` 保留作者自己的换行，`fill` 重新排满每一行，`sentence` 每个句子单独一行。

## 用法

`tola fmt` 按这里的样式书写站点自己的 `.typ` 文件，语言服务也用它格式化编辑器里正在编辑的那个文件：

```toml
[formatter]
indent = 2
print-width = 120
prose-wrap = "none"
```

没有 `tola.toml` 的目录就按上面这份默认配置格式化：在这个目录里运行命令，它用这些默认值格式化该目录下的 `.typ` 文件。
