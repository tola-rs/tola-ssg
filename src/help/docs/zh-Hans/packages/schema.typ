#show: package(
  summary: [声明、组合、检查并执行元数据与其他 Typst 值的校验 schema。],
  related: ("@tola/source#parse-sources",),
)

向 `schema(...)` 传入字段规则字典，每个字段可填 Typst 类型（如 `str`、`int`）或规则函数（如 `array-of(str)`）。包装器由内向外求值：例如 `non-empty(trim(str))` 先去掉首尾空白，再拒绝空字符串。

用 `parse` 直接返回校验结果或在失败时报错；用 `try-parse` 捕获 `(ok:, value:)` / `(ok:, issues:)` 自行处理。校验内容文件元数据时，优先使用 #ref("@tola/source#parse-sources")，诊断会自动定位到源文件。

#example(title: [为文章元数据声明默认值与约束], check: "default")[
```typst
#import "@tola/schema:0.0.0": array-of, date, non-empty, optional, parse, schema, trim

#let post-schema = schema((
  title: non-empty(trim(str)),
  draft: optional(bool, default: false),
  published: optional(date),
  tags: optional(array-of(non-empty(trim(str))), default: ()),
))

#let meta = parse((title: "  笔记  "), post-schema)
```
]

#export(
  "any",
  summary: [原样接受任意 Typst 值的 schema。],
)

#export(
  "schema",
  summary: [声明字典字段到类型或子 schema 的映射规则。],
  params: (
    fields: [字段名到 Typst 类型或 `@tola/schema` 规则的字典。],
    unknown: [未声明字段的处理方式：`"error"`（默认，报错）、`"strip"`（丢弃）或 `"keep"`（原样保留）。],
  ),
  related: ("optional", "nullable", "partial", "defaults", "parse"),
)

#export(
  "optional",
  summary: [允许字典字段缺失，并可在缺失时提供默认值。],
  params: (
    member: [字段存在时应用的内部类型或 schema。],
    options: [可选具名参数 `default: 值`，字段缺失时采用（默认值本身须满足 `member`）。字段已提供但值无效时会报错，不会回退到默认值。],
  ),
  related: ("nullable", "schema", "defaults"),
)

#export(
  "nullable",
  summary: [在 `member` 允许的值之外，额外接受显式 `none`。],
  params: (
    member: [值不为 `none` 时应用的内部类型或 schema。],
  ),
  related: ("optional", "union"),
)

#export(
  "literal",
  summary: [仅匹配单个精确的 Typst 常量值。],
  params: (
    value: [期望匹配的精确值。],
  ),
  related: ("enum-of", "variant"),
)

#export(
  "enum-of",
  summary: [匹配等于 `values` 数组中任意一项的值。],
  params: (
    values: [允许值组成的非空数组。],
  ),
  related: ("literal", "union"),
)

#export(
  "array-of",
  summary: [校验数组，要求每个元素均满足 `member`。],
  params: (
    member: [数组元素的类型或 schema。],
  ),
  related: ("one-or-many", "tuple", "min-length", "max-length"),
)

#export(
  "dictionary-of",
  summary: [校验字典，要求每个键值均满足 `member`。],
  params: (
    member: [字典值对应的类型或 schema。],
  ),
  related: ("schema",),
)

#export(
  "one-or-many",
  summary: [接受单个 `member` 值或 `member` 数组，统一规范化为数组。],
  params: (
    member: [元素类型或 schema。],
  ),
  related: ("array-of",),
)

#export(
  "tuple",
  summary: [校验固定位置数组，各位置独立声明 schema，并支持可选尾部 `rest` 规则。],
  params: (
    members: [按索引顺序传入的各必填位置 schema。],
    rest: [超出固定位置后的剩余元素 schema；`none`（默认）拒绝多余元素。],
  ),
  related: ("array-of",),
)

#export(
  "lazy",
  summary: [延迟构造 schema，用于定义递归数据结构。],
  params: (
    factory: [按需调用的无参函数 `() => schema`。],
  ),
)

#export(
  "union",
  summary: [按顺序尝试多个候选 schema，采用首个匹配项。],
  params: (
    members: [两个及以上按从左到右尝试的候选类型或 schema。],
  ),
  related: ("variant", "enum-of"),
)

#export(
  "variant",
  summary: [根据判别字段（`tag-key`）的值选择对应的分支对象 schema。],
  params: (
    tag-key: [用于区分分支的字段名。],
    branches: [多个候选 `schema(...)`，各分支通过 `literal(...)` 或 `enum-of(...)` 固定 `tag-key`。],
  ),
  related: ("union", "literal", "schema"),
)

#export(
  "trim",
  summary: [先去除字符串首尾空白，再交给 `member` 校验。],
  params: (
    member: [内部字符串 schema（通常为 `str`）。],
  ),
  related: ("non-empty",),
)

#export(
  "non-empty",
  summary: [在通过 `member` 校验后，拒绝空字符串、空数组、空字典或空字节。],
  params: (
    member: [内部类型或 schema。],
  ),
  related: ("trim", "min-length"),
)

#export(
  "refine",
  summary: [为 `member` 附加布尔谓词校验。],
  params: (
    member: [先执行的内部类型或 schema。],
    predicate: [判断合法性的函数 `(value) => bool`。],
    message: [谓词返回 `false` 时的可选错误消息。],
  ),
  related: ("check", "map", "convert"),
)

#export(
  "check",
  summary: [为 `member` 附加可返回多条诊断的自定义检查函数。],
  params: (
    member: [先执行的内部类型或 schema。],
    inspect: [返回零条或多条 `issue(...)` 的函数 `(value) => array`。],
    on: [可选的附加字段路径或键名。],
  ),
  related: ("refine", "issue"),
)

#export(
  "map",
  summary: [在通过 `member` 校验后，用不会产生 schema 错误的转换函数映射值。],
  params: (
    member: [先执行的内部类型或 schema。],
    convert-value: [转换函数 `(value) => new-value`。],
    output: [可选输出类型 schema，供 `inspect` 描述输出形状。],
  ),
  related: ("convert", "refine"),
)

#export(
  "convert",
  summary: [在通过 `member` 校验后，用可失败的转换函数返回 `ok(...)` 或 `err(...)`。],
  params: (
    member: [先执行的内部类型或 schema。],
    convert-value: [返回 `ok(v)` 或 `err(...)` 的转换函数。],
    output: [可选输出类型 schema，供 `inspect` 描述输出形状。],
  ),
  related: ("ok", "err", "issue", "map"),
)

#export(
  "describe",
  summary: [为 `member` 附加供 `inspect` 读取的说明文字。],
  params: (
    member: [内部类型或 schema。],
    description: [非空说明字符串。],
  ),
  related: ("inspect",),
)

#export(
  "inspect",
  summary: [不解析实际数据，直接返回 schema 的字段、类型、默认值与说明结构。],
  params: (
    member: [待检查的 schema。],
    input: [设为 `true` 返回转换前的输入形状；`false`（默认）返回输出形状。],
  ),
  related: ("describe", "defaults"),
)

#export(
  "parse",
  summary: [按 `member` 校验 `value`，成功返回解析结果，失败则抛出格式化错误。],
  params: (
    value: [待校验的输入值。],
    member: [类型或 schema 声明。],
  ),
  related: ("try-parse", "format-issues", "@tola/source#parse-sources"),
)

#export(
  "try-parse",
  summary: [按 `member` 校验 `value` 而不中断求值，返回 `(ok: true, value: ...)` 或 `(ok: false, issues: ...)`。],
  params: (
    value: [待校验的输入值。],
    member: [类型或 schema 声明。],
  ),
  related: ("parse", "format-issues"),
)

#export(
  "partial",
  summary: [将对象 `schema` 的全部字段变为可选（`fixed` 列出的字段除外）。],
  params: (
    member: [由 `schema(...)` 创建的对象 schema。],
    fixed: [保持必填的可选字段名列表。],
  ),
  related: ("schema", "optional"),
)

#export(
  "defaults",
  summary: [根据对象 `schema` 生成默认字典（要求每个字段均为可选或带有默认值）。],
  params: (
    member: [待提取默认值的对象 schema。],
  ),
  related: ("schema", "optional", "inspect"),
)

#export(
  "ok",
  summary: [在 `convert` 回调中包装转换成功的结果值。],
  params: (
    value: [转换后的值。],
  ),
  related: ("err", "convert"),
)

#export(
  "err",
  summary: [在 `convert` 回调中包装一条或多条校验错误。],
  params: (
    issues: [一条或多条 `issue(...)` 记录（或消息字符串）。],
  ),
  related: ("ok", "issue", "convert"),
)

#export(
  "issue",
  summary: [构造供 `check` 或 `convert` 返回的结构化校验错误。],
  params: (
    message: [面向作者的问题描述。],
    code: [可选的错误代码标识。],
    path: [可选的字段或索引路径数组。],
    notes: [可选的补充说明字符串数组。],
  ),
  related: ("err", "check", "format-issues"),
)

#export(
  "format-issues",
  summary: [将 `try-parse` 返回的错误数组格式化为易读的多行文本。],
  params: (
    issues: [错误记录数组。],
  ),
  related: ("try-parse", "issue"),
)

#export(
  "min-length",
  summary: [要求字符串、数组、字典或字节长度不小于 `minimum`。],
  params: (
    member: [内部类型或 schema。],
    minimum: [非负最小长度。],
  ),
  related: ("max-length", "non-empty"),
)

#export(
  "max-length",
  summary: [要求字符串、数组、字典或字节长度不大于 `maximum`。],
  params: (
    member: [内部类型或 schema。],
    maximum: [非负最大长度。],
  ),
  related: ("min-length",),
)

#export(
  "matches",
  summary: [要求字符串匹配指定的正则表达式。],
  params: (
    member: [内部字符串 schema。],
    expression: [Typst `regex(...)` 或正则字符串。],
  ),
)

#export(
  "at-least",
  summary: [要求数值（`int` 或有限 `float`）大于等于 `minimum`。],
  params: (
    member: [内部数值 schema。],
    minimum: [闭区间下限。],
  ),
  related: ("at-most",),
)

#export(
  "at-most",
  summary: [要求数值（`int` 或有限 `float`）小于等于 `maximum`。],
  params: (
    member: [内部数值 schema。],
    maximum: [闭区间上限。],
  ),
  related: ("at-least",),
)

#export(
  "email",
  summary: [校验电子邮箱格式字符串的内置 schema。],
)

#export(
  "ipv4",
  summary: [校验点分十进制 IPv4 地址的内置 schema。],
  related: ("ipv6", "ip"),
)

#export(
  "ipv6",
  summary: [校验完整或 `::` 压缩 IPv6 地址的内置 schema。],
  related: ("ipv4", "ip"),
)

#export(
  "ip",
  summary: [同时接受 IPv4 与 IPv6 地址的内置 schema。],
  related: ("ipv4", "ipv6"),
)

#export(
  "http-url",
  summary: [校验绝对 `http://` 或 `https://` URL 的内置 schema。],
  related: ("https-url", "url-path"),
)

#export(
  "https-url",
  summary: [校验绝对 `https://` URL 的内置 schema。],
  related: ("http-url",),
)

#export(
  "url-path",
  summary: [校验站点根 URL 路径（如固定链接）并解码为可传给 `output-of` 或 `route-url` 的路由。],
  related: ("@tola/address#output-of", "@tola/address#decode-url-path"),
)

#export(
  "date",
  summary: [将 `"YYYY-MM-DD"` 或 Typst `datetime` 解析为日历日期（Typst `datetime`）的内置 schema。],
  related: ("partial-date", "timestamp"),
)

#export(
  "partial-date",
  summary: [将年份（`"YYYY"`）、年月（`"YYYY-MM"`）或完整日期解析为 `(year:, month:, day:, precision:, iso:)` 的内置 schema。],
  related: ("date", "timestamp"),
)

#export(
  "timestamp",
  summary: [将带时区偏移的 RFC 3339 时间戳字符串解析为 `(datetime:, offset:, rfc3339:)` 的内置 schema。],
  related: ("date", "@tola/feed#feed", "@tola/sitemap#sitemap"),
)[
  可直接将返回值的 `.rfc3339` 传给 #ref("@tola/feed#feed") 或 #ref("@tola/sitemap#sitemap")。
]
