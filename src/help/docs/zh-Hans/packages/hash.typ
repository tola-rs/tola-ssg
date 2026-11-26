#show: package(
  summary: [计算 BLAKE3 哈希摘要，以及将字节编码为小写十六进制字符串。],
)

#export(
  "blake3",
  summary: [返回输入字节的 32 字节 BLAKE3 摘要。],
  params: (
    value: [待计算摘要的字节。字符串可先用 `bytes(text)` 转为 UTF-8 字节。],
  ),
  related: ("bytes-to-hex",),
)[
  #example(title: [计算字符串的十六进制 BLAKE3 摘要], check: "default")[
  ```typst
  #import "@tola/hash:0.0.0": blake3, bytes-to-hex

  #let digest = bytes-to-hex(blake3(bytes("hello")))
  ```
  ]
]

#export(
  "bytes-to-hex",
  summary: [将字节编码为小写十六进制字符串（每字节两位）。],
  params: (
    value: [待编码的字节。],
  ),
  related: ("blake3",),
)
