# `@tola/datalog`

用 `relation` 声明关系，再用事实、递归规则和查询组合它们。关系名不能为空，参数个数固定。字符串是常量；变量用 `var` 显式构造。

事实是没有变量的原子。规则和查询采用集合语义，相同的结果只出现一次。规则头、查询返回列和否定条件中的变量，都必须出现在同一规则或查询的非否定条件中。否定必须分层：依赖环中不能有否定边。查询结果始终是元组组成的数组。

常量支持 `none`、布尔值、整数、有限浮点数、decimal、字符串、bytes 和 datetime。类型彼此区分，例如 `1` 和 `1.0` 是不同的常量。decimal 按数值匹配，返回值保留查询选中的 decimal 表示。内容和函数等复杂值可通过 ID 连接，再从自己的记录中查回原值。

## `relation`

创建一个函数，用来构造指定关系的原子。

同一数据库或查询中，同名关系的参数个数必须一致。没有事实或推导规则的关系没有数据行。

### `name`

按原样比较的非空关系名。

### `arity`

参数个数，必须是非负整数。

## `var`

创建变量。同一规则或查询中，同名变量表示同一个变量。

### `name`

按原样比较的变量名。

## `rule`

用头部原子和零个或多个正条件、否定条件创建规则。

### `head`

要推导的原子。

### `body`

以位置参数传入的条件。

## `negate`

把原子变成否定条件。

### `atom`

不得存在匹配行的原子。

## `database`

用没有变量的事实原子和规则构建不可变数据库。

Example - 用递归规则计算可达关系：

```typst
#import "@tola/datalog:0.0.0": relation, var, rule, database, query
#let edge = relation("edge", 2)
#let reach = relation("reach", 2)
#let x = var("x")
#let y = var("y")
#let z = var("z")
#let db = database(facts: (edge("a", "b"), edge("b", "c")), rules: (
  rule(reach(x, y), edge(x, y)),
  rule(reach(x, z), reach(x, y), edge(y, z)),
))
#assert(query(db, select: (z,), where: (reach("a", z),)).contains(("c",)))
```

### `facts`

没有变量的事实原子，默认为 `()`。

### `rules`

规则，默认为 `()`。

## `query`

用正条件和否定条件查询数据库。

Example - 连接参考条目与记录，并排除已撤回的条目：

```typst
#import "@tola/datalog:0.0.0": relation, var, negate, database, query
#let cites = relation("cites", 2)
#let title = relation("title", 2)
#let withdrawn = relation("withdrawn", 1)
#let id = var("id")
#let name = var("name")
#let db = database(facts: (
  cites("essay", "r1"), cites("essay", "r2"),
  title("r1", "Logic"), title("r2", "Old"), withdrawn("r2"),
))
#assert.eq(query(db, select: (name,), where: (
  cites("essay", id), title(id, name), negate(withdrawn(id)),
)), (("Logic",),))
```

`select` 按输出列的顺序列出变量。选择列为空时，所有条件成立返回 `((),)`，否则返回 `()`。结果行有序且不重复，即使只有一列，每行也仍是元组。正条件按书写顺序执行，否定条件在正条件之后执行。

### `database`

要查询的数据库。

### `select`

按输出列顺序排列的变量。

### `where`

要匹配的条件。
