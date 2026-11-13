# `@tola/collection`

给文章分组、按标签筛选、找上一篇和下一篇，或按父子关系查找成员。

- 索引与分组：`index-by`、`group-by`、`group-by-keys`
- 筛选与取字段：`select-members`、`pick`
- 顺序与邻居：`adjacent`、`window`
- 层级：`children`、`descendants`、`ancestors`、`lineage`、`siblings`

`key` 从成员取一个键，`keys` 从成员取多个键，例如文章的 tag。`parent` 接收成员的键，返回父节点的键。根节点的父键是 `none`。

查找指定成员时，找不到该成员返回 `none`。找到成员但没有匹配项时，返回空数组或省略相应的字段，具体见各函数。`children` 和 `descendants` 可以查找未列入成员数组的父节点。分组按键首次出现的顺序排列，筛选保留输入顺序。

## `pick`

按 `keys` 给出的顺序，从 `fields` 复制同名字段，返回新字典。

只复制 `fields` 中存在的键。重复的键只取一次，值原样保留，包括 `none`。键须为字符串，例如 `"a.b"` 是一个完整的键。

示例：复制选中的字段（含带点的键）：

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

为每个成员取一个键，返回以这些键查找成员的字典。

每个成员调用一次 `key`。两个成员的键相同就会报错。

示例：按路径建立索引：

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

从每个成员取出一个字符串作键。省略时，直接用成员本身，成员必须是字符串。

## `group-by`

按单个键分组。例如，用文章的栏目分组，每篇文章只进一个组。按多个 tag 分组时用 `group-by-keys`。

每个成员调用一次 `key`。组按键首次出现的顺序排列，组内保持输入顺序。

示例：按栏目分组：

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

从每个成员取出一个字符串作键。省略时，直接用成员本身，成员必须是字符串。

## `group-by-keys`

按多个键分组。例如，一篇文章可以出现在每个 tag 对应的组里。每篇只有一个键时，用 `group-by`。

每个成员调用一次 `keys`。重复的键只算一次，没有键的成员不进组。组按键首次出现的顺序排列，组内保留输入顺序。

示例：按标签给笔记分组：

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

从每个成员取出一个字符串数组作分组键。省略时，成员本身须是字符串数组。

## `select-members`

按键筛选成员。例如，用 `keys: note => note.tags` 查找带有指定 tag 的笔记。

`"any"` 匹配 `wanted` 中任意一个键，`"all"` 要求匹配全部。`(none,)` 含有一个键，`()` 没有键。`wanted` 为空时，`"any"` 不选任何成员，`"all"` 选全部成员。

示例：匹配任意键或全部键：

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

示例：用空的 `wanted` 筛选成员：

```typst
#import "@tola/collection:0.0.0": select-members
#let notes = ((id: "A", tags: ("web",)), (id: "B", tags: ("typst",)))
#assert.eq(select-members(notes, (), keys: note => note.tags), ())
#assert.eq(select-members(notes, (), keys: note => note.tags, match: "all"), notes)
```

### `members`

要过滤的成员。

### `wanted`

要匹配的键。

### `keys`

从每个成员取出一个键数组，键可以是 `none`。省略时，成员本身须是数组。

### `match`

`"any"` 匹配任意一个键，是默认值。`"all"` 要求匹配全部。

## `adjacent`

查找 `at` 对应成员的上一项和下一项，分别放在 `before` 和 `after` 中。

找不到 `at` 时返回 `none`。如果成员已经是第一项，结果没有 `before`。如果它是最后一项，结果没有 `after`。存在的上一项或下一项，其值可以是 `none`。

每个成员的键必须唯一。键可以是任何可比较的值，包括 `none`。

示例：查找上一项和下一项：

```typst
#import "@tola/collection:0.0.0": adjacent
#let pages = ((id: "a", title: "A"), (id: "b", title: "B"), (id: "c", title: "C"))
#let neighbors = adjacent(pages, "b", key: page => page.id)
#assert.eq(neighbors.before.title, "A")
#assert.eq(neighbors.after.title, "C")
#assert.eq(adjacent(pages, "z", key: page => page.id), none)
```

示例：查找列表首尾的相邻项：

```typst
#import "@tola/collection:0.0.0": adjacent
#let steps = (none, "middle", "last")
#assert.eq(adjacent(steps, none), (after: "middle"))
#assert.eq(adjacent(steps, "last"), (before: "middle"))
#assert.eq(adjacent(steps, "middle"), (before: none, after: "last"))
```

示例：链接到上一篇和下一篇：

在用 `tola init` 创建的站点里，把下面的代码保存为 `site.typ`，运行 `tola dev`。在终端打印的站点地址下打开 `/guide/install/`，可以看到指向 Guide 和 Deploy 的链接。数组里的顺序就是阅读顺序。

```typst
#import "@tola/address:0.0.0": output-url
#import "@tola/collection:0.0.0": adjacent

#let pages = (
  (output: "guide/index.html", title: "Guide"),
  (output: "guide/install/index.html", title: "Install"),
  (output: "guide/deploy/index.html", title: "Deploy"),
)

#for page in pages {
  let neighbors = adjacent(pages, page.output, key: item => item.output)
  document(page.output, title: page.title)[
    #heading(level: 1, page.title)
    #for direction in ("before", "after") {
      if direction in neighbors {
        let item = neighbors.at(direction)
        block(link(output-url(item.output), item.title))
      }
    }
  ]
}
```

### `members`

要搜索的成员。

### `at`

要查找上一项和下一项的成员键。

### `key`

从每个成员取出一个键。省略时，直接用成员本身。

## `window`

在 `at` 对应的成员前后分别取最多 `before` 项和 `after` 项，结果保持输入顺序，省略该成员本身。

找不到 `at` 时返回 `none`。每个成员的键必须唯一。

示例：取指定成员前后的几项：

```typst
#import "@tola/collection:0.0.0": window
#let pages = ((id: "a"), (id: "b"), (id: "c"), (id: "d"))
#let around = window(pages, "c", key: page => page.id, before: 1, after: 1)
#assert.eq(around.map(page => page.id), ("b", "d"))
#assert.eq(window(pages, "z", key: page => page.id), none)
```

示例：只保留 `at` 一侧的成员：

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

从这个成员的位置向前或向后取值。

### `key`

从每个成员取出一个键。省略时，直接用成员本身。

### `before`

取前面几项。必须是非负整数，省略时一项也不取。

### `after`

取后面几项。必须是非负整数，省略时一项也不取。

## `children`

列出父键为 `of` 的成员。

`of` 是要查找子项的父节点键，即使父节点没列在 `members` 中也可以查。`of` 和成员键都不能是 `none`，成员键也不能重复。成员把自己设为父节点时会报错。

示例：查找直接子项：

```typst
#import "@tola/collection:0.0.0": children
#let pages = ((id: "root"), (id: "api"), (id: "usage"), (id: "faq"))
#let parents = (api: "root", usage: "api", faq: "root", root: none)
#let kids = children(pages, "root", key => parents.at(key), key: page => page.id)
#assert.eq(kids.map(page => page.id), ("api", "faq"))
```

示例：查找未列入 `members` 的父节点的子项：

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

要查找的父节点的键。

### `parent`

接收成员键而不是成员；返回它的父键，在根处返回 `none`。

### `key`

从每个成员取出一个键。省略时，直接用成员本身。

## `descendants`

列出 `of` 对应节点的所有后代，省略该节点。

`of` 可以指向没有列在 `members` 中的父节点。`of` 和成员键都不能是 `none`，成员键也不能重复。

从每个成员沿 `parent` 向上查找。中途经过 `members` 里没有的键时仍继续。即使已经找到 `of`，也会检查剩下的路径是否有环。

示例：查找节点的所有后代：

```typst
#import "@tola/collection:0.0.0": descendants
#let pages = ((id: "root"), (id: "api"), (id: "usage"), (id: "faq"))
#let parents = (api: "root", usage: "api", faq: "root", root: none)
#let nested = descendants(pages, "root", key => parents.at(key), key: page => page.id)
#assert.eq(nested.map(page => page.id), ("api", "usage", "faq"))
```

示例：查找后代时经过未列入 `members` 的节点：

```typst
#import "@tola/collection:0.0.0": descendants
#let pages = ((id: "intro"), (id: "api"))
#let parents = (intro: "guide", api: "intro", guide: none)
#assert.eq(descendants(pages, "guide", key => parents.at(key), key: page => page.id).map(page => page.id), ("intro", "api"))
```

### `members`

要搜索的成员。

### `of`

要查找其后代的节点的键。

### `parent`

返回成员键的父键，在根处返回 `none`。

### `key`

从每个成员取出一个键。省略时，直接用成员本身。

## `ancestors`

列出 `at` 对应成员的祖先，从根节点排到它的父节点。

找不到 `at` 时返回 `none`。`at` 和成员键都不能是 `none`，成员键也不能重复。

沿 `parent` 向上查找，直到它返回 `none`。不在 `members` 里的节点可以经过，但不会出现在结果里。路径上遇到重复的键会报错。

示例：从根节点到父节点列出所有祖先：

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

返回成员键的父键，在根处返回 `none`。

### `key`

从每个成员取出一个键。省略时，直接用成员本身。

## `lineage`

返回从根到 `at` 的成员列表，可用于面包屑导航。

找不到 `at` 时返回 `none`。`at` 和成员键都不能是 `none`，成员键也不能重复。

沿 `parent` 向上查找，直到它返回 `none`。不在 `members` 里的节点可以经过，但不会出现在结果里。路径上遇到重复的键会报错。

示例：查找祖先时经过未列入 `members` 的节点：

```typst
#import "@tola/collection:0.0.0": lineage
#let pages = ((id: "leaf", title: "Leaf"), (id: "root", title: "Root"))
#let parents = (leaf: "section", section: "root", root: none)
#let trail = lineage(pages, "leaf", key => parents.at(key), key: page => page.id)
#assert.eq(trail.map(page => page.title), ("Root", "Leaf"))
```

示例：指定的成员不在 `members` 中：

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

返回成员键的父键，在根处返回 `none`。

### `key`

从每个成员取出一个键。省略时，直接用成员本身。

## `siblings`

查找和 `at` 有同一个父节点的其他成员。

找不到 `at` 时返回 `none`。如果 `at` 的 `parent` 返回 `none`，它是根节点，结果为 `()`。

`at` 和成员键都不能是 `none`，成员键也不能重复。成员把自己设为父节点时会报错。

示例：查找同一个父节点下的成员：

```typst
#import "@tola/collection:0.0.0": siblings
#let pages = ((id: "root"), (id: "api"), (id: "faq"), (id: "usage"))
#let parents = (api: "root", faq: "root", usage: "api", root: none)
#let peers = siblings(pages, "api", key => parents.at(key), key: page => page.id)
#assert.eq(peers.map(page => page.id), ("faq",))
```

示例：检查根节点的同级成员：

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

要查找同级成员的键。

### `parent`

返回成员键的父键，在根处返回 `none`。

### `key`

从每个成员取出一个键。省略时，直接用成员本身。
