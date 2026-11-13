# `@tola/hash`

计算 BLAKE3 摘要，把字节写成小写十六进制。

## `blake3`

返回输入的 32 字节 BLAKE3 摘要。

用 `bytes-to-hex` 可把摘要转成 64 个十六进制字符。


用 `bytes("abc")` 编码字符串，得到的 BLAKE3 摘要长 32 字节。

```typst
#import "@tola/hash:0.0.0": blake3
#assert.eq(blake3(bytes("abc")).len(), 32)
```

### `value`

要计算摘要的字节。字符串可先用 `bytes(text)` 转成 UTF-8 字节。

## `bytes-to-hex`

将字节转为小写十六进制文本，每字节两位。


```typst
#import "@tola/hash:0.0.0": bytes-to-hex
#assert.eq(bytes-to-hex(bytes((1, 255))), "01ff")
```

### `value`

要写成十六进制的字节。
