#show: package(
  summary: [Normalize UUID strings and generate deterministic UUID v5 identifiers.],
)

#export(
  "normalize",
  summary: [Format a UUID string as canonical lowercase hyphenated form.],
  params: (
    value: [UUID string: 32 hex digits, hyphenated UUID, braced `{...}`, or `urn:uuid:...` (no leading/trailing whitespace).],
  ),
  related: ("v5", "namespace"),
)

#export(
  "v5",
  summary: [Generate a deterministic RFC 4122 UUID v5 from a namespace UUID and a name.],
  params: (
    namespace: [Namespace UUID string (same formats as `normalize`, or one of `namespace.*`).],
    name: [Name as a string (UTF-8 encoded verbatim) or raw `bytes`.],
  ),
  related: ("namespace", "normalize"),
)[
  Identical `(namespace, name)` pairs always yield the same UUID. If you need a feed entry ID to remain stable after a page moves, save the generated UUID in metadata rather than recomputing it from the path.

  #example(title: [Derive a deterministic DNS UUID v5], check: "default")[
  ```typst
  #import "@tola/uuid:0.0.0": namespace, v5

  #let id = v5(namespace.dns, "www.example.com")
  #document("index.html")[#id]
  ```
  ]
]

#export(
  "namespace",
  summary: [Dictionary of standard RFC 4122 namespace UUIDs: `dns`, `url`, `oid`, and `x500`.],
  related: ("v5",),
)
