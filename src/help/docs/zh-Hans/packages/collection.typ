#show: package(
  summary: [对站点记录执行索引、分组、筛选、相邻分页与层级树遍历。],
)

通用约定：
- `key` 为每个成员提取单个键；`keys` 为每个成员提取键数组（如文章标签）。
- `parent` 接收*成员的键*（而非成员记录本身），返回其父键，根节点返回 `none`。
- 指定成员查找（`at`）在找不到该成员时返回 `none`。分组按键首次出现顺序排列，筛选保留原顺序。

#export(
  "pick",
  summary: [按 `keys` 顺序从 `fields` 复制存在的字符串键，返回新字典。],
  params: (
    fields: [源字典。],
    keys: [要复制的字符串键数组。缺失键自动跳过，重复键只取一次，`none` 值原样保留。],
  ),
)

#export(
  "index-by",
  summary: [按唯一字符串键为 `members` 建立字典索引。],
  params: (
    members: [待索引的成员数组。],
    key: [为每个成员返回唯一字符串键的函数；省略时以成员本身为键。],
  ),
  related: ("group-by",),
)[
  若有两个成员生成相同键，则报错。
]

#export(
  "group-by",
  summary: [按单个字符串键将 `members` 分组为键到成员数组的字典。],
  params: (
    members: [待分组的成员数组。],
    key: [为每个成员返回字符串键的函数；省略时以成员本身为键。],
  ),
  related: ("group-by-keys", "index-by"),
)

#export(
  "group-by-keys",
  summary: [当每个成员可对应多个键（如多标签）时，将 `members` 按键分组。],
  params: (
    members: [待分组的成员数组。],
    keys: [为每个成员返回字符串键数组的函数；省略时以成员本身为键数组。],
  ),
  related: ("group-by", "select-members"),
)[
  #example(title: [按标签分组文章], check: "default")[
  ```typst
  #import "@tola/collection:0.0.0": group-by-keys

  #let posts = (
    (title: "Intro", tags: ("typst", "guide")),
    (title: "Math", tags: ("typst", "math")),
  )
  #let by-tag = group-by-keys(posts, keys: p => p.tags)
  ```
  ]
]

#export(
  "select-members",
  summary: [筛选提取出的键满足 `wanted` 条件（任一或全部）的成员。],
  params: (
    members: [待筛选的成员数组。],
    wanted: [期望匹配的目标键数组。],
    keys: [为每个成员返回键数组的函数（可含 `none`）；省略时以成员本身为键数组。],
    match: [`"any"`（默认）匹配 `wanted` 中至少一个键；`"all"` 要求同时包含 `wanted` 中的全部键。],
  ),
  related: ("group-by-keys",),
)

#export(
  "adjacent",
  summary: [返回键为 `at` 的成员的前后邻居（`(prev:, next:)`）。],
  params: (
    members: [有序成员数组。],
    at: [目标成员的键。],
    key: [提取成员键的函数；省略时以成员本身为键。],
  ),
  related: ("window", "siblings"),
)[
  找不到 `at` 时返回 `none`；位于首尾时省略对应的 `prev` 或 `next` 字段。
]

#export(
  "window",
  summary: [返回以键 `at` 为中心的连续成员切片。],
  params: (
    members: [有序成员数组。],
    at: [中心成员的键。],
    key: [提取成员键的函数；省略时以成员本身为键。],
    before: [向前包含的成员数（非负整数，默认 `0`）。],
    after: [向后包含的成员数（非负整数，默认 `0`）。],
  ),
  related: ("adjacent",),
)[
  找不到 `at` 时返回 `none`。
]

#export(
  "children",
  summary: [按输入顺序返回容器键 `of` 的直接子成员。],
  params: (
    members: [树节点成员数组。],
    of: [父节点键（可不在 `members` 中）。],
    parent: [接收成员键并返回其父键的函数，根节点返回 `none`。],
    key: [提取成员键的函数；省略时以成员本身为键。],
  ),
  related: ("descendants", "ancestors", "siblings"),
)

#export(
  "descendants",
  summary: [以前序遍历返回容器键 `of` 的全部递归后代成员。],
  params: (
    members: [树节点成员数组。],
    of: [祖先节点键（可不在 `members` 中）。],
    parent: [接收成员键并返回其父键的函数，根节点返回 `none`。],
    key: [提取成员键的函数；省略时以成员本身为键。],
  ),
  related: ("children", "ancestors", "lineage"),
)

#export(
  "ancestors",
  summary: [返回成员 `at` 从根节点到直接父节点的全部祖先成员。],
  params: (
    members: [树节点成员数组。],
    at: [目标成员的键。],
    parent: [接收成员键并返回其父键的函数，根节点返回 `none`。],
    key: [提取成员键的函数；省略时以成员本身为键。],
  ),
  related: ("lineage", "children", "descendants"),
)[
  找不到 `at` 时返回 `none`；根节点返回 `()`。
]

#export(
  "lineage",
  summary: [返回从根节点到 `at`（含 `at` 自身）的完整路径，适合渲染面包屑导航。],
  params: (
    members: [树节点成员数组。],
    at: [目标成员的键。],
    parent: [接收成员键并返回其父键的函数，根节点返回 `none`。],
    key: [提取成员键的函数；省略时以成员本身为键。],
  ),
  related: ("ancestors", "children"),
)[
  找不到 `at` 时返回 `none`。
]

#export(
  "siblings",
  summary: [返回与 `at` 共享同一父键的兄弟成员（不含 `at` 自身）。],
  params: (
    members: [树节点成员数组。],
    at: [目标成员的键。],
    parent: [接收成员键并返回其父键的函数，根节点返回 `none`。],
    key: [提取成员键的函数；省略时以成员本身为键。],
  ),
  related: ("children", "adjacent"),
)[
  找不到 `at` 时返回 `none`。
]
