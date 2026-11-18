# Nu Helpers

Select values from a searchable terminal table with `nu-helpers pick`.

```nu
use tola nu-helpers

[{id: 1, name: Guide} {id: 2, name: Notes}]
| nu-helpers pick --columns [name] --multi
```

Press `/` to filter, Enter to confirm, or `q` to cancel. With `--multi`, Space toggles marks that survive filtering.

`pick` returns the original values in input order, including fields hidden by `--columns`. Canceling or passing an empty list returns `[]`.