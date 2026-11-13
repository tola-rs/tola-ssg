# `@tola/datalog`

用已知数据和规则查询关系。例如，给出页面之间的直接链接，查询从某个页面出发能到达哪里。

用 `relation` 声明表名和列数，空表也要声明。用 `atom` 写一行已知数据，例如 `edge.atom("a", "b")`。用 `rule` 声明可以根据哪些条件推导出新的一行。

规则的结论、`select` 和 `negate` 用到的变量，必须也出现在同一条规则或查询中没被否定的条件里，例如 `reach.atom("a", z)`。这样才能知道变量可以取哪些值。

例如，定义 `visible` 的规则可以写 `negate(hidden.atom(z))`，前提是 `hidden` 的定义不会直接或间接用到 `visible`。这叫分层否定。

查询结果按列排序并去重。不同类型的值各自比较，类型顺序为 `none`、`bool`、`int`、有限 `float`、`decimal`、`str`、`bytes`、`datetime`。字符串和字节按字典序比较。日期、时间、日期时间分别排序，依次排列。

## `relation`

声明关系名和列数。例如，`relation("edge", 2)` 声明两列，可分别记录链接的起点和终点。用 `atom` 写出一条事实，或在规则和查询里匹配数据。

### `name`

关系名，不能为空。名称区分大小写。

### `arity`

关系的列数，必须是非负整数。

### `.atom.terms`

每列传一个常量或变量。如果 `edge = relation("edge", 2)`，`edge.atom("a", "b")` 可作为一条事实传给 `database` 的 `facts`。在查询中，`edge.atom("a", var("target"))` 匹配从 `"a"` 出发的链接。零列关系直接调用 `atom()`。要引用 Typst 内容或函数，可用字符串 ID。

## `var`

创建逻辑变量。同一条规则或查询中，同名变量表示同一个变量。

### `name`

变量名，不能为空。名称区分大小写。

## `rule`

条件全部满足时，把指定的一行加入结果关系。至少写一个条件。

### `head`

条件成立时要推导出的那一行。

### `literal`

第一项条件，可以是正原子或 `negate` 条件。

### `rest`

其余条件，接在 `literal` 后面逐个传入。

## `negate`

关系中没有与这些值匹配的行时，条件成立。

### `atom`

要在关系中查找的原子。

## `database`

用关系声明、已知事实和规则创建数据库。

用 `db.query(select: …, where: …)` 查询数据库。例如，数据库里有事实 `edge.atom("a", "b")`，且 `target = var("target")`，调用 `db.query(select: (target,), where: (edge.atom("a", target),))` 会返回 `(("b",),)`。

查询返回行数组，按 `select` 指定的列排序，再去掉重复行。Datalog 比较时区分数值类型，例如 `1` 和 `1.0`。选择列为空时，条件成立返回 `((),)`，否则返回 `()`。空条件成立。

示例：查找沿着链接可以到达的页面：

用下面的代码替换新站点的 `site.typ`，从站点根目录运行 `tola build`。两条规则把直接和间接可达的页面加入 `reach`。查询会排除 `hidden` 中的 `b`，但仍然可以经过 `b` 到达 `c`。使用默认发布目录时，`public/index.html` 会显示 `Reachable pages: c`。

```typst
#import "@tola/datalog:0.0.0": relation, var, rule, negate, database
#let edge = relation("edge", 2)
#let reach = relation("reach", 2)
#let hidden = relation("hidden", 1)
#let x = var("x")
#let y = var("y")
#let z = var("z")
#let db = database((edge, reach, hidden),
  facts: (edge.atom("a", "b"), edge.atom("b", "c"), hidden.atom("b")),
  rules: (
    rule(reach.atom(x, y), edge.atom(x, y)),
    rule(reach.atom(x, z), reach.atom(x, y), edge.atom(y, z)),
  ),
)
#let rows = db.query(select: (z,), where: (
  reach.atom("a", z), negate(hidden.atom(z)),
))
#assert.eq(rows, (("c",),))
#document("index.html")[Reachable pages: #rows.map(row => row.first()).join(", ")]
```

### `relations`

关系声明数组。每个关系名只声明一次。关系可以没有数据。

### `facts`

已知数据行的数组。每行只能用常量。默认是 `()`。

### `rules`

推导规则，默认为 `()`。

### `.query.select`

按结果的列顺序传入变量数组。可以重复使用同一个变量。

### `.query.where`

传入条件数组，可以包含正原子和 `negate` 条件。所有条件都要成立。传 `()` 表示没有条件。
