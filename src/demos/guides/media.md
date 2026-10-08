## What to look for

The inline leaf takes the blue text color; the sun retains its orange fill. The separately published leaf is an image of its own and does not inherit the paragraph's color. The 128×64 input becomes a 48×24 PNG derivative. A separate asset-tree copy remains 128×64.

## Configure resources before using their URLs

The browser asset tree and local SVG collection have different jobs. A public asset URL names what `[assets]` publishes; an icon ID names a member of the configured collection. Fonts stay local to the compiler, with system fonts disabled.

{{file:tola.toml}}
{{file:static/icons/leaf.svg}}
{{file:static/icons/sun.svg}}

## Inspect and resize a file input

`image-metadata` reads the private input's dimensions. `resize-image` also takes that input path, requests a derivative, and returns an already mounted URL with its dimensions. `fit-width` preserves the 2:1 ratio. Neither operation automatically publishes the original input.

{{file:content/index.typ}}

The original copy under `static/web/` is published because that directory is mapped. If only derivatives are wanted, keep originals outside the mapped tree. An icon label names the icon; a surrounding button would still need its own accessible name.

## Assemble the page

The root includes the source through its native `file` path, and the wrapper loads the declared stylesheet through `asset-url`.

{{file:site.typ}}
{{file:site/page.typ}}
