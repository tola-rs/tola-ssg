# `@tola/schema`

用 `schema` 规定字典里每个字段接受什么值。规则可以是 Typst 类型，例如 `str`，也可以是 `array-of(str)` 这样的组合。允许缺少键时用 `optional`，允许值为 `none` 时用 `nullable`。

`parse` 校验输入并返回解析后的值，失败时会报错。要自己处理失败，用 `try-parse`，再用 `format-issues` 把返回的 `issues` 转成文字。

规则从内向外执行。例如，`non-empty(trim(str))` 先去掉首尾空白，再检查字符串是否为空。`union` 按顺序尝试几种规则，`variant` 按字段值选一条规则。

要校验文章元数据，在 `site.typ` 中调用 `@tola/source` 的 `parse-sources`。

## `any`

接受任何存在的值，包括 `none`，原样返回。

字典里缺少的键仍要用 `optional`。

示例：接受任何存在的值：

```typst
#import "@tola/schema:0.0.0": any, parse
#assert.eq(parse(none, any), none)
#assert.eq(parse((answer: 42), any), (answer: 42))
```

## `schema`

按 `fields` 声明的顺序检查字典字段，返回解析后的字典。

`fields` 的每个值可以是 Typst 类型或其他 schema 规则。默认遇到未声明的键就报错。设 `unknown: "keep"` 会原样保留这些键和值。必填字段缺失时会报错。

示例：去掉标题首尾空白，并为标签填默认值：

下面的 schema 去掉标题首尾的空白，缺少 `tags` 时补上空数组。

```typst
#import "@tola/schema:0.0.0": schema, parse, non-empty, trim, optional, array-of
#let page-schema = schema((
  title: non-empty(trim(str)),
  tags: optional(array-of(str), default: ()),
))
#assert.eq(parse((title: " Notes "), page-schema), (title: "Notes", tags: ()))
```

示例：用 `unknown` 保留未声明的键：

```typst
#import "@tola/schema:0.0.0": parse, schema, try-parse
#let page = schema((title: str), unknown: "keep")
#assert.eq(parse((title: "Hello", draft: true), page), (title: "Hello", draft: true))
#assert.eq(try-parse((title: "Hello", draft: true), schema((title: str))).ok, false)
```

### `fields`

为每个字段给出一个 Typst 类型或另一份 schema 声明。

### `unknown`

省略或设为 `"error"` 时，遇到未声明的键就报错。设为 `"keep"` 时保留这些键，值不经校验。

## `optional`

允许省略一个键。键存在时，用 `member` 校验并返回结果。

用 `default` 给缺少的键补值，例如 `optional(int, default: 3)`。传入的值和用到的默认值都须通过 `member` 校验。默认值在声明规则时就算好。传函数作默认值时，保留该函数本身。

缺少的键如果没有默认值，结果里也没有这个键。`none`、`auto`、空字符串和空数组都是实际的值，会交给 `member` 校验。`optional(nullable(str))` 允许省略或传 `none`，`optional(str)` 允许省略或传字符串。数组和元组里的位置仍然需要填。

示例：为缺失的键提供默认值：

```typst
#import "@tola/schema:0.0.0": schema, optional, parse
#let settings = schema((retries: optional(int, default: 3)))
#assert.eq(parse((:), settings), (retries: 3))
```

示例：区分键缺失与显式 `none`：

```typst
#import "@tola/schema:0.0.0": nullable, optional, parse, schema, try-parse
#let skippable = schema((nickname: optional(str)))
#assert.eq(parse((:), skippable), (:))
#assert.eq(try-parse((nickname: none), skippable).ok, false)
#let explicit = schema((nickname: optional(nullable(str))))
#assert.eq(parse((nickname: none), explicit), (nickname: none))
```

### `member`

有值时用这条规则校验。键缺失且没有默认值时跳过。

## `nullable`

允许直接传入 `none`。

其余输入交给 `member` 校验。要允许省略键，再用 `optional`。

示例：接受 `none` 作为字段值：

```typst
#import "@tola/schema:0.0.0": schema, optional, nullable, parse
#let profile = schema((nickname: optional(nullable(str))))
#assert.eq(parse((nickname: none), profile), (nickname: none))
#assert.eq(parse((:), profile), (:))
```

### `member`

用于校验 `none` 以外的输入。

## `literal`

只接受等于指定 Typst 值的输入，原样返回。

示例：接受一个精确的值：

```typst
#import "@tola/schema:0.0.0": literal, parse, try-parse
#assert.eq(parse("draft", literal("draft")), "draft")
#assert.eq(try-parse("final", literal("draft")).ok, false)
```

### `value`

允许的值。

## `enum-of`

只接受 `values` 数组里的值，原样返回。

`values` 至少要有一项。

示例：把值限制在固定集合中：

```typst
#import "@tola/schema:0.0.0": enum-of, parse, try-parse
#let channel = enum-of(("r", "g", "b"))
#assert.eq(parse("g", channel), "g")
#assert.eq(try-parse("x", channel).ok, false)
```

### `values`

可接受值的非空数组。

## `array-of`

依次校验数组的每一项，返回解析后的数组。

某一项校验失败时，错误会指出该项的索引。默认接受空数组。要至少有一项，用 `non-empty(array-of(member))`。传入非数组值会报类型错误。

示例：规范化数组的每个成员：

```typst
#import "@tola/schema:0.0.0": array-of, trim, parse
#assert.eq(parse((" a ", "b"), array-of(trim(str))), ("a", "b"))
```

示例：用 `non-empty` 拒绝空数组：

```typst
#import "@tola/schema:0.0.0": array-of, non-empty, parse, try-parse
#assert.eq(parse((), array-of(str)), ())
#assert.eq(try-parse((), non-empty(array-of(str))).ok, false)
```

### `member`

数组每个成员的 Typst 类型或 schema 声明。

## `dictionary-of`

按原顺序校验字典的每个值，保留原来的键。

某个值校验失败时，错误会指出该键。空字典也能通过。例如，`dictionary-of(int)` 允许任意键，只要值是整数。各键需要不同规则时，用 `schema`。传入非字典值会报类型错误。

示例：规范化字典的每个值：

```typst
#import "@tola/schema:0.0.0": dictionary-of, trim, parse
#assert.eq(parse((first: " Ada "), dictionary-of(trim(str))), (first: "Ada"))
```

示例：在值不符合要求时指出对应的键：

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

输入可以是一个值或一个数组，结果都是数组。

例如，`parse("tag", one-or-many(str))` 返回 `("tag",)`。空数组返回 `()`。单个值失败时，报告里没有数组索引。

示例：接受单个成员或成员数组：

```typst
#import "@tola/schema:0.0.0": one-or-many, parse
#assert.eq(parse("tag", one-or-many(str)), ("tag",))
#assert.eq(parse(("a", "b"), one-or-many(str)), ("a", "b"))
```

### `member`

解析每个成员时使用的 Typst 类型或 schema 声明。

## `tuple`

可以为每个位置指定不同的规则，剩余成员共用可选的 `rest` 规则。返回解析后的数组。

数组至少要填满固定位置，即使用了 `optional`。省略 `rest` 时也不能多填。成员数不对时会报告 `schema.arity`，并继续检查已经填了的成员。

示例：为前几项各设规则，剩下的项共用一条规则：

```typst
#import "@tola/schema:0.0.0": tuple, parse, try-parse
#let pair = tuple(str, int, rest: bool)
#assert.eq(parse(("x", 2, true), pair), ("x", 2, true))
#assert.eq(try-parse(("x",), pair).ok, false)
```

示例：不写 `rest` 时成员数固定：

```typst
#import "@tola/schema:0.0.0": parse, tuple, try-parse
#let pair = tuple(str, int)
#assert.eq(parse(("x", 2), pair), ("x", 2))
#assert.eq(try-parse(("x", 2, true), pair).ok, false)
```

### `members`

固定位置的 schema，按顺序逐个传入，如 `tuple(str, int)`。

### `rest`

剩余成员共用的 schema。省略或传 `none` 时，成员数由 `members` 固定。

## `lazy`

等到校验当前值时，再调用一个无参数函数取得要用的 schema。

函数可以返回一条引用自身的规则，用于校验树之类的递归结构。返回值必须是有效的 schema。

示例：带可选子节点的树：

```typst
#import "@tola/schema:0.0.0": schema, optional, array-of, lazy, parse
#let node-schema() = schema((
  name: str,
  children: optional(array-of(lazy(node-schema)), default: ()),
))
#assert.eq(parse((name: "root"), node-schema()), (name: "root", children: ()))
```

### `factory`

返回 schema 的无参数函数，可以返回递归规则。

## `union`

按顺序尝试规则，返回第一条通过的结果。允许缺少键的规则也可能先通过。

所有规则都失败时，报告会列出失败原因。只因缺少同一个键而失败的分支通常合并成一条 `is required`，带备注或子问题的分支仍会单独报告。

规则写错或回调出错会中止编译。用 `union(str, int)` 这样的写法逐个传入规则。想按字典里的字段值选定一条规则时，用 `variant`。

示例：取第一个成功的分支：

```typst
#import "@tola/schema:0.0.0": union, trim, parse
#assert.eq(parse(" x ", union(trim(str), str)), "x")
```

示例：在两种字典结构中选择：

```typst
#import "@tola/schema:0.0.0": parse, schema, union
#let contact = union(schema((email: str)), schema((phone: str)))
#assert.eq(parse((email: "ada@example.com"), contact), (email: "ada@example.com"))
#assert.eq(parse((phone: "555-0100"), contact), (phone: "555-0100"))
```

## `variant`

根据字典中某个字段的值选择规则。

`tag-key` 是用来选择规则的字段名。每条分支都用 `schema(...)` 声明，并为该字段指定一个必填的 `literal(...)` 值。各分支的值必须不同。可以用 `describe` 给分支加说明。

没有填写该字段，或它的值不在任何分支中时，错误会指向这个字段。选中分支后，返回该分支的解析结果。要添加校验或转换，可放到分支的字段里，或包住整个 `variant`。

示例：按 tag 选择要解析的字典结构：

```typst
#import "@tola/schema:0.0.0": variant, schema, literal, parse
#let block-schema = variant("kind",
  schema((kind: literal("text"), body: str)),
  schema((kind: literal("image"), width: int)),
)
#assert.eq(parse((kind: "text", body: "Hello"), block-schema).body, "Hello")
```

示例：缺少标签字段时查看报错：

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

去掉解析后字符串的首尾空白。

传入非字符串值会报类型错误。

示例：去掉字符串首尾空白：

```typst
#import "@tola/schema:0.0.0": trim, parse, try-parse
#assert.eq(parse("  padded\n", trim(str)), "padded")
#assert.eq(try-parse(7, trim(str)).ok, false)
```

### `member`

解析字符串的规则。解析成功后再去掉首尾空白。

## `non-empty`

要求解析后的字符串、数组或字典非空，通过后原样返回。其他类型会报错。

顺序很重要：`non-empty(trim(str))` 拒绝只有空白字符的文本；`trim(non-empty(str))` 接受这段文本，并返回空字符串。

示例：拒绝空字符串：

```typst
#import "@tola/schema:0.0.0": non-empty, try-parse
#assert.eq(try-parse("x", non-empty(str)).ok, true)
#assert.eq(try-parse("", non-empty(str)).ok, false)
```

### `member`

规范化结果必须是非空字符串、数组或字典的 schema。

## `refine`

先解析，再用你提供的函数检查结果。函数返回 `true` 时保留该值，返回 `false` 时报告必填的 `message`。

示例：用函数检查解析结果：

```typst
#import "@tola/schema:0.0.0": refine, parse, try-parse
#let even = refine(int, value => calc.rem(value, 2) == 0, message: "must be even")
#assert.eq(parse(4, even), 4)
#assert.eq(try-parse(5, even).ok, false)
```

### `member`

先用这个 schema 解析，再把结果交给检查函数。

### `predicate`

接收解析后的值并返回布尔值的函数。

### `message`

检查函数返回 `false` 时报告的消息，代码为 `custom`。必填。

## `check`

检查多个字段之间的规则，例如结束日期不能早于开始日期。

用 `on` 列出要检查的字段，省略时传入整个解析结果。选中的字段解析失败时会跳过检查，未选中的字段出错时仍可检查。缺失的字段不会传入。检查通过时返回 `()`，有问题时返回 `(issue(...), ...)`。问题路径从当前检查的值算起。

示例：要求 `end` 大于或等于 `start`：

```typst
#import "@tola/schema:0.0.0": schema, check, issue, try-parse
#let interval = check(schema((start: int, end: int)), bounds => {
  if bounds.end >= bounds.start { () }
  else { (issue("must not precede start", path: ("end",)),) }
}, on: ("start", "end"))
#assert.eq(try-parse((start: 2, end: 1), interval).ok, false)
```

### `member`

先用这个 schema 解析，再检查结果。

### `inspect`

接收解析结果或 `on` 指定字段的函数。检查通过时返回 `()`，发现问题时返回问题数组。

### `on`

要交给检查函数的字段名。每个字段只列一次，且必须直接声明在被检查的字典 schema 中。写在转换、`union`、`variant` 或 `lazy` 里的字段不能从外部用 `on` 选取。

## `map`

用函数转换解析结果，再用 `output` 检查转换后的值。

例如，可以用 `str` 把整数转成字符串。输入解析失败时会跳过转换。转换结果必须通过 `output` 的校验，省略时接受任意结果。要在转换时返回校验问题，用 `convert`。

示例：用普通函数转换解析结果：

```typst
#import "@tola/schema:0.0.0": map, parse
#assert.eq(parse(7, map(int, str, output: str)), "7")
```

### `member`

先用这个 schema 解析，再把结果传给转换函数。

### `convert-value`

返回普通值的函数，或可调用的 Typst 类型。

### `output`

校验转换结果的 schema。

## `convert`

用函数转换解析结果，也可以返回转换失败的原因。

函数返回 `ok(value)` 时，Tola 用 `output` 校验其中的值，默认接受任意类型。返回 `err(issue(...))` 时，`try-parse` 会返回这些问题。函数调用 `panic` 会中止编译。若函数直接返回普通值，请用 `map`。

示例：转换失败时返回一条校验问题：

```typst
#import "@tola/schema:0.0.0": convert, ok, err, issue, try-parse
#let port = convert(int, value => {
  if value >= 0 and value <= 65535 { ok(str(value)) }
  else { err(issue("must be between 0 and 65535")) }
}, output: str)
#assert.eq(try-parse(443, port), (ok: true, value: "443"))
```

示例：在 `try-parse` 的结果中读取转换失败的原因：

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

先用这个 schema 解析，再把结果传给转换函数。

### `convert-value`

返回 `ok(value)` 或 `err(..issues)` 的回调。

### `output`

用于检查转换结果的 schema。

## `describe`

为 schema 的解析结果添加说明。

说明会出现在 `inspect` 的结果中。给同一处加了两次说明时，以外层为准。要描述某个嵌套字段，给该字段的 schema 单独加 `describe`。要给诊断加备注，用 `issue(..., notes: ...)`。

示例：为解析结果添加说明：

```typst
#import "@tola/schema:0.0.0": describe, parse
#let slug = describe(str, "lowercase URL segment")
#assert.eq(parse("post", slug), "post")
```

### `member`

要添加说明的 schema。

### `description`

描述解析结果的文字。

## `inspect`

查看 schema 声明的字段、类型和默认值。这个函数不运行回调。

每条规则都有 `kind`、`presence` 和 `description`。Typst 类型另有 `type`，字典规则另有 `fields` 和 `unknown`。数组等容器会列出成员规则，`union` 和 `variant` 会列出分支。`lazy` 的 `kind` 是 `"unknown"`。

`presence` 表示字段是否会出现，取值是 `"required"`、`"optional"` 或 `"unknown"`。存在默认值时，`default.value` 是校验或转换之前的原值。数组和元组中的位置始终存在。`map` 和 `convert` 默认显示输出规则，字段是否存在及默认值仍取自输入声明。

默认描述解析后的值。用 `input: true` 查看 schema 接受的输入，此时带默认值的字段会标为可省略，转换会显示它接受的类型。要检查一个具体值，用 `parse` 或 `try-parse`。

示例：查看解析结果有哪些字段和类型：

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

设为 `true` 时查看允许的输入。默认查看解析结果。

## `parse`

校验输入并返回解析结果。失败时会报错，消息与 `format-issues` 的输出相同。

示例：解析字典并补上默认值：

```typst
#import "@tola/schema:0.0.0": non-empty, trim, parse
#assert.eq(parse("  Notes  ", non-empty(trim(str))), "Notes")
```

### `value`

要解析的值。

### `member`

校验这个值的 schema。

## `try-parse`

成功时返回 `(ok: true, value: value)`，失败时返回 `(ok: false, issues: issues)`。

校验失败时，`issues` 至少有一项。用 `format-issues` 查看原因。schema 写错、回调返回了约定之外的值，或调用了 `panic`，仍会中止编译。传入 `none` 时，schema 会按一个实际值处理。缺失的键只出现在字典中。默认值或转换结果出错时，诊断会指明值的来源。

示例：检查输入，并根据 `ok` 处理结果：

```typst
#import "@tola/schema:0.0.0": try-parse
#assert.eq(try-parse(7, int), (ok: true, value: 7))
#assert.eq(try-parse("7", int).ok, false)
```

### `value`

要解析的值。

### `member`

校验这个值的 schema。

## `partial`

先给字典填一部分字段，返回一个可以补齐其他字段的函数。

调用返回的函数时，用具名参数补齐字段，再一起校验。两次都没填写的字段按 schema 的默认值处理。已经固定的字段不能再传一次。

示例：让许多页面共享同一个版式：

```typst
#import "@tola/schema:0.0.0": optional, partial, schema
#let page-schema = schema((title: str, layout: optional(str, default: "page")))
#let note = partial(page-schema, layout: "note")
#assert.eq(note(title: "Notes"), (title: "Notes", layout: "note"))
```

### `member`

校验这些字段的字典 schema。

## `defaults`

读取字典各字段声明的默认值，保留解析前的值。读取嵌套字段的默认值需要为包含它们的字典本身声明默认值。

示例：读取默认值：

```typst
#import "@tola/schema:0.0.0": array-of, defaults, optional, schema
#let page-schema = schema((title: str, tags: optional(array-of(str), default: ())))
#assert.eq(defaults(page-schema), (tags: ()))
```

### `member`

要读取默认值的对象 schema。

## `ok`

返回成功的 Result，包含给定的值。

`ok(none)` 是一次成功，值是 `none`。

示例：构造成功的 Result：

```typst
#import "@tola/schema:0.0.0": ok
#assert.eq(ok(none), (ok: true, value: none))
```

### `value`

成功 Result 的值。

## `err`

返回失败的 Result，至少提供一条问题。

结果包含 `ok: false` 和传入的 `issues`。

示例：返回一条校验问题：

```typst
#import "@tola/schema:0.0.0": err, issue
#let failure = err(issue("too small", code: "size", path: ("count",)))
#assert.eq(failure.ok, false)
#assert.eq(failure.issues.at(0).code, "size")
```

## `issue`

描述一条校验问题，可以指定哪个字段出错。

路径从正在校验的值算起。`("author.name",)` 指一个带点的键，`("author", "name")` 指嵌套字段，`("authors", 0)` 指数组的第一项。要补充下一级原因，可展开这条问题的字典，再给 `children` 填一个问题数组。

示例：构造带路径的 Issue：

```typst
#import "@tola/schema:0.0.0": issue
#let problem = issue("duplicate tag", path: ("tags", 0), notes: ("remove the repeat",))
#assert.eq(problem.path, ("tags", 0))
```

### `message`

显示给读者的问题说明。

### `code`

供代码识别问题的字符串。

### `path`

以正在检查的值为基准的字面量字符串键或非负索引。

### `notes`

字符串备注数组，默认为空。

## `format-issues`

把一条校验问题转成可显示的文字。

先显示出错的字段和原因，再缩进显示下一级原因和备注。默认值或转换结果出错时，也会指出值的来源。

示例：把 Issue 渲染为文本：

```typst
#import "@tola/schema:0.0.0": issue, format-issues
#let problem = issue("is required", code: "schema.missing", path: ("title",))
#assert.eq(format-issues((problem,)), "`$.title`: is required")
```

示例：渲染嵌套原因与备注：

```typst
#import "@tola/schema:0.0.0": format-issues, issue
#let cause = issue("is required", code: "schema.missing", path: ("title",), notes: ("add a title",))
#let tree = (..issue("metadata is invalid", code: "metadata.invalid"), children: (cause,))
#assert.eq(format-issues((tree,)), "`$`: metadata is invalid\n  `$.title`: is required\n    note: add a title")
```

示例：先给出声明默认值的原因：

```typst
#import "@tola/schema:0.0.0": format-issues, optional, schema, try-parse
#let cause = try-parse((:), schema((count: optional(int, default: "bad")))).issues.first()
#assert.eq(format-issues((cause,)), "`$.count` is not set, and the schema's default value is invalid: expected `int`, found `str`")
```

### `issues`

要渲染的 Issue 值。

## `min-length`

要求字符串、数组或字典的长度至少为 `minimum`。字符串按字素簇计数。

通过后原样返回。其他类型会报类型错误。

示例：要求最小长度：

```typst
#import "@tola/schema:0.0.0": min-length, try-parse
#assert.eq(try-parse("ab", min-length(str, 3)).ok, false)
#assert.eq(try-parse("abc", min-length(str, 3)).ok, true)
```

### `member`

先用这个 schema 解析，再检查结果的长度。

### `minimum`

最少多少个字素簇、数组项或字典条目。必须是非负整数。

## `max-length`

要求字符串、数组或字典的长度最多为 `maximum`。字符串按字素簇计数。

通过后原样返回。其他类型会报类型错误。

示例：限制值的最大长度：

```typst
#import "@tola/schema:0.0.0": max-length, try-parse
#assert.eq(try-parse((1, 2), max-length(array, 2)).ok, true)
#assert.eq(try-parse((1, 2, 3), max-length(array, 2)).ok, false)
```

### `member`

先用这个 schema 解析，再检查结果的长度。

### `maximum`

最多多少个字素簇、数组项或字典条目。必须是非负整数。

## `matches`

用 Typst 的 `regex(...)` 校验字符串。

默认匹配字符串中的任意位置。要检查整串，用 `^` 和 `$` 限定开头和结尾。通过后原样返回。

示例：要求匹配正则表达式：

```typst
#import "@tola/schema:0.0.0": matches, try-parse
#let slug = matches(str, regex("^[a-z0-9-]+$"))
#assert.eq(try-parse("my-post", slug).ok, true)
#assert.eq(try-parse("My Post", slug).ok, false)
```

### `member`

先用这条规则解析字符串，再检查正则表达式。

### `expression`

在字符串中搜索的正则表达式。

## `at-least`

要求整数或浮点数大于或等于 `minimum`。

通过的值原样返回。NaN 判失败；无穷与普通数字一样比较。

示例：允许等于下限：

```typst
#import "@tola/schema:0.0.0": at-least, try-parse
#assert.eq(try-parse(5, at-least(int, 5)).ok, true)
#assert.eq(try-parse(4, at-least(int, 5)).ok, false)
```

### `member`

先用这个 schema 解析，再比较结果。

### `minimum`

下界，非 NaN 的整数或浮点数。

## `at-most`

要求整数或浮点数小于或等于 `maximum`。

通过的值原样返回。NaN 判失败；无穷与普通数字一样比较。

示例：允许等于上限：

```typst
#import "@tola/schema:0.0.0": at-most, try-parse
#assert.eq(try-parse(5, at-most(int, 5)).ok, true)
#assert.eq(try-parse(6, at-most(int, 5)).ok, false)
```

### `member`

先用这个 schema 解析，再比较结果。

### `maximum`

上界，非 NaN 的整数或浮点数。

## `email`

校验未加引号的 ASCII 邮箱地址，使用 RFC 5322 的 dot-atom 格式。

本地部分由非空、以点分隔的 atext 原子组成，至多 64 字节，后面是 `@` 和一个或多个 DNS 风格标签。每个标签由 1-63 个 ASCII 字母、数字或连字符组成，首尾是字母或数字；整个地址至多 254 字节，接受单标签域名。

只传地址本身。带显示名、引号、注释、域名字面量或 Unicode 的地址会报错。这个函数只校验格式。

示例：校验邮箱地址：

```typst
#import "@tola/schema:0.0.0": email, try-parse
#assert.eq(try-parse("ada@example.com", email).ok, true)
#assert.eq(try-parse("not an address", email).ok, false)
```

## `ipv4`

校验 IPv4 地址，由四个 0–255 的十进制数组成，用点分隔。多余的前导零会报错。

通过后保留原写法。只给地址，省略方括号、端口、区域标识和首尾空白。

示例：校验 IPv4 地址：

```typst
#import "@tola/schema:0.0.0": ipv4, try-parse
#assert.eq(try-parse("192.168.0.1", ipv4).ok, true)
#assert.eq(try-parse("192.168.0.256", ipv4).ok, false)
```

## `ipv6`

校验 IPv6 地址。可以写八组 1–4 位十六进制数，也可以用一处 `::` 省略至少一组。

最后两组可以用点分十进制的 IPv4 地址代替。通过后保留原写法。只给地址，省略方括号、端口、区域标识和首尾空白。

示例：校验 IPv6 地址：

```typst
#import "@tola/schema:0.0.0": ipv6, try-parse
#assert.eq(try-parse("2001:db8::1", ipv6).ok, true)
#assert.eq(try-parse("2001:db8::1::2", ipv6).ok, false)
```

## `ip`

IPv4 或 IPv6 地址，原样返回。

方括号、zone 标识和端口一律拒绝。

示例：接受 IPv4 或 IPv6 地址：

```typst
#import "@tola/schema:0.0.0": ip, try-parse
#assert.eq(try-parse("::1", ip).ok, true)
#assert.eq(try-parse("127.0.0.1:80", ip).ok, false)
```

## `http-url`

校验绝对 HTTP(S) URL，可以包含 Unicode。协议名不区分大小写，必须有主机名或 IP 地址。

按 RFC 3986 和 RFC 3987 检查格式。通过后原样返回。支持用户名和密码、IPv6 或 IPvFuture 地址、百分号编码和端口。端口可留空，填写时须是 0–65535 的整数。包含空白或控制字符时会报错。

示例：校验绝对的 HTTP(S) URL：

```typst
#import "@tola/schema:0.0.0": http-url, try-parse
#assert.eq(try-parse("https://example.com/posts", http-url).ok, true)
#assert.eq(try-parse("example.com/posts", http-url).ok, false)
```

## `https-url`

校验绝对 HTTPS URL，可以包含 Unicode。其余规则与 `http-url` 相同。

通过后原样返回，只检查格式。

示例：把 URL 限定为 HTTPS：

```typst
#import "@tola/schema:0.0.0": https-url, try-parse
#assert.eq(try-parse("https://example.com", https-url).ok, true)
#assert.eq(try-parse("http://example.com", https-url).ok, false)
```

## `url-path`

校验以 `/` 开头的 URL 路径，返回解码后的路由。

百分号转义只解码一次。只传路径，开头恰好一个 `/`。路径的各段要能作为跨平台文件名。段内允许 Unicode 和空格，整条路径不能有首尾空白，编码后的斜杠和反斜杠也会报错。传入前自行去掉域名、部署前缀、查询参数和锚点。

返回的 route 直接交给 `@tola/address` 的 `output-of` 或 `route-url`。文件名中的字面 `%`、`#` 在输入中分别写成 `%25`、`%23`。

示例：校验文章的 `permalink`：

```typst
#import "@tola/schema:0.0.0": parse, try-parse, schema, url-path
#let page = schema((permalink: url-path))
#assert.eq(parse((permalink: "/100%25/"), page).permalink, "/100%/")
#assert.eq(parse((permalink: "/a%2523b/"), page).permalink, "/a%23b/")
#assert.eq(try-parse((permalink: "/notes/#part"), page).issues.at(0).path, ("permalink",))
```

## `date`

校验年月日，返回不带时间的 Typst `datetime`。

接受不带时间的 Typst 日期或 `YYYY-MM-DD` 字符串。年份必须在 1 到 9999 之间，月份和日期必须有效。字符串首尾不能有空白。

示例：解析、显示日期并报告字段错误：

```typst
#import "@tola/schema:0.0.0": date, parse, schema, try-parse
#let published = parse("2026-10-14", date)
#assert.eq(published, datetime(year: 2026, month: 10, day: 14))
#assert.eq(published.display("[year]-[month]-[day]"), "2026-10-14")
#let checked = try-parse((published: "2026-02-30"), schema((published: date)))
#assert.eq(checked.issues.first().path, ("published",))
```

## `partial-date`

校验日期，可以只填写年份，或只填写年和月。

可传 `YYYY`、`YYYY-MM`、`YYYY-MM-DD` 字符串，或不带时间的 Typst 日期。年份必须在 1 到 9999 之间，填写了月份和日期时也必须有效。字符串首尾不能有空白。

返回 `(precision: "year", year: int)`、`(precision: "month", year: int, month: int)` 或 `(precision: "day", year: int, month: int, day: int)`。Typst 日期返回到日。没填的部分不会补齐。

示例：保留输入中的年、月或日，并检查无效日期：

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

`rfc3339` 保留原字符串。传入 Typst 值时，生成以 `Z` 结尾的 UTC 字符串。`unix-seconds` 是从 Unix 纪元起算的整秒数。`subsecond` 是小数点后的数字，去掉末尾的零，为零时返回空字符串。排序时先比 `unix-seconds`，相同再按字符串比较 `subsecond`。显示日期或传给 feed、sitemap 时用 `.rfc3339`。feed 和 sitemap 最多保留纳秒，RSS 只保留整秒。

示例：比较时区偏移，并对带 18 位小数的时间戳排序：

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
