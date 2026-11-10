# `@tola/uuid`

确定性的 UUID 规范化与版本 5 名称派生

## `normalize`

将 UUID 字符串转换为小写、带连字符的标准格式。支持 32 位十六进制字符串、带连字符的字符串，以及带连字符并加花括号或 `urn:uuid:` 前缀的写法；无效输入会报错，不自动去除空白。

### `value`

要规范化的 UUID 字符串。

## `v5`

根据命名空间和名称确定性计算版本 5 UUID，返回小写带连字符的字符串。字符串名称使用 UTF-8 编码，不进行 Unicode 规范化；字节名称按原样使用。任一输入变化都会改变身份；若名称或路径会变化，应保存 ID。

```typst
#import "@tola/uuid:0.0.0": namespace, v5
#assert.eq(v5(namespace.dns, "www.example.com"), "2ed6657d-e927-568b-95e1-2665a8aea6a2")
```

### `namespace`

命名空间 UUID 字符串。

### `name`

字符串或字节形式的名称。

## `namespace`

标准命名空间 UUID 字符串字典，包含 dns、url、oid 和 x500。
