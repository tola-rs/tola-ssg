# `@tola/collection`

对一组你自己提供的值（成员数组）做四类操作：查、分、筛、层级。

- 索引与分组：`index-by`、`group-by`、`group-by-keys`
- 筛选与取字段：`select-members`、`pick`
- 顺序与邻居：`adjacent`、`window`
- 层级：`children`、`descendants`、`ancestors`、`lineage`、`siblings`

共用约定：`key`、`keys`、`parent` 都是你提供的函数——`key` 接收成员、返回它的键；`keys` 接收成员、返回它带的归属；`parent` 接收成员键、返回它的容器键，根节点返回 `none`。以成员为锚点的查询在该成员缺席时返回 `none`；`children` 和 `descendants` 允许容器不在成员数组中。空数组表示没有选中成员。筛选结果保持 `members` 的输入顺序，`ancestors` 和 `lineage` 则沿父链从根向下排列。分组按组键首次出现的顺序。`index-by`、`adjacent`、`window` 与层级类函数要求成员的键唯一（重复会报错）；`group-by`、`group-by-keys`、`select-members` 不要求。

## `pick`

返回新字典：`fields` 里由 `keys` 命名的条目，顺序按 `keys` 列出的顺序。

约束：`keys` 里的每个键都必须是字符串；只复制 `fields` 里确实存在的键；同一个键列两次只算一次；值原样复制（`none` 也不例外）；键里的点是普通字符，所以 `"a.b"` 是一个键。

Example - 复制选中的字段（含带点的键）：

```typst
#import "@tola/collection:0.0.0": pick
#let fields = (title: "Intro", "a.b": 1, draft: true)
#assert.eq(pick(fields, ("a.b", "title", "a.b", "missing")), ("a.b": 1, title: "Intro"))
```

### `fields`

要从中复制条目的字典。

### `keys`

要复制的字符串键。

## `index-by`

用 `key` 为每个成员取一个键，建立「键 → 成员」的索引。

约束：`key` 每个成员调用一次，必须返回字符串——返回数组或 `none` 都算错误；两个成员的键相同也算错误。

Example - 按路径建立索引：

```typst
#import "@tola/collection:0.0.0": index-by
#let members = (
  (path: "index.html", title: "Home"),
  (path: "404.html", title: "Not Found"),
)
#let by-path = index-by(members, key: member => member.path)
#assert.eq(by-path.at("404.html"), members.at(1))
#assert.eq(by-path.at("index.html"), members.at(0))
```

### `members`

要索引的成员。

### `key`

接收一个成员，返回它的字符串键。

## `group-by`

按每个成员的一个键分组：每个成员只进一组。（成员有多个归属时用 `group-by-keys`。）

约束：`key` 每个成员调用一次，必须返回字符串。组的顺序是组键首次出现的顺序，组内成员保持输入顺序。

Example - 按每个成员各自的一个键分组：

```typst
#import "@tola/collection:0.0.0": group-by
#let pages = ((section: "Guides", title: "Setup"), (section: "Blog", title: "Hello"), (section: "Guides", title: "Deploy"))
#let groups = group-by(pages, key: page => page.section)
#assert.eq(groups.keys(), ("Guides", "Blog"))
#assert.eq(groups.at("Guides").map(page => page.title), ("Setup", "Deploy"))
```

### `members`

要分组的成员。

### `key`

接收一个成员，返回它的字符串键。

## `group-by-keys`

按每个成员携带的多个键分组：一个成员可以进多个组。（每个成员只有一个归属时用 `group-by`。）

约束：`keys` 每个成员调用一次，返回该成员的归属；同一个成员里重复的归属只算一次；归属为空的成员不进任何组。组的顺序是归属键首次出现的顺序，组内成员保持输入顺序。

Example - 每个成员各自带来新的归属：

```typst
#import "@tola/collection:0.0.0": group-by-keys
#let notes = (
  (id: "A", tags: ("web", "typst")),
  (id: "B", tags: ("cli",)),
  (id: "C", tags: ("design", "typst")),
  (id: "D", tags: ()),
)
#let groups = group-by-keys(notes, keys: note => note.tags)
#assert.eq(groups.keys(), ("web", "typst", "cli", "design"))
#assert.eq(groups.at("web").map(note => note.id), ("A",))
#assert.eq(groups.at("typst").map(note => note.id), ("A", "C"))
#assert.eq(groups.at("cli").map(note => note.id), ("B",))
```

### `members`

要分组的成员。

### `keys`

接收一个成员，返回它的归属键。

## `select-members`

按成员的归属筛选：`"any"` 命中一个就保留，`"all"` 要求全部命中。

约束：`keys` 返回一个成员的归属；归属是普通值，所以 `(none,)` 算一个归属、`()` 一个也不算。`wanted` 为空时，`"any"` 什么都不选，`"all"` 全选。

Example - any 与 all：

```typst
#import "@tola/collection:0.0.0": select-members
#let notes = (
  (id: "A", tags: ("web", "typst")),
  (id: "B", tags: ("cli", "web")),
  (id: "C", tags: ()),
)
#assert.eq(select-members(notes, ("web",), keys: note => note.tags).map(note => note.id), ("A", "B"))
#assert.eq(
  select-members(notes, ("web", "typst"), keys: note => note.tags, match: "all").map(note => note.id),
  ("A",),
)
```

Example - 空的 `wanted`：

```typst
#import "@tola/collection:0.0.0": select-members
#let notes = ((id: "A", tags: ("web",)), (id: "B", tags: ("typst",)))
#assert.eq(select-members(notes, (), keys: note => note.tags), ())
#assert.eq(select-members(notes, (), keys: note => note.tags, match: "all"), notes)
```

### `members`

要过滤的成员。

### `wanted`

成员必须具有的归属。

### `keys`

接收一个成员，返回它的归属。

### `match`

有一个归属命中就够，还是必须全部命中。

## `adjacent`

取键为 `at` 的成员的前后邻居，返回 `{ before, after }`。

约束：没有成员以 `at` 为键时，结果是 `none`；只有邻居确实存在时，字典里才出现对应的 `before` 或 `after` 键，而邻居本身可以是 `none`；成员键必须唯一（`at` 不存在时也一样）；键可以是任何可比较的值，包括 `none`——`index-by` 更严格，要求字符串。

Example - 查找成员的邻居：

```typst
#import "@tola/collection:0.0.0": adjacent
#let pages = ((id: "a", title: "A"), (id: "b", title: "B"), (id: "c", title: "C"))
#let neighbors = adjacent(pages, "b", key: page => page.id)
#assert.eq(neighbors.before.title, "A")
#assert.eq(neighbors.after.title, "C")
#assert.eq(adjacent(pages, "z", key: page => page.id), none)
```

Example - 走到链的两端：

```typst
#import "@tola/collection:0.0.0": adjacent
#let steps = (none, "middle", "last")
#assert.eq(adjacent(steps, none), (after: "middle"))
#assert.eq(adjacent(steps, "last"), (before: "middle"))
#assert.eq(adjacent(steps, "middle"), (before: none, after: "last"))
```

Example - 链接到一节里的上一篇与下一篇：

```typst site
#import "@tola/address:0.0.0": join-route, output-of
#import "@tola/collection:0.0.0": adjacent
#import "@tola/source:0.0.0": all-sources

// The guide's pages, in the order the section declares.
#let pages = (
  all-sources()
    .filter(source => source.path.starts-with("guide/"))
    .sorted(key: source => (source.meta.order, source.id))
    .map(source => (
      output: output-of(join-route(source.route-segments)),
      title: source.meta.title,
    ))
)

#let neighbors = adjacent(pages, "guide/install/index.html", key: page => page.output)
#assert.eq(neighbors.before.title, "Guide")
#assert.eq(neighbors.after.title, "Deploy")
```

### `members`

要搜索的成员。

### `at`

要取邻居的成员键。

### `key`

接收一个成员，返回它的键。

## `window`

取键为 `at` 的成员前后各 `before`/`after` 个邻居，不含它自己。

结果保持输入顺序；没有成员以 `at` 为键时，结果是 `none`。

约束：成员键必须唯一（`at` 不存在时也一样）；`before` 与 `after` 必须是非负整数。

Example - 取某个成员周围的窗口：

```typst
#import "@tola/collection:0.0.0": window
#let pages = ((id: "a"), (id: "b"), (id: "c"), (id: "d"))
#let around = window(pages, "c", key: page => page.id, before: 1, after: 1)
#assert.eq(around.map(page => page.id), ("b", "d"))
#assert.eq(window(pages, "z", key: page => page.id), none)
```

Example - 只保留 `at` 一侧的成员：

```typst
#import "@tola/collection:0.0.0": window
#let pages = ((id: "a"), (id: "b"), (id: "c"))
#assert.eq(window(pages, "b", key: page => page.id), ())
#assert.eq(window(pages, "b", key: page => page.id, before: 1), (pages.at(0),))
#assert.eq(window(pages, "c", key: page => page.id, before: 5), (pages.at(0), pages.at(1)))
```

### `members`

要搜索的成员。

### `at`

窗口以哪个成员键为中心。

### `key`

接收一个成员，返回它的键。

### `before`

向前包含多少个成员；`0` 表示不包含。必须是非负整数。

### `after`

向后包含多少个成员；`0` 表示不包含。必须是非负整数。

## `children`

取容器键 `of` 的直接子成员。

约束：`of` 不能是 `none`；容器本身可以不出现在 `members` 里；成员键必须唯一且不能是 `none`；`parent` 接收成员键（不是成员）、返回它的容器键，在根处返回 `none`；只看直接边，自环算错误。

Example - 列出容器的直接子成员：

```typst
#import "@tola/collection:0.0.0": children
#let pages = ((id: "root"), (id: "api"), (id: "usage"), (id: "faq"))
#let parents = (api: "root", usage: "api", faq: "root", root: none)
#let kids = children(pages, "root", key => parents.at(key), key: page => page.id)
#assert.eq(kids.map(page => page.id), ("api", "faq"))
```

Example - 列出不在 `members` 里的容器的子成员：

```typst
#import "@tola/collection:0.0.0": children
#let pages = ((id: "intro"), (id: "api"))
#let parents = (intro: "guide", api: "guide")
#let ids = children(pages, "guide", key => parents.at(key), key: page => page.id)
#assert.eq(ids.map(page => page.id), ("intro", "api"))
```

### `members`

要搜索的成员。

### `of`

要取其直接子成员的容器键。

### `parent`

接收成员键而不是成员；返回它的容器键，在根处返回 `none`。

### `key`

接收一个成员，返回它的键。

## `descendants`

取容器键 `of` 的所有后代（任意深度），不含容器本身。

约束：`of` 不能是 `none`；容器本身可以不出现在 `members` 里；成员键必须唯一且不能是 `none`，`parent` 在根处返回 `none`。每个成员的父链都会完整走一遍来发现环（包括匹配到容器之后的部分），链上 `members` 里没有对应成员的键也照常经过。父查找开销大时，可以复用预先算好的「键 → 父键」字典。

Example - 列出容器的所有后代：

```typst
#import "@tola/collection:0.0.0": descendants
#let pages = ((id: "root"), (id: "api"), (id: "usage"), (id: "faq"))
#let parents = (api: "root", usage: "api", faq: "root", root: none)
#let nested = descendants(pages, "root", key => parents.at(key), key: page => page.id)
#assert.eq(nested.map(page => page.id), ("api", "usage", "faq"))
```

Example - 沿着链经过不是成员的键：

```typst
#import "@tola/collection:0.0.0": descendants
#let pages = ((id: "intro"), (id: "api"))
#let parents = (intro: "guide", api: "intro", guide: none)
#assert.eq(descendants(pages, "guide", key => parents.at(key), key: page => page.id).map(page => page.id), ("intro", "api"))
```

### `members`

要搜索的成员。

### `of`

要取其所有后代的容器键。

### `parent`

返回成员键的容器键，在根处返回 `none`。

### `key`

接收一个成员，返回它的键。

## `ancestors`

取键为 `at` 的成员的祖先，从根排到它的父键，不含它自己。

约束：只包含 `members` 里有的祖先；成员键必须唯一，成员键和 `at` 都不能是 `none`；`at` 不在 `members` 里时返回 `none`。上溯沿 `parent` 走到 `none` 为止，链上 `members` 里没有对应成员的键也照常经过；链中出现重复的键算错误。

Example - 上溯成员的祖先链：

```typst
#import "@tola/collection:0.0.0": ancestors
#let pages = ((id: "root"), (id: "api"), (id: "usage"), (id: "faq"))
#let parents = (api: "root", usage: "api", root: none)
#let trail = ancestors(pages, "usage", key => parents.at(key), key: page => page.id)
#assert.eq(trail.map(page => page.id), ("root", "api"))
```

### `members`

要搜索的成员。

### `at`

要取其祖先的成员键。

### `parent`

返回成员键的容器键，在根处返回 `none`。

### `key`

接收一个成员，返回它的键。

## `lineage`

取键为 `at` 的成员的祖先，并把 `at` 自己排在最后——面包屑用。

约束：只包含 `members` 里有的祖先；成员键必须唯一，成员键和 `at` 都不能是 `none`；`at` 不在 `members` 里时返回 `none`。上溯沿 `parent` 走到 `none` 为止，链上 `members` 里没有对应成员的键也照常经过；链中出现重复的键算错误。

Example - 沿着链经过不是成员的键：

```typst
#import "@tola/collection:0.0.0": lineage
#let pages = ((id: "leaf", title: "Leaf"), (id: "root", title: "Root"))
#let parents = (leaf: "section", section: "root", root: none)
#let trail = lineage(pages, "leaf", key => parents.at(key), key: page => page.id)
#assert.eq(trail.map(page => page.title), ("Root", "Leaf"))
```

Example - 锚点不在 `members` 里：

```typst
#import "@tola/collection:0.0.0": lineage
#let pages = ((id: "root", title: "Root"),)
#assert.eq(lineage(pages, "missing", key => none, key: page => page.id), none)
```

### `members`

要搜索的成员。

### `at`

要取其祖先链的成员键。

### `parent`

返回成员键的容器键，在根处返回 `none`。

### `key`

接收一个成员，返回它的键。

## `siblings`

取与键为 `at` 的成员同父的其它成员。

约束：成员键必须唯一，成员键和 `at` 都不能是 `none`；`at` 不在 `members` 里时返回 `none`。`parent` 返回 `none` 的成员是根；所有根都共享 `parent: none`，但根没有兄弟。只看直接父边，自环算错误。

Example - 列出同父的成员：

```typst
#import "@tola/collection:0.0.0": siblings
#let pages = ((id: "root"), (id: "api"), (id: "faq"), (id: "usage"))
#let parents = (api: "root", faq: "root", usage: "api", root: none)
#let peers = siblings(pages, "api", key => parents.at(key), key: page => page.id)
#assert.eq(peers.map(page => page.id), ("faq",))
```

Example - 根没有兄弟：

```typst
#import "@tola/collection:0.0.0": siblings
#let pages = ((id: "root"), (id: "aside"), (id: "api"))
#let parents = (root: none, aside: none, api: "root")
#assert.eq(siblings(pages, "root", key => parents.at(key), key: page => page.id), ())
#assert.eq(siblings(pages, "aside", key => parents.at(key), key: page => page.id), ())
```

### `members`

要搜索的成员。

### `at`

要取其兄弟的成员键。

### `parent`

返回成员键的容器键，在根处返回 `none`。

### `key`

接收一个成员，返回它的键。
