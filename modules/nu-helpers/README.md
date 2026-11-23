# Nu Helpers

Pick values from a searchable terminal table.

```nu
use tola nu-helpers

[{id: 1, name: Guide} {id: 2, name: Notes}]
| nu-helpers pick --columns [name] --multi
```

Use `/` to filter, Enter to pick, and `q` to cancel. With `--multi`, Space toggles
marks that survive filtering.

`pick` returns the original values in input order. Empty input or cancellation
returns `[]`.
