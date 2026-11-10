# `@tola/schema`

声明值的形状，再把值解析进去。声明是可以复用的 Typst 值：你可以给它取名、保存它，也可以把它嵌进另一份声明。

从 `schema` 开始：它为每个字段命名，并给每个字段一份声明，可以是 `str` 这样的 Typst 类型，也可以用下面的组合子搭出来。键可能缺失时加 `optional`，键可能是 `none` 时加 `nullable`。

内部形状交给 `array-of`、`dictionary-of`、`tuple`、`one-or-many`、`enum-of` 和 `literal`。要按顺序尝试多个分支用 `union`，要按 tag 值选中一个分支用 `variant`。

嵌套就是普通的函数调用，内层声明先做决定：`optional(array-of(trim(str)))` 接受缺失的键，也接受成员都经过 trim 的数组。

把值和声明交给 `parse` 得到规范化值，或交给 `try-parse` 得到 Result。你传入的值不会被改动；规范化值是一个新值。

包装一份声明可以改变它的值如何被检查、如何被塑造。`trim` 与 `non-empty` 做规范化和拒绝，`refine` 与 `check` 用回调做校验，`map` 做变换，`convert` 做可能被拒绝的变换。包装器从内向外执行：`non-empty(trim(str))` 先 trim，再拒绝空字符串。

失败以 Issue 表示，用 `format-issues` 渲染成文本。`describe` 为值附加文档，`inspect` 只从声明本身推导输出形状，不运行任何回调。声明本身写错、回调里有编程错误都会中止编译；只有值层面的失败才成为 Issue。外层 Typst 上下文重新求值时，回调可能再次运行，所以每次解析都从头执行。

解析发生在值进入的地方：根程序先解析全部源的 metadata，之后的选择与模板直接读取解析结果，不再重复校验。

声明和解析只作用于你传入的值；源的 metadata 请用 `@tola/source` 的 `parse-sources`。

## `any`

接受任何存在的值，包括 `none`，原样返回。

不会拒绝任何值；对象中缺失的键仍然需要 `optional`。

Example - 接受任何存在的值：

```typst
#import "@tola/schema:0.0.0": any, parse
#assert.eq(parse(none, any), none)
#assert.eq(parse((answer: 42), any), (answer: 42))
```

## `schema`

声明字典字段，字段按你书写的顺序求值，返回规范化后的字典。

`fields` 为每个键给出一个 Typst 类型或另一份声明；`unknown` 决定你没有声明的键：`"error"` 拒绝多余键，`"keep"` 原样保留但不校验它们的值。声明的键缺失时报错，除非它的声明接受缺失；声明过的值存在时，必须满足对应的声明。

Example - 校验页面 metadata，并补上缺失的 tag 列表：

```typst
#import "@tola/schema:0.0.0": schema, parse, non-empty, trim, optional, array-of
#let page-schema = schema((
  title: non-empty(trim(str)),
  tags: optional(array-of(str), default: ()),
))
#assert.eq(parse((title: " Notes "), page-schema), (title: "Notes", tags: ()))
```

Example - 用 `unknown` 保留未声明的键：

```typst
#import "@tola/schema:0.0.0": parse, schema, try-parse
#let page = schema((title: str), unknown: "keep")
#assert.eq(parse((title: "Hello", draft: true), page), (title: "Hello", draft: true))
#assert.eq(try-parse((title: "Hello", draft: true), schema((title: str))).ok, false)
```

### `fields`

为每个字段给出一个 Typst 类型或另一份 schema 声明。

### `unknown`

`"error"` 拒绝多余的键；`"keep"` 原样保留，但不校验它们的值。

## `optional`

允许对象键缺失；键存在时，返回内部声明的值。

`default: value` 为缺失的键提供值，且只在键缺失时经内部 schema 解析。值存在时一律走内部 schema，无效就报错。默认表达式在你构造声明时求值；作为默认值传入的函数会当成普通值保留。

没有默认值时，缺失的键在规范化字典中保持缺失。存在的 `none`、`auto`、空字符串或空数组都是值，会进入内部 schema：所以 `optional(nullable(str))` 接受缺失或 `none`，而 `optional(str)` 只接受缺失或字符串。可选的数组、元组成员仍占用必需的位置。

Example - 为缺失的键提供默认值：

```typst
#import "@tola/schema:0.0.0": schema, optional, parse
#let settings = schema((retries: optional(int, default: 3)))
#assert.eq(parse((:), settings), (retries: 3))
```

Example - 区分键缺失与显式 `none`：

```typst
#import "@tola/schema:0.0.0": nullable, optional, parse, schema, try-parse
#let skippable = schema((nickname: optional(str)))
#assert.eq(parse((:), skippable), (:))
#assert.eq(try-parse((nickname: none), skippable).ok, false)
#let explicit = schema((nickname: optional(nullable(str))))
#assert.eq(parse((nickname: none), explicit), (nickname: none))
```

### `member`

值存在时交给它解析、键缺失时跳过的 schema。

## `nullable`

看到显式的 `none` 就直接接受，不再交给内部 schema。

其他任何值，包括对象中缺失的键，都会进入内部 schema。

Example - 接受显式的 none：

```typst
#import "@tola/schema:0.0.0": schema, optional, nullable, parse
#let profile = schema((nickname: optional(nullable(str))))
#assert.eq(parse((nickname: none), profile), (nickname: none))
#assert.eq(parse((:), profile), (:))
```

### `member`

除显式的 `none` 外，其他任何值都交给它解析的 schema。

## `literal`

接受等于所声明 Typst 值的值，原样返回。

其他值都按相等性判为不匹配。

Example - 接受一个精确的值：

```typst
#import "@tola/schema:0.0.0": literal, parse, try-parse
#assert.eq(parse("draft", literal("draft")), "draft")
#assert.eq(try-parse("final", literal("draft")).ok, false)
```

### `value`

按相等性接受的 Typst 值。

## `enum-of`

接受等于 `values` 中某一项的值，原样返回。

其他值一律拒绝；`values` 为空数组属于编程错误。

Example - 把值限制在固定集合中：

```typst
#import "@tola/schema:0.0.0": enum-of, parse, try-parse
#let channel = enum-of(("r", "g", "b"))
#assert.eq(parse("g", channel), "g")
#assert.eq(try-parse("x", channel).ok, false)
```

### `values`

可接受值的非空数组。

## `array-of`

按索引顺序解析数组的每个成员，返回由规范化成员组成的新数组。

某个成员失败时贡献它自己的问题，问题路径以该成员的索引开头。空数组算成功；要求至少一个成员时，用 `non-empty(array-of(member))`。其他类型的值算类型问题。

Example - 规范化数组的每个成员：

```typst
#import "@tola/schema:0.0.0": array-of, trim, parse
#assert.eq(parse((" a ", "b"), array-of(trim(str))), ("a", "b"))
```

Example - 用 `non-empty` 拒绝空数组：

```typst
#import "@tola/schema:0.0.0": array-of, non-empty, parse, try-parse
#assert.eq(parse((), array-of(str)), ())
#assert.eq(try-parse((), non-empty(array-of(str))).ok, false)
```

### `member`

数组每个成员的 Typst 类型或 schema 声明。

## `dictionary-of`

按插入顺序解析字典的每个值，返回保留每个字符串字面量键的新字典。

某个值失败时，在该键下贡献它自己的问题。空字典算成功。键各有不同要求时改用 `schema`；`dictionary-of(int)` 接受任意键，只要值是整数。其他类型的值算类型问题。

Example - 规范化字典的每个值：

```typst
#import "@tola/schema:0.0.0": dictionary-of, trim, parse
#assert.eq(parse((first: " Ada "), dictionary-of(trim(str))), (first: "Ada"))
```

Example - 失败的值报告在它所在的键上：

```typst
#import "@tola/schema:0.0.0": dictionary-of, parse, try-parse
#let counts = dictionary-of(int)
#assert.eq(parse((one: 1, two: 2), counts), (one: 1, two: 2))
#let failure = try-parse((one: 1, two: "two"), counts)
#assert.eq(failure.ok, false)
#assert.eq(failure.issues.at(0).path, ("two",))
```

### `member`

字典每个值的 Typst 类型或 schema 声明。

## `one-or-many`

数组按多个成员接受，标量按一个成员接受，返回数组。

只有非数组值会走标量路径，标量失败不带数组索引。`parse("tag", one-or-many(str))` 返回 `("tag",)`，空数组返回 `()`。

Example - 接受单个成员或成员数组：

```typst
#import "@tola/schema:0.0.0": one-or-many, parse
#assert.eq(parse("tag", one-or-many(str)), ("tag",))
#assert.eq(parse(("a", "b"), one-or-many(str)), ("a", "b"))
```

### `member`

解析每个成员时使用的 Typst 类型或 schema 声明。

## `tuple`

解析固定位置上的成员，以及可选的、后面成员共用的 `rest`，返回规范化后的数组。

固定成员决定成员数的下限，即使它们的 schema 是可选的；`rest` 为 `none` 时成员数固定，多余成员一律拒绝。成员数不对时，存在的成员仍会逐个检查：`schema.arity` 报告成员数问题，同时保留各成员的问题。

Example - 解析固定位置与同质 rest：

```typst
#import "@tola/schema:0.0.0": tuple, parse, try-parse
#let pair = tuple(str, int, rest: bool)
#assert.eq(parse(("x", 2, true), pair), ("x", 2, true))
#assert.eq(try-parse(("x",), pair).ok, false)
```

Example - 不写 `rest` 时成员数固定：

```typst
#import "@tola/schema:0.0.0": parse, tuple, try-parse
#let pair = tuple(str, int)
#assert.eq(parse(("x", 2), pair), ("x", 2))
#assert.eq(try-parse(("x", 2, true), pair).ok, false)
```

### `rest`

固定位置之后成员的 schema；`none` 禁止多余成员。

## `lazy`

把零参数 schema 工厂推迟到真正解析持有它的节点时再调用。

工厂返回的 schema 决定这个值，它可以是递归 schema。工厂返回的不是合法声明属于编程错误。

Example - 带可选子节点的树：

```typst
#import "@tola/schema:0.0.0": schema, optional, array-of, lazy, parse
#let node-schema() = schema((
  name: str,
  children: optional(array-of(lazy(node-schema)), default: ()),
))
#assert.eq(parse((name: "root"), node-schema()), (name: "root", children: ()))
```

### `factory`

零参数 schema 工厂；它可以返回递归 schema，不需要注册表，不需要给默认值做缓存，也不用自己设递归上限。

## `union`

接受第一个成功的分支，也包括允许键缺失的分支。

分支按你传入的顺序依次尝试，所以多个分支都接受同一个值时，顺序也决定规范化结果。键缺失时只报告一条 `is required`，并丢掉那些只因这个缺失而失败的分支；自带备注或子问题的分支保留自己的报告。因其他原因失败的分支都会贡献问题。

返回第一个成功分支的值。分支里的编程错误会直接抛出，而不是让该分支判为不匹配。带 tag 的对象只应运行一个分支时用 `variant`；schema 按位置传入，例如 `union(str, int)`。

Example - 取第一个成功的分支：

```typst
#import "@tola/schema:0.0.0": union, trim, parse
#assert.eq(parse(" x ", union(trim(str), str)), "x")
```

Example - 在两个对象形状中选择：

```typst
#import "@tola/schema:0.0.0": parse, schema, union
#let contact = union(schema((email: str)), schema((phone: str)))
#assert.eq(parse((email: "ada@example.com"), contact), (email: "ada@example.com"))
#assert.eq(parse((phone: "555-0100"), contact), (phone: "555-0100"))
```

## `variant`

按显式 tag 选择一个对象分支。

`tag-key` 是选择分支的字符串键；每个分支都必须把它声明为互不相同的必需 `literal(...)`。分支写成 `schema(...)` 声明，可以用 `describe` 注解。

tag 缺失或未知时，只在该键处报告，其他分支一律跳过。返回被选中分支的值。校验和转换要放进各自字段里，或者放在整个 variant 之外。

Example - 选择一种对象形状，而不求值无关分支：

```typst
#import "@tola/schema:0.0.0": variant, schema, literal, parse
#let block-schema = variant("kind",
  schema((kind: literal("text"), body: str)),
  schema((kind: literal("image"), width: int)),
)
#assert.eq(parse((kind: "text", body: "Hello"), block-schema).body, "Hello")
```

Example - tag 缺失时报在它所在的键上：

```typst
#import "@tola/schema:0.0.0": literal, schema, try-parse, variant
#let block-schema = variant("kind", schema((kind: literal("text"), body: str)))
#let failure = try-parse((body: "Hello"), block-schema)
#assert.eq(failure.ok, false)
#assert.eq(failure.issues.at(0).path, ("kind",))
```

### `tag-key`

选择分支的字面量字符串键。

## `trim`

对成功的字符串做 trim 并返回。

其他类型的值，包括 `none`，一律判失败。

Example - 对成功的字符串做 trim：

```typst
#import "@tola/schema:0.0.0": trim, parse, try-parse
#assert.eq(parse("  padded\n", trim(str)), "padded")
#assert.eq(try-parse(7, trim(str)).ok, false)
```

### `member`

要 trim 其成功字符串的 schema。

## `non-empty`

要求成功的字符串、数组或字典非空，并原样返回。

空字符串、空数组或空字典判失败，其他类型的值同样判失败。

顺序很重要：`non-empty(trim(str))` 拒绝只有空白字符的文本；`trim(non-empty(str))` 接受这段文本，并返回空字符串。

Example - 拒绝空值：

```typst
#import "@tola/schema:0.0.0": non-empty, try-parse
#assert.eq(try-parse("x", non-empty(str)).ok, true)
#assert.eq(try-parse("", non-empty(str)).ok, false)
```

### `member`

规范化结果必须是非空字符串、数组或字典的 schema。

## `refine`

谓词返回 `false` 时拒绝成功的值。

成功的值原样返回。谓词返回非布尔值，或者回调 panic，都属于编程错误。

请显式传入字符串 `message`。虽然声明签名将它默认为 `none`，构造声明时仍会拒绝 `none` 和省略消息的调用。

Example - 用谓词拒绝值：

```typst
#import "@tola/schema:0.0.0": refine, parse, try-parse
#let even = refine(int, value => calc.rem(value, 2) == 0, message: "must be even")
#assert.eq(parse(4, even), 4)
#assert.eq(try-parse(5, even).ok, false)
```

### `member`

成功值会传给谓词的 schema。

### `predicate`

施加在成功值上的布尔谓词。

### `message`

必须提供的字符串消息；谓词返回 `false` 时报告为 `custom` 问题。

## `check`

检查已声明字段的成功值，并报告回调给出的 Issues。

`on` 接收一组互不相同的已声明字段键，它们的成功值组成回调的输入；省略 `on` 就检查整个成功值。指定的字段取自同一次对象解析，即使某个无关字段失败也一样。依赖的字段失败时，回调不运行；缺失的字段保持缺失。成功返回 `()`，失败返回 `(issue(...), ...)`；路径以正在检查的值为基准。

Example - 在两个字段解析完成后校验它们的关系：

```typst
#import "@tola/schema:0.0.0": schema, check, issue, try-parse
#let interval = check(schema((start: int, end: int)), bounds => {
  if bounds.end >= bounds.start { () }
  else { (issue("must not precede start", path: ("end",)),) }
}, on: ("start", "end"))
#assert.eq(try-parse((start: 2, end: 1), interval).ok, false)
```

### `member`

回调会检查其成功值的 schema。

### `inspect`

接收成功值并返回 Issues 数组的回调。

### `on`

一组互不相同的已声明字段键；它们的成功值组成回调的输入。`on` 不能跨越 conversion、union、variant 或 lazy 边界。

## `map`

把内部成功值转换一次，再让结果经 `output` 解析。

转换器可以是函数，也可以是可调用的 Typst 类型；`output` 默认为 `any`。成功时返回转换后的值；内部失败或未通过 `output` 都会报告问题。形如 Result 的字典在这里是普通值：例如 `parse(7, map(int, str, output: non-empty(str)))` 返回 `"7"`。

Example - 用普通函数转换成功值：

```typst
#import "@tola/schema:0.0.0": map, parse
#assert.eq(parse(7, map(int, str, output: str)), "7")
```

### `member`

成功值会传给转换器的 schema。

### `convert-value`

返回普通值的函数，或可调用的 Typst 类型。

### `output`

转换后的值要经其解析的 schema。

## `convert`

用一个返回 Result 的回调把内部成功值转换一次，再让结果经 `output` 解析。

回调可以是函数，也可以是可调用的 Typst 类型；`output` 默认为 `any`。成功时返回转换后的值。返回 `err(...)` 表示值层面的失败；回调里 panic 属于编程错误，会直接穿过 `try-parse`。回调返回普通值时请用 `map`。

Example - 以结构化原因拒绝一次转换：

```typst
#import "@tola/schema:0.0.0": convert, ok, err, issue, try-parse
#let port = convert(int, value => {
  if value >= 0 and value <= 65535 { ok(str(value)) }
  else { err(issue("must be between 0 and 65535")) }
}, output: str)
#assert.eq(try-parse(443, port), (ok: true, value: "443"))
```

Example - 把回调拒绝时给出的 Issue 带回结果：

```typst
#import "@tola/schema:0.0.0": convert, err, issue, ok, try-parse
#let port = convert(int, value => {
  if value >= 0 and value <= 65535 { ok(str(value)) }
  else { err(issue("must be between 0 and 65535", code: "port.range")) }
}, output: str)
#let failure = try-parse(70000, port)
#assert.eq(failure.ok, false)
#assert.eq(failure.issues.at(0).code, "port.range")
```

### `member`

成功值会传给转换器的 schema。

### `convert-value`

返回 `ok(value)` 或 `err(..issues)` 的回调。

### `output`

转换后的成功值要经其解析的 schema。

## `describe`

为声明的成功值附加文档。

成功的值原样返回。同一节点上，外层描述会替换内层描述。要让嵌套字段有自己的文档，单独 describe 那个字段；诊断信息请用 `issue(..., notes: ...)`。

Example - 为成功值附加文档：

```typst
#import "@tola/schema:0.0.0": describe, parse
#let slug = describe(str, "lowercase URL segment")
#assert.eq(parse("post", slug), "post")
```

### `member`

成功值由这条描述记录的 schema。

### `description`

附加到成功值上的文档。

## `inspect`

只从声明本身推导 schema 的形状，不运行任何回调。

返回树的每个节点都带 `kind`、`presence` 与 `description`。Typst 类型的节点还带 `type`；对象带 `fields` 与 `unknown`；容器带自己的成员；union 与 variant 保留各自的备选。lazy 工厂保持 `kind: "unknown"`。

`presence: "required"` 表示成功的对象字段一定存在；`"optional"` 表示一条已声明的省略路径，更早的 union 分支可能遮蔽它；`"unknown"` 表示无法证明。`default: (value: ...)` 给出原始声明的默认值，包括 `none`，不做校验也不做转换；缺失的默认键保持缺失。容器的位置始终存在。map 与 convert 描述各自的输出形状，同时保留输入的 presence 与 default。输入文档永远不会描述生成的输出。

默认描述成功解析后的值。传入 `input: true` 可查看解析前的形状，此时有默认值的字段可以省略，转换描述的是输入。两种描述都来自声明本身，具体值能否通过校验由解析决定。

Example - 推导输出描述而不运行回调：

```typst
#import "@tola/schema:0.0.0": schema, optional, inspect
#let declaration = schema((title: str, subtitle: optional(str), draft: optional(bool, default: false)))
#let profile = inspect(declaration)
#assert.eq(profile.fields.title.presence, "required")
#assert.eq(profile.fields.subtitle.presence, "optional")
#assert.eq(profile.fields.draft.presence, "required")
#assert.eq(inspect(declaration, input: true).fields.draft.presence, "optional")
```

### `member`

要描述的 schema。

### `input`

为 true 时描述解析前的值，默认描述成功解析后的值。

## `parse`

解析一次并返回完整的规范化值；失败时，以 `format-issues` 的输出作为失败信息。

Example - 一次调用返回规范化值：

```typst
#import "@tola/schema:0.0.0": non-empty, trim, parse
#assert.eq(parse("  Notes  ", non-empty(trim(str))), "Notes")
```

### `value`

要解析的值。

### `member`

值要经其解析的 schema。

## `try-parse`

解析一次后，精确返回 `(ok: true, value: value)` 或 `(ok: false, issues: issues)` 之一。

失败时 issues 非空，也绝不会连同部分值一起返回。schema 校验失败会以 Result 返回；声明本身写错、回调返回形状非法或回调 panic 仍会中止编译。输入总是存在的，包括 `none`；只有解析对象时才会遇到缺失的键。失败请用 `format-issues` 渲染；schema 自己生成的值会在文本里说明，而不会伪装成输入提供的值。

Example - 返回 Result 而不是失败：

```typst
#import "@tola/schema:0.0.0": try-parse
#assert.eq(try-parse(7, int), (ok: true, value: 7))
#assert.eq(try-parse("7", int).ok, false)
```

### `value`

要解析的值。

### `member`

值要经其解析的 schema。

## `partial`

先固定一部分字段，其余字段在返回的函数被调用时补上。

返回的函数接收剩余字段（具名参数），并与固定字段一起解析，所以两次调用都没给的键仍由声明的默认值填充。给第一次已固定的字段再次赋值是错误，固定字段不会被静默覆盖。

Example - 让许多页面共享同一个版式：

```typst
#import "@tola/schema:0.0.0": optional, partial, schema
#let page-schema = schema((title: str, layout: optional(str, default: "page")))
#let note = partial(page-schema, layout: "note")
#assert.eq(note(title: "Notes"), (title: "Notes", layout: "note"))
```

### `member`

字段要经其检查的对象 schema。

## `defaults`

报告每个被省略的键会取什么值。

只列出声明了默认值的键，且列出的是声明里原样写下的默认值，而不是解析后的值。嵌套对象自身的默认值只在提供该对象时生效，所以不在这里列出：除非声明是带默认值的 optional，否则对象本身仍然必需。

Example - 读取声明提供的回退值：

```typst
#import "@tola/schema:0.0.0": array-of, defaults, optional, schema
#let page-schema = schema((title: str, tags: optional(array-of(str), default: ())))
#assert.eq(defaults(page-schema), (tags: ()))
```

### `member`

要读取默认值的对象 schema。

## `ok`

用任意值构造一个成功的 Result。

`ok(none)` 是一次成功，值是 `none`。

Example - 构造成功的 Result：

```typst
#import "@tola/schema:0.0.0": ok
#assert.eq(ok(none), (ok: true, value: none))
```

### `value`

成功 Result 的值。

## `err`

用一个或多个 Issues 构造失败的 Result。

不传 Issue 的 `err()` 是编程错误。Result 有 `ok: false` 和这些 Issues。

Example - 由 Issues 构造失败的 Result：

```typst
#import "@tola/schema:0.0.0": err, issue
#let failure = err(issue("too small", code: "size", path: ("count",)))
#assert.eq(failure.ok, false)
#assert.eq(failure.issues.at(0).code, "size")
```

## `issue`

构造一个 Issue：schema 报告的单个问题。

路径以正在检查的值为基准：`("author.name",)` 命名一个键，`("author", "name")` 命名嵌套键，`("authors", 0)` 命名数组成员。要加入嵌套原因，把 Issue 展开进字典，再把 `children` 换成 Issues 数组。`issue()` 描述输入提供的值；schema 生成值时会在 `origin` 里记录声明默认值或转换。

Example - 构造带路径的 Issue：

```typst
#import "@tola/schema:0.0.0": issue
#let problem = issue("duplicate tag", path: ("tags", 0), notes: ("remove the repeat",))
#assert.eq(problem.path, ("tags", 0))
```

### `message`

人类可读的问题描述。

### `code`

机器可读的问题代码。

### `path`

以正在检查的值为基准的字面量字符串键或非负索引。

### `notes`

字符串备注数组，默认为空。

## `format-issues`

把 Issue 渲染成站点作者可读的确定文本。

普通 issue 占一行，子原因缩进排在下面。来自声明默认值或转换的失败，先给字段和原因；只有需要时才会提到 schema 的默认值或转换，不会重复内部消息。其它原因和备注缩进排在下面。Issue 本身不合法会中止编译。

Example - 把 Issue 渲染为文本：

```typst
#import "@tola/schema:0.0.0": issue, format-issues
#let problem = issue("is required", code: "schema.missing", path: ("title",))
#assert.eq(format-issues((problem,)), "`$.title`: is required")
```

Example - 渲染嵌套原因与备注：

```typst
#import "@tola/schema:0.0.0": format-issues, issue
#let cause = issue("is required", code: "schema.missing", path: ("title",), notes: ("add a title",))
#let tree = (..issue("metadata is invalid", code: "metadata.invalid"), children: (cause,))
#assert.eq(format-issues((tree,)), "`$`: metadata is invalid\n  `$.title`: is required\n    note: add a title")
```

Example - 先给出声明默认值的原因：

```typst
#import "@tola/schema:0.0.0": format-issues, optional, schema, try-parse
#let cause = try-parse((:), schema((count: optional(int, default: "bad")))).issues.first()
#assert.eq(format-issues((cause,)), "`$.count` is not set, and the schema's default value is invalid: expected `int`, found `str`")
```

### `issues`

要渲染的 Issue 值。

## `min-length`

接受至少 `minimum` 个字素簇、数组成员或字典条目的值，短于它的值一律拒绝。

其他类型的值算类型问题。被度量的值原样返回。

Example - 要求最小长度：

```typst
#import "@tola/schema:0.0.0": min-length, try-parse
#assert.eq(try-parse("ab", min-length(str, 3)).ok, false)
#assert.eq(try-parse("abc", min-length(str, 3)).ok, true)
```

### `member`

要度量其成功值的 schema。

### `minimum`

非负的最小簇、成员或条目数。

## `max-length`

接受至多 `maximum` 个字素簇、数组成员或字典条目的值，长于它的值一律拒绝。

其他类型的值算类型问题。被度量的值原样返回。

Example - 限制值的最大长度：

```typst
#import "@tola/schema:0.0.0": max-length, try-parse
#assert.eq(try-parse((1, 2), max-length(array, 2)).ok, true)
#assert.eq(try-parse((1, 2, 3), max-length(array, 2)).ok, false)
```

### `member`

要度量其成功值的 schema。

### `maximum`

非负的最大簇、成员或条目数。

## `matches`

要求字符串匹配一个 Typst regex 值；字符串形式的模式一律拒绝。

匹配搜索字符串中任意位置，要约束整串请加 anchor。匹配直接作用于解析后的字符串本身，通过后原样返回。

Example - 要求匹配正则表达式：

```typst
#import "@tola/schema:0.0.0": matches, try-parse
#let slug = matches(str, regex("^[a-z0-9-]+$"))
#assert.eq(try-parse("my-post", slug).ok, true)
#assert.eq(try-parse("My Post", slug).ok, false)
```

### `member`

成功字符串要参与匹配的 schema。

### `expression`

在字符串中搜索的正则表达式。

## `at-least`

要求整数或浮点数大于或等于 `minimum`。

通过的值原样返回。NaN 判失败；无穷与普通数字一样比较。

Example - 施加包含下界：

```typst
#import "@tola/schema:0.0.0": at-least, try-parse
#assert.eq(try-parse(5, at-least(int, 5)).ok, true)
#assert.eq(try-parse(4, at-least(int, 5)).ok, false)
```

### `member`

其成功值要参与比较的 schema。

### `minimum`

下界，非 NaN 的整数或浮点数。

## `at-most`

要求整数或浮点数小于或等于 `maximum`。

通过的值原样返回。NaN 判失败；无穷与普通数字一样比较。

Example - 施加包含上界：

```typst
#import "@tola/schema:0.0.0": at-most, try-parse
#assert.eq(try-parse(5, at-most(int, 5)).ok, true)
#assert.eq(try-parse(6, at-most(int, 5)).ok, false)
```

### `member`

其成功值要参与比较的 schema。

### `maximum`

上界，非 NaN 的整数或浮点数。

## `email`

符合 RFC 5322 的未加引号 ASCII dot-atom 邮箱。

本地部分由非空、以点分隔的 atext 原子组成，至多 64 字节，后面是 `@` 和一个或多个 DNS 风格标签。每个标签由 1-63 个 ASCII 字母、数字或连字符组成，首尾是字母或数字；整个地址至多 254 字节，接受单标签域名。

只接受裸地址本身：引号、注释、显示名、域名字面量和 Unicode 都在语法之外。检查只做语法判断，不做 trim、DNS 查询或可投递性验证。

Example - 校验邮箱地址：

```typst
#import "@tola/schema:0.0.0": email, try-parse
#assert.eq(try-parse("ada@example.com", email).ok, true)
#assert.eq(try-parse("not an address", email).ok, false)
```

## `ipv4`

点分十进制 IPv4 地址：四个 0-255 的八位组，不允许多余的前导零。

通过的拼写原样返回。方括号、端口、zone 标识和首尾空白一律拒绝。

Example - 校验 IPv4 地址：

```typst
#import "@tola/schema:0.0.0": ipv4, try-parse
#assert.eq(try-parse("192.168.0.1", ipv4).ok, true)
#assert.eq(try-parse("192.168.0.256", ipv4).ok, false)
```

## `ipv6`

IPv6 地址：八个 1-4 位十六进制组，或者用一处 `::` 替换至少一个组。

严格点分十进制的 IPv4 后缀可以替换最后两组。拼写原样返回。方括号、端口、zone 标识和首尾空白一律拒绝。

Example - 校验 IPv6 地址：

```typst
#import "@tola/schema:0.0.0": ipv6, try-parse
#assert.eq(try-parse("2001:db8::1", ipv6).ok, true)
#assert.eq(try-parse("2001:db8::1::2", ipv6).ok, false)
```

## `ip`

IPv4 或 IPv6 地址，原样返回。

方括号、zone 标识和端口一律拒绝。

Example - 校验任一 IP 族：

```typst
#import "@tola/schema:0.0.0": ip, try-parse
#assert.eq(try-parse("::1", ip).ok, true)
#assert.eq(try-parse("127.0.0.1:80", ip).ok, false)
```

## `http-url`

绝对的 HTTP(S) URI/IRI，scheme 大小写不敏感，authority/host 必须非空。

RFC 3986 的各组成部分都支持：userinfo、registered name、方括号形式的 IPv6/IPvFuture、合法的百分号转义，以及留空的端口或 0-65535 的十进制端口。Unicode 部分使用 RFC 3987 的 ucschar，私有使用字符只能出现在 query 里。通过的拼写原样返回。空白和控制字符一律拒绝；检查只做语法判断，不做 trim、百分号解码或规范化，也不查 DNS。

Example - 校验绝对的 HTTP(S) URL：

```typst
#import "@tola/schema:0.0.0": http-url, try-parse
#assert.eq(try-parse("https://example.com/posts", http-url).ok, true)
#assert.eq(try-parse("example.com/posts", http-url).ok, false)
```

## `https-url`

绝对的 HTTPS URI/IRI，语法与 `http-url` 相同，scheme 只能是大小写不敏感的 `https`。

通过的拼写原样返回，检查不涉及网络和可投递性。

Example - 把 URL 限定为 HTTPS：

```typst
#import "@tola/schema:0.0.0": https-url, try-parse
#assert.eq(try-parse("https://example.com", https-url).ok, true)
#assert.eq(try-parse("http://example.com", https-url).ok, false)
```

## `url-path`

站点根 URL 路径，解析后返回解码的 route。

百分号转义只解码一次。路径必须以单个 `/` 开头，不包含 origin、query 或 fragment，并满足 Tola 可跨平台发布的文件名规则。文件名内部允许 Unicode 和空格；首尾空白与编码后的路径分隔符不被接受。解析不会移除部署前缀，也不检查 output 是否存在。

返回的 route 直接交给 `@tola/address` 的 `output-of` 或 `route-url`。文件名中的字面 `%`、`#` 在输入中分别写成 `%25`、`%23`。

Example - 解析 permalink 并收集字段错误：

```typst
#import "@tola/schema:0.0.0": parse, try-parse, schema, url-path
#let page = schema((permalink: url-path))
#assert.eq(parse((permalink: "/100%25/"), page).permalink, "/100%/")
#assert.eq(parse((permalink: "/a%2523b/"), page).permalink, "/a%23b/")
#assert.eq(try-parse((permalink: "/notes/#part"), page).issues.at(0).path, ("permalink",))
```

## `date`

日历日期，返回原生的纯日期 `datetime`。

接受不带时间的原生日期，或严格的 `YYYY-MM-DD` 字符串，年份范围为 1 到 9999，并检查日历日期是否合法。完整日期时间、纯时间值、时间戳字符串和首尾空白都会被拒绝，不会截去时间或自动 trim。

Example - 解析、显示日期并报告字段错误：

```typst
#import "@tola/schema:0.0.0": date, parse, schema, try-parse
#let published = parse("2026-10-14", date)
#assert.eq(published, datetime(year: 2026, month: 10, day: 14))
#assert.eq(published.display("[year]-[month]-[day]"), "2026-10-14")
#let checked = try-parse((published: "2026-02-30"), schema((published: date)))
#assert.eq(checked.issues.first().path, ("published",))
```

## `partial-date`

日历日期，保留输入声明的年、月或日粒度。

接受严格的 `YYYY`、`YYYY-MM`、`YYYY-MM-DD` 字符串，或不带时间的原生日期。年份范围为 1 到 9999，提供的月份和日期必须构成合法日历日期。完整日期时间、纯时间值、时间戳字符串和首尾空白都会被拒绝。

结果只包含声明的分量：`(precision: "year", year: int)`、`(precision: "month", year: int, month: int)` 或 `(precision: "day", year: int, month: int, day: int)`。原生日期具有日粒度。缺失的分量保持缺失；解析不会选择时间或时区。

Example - 保留日期粒度并报告数组成员错误：

```typst
#import "@tola/schema:0.0.0": array-of, parse, partial-date, schema, try-parse
#assert.eq(parse("2026", partial-date), (precision: "year", year: 2026))
#assert.eq(parse("2026-11", partial-date), (precision: "month", year: 2026, month: 11))
#assert.eq(parse("2024-02-29", partial-date), (precision: "day", year: 2024, month: 2, day: 29))
#let checked = try-parse((dates: ("2026", "2026-02-30")), schema((dates: array-of(partial-date))))
#assert.eq(checked.issues.first().path, ("dates", 1))
```

## `timestamp`

时间戳，返回 `(rfc3339: str, unix-seconds: int, subsecond: str)`。

接受含完整日历日期的原生 datetime，或严格的 RFC 3339 字符串。原生值按 UTC 解释，只有日期而没有时间时取午夜。字符串要求四位年份，范围为 1 到 9999，日期与时间用 `T` 或 `t` 分隔，时区为 `Z`、`z` 或数值偏移 `±hh:mm`。纯日期字符串、纯时间值、首尾空白和闰秒都会被拒绝。

`rfc3339` 保留合法字符串的原拼写；原生输入生成以 `Z` 结尾的 UTC 文本。`unix-seconds` 是自 Unix epoch 起的 UTC 整秒数。`subsecond` 保留全部小数数字，不含小数点并去掉尾随零；零用空字符串表示。排序先比较 `unix-seconds`，整秒相同时再按字符串比较 `subsecond`。解析和这种排序保留任意位小数精度。显示或传给 feed、sitemap 声明时用 `.rfc3339`；它们的时间戳格式化最多保留纳秒，RSS 日期只保留整秒。

Example - 比较时区偏移，并对带 18 位小数的时间戳排序：

```typst
#import "@tola/schema:0.0.0": timestamp, parse, schema, try-parse
#let early = parse("2026-10-14T08:30:00.123456789012345678+08:00", timestamp)
#let late = parse("2026-10-14t00:30:00.123456789012345679z", timestamp)
#assert.eq(early.unix-seconds, late.unix-seconds)
#assert.eq(early.subsecond, "123456789012345678")
#let ordered = (late, early).sorted(key: moment => (moment.unix-seconds, moment.subsecond))
#assert.eq(ordered.map(moment => moment.rfc3339), (early.rfc3339, late.rfc3339))
#assert.eq(parse(datetime(year: 2026, month: 10, day: 14), timestamp).rfc3339,
  "2026-10-14T00:00:00Z")
#let checked = try-parse((published: "2026-10-14"), schema((published: timestamp)))
#assert.eq(checked.issues.first().path, ("published",))
```
