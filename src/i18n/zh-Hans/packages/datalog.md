# `@tola/datalog`

数据库须显式声明所有关系，包括空关系。一个名字在数据库中只对应一种参数个数。事实、规则和查询引用未声明的关系会报错。

事实是只含常量的原子。规则头、查询返回列和否定条件中的变量，必须出现在同一条规则或查询的正原子中。否定必须分层，依赖环不能经过否定边。

常量按类型和值比较。类型顺序为 `none`、`bool`、`int`、有限 `float`、`decimal`、`str`、`bytes`、`datetime`。布尔和数值按值排序，字符串和字节按字典序排序。日期、时间、日期时间依次排列，各自按时间顺序排序。Decimal 去掉小数部分的尾零，浮点数和 decimal 各自使用唯一的零表示。

## `relation`

创建关系声明。用 `.atom(..terms)` 创建该关系的原子，每列传入一个常量或变量。零列关系用 `.atom()`。

### `name`

按原样比较的非空关系名。

### `arity`

参数个数，必须是非负整数。

## `var`

创建逻辑变量。同一条规则或查询中，同名变量表示同一个变量。

### `name`

按原样比较的非空变量名。

## `rule`

规则体的条件全部成立时，推导规则头。至少传入一个条件。

### `head`

要推导的原子。

### `literal`

第一个体条件，可以是正原子或否定条件。

### `rest`

以位置参数传入的其余体条件。

## `negate`

构造否定条件。对当前变量绑定，没有匹配行时成立。

### `atom`

要检查是否缺席的原子。

## `database`

构建不可变数据库。一个关系可以有输入事实和推导规则。比较数据库时，比较关系声明、事实集合和规则结构，源位置只用于诊断。

用 `db.query(select: …, where: …)` 查询。两项都是必填数组。`select` 按输出列顺序列出变量，允许重复。`where` 的所有条件必须成立。

查询返回二维数组，按投影列做字典序排序，并按 Datalog 的常量相等性去重。Typst 的 `==` 可能把不同数值类型判为相等。选择列为空时，条件成立返回 `((),)`，否则返回 `()`。空条件成立。下面的例子查询从 `a` 可达且不在 `hidden` 中的节点，结果是 `("b",)` 和 `("c",)` 两行。

```typst
#import "@tola/datalog:0.0.0": relation, var, rule, negate, database
#let edge = relation("edge", 2)
#let reach = relation("reach", 2)
#let hidden = relation("hidden", 1)
#let x = var("x")
#let y = var("y")
#let z = var("z")
#let db = database((edge, reach, hidden),
  facts: (edge.atom("a", "b"), edge.atom("b", "c")),
  rules: (
    rule(reach.atom(x, y), edge.atom(x, y)),
    rule(reach.atom(x, z), reach.atom(x, y), edge.atom(y, z)),
  ),
)
#assert.eq(db.query(select: (z,), where: (
  reach.atom("a", z), negate(hidden.atom(z)),
)), (("b",), ("c",)))
```

### `relations`

关系声明数组。同一名字只能声明一次。显式声明的关系可以为空。

### `facts`

调用者直接提供的不含变量的原子，默认为 `()`。

### `rules`

推导规则，默认为 `()`。
