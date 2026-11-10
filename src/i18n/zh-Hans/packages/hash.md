# `@tola/hash`

BLAKE3 摘要，以及它们的小写十六进制写法

## `blake3`

用 BLAKE3 对字节求摘要。

结果是 32 字节，也就是 `bytes-to-hex` 写成 64 个十六进制字符的同一份摘要。

Example - 字符串与它的 UTF-8 编码得到相同摘要：

```typst
#import "@tola/hash:0.0.0": blake3
#assert.eq(blake3(bytes("abc")).len(), 32)
```

### `value`

要求摘要的字节。

## `bytes-to-hex`

把字节写成小写十六进制，每字节两位。

Example：

```typst
#import "@tola/hash:0.0.0": bytes-to-hex
#assert.eq(bytes-to-hex(bytes((1, 255))), "01ff")
```

### `value`

要写成十六进制的字节。
