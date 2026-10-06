# tola-address

Address identity for Tola sites: slugs, routes, logical output paths, and browser URLs. The four
are distinct; this crate defines them and the conversions between them.

- **slug** — the name of one route segment. Turning text into a slug belongs to `tola-slugify`;
  `slugify_segments` applies it to ordered segments and validates the joined route, so a segment
  can never change the hierarchy.
- **route** (`UrlPath`) — a decoded site-root address, `/posts/hello/`. A trailing slash is
  identity: `/x/` names a directory, `/x` names a file. Percent decoding happens once, when an
  encoded URL path becomes a route.
- **logical output path** (`OutputPath`) — where a document is written in the published site:
  `/x/` → `x/index.html`, `/x` → `x`. `OutputPath::from_route` converts route → output;
  `route_for_output` is the canonical reverse (`index.html` → `/`). One output path has one
  owner.
- **browser URL** — a route with the deployment mount applied and optionally an origin:
  `browser_url`, with `SiteUrlMount` and `SiteOrigin`; `browser_location` renders a redirect URI.

## Example

```rust
use tola_address::{
    OutputPath, SiteUrlMount, UrlPath, browser_url, route_for_output, slugify_segments,
};
use tola_slugify::NamingRules;

// identity segments → route, each segment slugged through tola-slugify
let route = slugify_segments(
    &["posts".to_owned(), "Héllo World".to_owned()],
    NamingRules::default(),
)?;
assert_eq!(route.as_str(), "/posts/héllo-world/");

// route → logical output path, and back to the canonical route
let output = OutputPath::from_route(&route);
assert_eq!(output.as_str(), "posts/héllo-world/index.html");
assert_eq!(route_for_output(&output).as_str(), "/posts/héllo-world/");

// route + deployment mount → browser URL
let mount = SiteUrlMount::from_base_path("/blog/")?;
assert_eq!(browser_url(&route, &mount, None), "/blog/posts/h%C3%A9llo-world/");

// an encoded URL path is decoded once, at this boundary
let route = UrlPath::parse("/posts/caf%C3%A9/")?;
assert_eq!(route.as_str(), "/posts/café/");
assert_eq!(route.to_encoded(), "/posts/caf%C3%A9/");
```

References are parsed here too: `SiteReference` keeps a decoded route with its query and serialized
fragment. `decoded_fragment()` provides a decoded view; `to_browser_url()` renders the mounted
route with its suffixes. Fragment escapes stay intact until a consumer interprets them: HTML
matches an id against the serialized spelling before trying the decoded spelling.
`resolve_browser_reference` resolves the document base and classifies the destination as a site
reference, outside the site, or external. `split_destination` splits the raw string without decoding.

Portable components are compared through conservative keys: two spellings that a
case-insensitive or normalization-aware filesystem would treat as one file also collide here.
`portable_collision_key` folds one component (case and Unicode normalization);
`portable_keys_overlap` decides whether two paths claim the same file or an ancestor of each
other; `portable_key_is_reserved` marks the reserved `_tola` namespace.
