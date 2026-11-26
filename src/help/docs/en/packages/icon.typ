#show: package(
  summary: [Render configured SVG icons inline in HTML or publish them as standalone SVG assets.],
  related: ("[icons]",),
)

Icons whose `fill` or `stroke` uses `currentColor` automatically inherit surrounding text color when rendered inline with `icon(...)`; explicit colors in the SVG stay unchanged.

#export(
  "icon",
  summary: [Insert a configured icon as a 1em inline SVG inside HTML documents.],
  params: (
    id: [Case-sensitive `"namespace:name"` identifier declared under `[icons.collections]`.],
    module: [Optional module alias owning the collection. Omit to use site collections.],
    label: [Accessible label for screen readers (non-blank). Omit or pass `none` to mark the icon decorative (`aria-hidden="true"`).],
    attrs: [Extra string-valued attributes on the root `<svg>` (for example `(class: "brand",)`). `class` and `style` append; accessibility attributes (`aria-label`, `aria-labelledby`, `aria-hidden`, `role`, `focusable`) are managed by `label` and rejected in `attrs`.],
  ),
  related: ("icon-url", "icon-bytes", "[icons]"),
)[
  Validates `id` at call time. Works only in HTML documents; for PDF or other formats use `icon-bytes` or `icon-url`.

  #example(title: [Render decorative and labeled inline icons], check: "site")[
  ```typst
  #import "@tola/icon:0.0.0": icon

  #document("index.html")[
    #icon("brand:mark")
    #icon("brand:mark", label: "Site logo", attrs: (class: "brand",))
  ]
  ```
  ]
]

#export(
  "icon-bytes",
  summary: [Return the raw SVG bytes of a configured icon for Typst `image(...)` or custom `asset(...)` publishing.],
  params: (
    id: [Case-sensitive `"namespace:name"` icon identifier.],
    module: [Optional module alias owning the collection. Omit to use site collections.],
  ),
  related: ("icon", "icon-url"),
)

#export(
  "icon-url",
  summary: [Publish a configured icon as a standalone SVG file and return its browser URL (with `site.base-path`).],
  params: (
    id: [Case-sensitive `"namespace:name"` icon identifier.],
    module: [Optional module alias owning the collection. Omit to use site collections.],
  ),
  related: ("icon", "icon-bytes"),
)[
  Only icons referenced during evaluation are published.

  #example(title: [Load a published icon with `<img>`], check: "site")[
  ```typst
  #import "@tola/icon:0.0.0": icon-url

  #document("index.html")[
    #html.img(src: icon-url("brand:mark"), alt: "Site mark")
  ]
  ```
  ]
]
