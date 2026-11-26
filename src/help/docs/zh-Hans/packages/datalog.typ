#show: package(
  summary: [基于关系、事实与分层规则执行 Datalog 逻辑查询。],
)

用 `relation` 声明关系（包括空关系），用 `.atom(...)` 构造事实与条件，用 `rule` 声明推导规则，再在 `database` 上执行查询。

- *值域约束*：规则头部、`select` 与 `negate(...)` 中出现的变量，必须同时出现在同一规则或查询的至少一个正条件中。
- *分层否定*：关系仅在其定义不直接或间接依赖规则头部关系时，才可出现在 `negate(...)` 中。
- *确定性排序*：查询结果自动去重并按列排序。不同类型按固定顺序比较：`none`、`bool`、`int`、有限 `float`、`decimal`、`str`、`bytes`、`datetime`（`1` 与 `1.0` 按不同类型区分）。

#export(
  "relation",
  summary: [声明一个具有固定列数的命名关系。],
  params: (
    name: [非空、区分大小写的关系名。],
    arity: [非负整数列数。],
  ),
  related: ("database", "rule", "var"),
)[
  在返回值上调用 `.atom(...)` 构造事实行或规则/查询条件。
]

#method(
  "atom",
  summary: [构造匹配该关系列数的事实行或查询模式。],
  params: (
    terms: [每列传入一个常量或 `var(...)`；零列关系直接调用 `atom()`。如需引用 Typst 内容或函数，请使用字符串 ID。],
  ),
)

#export(
  "var",
  summary: [创建作用域限于单条规则或查询的逻辑变量。],
  params: (
    name: [非空、区分大小写的变量名。同一规则或查询中的同名变量表示同一个变量。],
  ),
  related: ("relation", "rule", "database"),
)

#export(
  "rule",
  summary: [当所有条件同时成立时，推导生成 `head` 元组。],
  params: (
    head: [条件满足时推导出的目标原子。],
    literal: [第一条条件（正原子或 `negate(...)`）。],
    rest: [零条或多条附加条件。],
  ),
  related: ("negate", "database"),
)

#export(
  "negate",
  summary: [当关系中不存在匹配 `atom` 的行时条件成立。],
  params: (
    atom: [待检查不存在的原子。],
  ),
  related: ("rule", "database"),
)

#export(
  "database",
  summary: [由关系声明、已知事实与推导规则构建 Datalog 数据库。],
  params: (
    relations: [去重后的 `relation(...)` 声明数组（可包含无初始数据的关系）。],
    facts: [已知事实数组（每行仅含常量），默认 `()`。],
    rules: [由 `rule(...)` 构造的推导规则数组，默认 `()`。],
  ),
  related: ("relation", "rule", "var", "negate"),
)[
  通过 `db.query(select: (...), where: (...))` 执行查询。`select` 为空时，条件成立返回 `((),)`，否则返回 `()`；`where` 为空时恒成立。

  #example(title: [查询从某页可达且未隐藏的页面], check: "default")[
  ```typst
  #import "@tola/datalog:0.0.0": database, negate, relation, rule, var

  #let edge = relation("edge", 2)
  #let reach = relation("reach", 2)
  #let hidden = relation("hidden", 1)
  #let (x, y, z) = (var("x"), var("y"), var("z"))

  #let db = database(
    (edge, reach, hidden),
    facts: (edge.atom("a", "b"), edge.atom("b", "c"), hidden.atom("b")),
    rules: (
      rule(reach.atom(x, y), edge.atom(x, y)),
      rule(reach.atom(x, z), reach.atom(x, y), edge.atom(y, z)),
    ),
  )

  #let visible = db.query(
    select: (z,),
    where: (reach.atom("a", z), negate(hidden.atom(z))),
  )
  #document("index.html")[可达页面：#visible.map(row => row.first()).join(", ")]
  ```
  ]
]

#method(
  "query",
  summary: [返回满足 `where` 全部条件的去重排序结果行。],
  params: (
    select: [指定输出列顺序的 `var(...)` 数组（同一变量可重复）。],
    where: [由正原子与 `negate(...)` 组成的条件数组，全部成立才匹配（`()` 恒成立）。],
  ),
)
