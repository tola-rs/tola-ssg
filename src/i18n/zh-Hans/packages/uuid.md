# `@tola/uuid`

统一 UUID 的写法，或按名称生成可重复的 UUID v5。

## `normalize`

将 UUID 字符串转换为小写、带连字符的标准格式。接受 32 个连续的十六进制字符，也接受带连字符的 UUID。带连字符的写法还可以加花括号或 `urn:uuid:` 前缀。首尾有空白会报错。

### `value`

要规范化的 UUID 字符串。

## `v5`

根据命名空间和名称生成 UUID v5。两项都相同就会得到同一个 UUID。名称可以直接传字节，也可以传字符串，字符串按原样编码为 UTF-8。页面改名后如果还要沿用原 ID，保存并继续使用第一次生成的 UUID。


在新站点中用下面的代码替换 `site.typ`，从站点根目录运行 `tola build`。使用默认发布目录时，`public/index.html` 会显示 `2ed6657d-e927-568b-95e1-2665a8aea6a2`。

```typst
#import "@tola/uuid:0.0.0": namespace, v5
#let id = v5(namespace.dns, "www.example.com")
#document("index.html")[#id]
```

### `namespace`

命名空间的 UUID 字符串，写法与 `normalize` 相同。可直接用 `namespace.dns` 等内置值。

### `name`

字符串或字节形式的名称。

## `namespace`

内置命名空间字典。按名称读取 UUID，例如 `namespace.dns`。可用 `dns`、`url`、`oid` 和 `x500`。
