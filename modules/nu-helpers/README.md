# Nu Helpers

`pick` returns the original values selected from a searchable table, or `[]` on cancel. Use `--multi` to mark several rows with Space.

```nu
use tola nu-helpers
[{name: Guide} {name: Notes}] | nu-helpers pick --columns [name]
```

In standalone Nushell 0.117+, import `use ./modules/nu-helpers` from the repository root.

`pick` 从可搜索的列表返回选中的原始值，取消返回 `[]`。`--multi` 模式下按空格标记多项。普通 Nushell 0.117+ 可从仓库根目录执行 `use ./modules/nu-helpers`。
