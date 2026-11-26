#show: package(
  summary: [Compute BLAKE3 digests and encode bytes as lowercase hexadecimal strings.],
)

#export(
  "blake3",
  summary: [Return the 32-byte BLAKE3 digest of `value`.],
  params: (
    value: [Input bytes to hash. Convert strings with `bytes(text)` first.],
  ),
  related: ("bytes-to-hex",),
)[
  #example(title: [Hash a string and format it as hex], check: "default")[
  ```typst
  #import "@tola/hash:0.0.0": blake3, bytes-to-hex

  #let digest = bytes-to-hex(blake3(bytes("hello")))
  ```
  ]
]

#export(
  "bytes-to-hex",
  summary: [Encode `value` as lowercase hexadecimal text (two characters per byte).],
  params: (
    value: [Bytes to encode.],
  ),
  related: ("blake3",),
)
