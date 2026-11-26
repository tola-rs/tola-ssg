#show: package(
  summary: [规范化 UUID 字符串格式，以及按名称生成确定性的 UUID v5。],
)

#export(
  "normalize",
  summary: [将 UUID 字符串格式化为小写带连字符的标准形式。],
  params: (
    value: [UUID 字符串：支持 32 位十六进制、带连字符格式、`{...}` 花括号或 `urn:uuid:...` 前缀（不允许首尾空白）。],
  ),
  related: ("v5", "namespace"),
)

#export(
  "v5",
  summary: [根据命名空间 UUID 与名称生成确定性的 RFC 4122 UUID v5。],
  params: (
    namespace: [命名空间 UUID 字符串（格式同 `normalize`，或使用 `namespace.*` 内置值）。],
    name: [字符串（按 UTF-8 原样编码）或原始 `bytes`。],
  ),
  related: ("namespace", "normalize"),
)[
  相同的 `(namespace, name)` 始终生成同一 UUID。若希望页面改名后 feed 条目 ID 保持不变，请在元数据中持久化最初生成的 UUID。

  #example(title: [生成确定性的 DNS UUID v5], check: "default")[
  ```typst
  #import "@tola/uuid:0.0.0": namespace, v5

  #let id = v5(namespace.dns, "www.example.com")
  #document("index.html")[#id]
  ```
  ]
]

#export(
  "namespace",
  summary: [RFC 4122 标准命名空间 UUID 字典，包含 `dns`、`url`、`oid` 与 `x500`。],
  related: ("v5",),
)
