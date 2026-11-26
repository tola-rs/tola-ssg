#show: package(
  summary: [Index, group, filter, paginate, and traverse hierarchical trees over site records.],
)

Conventions shared across functions:
- `key` extracts one key per member; `keys` extracts an array of keys (such as tags).
- `parent` receives a *member key* (not the member record) and returns its parent key, or `none` at the root.
- Lookups targeting a member (`at`) return `none` if that member is missing. Groupings preserve first-seen key order; filters preserve input order.

#export(
  "pick",
  summary: [Return a new dictionary copying only the listed string `keys` present in `fields`.],
  params: (
    fields: [Source dictionary.],
    keys: [Ordered array of string keys to copy. Missing keys are skipped; duplicate keys appear once; `none` values are preserved.],
  ),
)

#export(
  "index-by",
  summary: [Build a dictionary indexing `members` by a unique string key.],
  params: (
    members: [Array of members to index.],
    key: [Function returning a unique string key per member. Defaults to the member itself.],
  ),
  related: ("group-by",),
)[
  Raises an error if two members produce the same key.
]

#export(
  "group-by",
  summary: [Group `members` into a dictionary of arrays keyed by a single string key per member.],
  params: (
    members: [Array of members to group.],
    key: [Function returning a string key per member. Defaults to the member itself.],
  ),
  related: ("group-by-keys", "index-by"),
)

#export(
  "group-by-keys",
  summary: [Group `members` into a dictionary of arrays when each member can belong to multiple keys (such as tags).],
  params: (
    members: [Array of members to group.],
    keys: [Function returning an array of string keys per member. Defaults to the member itself.],
  ),
  related: ("group-by", "select-members"),
)[
  #example(title: [Group posts by tag], check: "default")[
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
  summary: [Filter `members` whose extracted keys match `wanted` (any or all).],
  params: (
    members: [Array of members to filter.],
    wanted: [Array of target keys to match against.],
    keys: [Function returning an array of keys per member (may contain `none`). Defaults to the member itself.],
    match: [`"any"` (default) keeps members matching at least one key in `wanted`; `"all"` requires every key in `wanted`.],
  ),
  related: ("group-by-keys",),
)

#export(
  "adjacent",
  summary: [Return the previous and next neighbors (`(prev:, next:)`) of the member at key `at`.],
  params: (
    members: [Ordered array of members.],
    at: [Key of the target member.],
    key: [Function extracting a key per member. Defaults to the member itself.],
  ),
  related: ("window", "siblings"),
)[
  Returns `none` if `at` is not found; omits `prev` or `next` when at the start or end of `members`.
]

#export(
  "window",
  summary: [Return a contiguous slice of `members` centered on key `at`.],
  params: (
    members: [Ordered array of members.],
    at: [Key of the center member.],
    key: [Function extracting a key per member. Defaults to the member itself.],
    before: [Non-negative number of preceding members to include (default `0`).],
    after: [Non-negative number of following members to include (default `0`).],
  ),
  related: ("adjacent",),
)[
  Returns `none` if `at` is not found.
]

#export(
  "children",
  summary: [Return direct children of container key `of` in input order.],
  params: (
    members: [Array of tree members.],
    of: [Parent key whose direct children are returned (may be absent from `members`).],
    parent: [Function taking a member key and returning its parent key, or `none` at the root.],
    key: [Function extracting a key per member. Defaults to the member itself.],
  ),
  related: ("descendants", "ancestors", "siblings"),
)

#export(
  "descendants",
  summary: [Return all recursive descendants of container key `of` in pre-order traversal.],
  params: (
    members: [Array of tree members.],
    of: [Ancestor key whose descendants are collected (may be absent from `members`).],
    parent: [Function taking a member key and returning its parent key, or `none` at the root.],
    key: [Function extracting a key per member. Defaults to the member itself.],
  ),
  related: ("children", "ancestors", "lineage"),
)

#export(
  "ancestors",
  summary: [Return ancestor members of `at` ordered from root down to the immediate parent.],
  params: (
    members: [Array of tree members.],
    at: [Key of the target member.],
    parent: [Function taking a member key and returning its parent key, or `none` at the root.],
    key: [Function extracting a key per member. Defaults to the member itself.],
  ),
  related: ("lineage", "children", "descendants"),
)[
  Returns `none` if `at` is not found; returns `()` for a root member.
]

#export(
  "lineage",
  summary: [Return the path from root down to `at`, including `at` itself (useful for breadcrumbs).],
  params: (
    members: [Array of tree members.],
    at: [Key of the target member.],
    parent: [Function taking a member key and returning its parent key, or `none` at the root.],
    key: [Function extracting a key per member. Defaults to the member itself.],
  ),
  related: ("ancestors", "children"),
)[
  Returns `none` if `at` is not found.
]

#export(
  "siblings",
  summary: [Return all members sharing the same parent key as `at` (excluding `at` itself).],
  params: (
    members: [Array of tree members.],
    at: [Key of the target member.],
    parent: [Function taking a member key and returning its parent key, or `none` at the root.],
    key: [Function extracting a key per member. Defaults to the member itself.],
  ),
  related: ("children", "adjacent"),
)[
  Returns `none` if `at` is not found.
]
